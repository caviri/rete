//! The [`RangeReader`] implementations behind `rete_graph.open()`: a local
//! file (positional reads), an HTTP(S) client (`Range` requests: rete-core's
//! shared reader, the one the CLI uses), and an adapter over a user-supplied Python
//! object — the escape hatch that lets fsspec/s3fs-style storage back a graph
//! without any auth code on this side.

use std::fs::File;
use std::io;

use pyo3::prelude::*;
use rete_core::RangeReader;

/// The one reader type the open path stores: dispatch instead of generics so
/// [`crate::Graph`] stays a single non-generic pyclass. The HTTP variant is
/// native-only — on emscripten (Pyodide) remote reads come in as a
/// [`PyRangeReader`] over the pure-Python sync-XHR reader.
pub enum AnyReader {
    Local(LocalRangeReader),
    #[cfg(not(target_os = "emscripten"))]
    Http(HttpRangeReader),
    Py(PyRangeReader),
}

impl RangeReader for AnyReader {
    fn len(&self) -> u64 {
        match self {
            Self::Local(r) => r.len(),
            #[cfg(not(target_os = "emscripten"))]
            Self::Http(r) => r.len(),
            Self::Py(r) => r.len(),
        }
    }

    fn read_at(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        match self {
            Self::Local(r) => r.read_at(offset, len),
            #[cfg(not(target_os = "emscripten"))]
            Self::Http(r) => r.read_at(offset, len),
            Self::Py(r) => r.read_at(offset, len),
        }
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> io::Result<Vec<Vec<u8>>> {
        match self {
            Self::Local(r) => r.read_many(ranges),
            #[cfg(not(target_os = "emscripten"))]
            Self::Http(r) => r.read_many(ranges),
            Self::Py(r) => r.read_many(ranges),
        }
    }

    // Forwarded, or the trait default (1) tells the planner every reader is
    // serial: it would then size its probe-vs-scan decisions for one request
    // at a time while the HTTP reader really keeps 16 in flight.
    fn concurrency(&self) -> usize {
        match self {
            Self::Local(r) => r.concurrency(),
            #[cfg(not(target_os = "emscripten"))]
            Self::Http(r) => r.concurrency(),
            Self::Py(r) => r.concurrency(),
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

/// HTTP(S) `Range` reader: rete-core's, the one the CLI uses — kept-alive
/// connections, the engine's batches 16 in flight and coalesced, transient
/// failures retried, a short body a hard error, and a host that ignores `Range`
/// served from its whole body up to 256 MiB (refused by name above that).
/// Extra request headers (auth tokens, custom UA) ride on every request.
/// Native-only: browsers have no sockets (see [`AnyReader`]).
#[cfg(not(target_os = "emscripten"))]
pub use rete_core::http::HttpRangeReader;

/// A reader backed by a Python object exposing `read_at(offset, length) ->
/// bytes` and a length (a `len()` method, or `__len__`). Each call re-acquires
/// the GIL, which is safe here: the engine only calls readers from inside
/// `detach` sections, where the GIL is free to take.
pub struct PyRangeReader {
    obj: Py<PyAny>,
    len: u64,
}

impl PyRangeReader {
    pub fn new(py: Python<'_>, obj: Py<PyAny>) -> PyResult<Self> {
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
        let buf: Vec<u8> = Python::attach(|py| {
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
