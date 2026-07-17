//! Range readers for the R client: a local file (positional reads) and an
//! HTTP(S) `Range` client — the same contract as the CLI's and the Python
//! client's vendored readers (206 required, short bodies are hard errors).

use std::fs::File;
use std::io::{self, Read};

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

/// HTTP(S) `Range` reader: the server MUST answer 206; short bodies error.
pub struct HttpRangeReader {
    agent: ureq::Agent,
    url: String,
    len: u64,
}

impl HttpRangeReader {
    pub fn open(url: &str) -> io::Result<Self> {
        let agent = ureq::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build();
        // HEAD-first length probe; hosts that omit Content-Length on HEAD
        // fall back to a 1-byte ranged GET's Content-Range total.
        let head_len = agent
            .head(url)
            .call()
            .ok()
            .and_then(|resp| resp.header("content-length")?.parse::<u64>().ok());
        let len = match head_len {
            Some(len) => len,
            None => {
                let resp = agent
                    .get(url)
                    .set("Range", "bytes=0-0")
                    .call()
                    .map_err(io::Error::other)?;
                resp.header("content-range")
                    .and_then(|v| v.rsplit('/').next()?.parse::<u64>().ok())
                    .ok_or_else(|| {
                        io::Error::other(format!(
                            "could not determine the file length of {url}: no Content-Length \
                             on HEAD and no Content-Range on a ranged GET"
                        ))
                    })?
            }
        };
        Ok(Self {
            agent,
            url: url.to_string(),
            len,
        })
    }
}

impl RangeReader for HttpRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let end = offset + len - 1; // HTTP ranges are inclusive
        let resp = self
            .agent
            .get(&self.url)
            .set("Range", &format!("bytes={offset}-{end}"))
            .call()
            .map_err(io::Error::other)?;
        if resp.status() != 206 {
            return Err(io::Error::other(format!(
                "server ignored Range (status {}, expected 206 Partial Content) for {}; \
                 the host must support HTTP range requests",
                resp.status(),
                self.url
            )));
        }
        let mut buf = Vec::with_capacity(len as usize);
        resp.into_reader().take(len).read_to_end(&mut buf)?;
        if (buf.len() as u64) < len {
            return Err(io::Error::other(format!(
                "short range response: got {} of {len} bytes at offset {offset} from {}",
                buf.len(),
                self.url
            )));
        }
        Ok(buf)
    }

    /// Concurrent range fetches across a small thread pool — 16 matches the
    /// CLI, the wasm fetch pool, and the Python client.
    fn read_many(&self, ranges: &[(u64, u64)]) -> io::Result<Vec<Vec<u8>>> {
        const MAX_CONCURRENCY: usize = 16;
        if ranges.len() <= 1 {
            return ranges.iter().map(|&(o, l)| self.read_at(o, l)).collect();
        }
        let workers = MAX_CONCURRENCY.min(ranges.len());
        let chunk = ranges.len().div_ceil(workers);
        std::thread::scope(|scope| {
            let handles: Vec<_> = ranges
                .chunks(chunk)
                .map(|group| {
                    scope.spawn(move || {
                        group
                            .iter()
                            .map(|&(o, l)| self.read_at(o, l))
                            .collect::<io::Result<Vec<Vec<u8>>>>()
                    })
                })
                .collect();
            let mut out = Vec::with_capacity(ranges.len());
            for handle in handles {
                out.extend(
                    handle
                        .join()
                        .map_err(|_| io::Error::other("a range-fetch worker thread panicked"))??,
                );
            }
            Ok(out)
        })
    }
}
