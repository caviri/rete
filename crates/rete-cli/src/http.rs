//! An HTTP(S)-backed [`RangeReader`] (SPEC.md §9): point the client at a URL and
//! it fetches only the byte ranges a query needs via HTTP `Range` requests.
//!
//! Uses blocking `ureq` with rustls (the `tls` feature), so `http://` and
//! `https://` both work — the file can live on S3, GitHub, or any CDN that honors
//! `Range`, which is the format's whole deployment story.
//!
//! Remote time is round trips, not bytes, so the reader does three things:
//!
//! - **One agent, kept-alive connections.** Every request goes through one
//!   `ureq::Agent` whose pool keeps up to [`FetchPolicy::concurrency`] idle
//!   connections to the host. `ureq::get` builds a fresh agent per call, so
//!   each range used to open (and, on `https://`, TLS-handshake) its own
//!   connection: one to three extra round trips per request.
//! - **Batches in parallel.** [`RangeReader::read_many`] (the engine's batch of
//!   tile or dictionary-chunk faults for one query step) goes out with up to
//!   [`FetchPolicy::concurrency`] requests in flight, from a work queue, so one
//!   slow range does not hold up the ranges queued behind it.
//! - **Nearby ranges coalesced.** Ranges of one batch at most
//!   [`FetchPolicy::coalesce_gap`] apart are fetched as one request (up to
//!   [`FetchPolicy::max_span`] long) and sliced apart again.
//!
//! A transient failure (a dropped connection, a short body, `408`/`429`/`5xx`)
//! is retried with exponential backoff; a permanent one (a host that ignores
//! `Range`, a `404`, a `Content-Range` for the wrong offset) fails at once. Any
//! range of a batch failing fails the batch, and the ranges still queued are
//! not started.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use rete_core::RangeReader;

/// How [`HttpRangeReader`] fetches: fan-out, coalescing and retries. Read once
/// from the environment when the reader opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FetchPolicy {
    /// Requests in flight at once in `read_many` (`RETE_HTTP_CONCURRENCY`,
    /// 1–64; `1` fetches a batch sequentially). Also what the reader reports
    /// to the planner as its [`RangeReader::concurrency`].
    pub(crate) concurrency: usize,
    /// Two ranges of one batch at most this many bytes apart are fetched as
    /// one request (`RETE_HTTP_COALESCE_KB`; `0` merges only touching or
    /// overlapping ranges).
    pub(crate) coalesce_gap: u64,
    /// Coalescing never grows a request past this many bytes, so one huge span
    /// does not serialize what could have gone out in parallel.
    pub(crate) max_span: u64,
    /// Extra attempts for a transiently failing request (`RETE_HTTP_RETRIES`,
    /// at most 10; `0` = no retry).
    pub(crate) retries: u32,
    /// The first backoff; it quadruples per retry, capped at 2 s.
    pub(crate) backoff_ms: u64,
    /// `RETE_HTTP_TRACE`: one stderr line per read and per batch, to see which
    /// ranges a query needed and which of them went out together.
    pub(crate) trace: bool,
}

impl FetchPolicy {
    /// 16 in flight matches the wasm client's fetch-worker pool and the Node
    /// client's `reteReadMany` cap; a CDN or S3 serves that many concurrent
    /// range reads happily.
    pub(crate) const DEFAULT: FetchPolicy = FetchPolicy {
        concurrency: 16,
        coalesce_gap: 32 << 10,
        max_span: 8 << 20,
        retries: 2,
        backoff_ms: 100,
        trace: false,
    };

    pub(crate) fn from_env() -> Self {
        let num = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
        };
        let d = Self::DEFAULT;
        FetchPolicy {
            concurrency: num("RETE_HTTP_CONCURRENCY")
                .map_or(d.concurrency, |n| n.clamp(1, 64) as usize),
            coalesce_gap: num("RETE_HTTP_COALESCE_KB")
                .map_or(d.coalesce_gap, |kb| kb.saturating_mul(1024)),
            max_span: d.max_span,
            retries: num("RETE_HTTP_RETRIES").map_or(d.retries, |n| n.min(10) as u32),
            backoff_ms: d.backoff_ms,
            trace: std::env::var_os("RETE_HTTP_TRACE").is_some(),
        }
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let ms = self
            .backoff_ms
            .saturating_mul(4u64.saturating_pow(attempt))
            .min(2000);
        Duration::from_millis(ms)
    }
}

