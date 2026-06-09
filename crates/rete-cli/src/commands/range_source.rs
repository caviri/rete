//! Shared local-or-HTTP range reader for commands that preview or execute
//! bounded reads over either a path or an HTTP(S) `.rete` URL.

use std::fs::File;
use std::io;

use rete_core::RangeReader;

use crate::http::HttpRangeReader;

pub(crate) fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
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
    fn open(path: &str) -> anyhow::Result<Self> {
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
