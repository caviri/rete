//! The [`RangeReader`] implementations behind `rete_graph.open()`: a local
//! file (positional reads), an HTTP(S) client (`Range` requests, mirroring
//! `crates/rete-cli/src/http.rs`), and an adapter over a user-supplied Python
//! object — the escape hatch that lets fsspec/s3fs-style storage back a graph
//! without any auth code on this side.

use std::fs::File;
use std::io::{self, Read};

use pyo3::prelude::*;
use rete_core::RangeReader;

/// The one reader type the open path stores: dispatch instead of generics so
/// [`crate::Graph`] stays a single non-generic pyclass.
pub enum AnyReader {
    Local(LocalRangeReader),
    Http(HttpRangeReader),
    Py(PyRangeReader),
}

impl RangeReader for AnyReader {
    fn len(&self) -> u64 {
        match self {
            Self::Local(r) => r.len(),
            Self::Http(r) => r.len(),
            Self::Py(r) => r.len(),
        }
    }

    fn read_at(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        match self {
            Self::Local(r) => r.read_at(offset, len),
            Self::Http(r) => r.read_at(offset, len),
            Self::Py(r) => r.read_at(offset, len),
        }
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> io::Result<Vec<Vec<u8>>> {
        match self {
            Self::Local(r) => r.read_many(ranges),
            Self::Http(r) => r.read_many(ranges),
            Self::Py(r) => r.read_many(ranges),
        }
    }
}

/// Positional reads against a local file (no mmap, no whole-file load), so a
/// multi-GB `.rete` on disk opens as lazily as a remote one.
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

/// HTTP(S) `Range` reader — the same contract as the CLI's: the server MUST
/// answer 206, and short bodies are hard errors, never silent truncation.
/// Extra request headers (auth tokens, custom UA) ride on every request.
pub struct HttpRangeReader {
    agent: ureq::Agent,
    url: String,
    len: u64,
    headers: Vec<(String, String)>,
}

impl HttpRangeReader {
    pub fn open(url: &str, headers: Vec<(String, String)>) -> io::Result<Self> {
        let agent = ureq::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build();
        // HEAD-first length probe; hosts that omit Content-Length on HEAD
        // (some object stores) fall back to a 1-byte ranged GET whose
        // `Content-Range: bytes 0-0/TOTAL` carries the length.
        let mut req = agent.head(url);
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        let head_len = req
            .call()
            .ok()
            .and_then(|resp| resp.header("content-length")?.parse::<u64>().ok());
        let len = match head_len {
            Some(len) => len,
            None => {
                let mut req = agent.get(url).set("Range", "bytes=0-0");
                for (k, v) in &headers {
                    req = req.set(k, v);
                }
                let resp = req.call().map_err(io::Error::other)?;
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
            headers,
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
        let mut req = self
            .agent
            .get(&self.url)
            .set("Range", &format!("bytes={offset}-{end}"));
        for (k, v) in &self.headers {
            req = req.set(k, v);
        }
        let resp = req.call().map_err(io::Error::other)?;
        // A `200 OK` means the server ignored `Range` and is streaming the
        // whole body from offset 0 — taking `len` bytes of that would silently
        // yield the wrong slice, so reject it loudly instead.
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

    /// Issue the (independent) ranges concurrently across a small thread pool:
    /// on a latency-bound link the coalesced faults of a query dominate wall
    /// time, and 16 matches both the CLI and the wasm fetch-worker pool.
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
            // Chunks are contiguous, so concatenating per-chunk results in
            // spawn order restores request order.
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

/// A reader backed by a Python object exposing `read_at(offset, length) ->
/// bytes` and a length (a `len()` method, or `__len__`). Each call re-acquires
/// the GIL, which is safe here: the engine only calls readers from inside
/// `allow_threads` sections, where the GIL is free to take.
pub struct PyRangeReader {
    obj: PyObject,
    len: u64,
}

impl PyRangeReader {
    pub fn new(py: Python<'_>, obj: PyObject) -> PyResult<Self> {
        let bound = obj.bind(py);
        let len: u64 = match bound.call_method0("len") {
            Ok(v) => v.extract()?,
            Err(_) => bound.len()? as u64,
        };
        Ok(Self { obj, len })
    }
}

impl RangeReader for PyRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        let buf: Vec<u8> = Python::with_gil(|py| {
            self.obj
                .bind(py)
                .call_method1("read_at", (offset, len))?
                .extract()
        })
        .map_err(|e: PyErr| io::Error::other(format!("python reader failed: {e}")))?;
        if buf.len() as u64 != len {
            return Err(io::Error::other(format!(
                "python reader returned {} of {len} bytes at offset {offset}",
                buf.len()
            )));
        }
        Ok(buf)
    }
}