/// Why one request failed: worth another attempt, or not.
enum Failure {
    Transient(std::io::Error),
    Permanent(std::io::Error),
}

/// `408 Request Timeout`, `429 Too Many Requests` and the 5xx a retry can cure
/// (not `501 Not Implemented` or `505 HTTP Version Not Supported`).
fn retryable_status(code: u16) -> bool {
    code == 408 || code == 429 || ((500..600).contains(&code) && code != 501 && code != 505)
}

fn classify(err: ureq::Error, what: &str) -> Failure {
    match err {
        ureq::Error::Status(code, _) => {
            let e = std::io::Error::other(format!("HTTP {code} for {what}"));
            if retryable_status(code) {
                Failure::Transient(e)
            } else {
                Failure::Permanent(e)
            }
        }
        ureq::Error::Transport(t) => {
            let transient = matches!(
                t.kind(),
                ureq::ErrorKind::Io
                    | ureq::ErrorKind::ConnectionFailed
                    | ureq::ErrorKind::Dns
                    | ureq::ErrorKind::ProxyConnect
            );
            let e = std::io::Error::other(format!("{t} ({what})"));
            if transient {
                Failure::Transient(e)
            } else {
                Failure::Permanent(e)
            }
        }
    }
}

/// The first byte of a `Content-Range: bytes a-b/N` value.
fn content_range_start(v: &str) -> Option<u64> {
    let v = v.trim().strip_prefix("bytes")?.trim_start();
    v.split(['-', '/']).next()?.trim().parse().ok()
}

/// One request of a coalesced batch.
#[derive(Debug, PartialEq, Eq)]
struct Span {
    offset: u64,
    len: u64,
}

/// Where each requested range of a batch lands: `(request, offset within
/// that request)`, or `None` for an empty range.
type Placement = Vec<Option<(usize, u64)>>;

/// Merge a batch's non-empty ranges into the requests that fetch them: sorted
/// by offset, a range joins the current request when it starts at most `gap`
/// bytes past its end and the request stays within `max_span`. Returns the
/// requests and, per input range, where its bytes are.
fn coalesce(
    ranges: &[(u64, u64)],
    gap: u64,
    max_span: u64,
) -> std::io::Result<(Vec<Span>, Placement)> {
    let mut order: Vec<usize> = (0..ranges.len()).filter(|&i| ranges[i].1 > 0).collect();
    for &i in &order {
        let (o, l) = ranges[i];
        if o.checked_add(l).is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("range {o}+{l} overflows"),
            ));
        }
    }
    order.sort_by_key(|&i| ranges[i]);
    let mut spans: Vec<Span> = Vec::new();
    let mut place: Placement = vec![None; ranges.len()];
    for i in order {
        let (o, l) = ranges[i];
        let end = o + l;
        let joins = spans.last().is_some_and(|s| {
            let s_end = s.offset + s.len;
            o <= s_end.saturating_add(gap) && end.max(s_end) - s.offset <= max_span
        });
        if joins {
            let s = spans.last_mut().expect("checked above");
            s.len = (s.offset + s.len).max(end) - s.offset;
        } else {
            spans.push(Span { offset: o, len: l });
        }
        let si = spans.len() - 1;
        place[i] = Some((si, o - spans[si].offset));
    }
    Ok((spans, place))
}

pub struct HttpRangeReader {
    url: String,
    len: u64,
    agent: ureq::Agent,
    policy: FetchPolicy,
}

impl HttpRangeReader {
    /// Probe the resource length with a HEAD request.
    pub fn open(url: &str) -> anyhow::Result<Self> {
        Self::open_with(url, FetchPolicy::from_env())
    }

