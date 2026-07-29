//! Shared local-or-HTTP range reader for commands that preview or execute
//! bounded reads over either a path or an HTTP(S) `.rete` URL.

use std::fs::File;
use std::io;

use rete_core::{auto_block, BlockCacheReader, RangeReader, Rete};

use crate::http::HttpRangeReader;

pub(crate) fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// File size above which a local `.rete` is opened lazily instead of read whole.
/// Overridable with `RETE_LOCAL_LAZY_ABOVE_MB` (0 forces lazy for everything).
const DEFAULT_LAZY_ABOVE_BYTES: u64 = 1 << 30; // 1 GiB

fn lazy_threshold_bytes() -> u64 {
    std::env::var("RETE_LOCAL_LAZY_ABOVE_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1 << 20))
        .unwrap_or(DEFAULT_LAZY_ABOVE_BYTES)
}

/// Open a local `.rete`, reading it whole only when that is actually cheaper.
///
/// `Rete::open` takes a full file image and decodes every dictionary chunk up
/// front, so peak memory is the file plus its *decompressed* dictionary. That is
/// fine for ordinary graphs and hopeless for one carrying embedded media: a
/// 23.5 GB file whose dictionary is 23.4 GB of base64 images needs ~55 GB to
/// open, and dies, even though answering a query touches a handful of chunks.
///
/// Above the threshold we therefore go through the same lazy path the HTTP
/// reader uses — `Rete::open_ranged_lazy` over a positional-read file handle —
/// which reads the header, the section directories and the index, then faults
/// dictionary chunks in on demand. Small files keep the read-it-all path, which
/// stays faster when the whole graph is going to be touched anyway.
///
/// The `BlockCacheReader` is not optional here. The engine re-reads the same
/// spans (a chunk directory during binary search, the header) many times over,
/// and a bare file handle re-does every one of them: a point lookup on a 23.5 GB
/// file read 15.9 GB and still had not finished. Block-aligned caching makes the
/// repeats free, which is what `sparql-url` has always done for the remote path.
pub(crate) fn open_local(path: &str) -> anyhow::Result<Rete> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if std::env::var("RETE_OPEN_DEBUG").is_ok() {
        eprintln!(
            "[open_local] {path}: len={len} threshold={} -> {}",
            lazy_threshold_bytes(),
            if len > lazy_threshold_bytes() { "LAZY" } else { "EAGER" }
        );
    }
    if len <= lazy_threshold_bytes() {
        return Ok(Rete::open(&std::fs::read(path)?)?);
    }
    let reader = std::sync::Arc::new(LocalRangeReader::open(path)?);
    // `RETE_BLOCK_KB` wins (0 disables), else auto-tune by length — same knob
    // and heuristic as the URL commands, so local and remote behave alike.
    let block: u64 = match std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(kb) => kb * 1024,
        None => auto_block(len),
    };
    Ok(if block == 0 {
        Rete::open_ranged_lazy(reader)?
    } else {
        Rete::open_ranged_lazy(std::sync::Arc::new(BlockCacheReader::new(reader, block)))?
    })
}

pub(crate) enum RangedSourceReader {
    Local(LocalRangeReader),
    Http(HttpRangeReader),
}

impl RangedSourceReader {
    pub(crate) fn open(source: &str) -> anyhow::Result<Self> {
        if is_url(source) {
            Ok(Self::Http(HttpRangeReader::open(source)?))
        } else {
            Ok(Self::Local(LocalRangeReader::open(source)?))
        }
    }
}

impl RangeReader for RangedSourceReader {
    fn len(&self) -> u64 {
        match self {
            Self::Local(r) => r.len(),
            Self::Http(r) => r.len(),
        }
    }

    fn read_at(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        match self {
            Self::Local(r) => r.read_at(offset, len),
            Self::Http(r) => r.read_at(offset, len),
        }
    }
}

pub(crate) struct LocalRangeReader {
    file: File,
    len: u64,
}

impl LocalRangeReader {
    pub(crate) fn open(path: &str) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl RangeReader for LocalRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "range out of bounds"))?;
        let size = usize::try_from(len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "range too large for memory")
        })?;
        let mut buf = vec![0u8; size];
        read_exact_at(&self.file, &mut buf, offset)?;
        debug_assert_eq!(end - offset, len);
        Ok(buf)
    }
}

#[cfg(unix)]
fn read_some_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

#[cfg(windows)]
fn read_some_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = read_some_at(file, buf, offset)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "range out of bounds",
            ));
        }
        offset += n as u64;
        let tmp = buf;
        buf = &mut tmp[n..];
    }
    Ok(())
}
