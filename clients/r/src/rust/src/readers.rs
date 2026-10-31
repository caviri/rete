//! Range readers for the R client: a local file (positional reads) and an
//! HTTP(S) `Range` client — rete-core's shared reader, the one the CLI and
//! the Python client use.

use std::fs::File;
use std::io;

use rete_core::RangeReader;

pub enum AnyReader {
    Local(LocalRangeReader),
    Http(HttpRangeReader),
}

impl RangeReader for AnyReader {
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

    fn read_many(&self, ranges: &[(u64, u64)]) -> io::Result<Vec<Vec<u8>>> {
        match self {
            Self::Local(r) => r.read_many(ranges),
            Self::Http(r) => r.read_many(ranges),
        }
    }

    // Forwarded, or the trait default (1) tells the planner the HTTP reader is
    // serial while it really keeps 16 requests in flight.
    fn concurrency(&self) -> usize {
        match self {
            Self::Local(r) => r.concurrency(),
            Self::Http(r) => r.concurrency(),
        }
    }
}

/// Positional reads against a local file — no mmap, no whole-file load.
pub struct LocalRangeReader {
    file: File,
    len: u64,
}

impl LocalRangeReader {
    pub fn open(path: &str) -> io::Result<Self> {
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
        offset
            .checked_add(len)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "range out of bounds"))?;
        let size = usize::try_from(len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "range too large for memory")
        })?;
        let mut buf = vec![0u8; size];
        read_exact_at(&self.file, &mut buf, offset)?;
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

/// HTTP(S) `Range` reader: rete-core's, the one the CLI and the Python client
/// use — kept-alive connections, the engine's batches 16 in flight and
/// coalesced, transient failures retried, a short body a hard error, and a
/// host that ignores `Range` served from its whole body up to 256 MiB (refused
/// by name above that).
pub use rete_core::http::HttpRangeReader;