    /// [`open`](Self::open) under an explicit fetch policy.
    pub(crate) fn open_with(url: &str, policy: FetchPolicy) -> anyhow::Result<Self> {
        let agent = ureq::AgentBuilder::new()
            // Keep a batch's worth of connections open between batches.
            .max_idle_connections(policy.concurrency.max(4) * 2)
            .max_idle_connections_per_host(policy.concurrency)
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(120))
            .build();
        let mut attempt = 0;
        let resp = loop {
            match agent.head(url).call() {
                Ok(r) => break r,
                Err(e) => match classify(e, &format!("HEAD {url}")) {
                    Failure::Transient(_) if attempt < policy.retries => {
                        std::thread::sleep(policy.backoff(attempt));
                        attempt += 1;
                    }
                    Failure::Transient(e) | Failure::Permanent(e) => return Err(e.into()),
                },
            }
        };
        let len = resp
            .header("content-length")
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("server did not report Content-Length for {url}"))?;
        Ok(Self {
            url: url.to_string(),
            len,
            agent,
            policy,
        })
    }

    /// A range past the length the HEAD probe reported is an error before any
    /// request: no retry can make the file longer.
    fn bounds(&self, offset: u64, len: u64) -> std::io::Result<()> {
        if offset.checked_add(len).is_none_or(|end| end > self.len) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "range out of bounds: {offset}+{len} past the {} bytes {} reported",
                    self.len, self.url
                ),
            ));
        }
        Ok(())
    }

    /// One GET for `[offset, offset+len)` (`len > 0`), classified on failure.
    fn fetch_once(&self, offset: u64, len: u64) -> Result<Vec<u8>, Failure> {
        let end = offset + len - 1; // HTTP ranges are inclusive
        let what = format!("bytes {offset}-{end} of {}", self.url);
        let resp = self
            .agent
            .get(&self.url)
            .set("Range", &format!("bytes={offset}-{end}"))
            .call()
            .map_err(|e| classify(e, &what))?;
        // The server MUST honor the range. A `200 OK` means it ignored `Range`
        // and is returning the whole body from offset 0 — taking `len` bytes of
        // that would silently yield the wrong slice, so reject it loudly instead.
        if resp.status() != 206 {
            return Err(Failure::Permanent(std::io::Error::other(format!(
                "server ignored Range (status {}, expected 206 Partial Content) for {}; \
                 the host must support HTTP range requests",
                resp.status(),
                self.url
            ))));
        }
        // A 206 for some other offset (a broken cache or proxy) is just as wrong.
        if let Some(start) = resp.header("content-range").and_then(content_range_start) {
            if start != offset {
                return Err(Failure::Permanent(std::io::Error::other(format!(
                    "server answered a Content-Range starting at {start} for a request at \
                     offset {offset} ({})",
                    self.url
                ))));
            }
        }
        let size = usize::try_from(len).map_err(|_| {
            Failure::Permanent(std::io::Error::other(format!(
                "a {len}-byte range is too large for memory"
            )))
        })?;
        let mut buf = Vec::with_capacity(size);
        // Read one byte past the range: an exact body then ends at EOF, which
        // is what hands the connection back to the agent's pool for reuse.
        resp.into_reader()
            .take(len + 1)
            .read_to_end(&mut buf)
            .map_err(|e| Failure::Transient(std::io::Error::other(format!("{e} ({what})"))))?;
        // A truncated transfer (server closed early, range past EOF, proxy
        // hiccup) must be a clean error, not a silently short buffer handed to
        // the format parsers — mirroring `SliceReader`'s out-of-bounds error.
        if (buf.len() as u64) < len {
            return Err(Failure::Transient(std::io::Error::other(format!(
                "short range response: got {} of {len} bytes at offset {offset} from {}",
                buf.len(),
                self.url
            ))));
        }
        buf.truncate(size);
        Ok(buf)
    }

    /// [`fetch_once`](Self::fetch_once) with the policy's retries.
    fn fetch(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let mut attempt = 0;
        loop {
            match self.fetch_once(offset, len) {
                Ok(b) => return Ok(b),
                Err(Failure::Permanent(e)) => return Err(e),
                Err(Failure::Transient(e)) if attempt >= self.policy.retries => {
                    return Err(if attempt == 0 {
                        e
                    } else {
                        std::io::Error::other(format!("{e} (after {} attempts)", attempt + 1))
                    });
                }
                Err(Failure::Transient(_)) => {
                    std::thread::sleep(self.policy.backoff(attempt));
                    attempt += 1;
                }
            }
        }
    }

    /// Fetch `spans` in order, up to `concurrency` at a time.
    fn fetch_spans(&self, spans: &[Span]) -> std::io::Result<Vec<Vec<u8>>> {
        let workers = self.policy.concurrency.min(spans.len());
        if workers <= 1 {
            return spans.iter().map(|s| self.fetch(s.offset, s.len)).collect();
        }
        let next = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        type Slot = Mutex<Option<std::io::Result<Vec<u8>>>>;
        let slots: Vec<Slot> = spans.iter().map(|_| Mutex::new(None)).collect();
        let panicked = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| loop {
                        if failed.load(Ordering::Relaxed) {
                            break; // the batch has failed: start nothing new
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(s) = spans.get(i) else { break };
                        let r = self.fetch(s.offset, s.len);
                        if r.is_err() {
                            failed.store(true, Ordering::Relaxed);
                        }
                        *slots[i].lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().is_err())
                .fold(false, |a, b| a | b)
        });
        if panicked {
            return Err(std::io::Error::other("read_many worker panicked"));
        }
        let mut out = Vec::with_capacity(spans.len());
        let mut first_err = None;
        for slot in slots {
            match slot.into_inner().unwrap_or_else(|p| p.into_inner()) {
                Some(Ok(b)) => out.push(b),
                Some(Err(e)) => {
                    first_err.get_or_insert(e);
                }
                None => {} // never started: the batch had already failed
            }
        }
        match first_err {
            Some(e) => Err(e),
            None if out.len() == spans.len() => Ok(out),
            None => Err(std::io::Error::other("read_many left a range unfetched")),
        }
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
        self.bounds(offset, len)?;
        if self.policy.trace {
            eprintln!("[http] read {offset}+{len}");
        }
        self.fetch(offset, len)
    }

    /// The planner's probe-vs-scan hint: how many requests
    /// [`read_many`](Self::read_many) keeps in flight.
    fn concurrency(&self) -> usize {
        self.policy.concurrency
    }

    /// Coalesce the batch, fetch its requests concurrently, slice the ranges
    /// back out in request order.
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        for &(o, l) in ranges {
            if l > 0 {
                self.bounds(o, l)?;
            }
        }
        let (spans, place) = coalesce(ranges, self.policy.coalesce_gap, self.policy.max_span)?;
        if self.policy.trace {
            let bytes: u64 = spans.iter().map(|s| s.len).sum();
            eprintln!(
                "[http] batch of {} range(s) -> {} request(s), {bytes} bytes, {} in flight",
                ranges.len(),
                spans.len(),
                self.policy.concurrency.min(spans.len())
            );
        }
        let blobs = self.fetch_spans(&spans)?;
        Ok(place
            .into_iter()
            .zip(ranges)
            .map(|(p, &(_, len))| match p {
                None => Vec::new(),
                Some((si, at)) => {
                    let at = at as usize;
                    blobs[si][at..at + len as usize].to_vec()
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

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
        /// `404` on every GET: permanent, never retried.
        NotFound,
        /// `503` for the first `n` GETs, then a real range host.
        Unavailable(u64),
        /// A broken cache: `206` with a `Content-Range` one byte off.
        WrongOffset,
        /// A real range host, except `404` for a request starting at this offset.
        FailAt(usize),
    }

    /// What the server saw: GETs served, and the most it had in flight at once.
    #[derive(Default)]
    struct Stats {
        gets: AtomicU64,
        in_flight: AtomicU64,
        max_in_flight: AtomicU64,
    }

    /// A throwaway localhost HTTP/1.1 server over the given bytes, with the
    /// given range behavior; one thread per connection, each GET held for
    /// `delay` so concurrent requests overlap. Returns the base URL and the
    /// server's counters; the server thread is detached.
    fn serve_with(data: Vec<u8>, mode: ServerMode, delay: Duration) -> (String, Arc<Stats>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let stats = Arc::new(Stats::default());
        let data = Arc::new(data);
        let st = stats.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (data, st) = (data.clone(), st.clone());
                std::thread::spawn(move || handle(stream, &data, mode, delay, &st));
            }
        });
        (url, stats)
    }

    fn serve(data: Vec<u8>, mode: ServerMode) -> String {
        serve_with(data, mode, Duration::ZERO).0
    }

    fn handle(
        mut stream: std::net::TcpStream,
        data: &[u8],
        mode: ServerMode,
        delay: Duration,
        st: &Stats,
    ) {
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
            return;
        }
        let n = st.gets.fetch_add(1, Ordering::SeqCst) + 1;
        let now = st.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        st.max_in_flight.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(delay);
        let status_only = |stream: &mut std::net::TcpStream, line: &str| {
            let _ = stream.write_all(
                format!("HTTP/1.1 {line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        };
        match (mode, range) {
            (ServerMode::Error500, _) => status_only(&mut stream, "500 Internal Server Error"),
            (ServerMode::NotFound, _) => status_only(&mut stream, "404 Not Found"),
            (ServerMode::Unavailable(k), _) if n <= k => {
                status_only(&mut stream, "503 Service Unavailable")
            }
            (ServerMode::FailAt(at), Some(r)) if r.starts_with(&format!("{at}-")) => {
                status_only(&mut stream, "404 Not Found")
            }
            (
                ServerMode::HonorRange
                | ServerMode::TruncateBody
                | ServerMode::Unavailable(_)
                | ServerMode::WrongOffset
                | ServerMode::FailAt(_),
                Some(r),
            ) => {
                let (a, b) = r.split_once('-').unwrap();
                let a: usize = a.parse().unwrap();
                let b: usize = b.parse().unwrap();
                let slice = &data[a.min(total)..=b.min(total - 1)];
                let claimed = if matches!(mode, ServerMode::WrongOffset) {
                    a + 1
                } else {
                    a
                };
                let h = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {claimed}-{b}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
                let _ = stream.write_all(data);
            }
        }
        st.in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    /// The default policy with quick retries, so failure tests stay fast.
    fn quick() -> FetchPolicy {
        FetchPolicy {
            backoff_ms: 1,
            ..FetchPolicy::DEFAULT
        }
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
        let (url, stats) = serve_with(data, ServerMode::IgnoreRange, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        let err = r.read_at(50, 10).unwrap_err();
        assert!(
            err.to_string().contains("ignored Range"),
            "expected a clear range-unsupported error, got: {err}"
        );
        assert_eq!(stats.gets.load(Ordering::SeqCst), 1, "permanent: no retry");
    }

    #[test]
    fn rejects_a_truncated_range_response() {
        // A 206 that claims the slice but delivers only part of it (dropped
        // connection, range past EOF, broken proxy) must surface as a clean
        // error — never as a silently short buffer.
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let (url, stats) = serve_with(data, ServerMode::TruncateBody, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        // ureq itself flags the Content-Length mismatch ("body closed before
        // all bytes were read"); our own check is the backstop for transports
        // that don't. Either way: an error, never a short buffer.
        let err = r.read_at(100, 40).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("short range response") || msg.contains("closed before"),
            "expected a truncation error, got: {err}"
        );
        assert_eq!(
            stats.gets.load(Ordering::SeqCst),
            3,
            "a truncated body is transient: retried twice"
        );
        // And a range that runs past EOF errors too, before any request.
        let data: Vec<u8> = (0..100u8).collect();
        let (url, stats) = serve_with(data, ServerMode::HonorRange, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        let err = r.read_at(90, 20).unwrap_err();
        assert!(err.to_string().contains("out of bounds"), "{err}");
        assert!(r.read_many(&[(0, 1), (90, 20)]).is_err());
        assert_eq!(stats.gets.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn server_errors_are_clean_errors() {
        let data: Vec<u8> = (0..100u8).collect();
        let (url, stats) = serve_with(data, ServerMode::Error500, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        // HEAD succeeded; the GET's 500 must come back as an error, not bytes.
        let err = r.read_at(0, 10).unwrap_err();
        assert!(err.to_string().contains("HTTP 500"), "{err}");
        assert!(err.to_string().contains("after 3 attempts"), "{err}");
        assert_eq!(stats.gets.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_missing_file_is_not_retried() {
        let (url, stats) = serve_with(vec![0; 100], ServerMode::NotFound, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        let err = r.read_at(0, 10).unwrap_err();
        assert!(err.to_string().contains("HTTP 404"), "{err}");
        assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_transient_failure_is_retried_and_recovers() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let (url, stats) = serve_with(data.clone(), ServerMode::Unavailable(2), Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        assert_eq!(r.read_at(10, 30).unwrap(), &data[10..40]);
        assert_eq!(stats.gets.load(Ordering::SeqCst), 3, "two 503s, then 206");
        // With retries off the same outage is an error.
        let (url, _) = serve_with(data, ServerMode::Unavailable(1), Duration::ZERO);
        let none = FetchPolicy {
            retries: 0,
            ..quick()
        };
        let r = HttpRangeReader::open_with(&url, none).unwrap();
        assert!(r.read_at(10, 30).unwrap_err().to_string().contains("503"));
    }

    #[test]
    fn rejects_a_range_answered_for_the_wrong_offset() {
        let (url, _) = serve_with(vec![7; 1000], ServerMode::WrongOffset, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        let err = r.read_at(100, 10).unwrap_err();
        assert!(
            err.to_string().contains("Content-Range starting at 101"),
            "{err}"
        );
    }

    /// The engine's batch goes out concurrently, bounded by the policy, and
    /// comes back in request order with exact bytes.
    #[test]
    fn read_many_overlaps_requests_up_to_the_bound() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1 << 20).collect();
        let (url, stats) = serve_with(
            data.clone(),
            ServerMode::HonorRange,
            Duration::from_millis(40),
        );
        let policy = FetchPolicy {
            concurrency: 4,
            coalesce_gap: 0,
            ..quick()
        };
        let r = HttpRangeReader::open_with(&url, policy).unwrap();
        // 12 ranges 64 KiB apart, given out of order, plus an empty one.
        let mut ranges: Vec<(u64, u64)> = (0..12).map(|i| (i * 65_536 + 7, 100)).collect();
        ranges.reverse();
        ranges.insert(3, (5, 0));
        let t = std::time::Instant::now();
        let got = r.read_many(&ranges).unwrap();
        let took = t.elapsed();
        assert_eq!(got.len(), ranges.len());
        for (&(o, l), b) in ranges.iter().zip(&got) {
            assert_eq!(b.as_slice(), &data[o as usize..(o + l) as usize]);
        }
        assert_eq!(stats.gets.load(Ordering::SeqCst), 12);
        assert_eq!(
            stats.max_in_flight.load(Ordering::SeqCst),
            4,
            "bounded fan-out"
        );
        // 12 requests, 4 at a time, 40 ms each: 3 waves, not 12.
        assert!(took < Duration::from_millis(12 * 40), "{took:?}");
    }

    /// `RETE_HTTP_CONCURRENCY=1` behaves exactly like the old sequential reader.
    #[test]
    fn concurrency_one_is_sequential() {
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let (url, stats) = serve_with(data, ServerMode::HonorRange, Duration::from_millis(5));
        let policy = FetchPolicy {
            concurrency: 1,
            coalesce_gap: 0,
            ..quick()
        };
        let r = HttpRangeReader::open_with(&url, policy).unwrap();
        assert_eq!(r.concurrency(), 1);
        r.read_many(&[(0, 10), (1000, 10), (3000, 10)]).unwrap();
        assert_eq!(stats.max_in_flight.load(Ordering::SeqCst), 1);
        assert_eq!(stats.gets.load(Ordering::SeqCst), 3);
    }

    /// Nearby ranges share one request; far ones do not.
    #[test]
    fn read_many_coalesces_nearby_ranges() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1 << 20).collect();
        let (url, stats) = serve_with(data.clone(), ServerMode::HonorRange, Duration::ZERO);
        let policy = FetchPolicy {
            coalesce_gap: 1024,
            ..quick()
        };
        let r = HttpRangeReader::open_with(&url, policy).unwrap();
        let ranges = [(5000, 100), (100, 50), (200, 10), (190, 30), (6000, 1)];
        let got = r.read_many(&ranges).unwrap();
        for (&(o, l), b) in ranges.iter().zip(&got) {
            assert_eq!(b.as_slice(), &data[o as usize..(o + l) as usize]);
        }
        // [100..220) is one request (overlapping and close); [5000..6001) another.
        assert_eq!(stats.gets.load(Ordering::SeqCst), 2);
    }

    /// One failing range fails the whole batch, and the error names it.
    #[test]
    fn read_many_fails_the_batch_on_one_bad_range() {
        let data: Vec<u8> = (0..100u8).collect();
        let url = serve(data, ServerMode::FailAt(50));
        let r = HttpRangeReader::open_with(
            &url,
            FetchPolicy {
                coalesce_gap: 0,
                ..quick()
            },
        )
        .unwrap();
        let err = r.read_many(&[(0, 10), (50, 10), (20, 10)]).unwrap_err();
        assert!(
            err.to_string().contains("HTTP 404 for bytes 50-59"),
            "{err}"
        );
        assert!(r.read_many(&[(0, 10), (20, 10)]).is_ok());
    }

    #[test]
    fn coalesce_plans_spans_and_placements() {
        let (spans, place) = coalesce(&[(10, 5), (0, 0), (18, 4), (100, 1)], 3, 1 << 20).unwrap();
        assert_eq!(
            spans,
            vec![
                Span {
                    offset: 10,
                    len: 12
                },
                Span {
                    offset: 100,
                    len: 1
                }
            ]
        );
        assert_eq!(place, vec![Some((0, 0)), None, Some((0, 8)), Some((1, 0))]);
        // The span cap keeps a merge from growing without bound.
        let (spans, _) = coalesce(&[(0, 10), (10, 10), (20, 10)], 0, 20).unwrap();
        assert_eq!(spans.len(), 2);
        assert!(coalesce(&[(u64::MAX, 2)], 0, 10).is_err());
    }

    #[test]
    fn parses_content_range() {
        assert_eq!(content_range_start("bytes 100-199/1000"), Some(100));
        assert_eq!(content_range_start(" bytes 0-0/*"), Some(0));
        assert_eq!(content_range_start("items 1-2/3"), None);
    }

    #[test]
    fn retryable_statuses() {
        for c in [408, 429, 500, 502, 503, 504] {
            assert!(retryable_status(c), "{c}");
        }
        for c in [400, 403, 404, 416, 501, 505] {
            assert!(!retryable_status(c), "{c}");
        }
    }

    /// The URL commands reach the HTTP reader through `RangedSourceReader`;
    /// it must hand them the batch (and the fan-out), not split it.
    #[test]
    fn ranged_source_hands_batches_to_the_http_reader() {
        use crate::commands::range_source::RangedSourceReader;
        let data: Vec<u8> = (0..=255u8).cycle().take(1 << 20).collect();
        let (url, stats) = serve_with(data, ServerMode::HonorRange, Duration::from_millis(30));
        let r = RangedSourceReader::open(&url).unwrap();
        assert!(r.concurrency() > 1);
        let ranges: Vec<(u64, u64)> = (0..8).map(|i| (i * 131_072, 64)).collect();
        r.read_many(&ranges).unwrap();
        assert!(
            stats.max_in_flight.load(Ordering::SeqCst) > 1,
            "the batch went out one range at a time"
        );
    }
}
