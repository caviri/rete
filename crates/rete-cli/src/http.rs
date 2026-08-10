//! An HTTP(S)-backed [`RangeReader`] (SPEC.md §9): point the client at a URL and
//! it fetches only the byte ranges a query needs via HTTP `Range` requests.
//!
//! Uses blocking `ureq` with rustls (the `tls` feature), so `http://` and
//! `https://` both work — the file can live on S3, GitHub, or any CDN that honors
//! `Range`, which is the format's whole deployment story.

use std::io::Read;

use rete_core::RangeReader;

pub struct HttpRangeReader {
    agent: ureq::Agent,
    url: String,
    len: u64,
}

fn range_does_not_fit_in_memory_error(len: u64, offset: u64, url: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "HTTP range does not fit in memory: requested {len} bytes at offset {offset} from {url}"
        ),
    )
}

fn parse_ascii_u64(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (unit, range_and_total) = value.split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") || range_and_total.contains(' ') {
        return None;
    }
    let (range, total) = range_and_total.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    Some((
        parse_ascii_u64(first)?,
        parse_ascii_u64(last)?,
        parse_ascii_u64(total)?,
    ))
}

fn invalid_content_range_error(
    len: u64,
    offset: u64,
    end: u64,
    total: u64,
    url: &str,
    actual: &str,
) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "invalid Content-Range for requested {len} bytes at offset {offset} from {url}: \
             expected bytes {offset}-{end}/{total}; got {actual}"
        ),
    )
}

