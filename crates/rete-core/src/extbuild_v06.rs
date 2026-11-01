//! The memory-bounded build's last step: a `0x06` file written straight from
//! the spill.
//!
//! The build leaves each section in its own spill file (dictionary sections,
//! one tile file per permutation, the quad index, the text index, the label
//! body). [`SpillImage`] lays them end to end as the `0x05` file the build used
//! to write — header, framing, sections — but only as an address space: no
//! byte of it is on disk as one file. [`crate::v06::parts::from_v05`] reads the
//! directories out of it and [`crate::v06::write::assemble_with`] copies the
//! payload once, to its final offset in the `0x06` output, deleting each spill
//! file as soon as its data is in. Peak disk is the spill plus the output, with
//! the two trading places as the write goes, and there is no intermediate
//! file and no convert pass.
//!
//! The same image answers the dictionary lookups of the label and schema passes
//! and, for `--card`, the card derivation, before the output exists.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::reader::RangeReader;

enum Src {
    Bytes(Vec<u8>),
    File(PathBuf),
}

struct Piece {
    start: u64,
    len: u64,
    src: Src,
    /// The `0x06` data section whose copy makes this piece's file disposable.
    release: Option<u8>,
}

/// Lays pieces end to end.
#[derive(Default)]
pub(crate) struct ImageBuilder {
    pieces: Vec<Piece>,
    at: u64,
}

impl ImageBuilder {
    /// Where the next piece starts.
    pub(crate) fn at(&self) -> u64 {
        self.at
    }

    /// Append bytes held in memory.
    pub(crate) fn bytes(&mut self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        self.pieces.push(Piece {
            start: self.at,
            len: b.len() as u64,
            src: Src::Bytes(b.to_vec()),
            release: None,
        });
        self.at += b.len() as u64;
    }

    /// Append a spill file of `len` bytes; `release` names the data section
    /// (a [`crate::v06::kind`]) after whose copy the file may be deleted.
    pub(crate) fn file(&mut self, path: &Path, len: u64, release: Option<u8>) {
        if len == 0 {
            return;
        }
        self.pieces.push(Piece {
            start: self.at,
            len,
            src: Src::File(path.to_path_buf()),
            release,
        });
        self.at += len;
    }

    /// Replace the first piece (the header's placeholder) once the layout is known.
    pub(crate) fn patch_head(&mut self, head: Vec<u8>) {
        if let Some(p) = self.pieces.first_mut() {
            debug_assert_eq!(p.len, head.len() as u64);
            p.src = Src::Bytes(head);
        }
    }

    pub(crate) fn finish(self) -> SpillImage {
        SpillImage {
            len: self.at,
            pieces: self.pieces,
            open: Mutex::new(HashMap::new()),
        }
    }
}

/// The spill files as one read-only address space.
pub(crate) struct SpillImage {
    len: u64,
    pieces: Vec<Piece>,
    open: Mutex<HashMap<usize, Arc<File>>>,
}

fn read_exact_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.read_exact_at(buf, offset)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (f, buf, offset);
        Err(std::io::ErrorKind::Unsupported.into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0usize;
        while done < buf.len() {
            let n = f.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
}

impl SpillImage {
    fn handle(&self, i: usize, path: &Path) -> std::io::Result<Arc<File>> {
        let mut open = self
            .open
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?;
        if let Some(f) = open.get(&i) {
            return Ok(f.clone());
        }
        let f = Arc::new(File::open(path)?);
        open.insert(i, f.clone());
        Ok(f)
    }

    /// Delete the spill files that fed data section `kind`.
    pub(crate) fn release(&self, kind: u8) {
        for (i, p) in self.pieces.iter().enumerate() {
            if p.release != Some(kind) {
                continue;
            }
            if let Ok(mut open) = self.open.lock() {
                open.remove(&i);
            }
            if let Src::File(path) = &p.src {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl RangeReader for SpillImage {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .filter(|&e| e <= self.len)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "past the image")
            })?;
        let mut out = vec![0u8; len as usize];
        // First piece that ends after `offset`.
        let mut i = self.pieces.partition_point(|p| p.start + p.len <= offset);
        let mut at = offset;
        while at < end {
            let p = self
                .pieces
                .get(i)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "gap"))?;
            let from = at - p.start;
            let n = (p.len - from).min(end - at) as usize;
            let dst = &mut out[(at - offset) as usize..(at - offset) as usize + n];
            match &p.src {
                Src::Bytes(b) => dst.copy_from_slice(&b[from as usize..from as usize + n]),
                Src::File(path) => read_exact_at(&*self.handle(i, path)?, dst, from)?,
            }
            at += n as u64;
            i += 1;
        }
        Ok(out)
    }
}

/// A shareable [`SpillImage`], for the readers that want to own one.
#[derive(Clone)]
pub struct SharedImage(pub(crate) Arc<SpillImage>);

impl RangeReader for SharedImage {
    fn len(&self) -> u64 {
        self.0.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.0.read_at(offset, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_across_pieces_and_releases_files() {
        let dir = std::env::temp_dir().join(format!("rete-spill-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, b"0123456789").unwrap();
        let mut b = ImageBuilder::default();
        b.bytes(b"abc");
        b.file(&f, 10, Some(7));
        b.bytes(b"XYZ");
        let img = b.finish();
        assert_eq!(img.len(), 16);
        assert_eq!(img.read_at(0, 16).unwrap(), b"abc0123456789XYZ");
        assert_eq!(img.read_at(2, 3).unwrap(), b"c01");
        assert_eq!(img.read_at(11, 4).unwrap(), b"89XY");
        assert!(img.read_at(10, 7).is_err());
        img.release(6);
        assert!(f.exists());
        img.release(7);
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
