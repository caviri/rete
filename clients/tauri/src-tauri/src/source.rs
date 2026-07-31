//! Open a `.rete` from a local path or an HTTP(S) URL behind one [`RangeReader`].
//!
//! This mirrors `crates/rete-cli/src/{http,commands/range_source}.rs`. The CLI
//! is a binary-only crate, so its readers cannot be depended on — and the
//! workspace rule is that `clients/` consumes `crates/`, never the reverse. The
//! logic is small and the behaviour it encodes (reject a `200` to a `Range`
//! request, fail loudly on a short read) is the part worth copying exactly.

use std::fs::File;
use std::io::Read;
use std::sync::Arc;

use rete_core::{auto_block, BlockCacheReader, CountingReader, RangeReader, Rete};

pub fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

// ------------------------------------------------------------------- HTTP

pub struct HttpRangeReader {
    url: String,
    len: u64,
}

impl HttpRangeReader {
    pub fn open(url: &str) -> anyhow::Result<Self> {
        let resp = ureq::head(url).call()?;
        let len = resp
            .header("content-length")
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("server did not report Content-Length for {url}"))?;
        Ok(Self {
            url: url.to_string(),
            len,
        })
    }
}

impl RangeReader for HttpRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let end = offset + len - 1; // HTTP ranges are inclusive
        let resp = ureq::get(&self.url)
            .set("Range", &format!("bytes={offset}-{end}"))
            .call()
            .map_err(std::io::Error::other)?;
        // A `200 OK` means the server ignored `Range` and is sending the whole
        // body from offset 0; taking `len` of that yields the wrong slice.
        if resp.status() != 206 {
            return Err(std::io::Error::other(format!(
                "server ignored Range (status {}, expected 206) for {}; \
                 the host must support HTTP range requests",
                resp.status(),
                self.url
            )));
        }
        let mut buf = Vec::with_capacity(len as usize);
        resp.into_reader().take(len).read_to_end(&mut buf)?;
        if (buf.len() as u64) < len {
            return Err(std::io::Error::other(format!(
                "short range response: got {} of {len} bytes at offset {offset} from {}",
                buf.len(),
                self.url
            )));
        }
        Ok(buf)
    }
}

// ------------------------------------------------------------------ local

/// Positional reads against an open file handle — the local twin of the HTTP
/// reader, so a multi-gigabyte local `.rete` faults in the same way a remote one
/// does instead of being read whole.
pub struct LocalRangeReader {
    file: File,
    len: u64,
}

impl LocalRangeReader {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl RangeReader for LocalRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; len as usize];
        read_exact_at(&self.file, &mut buf, offset)?;
        Ok(buf)
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0usize;
    while done < buf.len() {
        let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
        if n == 0 {
            return Err(std::io::Error::other("unexpected EOF in positional read"));
        }
        done += n;
    }
    Ok(())
}

// ------------------------------------------------------------------ either

pub enum Source {
    Http(HttpRangeReader),
    Local(LocalRangeReader),
}

impl RangeReader for Source {
    fn len(&self) -> u64 {
        match self {
            Source::Http(r) => r.len(),
            Source::Local(r) => r.len(),
        }
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        match self {
            Source::Http(r) => r.read_at(offset, len),
            Source::Local(r) => r.read_at(offset, len),
        }
    }
}

/// The handle the app keeps resident: the open graph plus the counting reader
/// underneath it, so the UI can report physical bytes and requests exactly the
/// way the browser build reports `RemoteGraph.stats()`.
pub struct Opened {
    pub rete: Rete,
    pub reader: Arc<CountingReader<Source>>,
}

/// Open a source lazily, behind a block cache.
///
/// The cache is not optional. The engine re-reads the same spans many times over
/// — a chunk directory during binary search, the header — and a bare reader
/// re-does every one of them. Block-aligned caching makes the repeats free.
pub fn open(source: &str) -> anyhow::Result<Opened> {
    let inner = if is_url(source) {
        Source::Http(HttpRangeReader::open(source)?)
    } else {
        Source::Local(LocalRangeReader::open(source)?)
    };
    let reader = Arc::new(CountingReader::new(inner));
    let total = reader.len();
    let block = match std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(kb) => kb * 1024,
        None => auto_block(total),
    };
    let rete = if block == 0 {
        Rete::open_ranged_lazy(reader.clone())?
    } else {
        Rete::open_ranged_lazy(Arc::new(BlockCacheReader::new(reader.clone(), block)))?
    };
    Ok(Opened { rete, reader })
}