impl HttpRangeReader {
    /// Probe the resource length with a HEAD request.
    pub fn open(url: &str) -> anyhow::Result<Self> {
        let agent = ureq::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build();
        let resp = agent.head(url).call()?;
        let len = resp
            .header("content-length")
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("server did not report Content-Length for {url}"))?;
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

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let end = offset.checked_add(len - 1).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "HTTP range end overflows u64",
            )
        })?; // HTTP ranges are inclusive
        let resp = self
            .agent
            .get(&self.url)
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
        let content_range_fields = resp
            .headers_names()
            .iter()
            .filter(|name| name.eq_ignore_ascii_case("content-range"))
            .count();
        if content_range_fields != 1 {
            return Err(invalid_content_range_error(
                len,
                offset,
                end,
                self.len,
                &self.url,
                &format!("{content_range_fields} fields"),
            ));
        }
        let actual = resp.header("content-range").ok_or_else(|| {
            invalid_content_range_error(len, offset, end, self.len, &self.url, "1 unreadable field")
        })?;
        if parse_content_range(actual) != Some((offset, end, self.len)) {
            return Err(invalid_content_range_error(
                len,
                offset,
                end,
                self.len,
                &self.url,
                &format!("{actual:?}"),
            ));
        }
        let capacity = usize::try_from(len)
            .map_err(|_| range_does_not_fit_in_memory_error(len, offset, &self.url))?;
        if let Some(declared) = resp
            .header("content-length")
            .and_then(|v| v.parse::<u64>().ok())
        {
            if declared != len {
                let kind = if declared < len { "short" } else { "overlong" };
                return Err(std::io::Error::other(format!(
                    "{kind} range response length mismatch: declared {declared}, expected {len} bytes \
                     at offset {offset} from {}",
                    self.url
                )));
            }
        }
        let mut body = Vec::with_capacity(capacity);
        resp.into_reader()
            .take(len.saturating_add(1))
            .read_to_end(&mut body)
            .map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "failed to read HTTP range response body for requested {len} bytes at \
                         offset {offset} from {}: {error}",
                        self.url
                    ),
                )
            })?;
        // A mismatched transfer (server closed early, range past EOF, proxy
        // hiccup, or extra bytes) must be a clean error, not a silently wrong
        // buffer handed to
        // the format parsers — mirroring `SliceReader`'s out-of-bounds error.
        match (body.len() as u64).cmp(&len) {
            std::cmp::Ordering::Less => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "short range response length mismatch: got {} of requested {len} bytes at offset {offset} from {}",
                    body.len(),
                    self.url
                ),
            )),
            std::cmp::Ordering::Greater => Err(std::io::Error::other(format!(
                "overlong range response length mismatch: got {} of {len} bytes at offset {offset} from {}",
                body.len(),
                self.url
            ))),
            std::cmp::Ordering::Equal => Ok(body),
        }
    }

    /// Issue the (independent) ranges concurrently across a small thread pool.
    /// On a latency-bound link the coalesced faults of a query dominate wall
    /// time; fetching them in parallel collapses N sequential round trips into
    /// ~N/P. `ureq`'s default agent pools connections and is safe to call from
    /// several threads at once.
    /// Matches `read_many`'s thread fan-out — the planner's probe-vs-scan hint.
    fn concurrency(&self) -> usize {
        16
    }

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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

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
        /// A flaky host: claims the range but close-delimits a short body.
        TruncateBodyWithoutContentLength,
        /// A broken host: returns the requested range plus one extra byte.
        OverlongBody,
        /// A broken host: reports a different first byte in Content-Range.
        WrongRangeStart,
        /// A broken host: reports a different last byte in Content-Range.
        WrongRangeEnd,
        /// A broken host: reports a different resource length in Content-Range.
        WrongRangeTotal,
        /// A broken host: omits Content-Range from a 206 response.
        MissingContentRange,
        /// A broken host: sends a syntactically invalid Content-Range.
        MalformedContentRange,
        /// A broken host: sends the wrong range unit.
        WrongRangeUnit,
        /// A broken host: does not disclose the complete resource length.
        UnknownRangeTotal,
        /// A conforming host: varies the range-unit case and pads numerals.
        SemanticContentRange,
        /// An ambiguous host: sends the same Content-Range field twice.
        DuplicateContentRange,
        /// An ambiguous host: sends conflicting Content-Range fields.
        ConflictingContentRange,
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
                    (
                        ServerMode::HonorRange
                        | ServerMode::TruncateBody
                        | ServerMode::TruncateBodyWithoutContentLength
                        | ServerMode::OverlongBody
                        | ServerMode::WrongRangeStart
                        | ServerMode::WrongRangeEnd
                        | ServerMode::WrongRangeTotal
                        | ServerMode::MissingContentRange
                        | ServerMode::MalformedContentRange
                        | ServerMode::WrongRangeUnit
                        | ServerMode::UnknownRangeTotal
                        | ServerMode::SemanticContentRange
                        | ServerMode::DuplicateContentRange
                        | ServerMode::ConflictingContentRange,
                        Some(r),
                    ) => {
                        let (a, b) = r.split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        let b: usize = b.parse().unwrap();
                        let slice = &data[a..=b.min(total - 1)];
                        let sent = if matches!(
                            mode,
                            ServerMode::TruncateBody | ServerMode::TruncateBodyWithoutContentLength
                        ) {
                            &slice[..slice.len() / 2] // lie, then hang up early
                        } else if matches!(mode, ServerMode::OverlongBody) {
                            &data[a..=b.min(total - 2) + 1]
                        } else {
                            slice
                        };
                        let declared_len = if matches!(mode, ServerMode::OverlongBody) {
                            sent.len()
                        } else {
                            slice.len()
                        };
                        let content_range = match mode {
                            ServerMode::WrongRangeStart => {
                                format!("Content-Range: bytes {}-{b}/{total}\r\n", a + 1)
                            }
                            ServerMode::WrongRangeEnd => {
                                format!("Content-Range: bytes {a}-{}/{total}\r\n", b - 1)
                            }
                            ServerMode::WrongRangeTotal => {
                                format!("Content-Range: bytes {a}-{b}/{}\r\n", total + 1)
                            }
                            ServerMode::MissingContentRange => String::new(),
                            ServerMode::MalformedContentRange => {
                                "Content-Range: bytes malformed\r\n".to_string()
                            }
                            ServerMode::WrongRangeUnit => {
                                format!("Content-Range: items {a}-{b}/{total}\r\n")
                            }
                            ServerMode::UnknownRangeTotal => {
                                format!("Content-Range: bytes {a}-{b}/*\r\n")
                            }
                            ServerMode::SemanticContentRange => {
                                format!("Content-Range: ByTeS {a:04}-{b:04}/{total:05}\r\n")
                            }
                            ServerMode::DuplicateContentRange => format!(
                                "Content-Range: bytes {a}-{b}/{total}\r\nContent-Range: bytes {a}-{b}/{total}\r\n"
                            ),
                            ServerMode::ConflictingContentRange => format!(
                                "Content-Range: bytes {a}-{b}/{total}\r\nContent-Range: bytes {}-{b}/{total}\r\n",
                                a + 1
                            ),
                            _ => format!("Content-Range: bytes {a}-{b}/{total}\r\n"),
                        };
                        let content_length =
                            if matches!(mode, ServerMode::TruncateBodyWithoutContentLength) {
                                String::new()
                            } else {
                                format!("Content-Length: {declared_len}\r\n")
                            };
                        let h = format!(
                            "HTTP/1.1 206 Partial Content\r\n{content_range}{content_length}Connection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(h.as_bytes());
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

    /// HTTP/1.1 range host that keeps accepted sockets open and counts them.
    /// It exits after serving `request_count` request heads across any number of
    /// connections, so the pre-agent implementation (one socket per request)
    /// and the pooled implementation (one socket total) both terminate.
    fn serve_keep_alive(data: Vec<u8>, request_count: usize) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let accepted = Arc::new(AtomicUsize::new(0));
        let accepted_server = accepted.clone();
        std::thread::spawn(move || {
            let mut served = 0usize;
            while served < request_count {
                let (mut stream, _) = listener.accept().unwrap();
                accepted_server.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                while served < request_count {
                    let mut req = Vec::new();
                    let mut byte = [0u8; 1];
                    while !req.ends_with(b"\r\n\r\n") {
                        use std::io::Read as _;
                        match stream.read(&mut byte) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => req.push(byte[0]),
                        }
                    }
                    if !req.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    let text = String::from_utf8_lossy(&req);
                    let is_head = text.starts_with("HEAD");
                    let range = text.lines().find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(|value| value.trim().to_string())
                    });
                    if is_head {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: keep-alive\r\n\r\n",
                            data.len()
                        );
                        stream.write_all(response.as_bytes()).unwrap();
                    } else {
                        let range = range.unwrap();
                        let (start, end) = range.split_once('-').unwrap();
                        let start: usize = start.parse().unwrap();
                        let end: usize = end.parse().unwrap();
                        let body = &data[start..=end];
                        let response = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                            data.len(),
                            body.len()
                        );
                        stream.write_all(response.as_bytes()).unwrap();
                        stream.write_all(body).unwrap();
                    }
                    stream.flush().unwrap();
                    served += 1;
                }
            }
        });
        (url, accepted)
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

    #[test]
    fn parse_content_range_accepts_exact_semantic_forms() {
        assert_eq!(
            parse_content_range("bytes 100-139/1000"),
            Some((100, 139, 1000))
        );
        assert_eq!(
            parse_content_range("BYTES 0100-0139/01000"),
            Some((100, 139, 1000))
        );
    }

    #[test]
    fn parse_content_range_rejects_ambiguous_or_malformed_forms() {
        let rejected = [
            "bytes */1000",
            "bytes 100-139/*",
            "bytes +100-139/1000",
            "bytes -100-139/1000",
            "bytes 100--139/1000",
            "bytes 100-+139/1000",
            "bytes 100-139/+1000",
            "bytes ١٠٠-139/1000",
            "bytes 100-١٣٩/1000",
            "bytes 100-139/١٠٠٠",
            "bytes\t100-139/1000",
            "bytes  100-139/1000",
            "bytes 100-139 /1000",
            "bytes 100-139/ 1000",
            "bytes 100-139/1000 ",
            "bytes 100-139/1000, bytes 200-239/1000",
            "bytes 100-139/1000/1000",
            "bytes 100-139-140/1000",
            "bytes -139/1000",
            "bytes 100-/1000",
            "bytes 100-139/",
            "items 100-139/1000",
            "bytes 18446744073709551616-18446744073709551616/18446744073709551616",
        ];

        for value in rejected {
            assert_eq!(parse_content_range(value), None, "accepted {value:?}");
        }
    }

    #[test]
    fn accepts_semantically_exact_content_range() {
        let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
        let url = serve(data.clone(), ServerMode::SemanticContentRange);
        let reader = HttpRangeReader::open(&url).unwrap();

        assert_eq!(reader.read_at(100, 40).unwrap(), &data[100..140]);
    }

    #[test]
    fn rejects_duplicate_content_range_fields() {
        for (mode, case) in [
            (ServerMode::DuplicateContentRange, "identical duplicates"),
            (
                ServerMode::ConflictingContentRange,
                "conflicting duplicates",
            ),
        ] {
            let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
            let url = serve(data, mode);
            let reader = HttpRangeReader::open(&url).unwrap();
            let err = reader.read_at(100, 40).unwrap_err();
            let message = err.to_string();

            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{case}");
            assert!(message.contains(&url), "{case}: {message}");
            assert!(message.contains("offset 100"), "{case}: {message}");
            assert!(message.contains("requested 40 bytes"), "{case}: {message}");
            assert!(
                message.contains("expected bytes 100-139/1000"),
                "{case}: {message}"
            );
            assert!(message.contains("got 2 fields"), "{case}: {message}");
        }
    }

    #[test]
    fn reuses_the_open_agent_for_sequential_ranges() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let (url, accepted) = serve_keep_alive(data.clone(), 3);
        let r = HttpRangeReader::open(&url).unwrap();

        assert_eq!(r.read_at(10, 8).unwrap(), &data[10..18]);
        assert_eq!(r.read_at(500, 16).unwrap(), &data[500..516]);
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_an_overflowing_http_range() {
        let data = vec![0u8; 32];
        let (url, _) = serve_keep_alive(data, 1);
        let r = HttpRangeReader::open(&url).unwrap();

        let err = r.read_at(u64::MAX, 2).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("range end overflows"));
    }

    #[test]
    fn allocation_error_identifies_the_failed_range_request() {
        let err = range_does_not_fit_in_memory_error(4_294_967_296, 17, "http://example.test/a");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        let message = err.to_string();
        assert!(message.contains("4294967296"), "{message}");
        assert!(message.contains("offset 17"), "{message}");
        assert!(message.contains("http://example.test/a"), "{message}");
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
        for mode in [
            ServerMode::TruncateBody,
            ServerMode::TruncateBodyWithoutContentLength,
        ] {
            let url = serve(data.clone(), mode);
            let r = HttpRangeReader::open(&url).unwrap();
            // `ureq` flags a declared Content-Length mismatch itself. The
            // close-delimited case reaches our own exact-length backstop.
            let err = r.read_at(100, 40).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
            let msg = err.to_string();
            assert!(
                msg.contains("short range response") || msg.contains("closed before"),
                "expected a truncation error, got: {err}"
            );
            assert!(msg.contains(&url), "missing URL context: {msg}");
            assert!(msg.contains("offset 100"), "missing offset context: {msg}");
            assert!(
                msg.contains("requested 40 bytes"),
                "missing requested-length context: {msg}"
            );
        }
        // And a range that genuinely runs past EOF on an honest host errors too.
        let data: Vec<u8> = (0..100u8).collect();
        let url = serve(data, ServerMode::HonorRange);
        let r = HttpRangeReader::open(&url).unwrap();
        let err = r.read_at(90, 20).unwrap_err();
        assert!(err.to_string().contains("short range response"), "{err}");
    }

    #[test]
    fn rejects_an_overlong_range_response() {
        let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
        let url = serve(data, ServerMode::OverlongBody);
        let reader = HttpRangeReader::open(&url).unwrap();
        let err = reader.read_at(100, 40).unwrap_err();
        assert!(err.to_string().contains("range response length"), "{err}");
    }

    #[test]
    fn rejects_missing_malformed_or_mismatched_content_range() {
        let cases = [
            (ServerMode::WrongRangeStart, "wrong start"),
            (ServerMode::WrongRangeEnd, "wrong end"),
            (ServerMode::WrongRangeTotal, "wrong total"),
            (ServerMode::MissingContentRange, "missing header"),
            (ServerMode::MalformedContentRange, "malformed header"),
            (ServerMode::WrongRangeUnit, "wrong unit"),
            (ServerMode::UnknownRangeTotal, "unknown total"),
        ];
        for (mode, case) in cases {
            let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
            let url = serve(data, mode);
            let reader = HttpRangeReader::open(&url).unwrap();
            let err = match reader.read_at(100, 40) {
                Err(error) => error,
                Ok(_) => panic!("accepted {case}"),
            };
            let message = err.to_string();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidData,
                "{case}: {message}"
            );
            assert!(message.contains("Content-Range"), "{case}: {message}");
            assert!(message.contains(&url), "{case}: {message}");
            assert!(message.contains("offset 100"), "{case}: {message}");
            assert!(message.contains("requested 40 bytes"), "{case}: {message}");
            assert!(
                message.contains("expected bytes 100-139/1000"),
                "{case}: {message}"
            );
            assert!(message.contains("got "), "{case}: {message}");
        }
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
