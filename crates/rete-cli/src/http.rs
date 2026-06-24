//! An HTTP(S)-backed [`RangeReader`] (SPEC.md §9): point the client at a URL and
//! it fetches only the byte ranges a query needs via HTTP `Range` requests.
//!
//! Uses blocking `ureq` with rustls (the `tls` feature), so `http://` and
//! `https://` both work — the file can live on S3, GitHub, or any CDN that honors
//! `Range`, which is the format's whole deployment story.

use std::io::Read;

use rete_core::RangeReader;

pub struct HttpRangeReader {
    url: String,
    len: u64,
}

impl HttpRangeReader {
    /// Probe the resource length with a HEAD request.
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
        // The server MUST honor the range. A `200 OK` means it ignored `Range`
        // and is returning the whole body from offset 0 — taking `len` bytes of
        // that would silently yield the wrong slice, so reject it loudly instead.
        if resp.status() != 206 {
            return Err(std::io::Error::other(format!(
                "server ignored Range (status {}, expected 206 Partial Content) for {}; \
                 the host must support HTTP range requests",
                resp.status(),
                self.url
            )));
        }
        let mut buf = Vec::with_capacity(len as usize);
        resp.into_reader().take(len).read_to_end(&mut buf)?;
        // A truncated transfer (server closed early, range past EOF, proxy
        // hiccup) must be a clean error, not a silently short buffer handed to
        // the format parsers — mirroring `SliceReader`'s out-of-bounds error.
        if (buf.len() as u64) < len {
            return Err(std::io::Error::other(format!(
                "short range response: got {} of {len} bytes at offset {offset} from {}",
                buf.len(),
                self.url
            )));
        }
        Ok(buf)
    }

    /// Issue the (independent) ranges concurrently across a small thread pool.
    /// On a latency-bound link the coalesced faults of a query dominate wall
    /// time; fetching them in parallel collapses N sequential round trips into
    /// ~N/P. `ureq`'s default agent pools connections and is safe to call from
    /// several threads at once.
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        /// Bounded so we never open a burst of sockets to a host for a big scan.
        /// 16 matches the wasm client's fetch-worker pool — a CDN/S3 serves this
        /// many concurrent range reads happily (it's the parallelism, not the
        /// bytes, that dominates wall time on a latency-bound link).
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
                            .collect::<std::io::Result<Vec<Vec<u8>>>>()
                    })
                })
                .collect();
            // Join in spawn order; chunks are contiguous, so concatenating their
            // results restores the original request order.
            let mut out = Vec::with_capacity(ranges.len());
            for h in handles {
                let part = h
                    .join()
                    .map_err(|_| std::io::Error::other("read_many worker panicked"))??;
                out.extend(part);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// How the throwaway test server treats range requests.
    #[derive(Clone, Copy)]
    enum ServerMode {
        /// A real range host: `206` with exactly the requested slice.
        HonorRange,
        /// A plain static server: ignores `Range`, replies `200` + whole body.
        IgnoreRange,
        /// A flaky host: claims the full range (`206` + Content-Length) but
        /// closes the connection after sending only half the bytes.
        TruncateBody,
        /// A broken host: `500` on every GET.
        Error500,
    }

    /// A throwaway localhost HTTP/1.1 server over the given bytes, with the
    /// given range behavior. Returns the bound `http://127.0.0.1:PORT/` base
    /// URL; the server thread is detached.
    fn serve(data: Vec<u8>, mode: ServerMode) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // Read the request head (until CRLFCRLF).
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                while !req.ends_with(b"\r\n\r\n") {
                    use std::io::Read as _;
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    req.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&req);
                let is_head = text.starts_with("HEAD");
                let range = text.lines().find_map(|l| {
                    let l = l.to_ascii_lowercase();
                    l.strip_prefix("range: bytes=")
                        .map(|r| r.trim().to_string())
                });

                let total = data.len();
                if is_head {
                    let h = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(h.as_bytes());
                    continue;
                }
                match (mode, range) {
                    (ServerMode::Error500, _) => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    (ServerMode::HonorRange | ServerMode::TruncateBody, Some(r)) => {
                        let (a, b) = r.split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        let b: usize = b.parse().unwrap();
                        let slice = &data[a..=b.min(total - 1)];
                        let h = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            slice.len()
                        );
                        let _ = stream.write_all(h.as_bytes());
                        let sent = if matches!(mode, ServerMode::TruncateBody) {
                            &slice[..slice.len() / 2] // lie, then hang up early
                        } else {
                            slice
                        };
                        let _ = stream.write_all(sent);
                    }
                    // Ignore Range (or no Range sent): 200 with the whole body.
                    _ => {
                        let h = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(h.as_bytes());
                        let _ = stream.write_all(&data);
                    }
                }
            }
        });
        url
    }

    #[test]
    fn reads_exact_ranges_from_a_range_host() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let url = serve(data.clone(), ServerMode::HonorRange);
        let r = HttpRangeReader::open(&url).unwrap();
        assert_eq!(r.len(), 1000);
        assert_eq!(r.read_at(0, 4).unwrap(), &data[0..4]);
        assert_eq!(r.read_at(500, 20).unwrap(), &data[500..520]);
        assert_eq!(r.read_at(996, 4).unwrap(), &data[996..1000]);
        assert!(r.read_at(0, 0).unwrap().is_empty());
    }

    /// Proves the `https://` transport works end-to-end against a real host that
    /// honors Range. Network-dependent, so `#[ignore]`d — CI never touches the
    /// network. Run on demand: `cargo test -p rete-cli -- --ignored https`.
    #[test]
    #[ignore = "hits the network (httpbin); run with --ignored"]
    fn reads_ranges_over_https() {
        let url = "https://httpbin.org/range/2048";
        let r = HttpRangeReader::open(url).unwrap();
        assert!(r.len() >= 2048);
        let chunk = r.read_at(100, 16).unwrap();
        assert_eq!(chunk.len(), 16);
    }

    #[test]
    fn rejects_a_host_that_ignores_range() {
        // A server that returns 200 with the whole body must be detected, not
        // silently mis-read as the requested slice.
        let data: Vec<u8> = (0..200u8).collect();
        let url = serve(data, ServerMode::IgnoreRange);
        let r = HttpRangeReader::open(&url).unwrap();
        let err = r.read_at(50, 10).unwrap_err();
        assert!(
            err.to_string().contains("ignored Range"),
            "expected a clear range-unsupported error, got: {err}"
        );
    }

    #[test]
    fn rejects_a_truncated_range_response() {
        // A 206 that claims the slice but delivers only part of it (dropped
        // connection, range past EOF, broken proxy) must surface as a clean
        // error — never as a silently short buffer.
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let url = serve(data, ServerMode::TruncateBody);
        let r = HttpRangeReader::open(&url).unwrap();
        // ureq itself flags the Content-Length mismatch ("body closed before
        // all bytes were read"); our own check is the backstop for transports
        // that don't. Either way: an error, never a short buffer.
        let err = r.read_at(100, 40).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("short range response") || msg.contains("closed before"),
            "expected a truncation error, got: {err}"
        );
        // And a range that genuinely runs past EOF on an honest host errors too.
        let data: Vec<u8> = (0..100u8).collect();
        let url = serve(data, ServerMode::HonorRange);
        let r = HttpRangeReader::open(&url).unwrap();
        let err = r.read_at(90, 20).unwrap_err();
        assert!(err.to_string().contains("short range response"), "{err}");
    }

    #[test]
    fn server_errors_are_clean_errors() {
        let data: Vec<u8> = (0..100u8).collect();
        let url = serve(data, ServerMode::Error500);
        let r = HttpRangeReader::open(&url).unwrap();
        // HEAD succeeded; the GET's 500 must come back as an error, not bytes.
        assert!(r.read_at(0, 10).is_err());
    }
}
