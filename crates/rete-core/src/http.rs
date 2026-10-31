//! An HTTP(S)-backed [`RangeReader`] for native hosts (SPEC.md §9): point it at
//! a URL and it fetches only the byte ranges a query needs via HTTP `Range`
//! requests. Behind the `http` feature, because it brings a network stack; the
//! CLI, the Python wheels and the R package all read remote files through this
//! one implementation. The browser engine has its own readers (sync XHR and
//! `fetch`), which follow the same contract.
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
//! is retried with exponential backoff; a permanent one (a `404`, a
//! `Content-Range` for the wrong offset) fails at once. Any range of a batch
//! failing fails the batch, and the ranges still queued are not started.
//!
//! **A host that ignores `Range`** answers `200` with the whole body (a plain
//! static server, a proxy that strips the header, a CDN serving a cached full
//! object). That body has crossed the network by the time the reader sees it,
//! and every later range request would download it again, so when it is the
//! file — exactly the length the open measured — and at most
//! [`FetchPolicy::whole_body_cap`] ([`WHOLE_BODY_CAP`], 256 MiB, the browser
//! readers' cap) it is kept, and every read from then on is a slice of it. A
//! larger body is refused by name, without being read: the reader never holds
//! more than the cap of a body nobody asked for. Taking the first `len` bytes
//! of a `200` as if they were the range is the one thing it never does.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::RangeReader;

/// The largest whole body (a `200` to a range request) the reader keeps and
/// serves reads from — the same 256 MiB the browser readers keep.
pub const WHOLE_BODY_CAP: u64 = 256 << 20;

/// How [`HttpRangeReader`] fetches: fan-out, coalescing, retries, and the
/// whole-body cap. [`FetchPolicy::from_env`] reads it once when a reader opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchPolicy {
    /// Requests in flight at once in `read_many` (`RETE_HTTP_CONCURRENCY`,
    /// 1–64; `1` fetches a batch sequentially). Also what the reader reports
    /// to the planner as its [`RangeReader::concurrency`].
    pub concurrency: usize,
    /// Two ranges of one batch at most this many bytes apart are fetched as
    /// one request (`RETE_HTTP_COALESCE_KB`; `0` merges only touching or
    /// overlapping ranges).
    pub coalesce_gap: u64,
    /// Coalescing never grows a request past this many bytes, so one huge span
    /// does not serialize what could have gone out in parallel.
    pub max_span: u64,
    /// Extra attempts for a transiently failing request (`RETE_HTTP_RETRIES`,
    /// at most 10; `0` = no retry).
    pub retries: u32,
    /// The first backoff; it quadruples per retry, capped at 2 s.
    pub backoff_ms: u64,
    /// `RETE_HTTP_TRACE`: one stderr line per read and per batch, to see which
    /// ranges a query needed and which of them went out together.
    pub trace: bool,
    /// The largest whole body a host that ignores `Range` may send and still
    /// have it kept ([`WHOLE_BODY_CAP`]); a larger one is refused by name.
    pub whole_body_cap: u64,
    /// Bytes the open reads first, as its length probe ([`OPEN_READ`]); `0`
    /// probes with `HEAD` instead and keeps nothing.
    pub open_read: u64,
}

impl FetchPolicy {
    /// 16 in flight matches the wasm client's fetch-worker pool and the Node
    /// client's `reteReadMany` cap; a CDN or S3 serves that many concurrent
    /// range reads happily.
    pub const DEFAULT: FetchPolicy = FetchPolicy {
        concurrency: 16,
        coalesce_gap: 32 << 10,
        max_span: 8 << 20,
        retries: 2,
        backoff_ms: 100,
        trace: false,
        whole_body_cap: WHOLE_BODY_CAP,
        open_read: OPEN_READ,
    };

    /// [`FetchPolicy::DEFAULT`] with the `RETE_HTTP_*` overrides applied.
    pub fn from_env() -> Self {
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
            whole_body_cap: d.whole_body_cap,
            open_read: d.open_read,
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

impl Default for FetchPolicy {
    fn default() -> Self {
        Self::DEFAULT
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

/// The total of a `Content-Range: bytes a-b/N` value (`None` for `/*`).
fn content_range_total(v: &str) -> Option<u64> {
    v.rsplit('/').next()?.trim().parse().ok()
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

/// The refusal for a whole body over the cap: named, so it reads as the host's
/// limitation rather than as a broken file.
fn whole_body_refusal(url: &str, bytes: Option<u64>, cap: u64) -> std::io::Error {
    let size = match bytes {
        Some(n) => format!("all {n} bytes"),
        None => format!("more than {cap} bytes"),
    };
    std::io::Error::other(format!(
        "the host ignored the Range header and sent {size} of {url} (status 200 instead \
         of 206 Partial Content); a full body over {} MiB is not kept in memory — host the \
         file where HTTP range requests work",
        cap >> 20
    ))
}

/// One read served from a whole-resource body, bounds-checked: the body is
/// whatever the host sent, so a read past its end is a clean error.
fn slice_whole(whole: &[u8], offset: u64, len: u64, url: &str) -> std::io::Result<Vec<u8>> {
    let end = offset
        .checked_add(len)
        .filter(|&end| end <= whole.len() as u64)
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "range {offset}+{len} is past the end of the {} bytes {url} sent",
                whole.len()
            ))
        })?;
    Ok(whole[offset as usize..end as usize].to_vec())
}

/// Read a `200` body whole, up to `cap` bytes: `Ok(None)` when it is longer
/// (the caller refuses it), never more than `cap + 1` bytes in memory.
fn read_capped(resp: ureq::Response, cap: u64, what: &str) -> Result<Option<Vec<u8>>, Failure> {
    let declared = resp
        .header("content-length")
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > cap) {
        return Ok(None); // refused unread: dropping the response closes it
    }
    let mut buf = Vec::with_capacity(declared.unwrap_or(0).min(cap) as usize);
    resp.into_reader()
        .take(cap.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| Failure::Transient(std::io::Error::other(format!("{e} ({what})"))))?;
    if buf.len() as u64 > cap {
        return Ok(None);
    }
    if declared.is_some_and(|n| n != buf.len() as u64) {
        return Err(Failure::Transient(std::io::Error::other(format!(
            "short body: got {} of the {} bytes {what} declared",
            buf.len(),
            declared.unwrap_or(0)
        ))));
    }
    Ok(Some(buf))
}

/// What opening learnt: the length, and the whole file when the host ignored
/// `Range` on the length probe itself.
struct Probe {
    len: u64,
    whole: Option<Vec<u8>>,
    /// The file's first bytes, when the length came from the opening ranged
    /// GET: later reads inside them cost nothing.
    prefix: Option<Vec<u8>>,
}

/// What the open reads first: the same 64 KiB a `0x06` reader plans from, so
/// the length probe *is* the first read and no `HEAD` is sent.
pub const OPEN_READ: u64 = 64 * 1024;

/// An HTTP(S) [`RangeReader`]: kept-alive connections, parallel and coalesced
/// batches, retries, and the bounded whole-body fallback for hosts that ignore
/// `Range` (see the module docs).
pub struct HttpRangeReader {
    url: String,
    len: u64,
    agent: ureq::Agent,
    policy: FetchPolicy,
    /// Sent with every request: auth tokens, a custom `User-Agent`.
    headers: Vec<(String, String)>,
    /// The whole resource, once a host has answered a range request with `200`
    /// and a body the reader kept (see the module docs).
    whole: OnceLock<Vec<u8>>,
    /// Held while a whole body is read, so concurrent `200`s in one batch keep
    /// one body, not one per worker.
    adopting: Mutex<()>,
    /// The first [`OPEN_READ`] bytes, fetched by the open (empty when the
    /// length came from a `HEAD`).
    prefix: Vec<u8>,
}

impl HttpRangeReader {
    /// Open `url` under [`FetchPolicy::from_env`], with no extra headers.
    pub fn open(url: &str) -> std::io::Result<Self> {
        Self::open_with(url, FetchPolicy::from_env())
    }

    /// [`open`](Self::open) under an explicit fetch policy.
    pub fn open_with(url: &str, policy: FetchPolicy) -> std::io::Result<Self> {
        Self::open_with_headers(url, Vec::new(), policy)
    }

    /// Open `url`, sending `headers` with every request. The length comes from
    /// a `HEAD`'s `Content-Length`; a host that omits it, or refuses `HEAD`
    /// (a presigned GET-only URL), is asked with a `bytes=0-0` GET instead,
    /// whose `Content-Range` carries the total — and a `200` to that GET is the
    /// whole file, kept under the whole-body rules.
    pub fn open_with_headers(
        url: &str,
        headers: Vec<(String, String)>,
        policy: FetchPolicy,
    ) -> std::io::Result<Self> {
        let agent = ureq::AgentBuilder::new()
            // Keep a batch's worth of connections open between batches.
            .max_idle_connections(policy.concurrency.max(4) * 2)
            .max_idle_connections_per_host(policy.concurrency)
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(120))
            .build();
        let mut reader = HttpRangeReader {
            url: url.to_string(),
            len: 0,
            agent,
            policy,
            headers,
            whole: OnceLock::new(),
            adopting: Mutex::new(()),
            prefix: Vec::new(),
        };
        let probe = reader.probe_len()?;
        reader.len = probe.len;
        if let Some(body) = probe.whole {
            let _ = reader.whole.set(body);
        }
        if let Some(p) = probe.prefix {
            reader.prefix = p;
        }
        Ok(reader)
    }

    /// The URL this reader fetches.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether the host ignored `Range` and every read is now served from the
    /// whole body held in memory.
    pub fn serves_whole_body(&self) -> bool {
        self.whole.get().is_some()
    }

    fn request(&self, method: &str) -> ureq::Request {
        let mut req = self.agent.request(method, &self.url);
        for (k, v) in &self.headers {
            req = req.set(k, v);
        }
        req
    }

    /// Run `call` with the policy's retries for transient failures.
    fn with_retries<T>(&self, mut call: impl FnMut() -> Result<T, Failure>) -> std::io::Result<T> {
        let mut attempt = 0;
        loop {
            match call() {
                Ok(v) => return Ok(v),
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

    /// The resource length: `HEAD`'s `Content-Length`, else a `bytes=0-0`
    /// GET's `Content-Range` total, else — when that GET is a `200` — the
    /// whole body's length, with the body kept.
    fn probe_len(&self) -> std::io::Result<Probe> {
        // The first read doubles as the length probe: `bytes=0-65535` answers
        // with the total in `Content-Range` and the bytes every open reads
        // first anyway. Only when that gives no length is `HEAD` asked.
        let open_read = self.policy.open_read;
        let first_what = format!("bytes 0-{} of {}", open_read.saturating_sub(1), self.url);
        let first_cap = self.policy.whole_body_cap;
        let first = self.with_retries(|| {
            if open_read == 0 {
                return Ok(None);
            }
            let resp = match self
                .request("GET")
                .set("Range", &format!("bytes=0-{}", open_read - 1))
                .call()
            {
                Ok(r) => r,
                // A range the server cannot satisfy (an empty file) or refuses
                // outright: let the HEAD path decide.
                Err(ureq::Error::Status(code, _)) if !retryable_status(code) => {
                    return Ok(None);
                }
                Err(e) => return Err(classify(e, &first_what)),
            };
            match resp.status() {
                206 => {
                    let cr = resp.header("content-range").map(str::to_string);
                    let total = cr.as_deref().and_then(content_range_total);
                    let start = cr.as_deref().and_then(content_range_start);
                    let Some(len) = total.filter(|_| start == Some(0)) else {
                        return Ok(None);
                    };
                    let mut body = Vec::new();
                    resp.into_reader()
                        .take(open_read)
                        .read_to_end(&mut body)
                        .map_err(Failure::Transient)?;
                    if body.len() as u64 != open_read.min(len) {
                        return Err(Failure::Transient(std::io::Error::other(format!(
                            "short body for {first_what}"
                        ))));
                    }
                    Ok(Some(Probe {
                        len,
                        whole: None,
                        prefix: Some(body),
                    }))
                }
                200 => match read_capped(resp, first_cap, &first_what)? {
                    Some(body) if body.is_empty() => Ok(None),
                    Some(body) => Ok(Some(Probe {
                        len: body.len() as u64,
                        whole: Some(body),
                        prefix: None,
                    })),
                    None => Err(Failure::Permanent(whole_body_refusal(
                        &self.url, None, first_cap,
                    ))),
                },
                _ => Ok(None),
            }
        })?;
        if let Some(p) = first {
            return Ok(p);
        }
        let head_what = format!("HEAD {}", self.url);
        let head = self.with_retries(|| {
            self.request("HEAD")
                .call()
                .map_err(|e| classify(e, &head_what))
        });
        let head_err = match head {
            Ok(resp) => match resp
                .header("content-length")
                .and_then(|v| v.trim().parse::<u64>().ok())
            {
                Some(len) => {
                    return Ok(Probe {
                        len,
                        whole: None,
                        prefix: None,
                    })
                }
                None => "no Content-Length".to_string(),
            },
            Err(e) => e.to_string(),
        };
        let what = format!("bytes 0-0 of {}", self.url);
        let cap = self.policy.whole_body_cap;
        let probe = self.with_retries(|| {
            let resp = self
                .request("GET")
                .set("Range", "bytes=0-0")
                .call()
                .map_err(|e| classify(e, &what))?;
            match resp.status() {
                206 => Ok(resp
                    .header("content-range")
                    .and_then(content_range_total)
                    .map(|len| Probe {
                        len,
                        whole: None,
                        prefix: None,
                    })),
                200 => match read_capped(resp, cap, &what)? {
                    Some(body) if body.is_empty() => {
                        Err(Failure::Permanent(std::io::Error::other(format!(
                            "{} answered 200 with an empty body",
                            self.url
                        ))))
                    }
                    Some(body) => Ok(Some(Probe {
                        len: body.len() as u64,
                        whole: Some(body),
                        prefix: None,
                    })),
                    None => Err(Failure::Permanent(whole_body_refusal(&self.url, None, cap))),
                },
                s => Err(Failure::Permanent(std::io::Error::other(format!(
                    "status {s} for {what}"
                )))),
            }
        })?;
        probe.ok_or_else(|| {
            std::io::Error::other(format!(
                "could not determine the file length of {}: {head_err} on HEAD, and no \
                 Content-Range total on a ranged GET",
                self.url
            ))
        })
    }

    /// The bytes of `offset..offset+len` when the opening read holds them.
    fn prefix_slice(&self, offset: u64, len: u64) -> Option<Vec<u8>> {
        let end = offset.checked_add(len)?;
        (end <= self.prefix.len() as u64)
            .then(|| self.prefix[offset as usize..end as usize].to_vec())
    }

    /// A range past the length the open measured is an error before any
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

    /// A `200` answered a range request: keep its body as the whole file when
    /// it is one (see the module docs). Concurrent `200`s of one batch keep
    /// one body — the first to take the lock reads it, the rest drop theirs.
    fn adopt_whole(&self, resp: ureq::Response, what: &str) -> Result<(), Failure> {
        let _guard = self.adopting.lock().unwrap_or_else(|p| p.into_inner());
        if self.whole.get().is_some() {
            return Ok(());
        }
        let cap = self.policy.whole_body_cap;
        let declared = resp
            .header("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        if let Some(n) = declared {
            if n > cap {
                return Err(Failure::Permanent(whole_body_refusal(
                    &self.url,
                    Some(n),
                    cap,
                )));
            }
            if n != self.len {
                return Err(Failure::Permanent(std::io::Error::other(format!(
                    "the host ignored the Range header and sent {n} bytes of {}, which is not \
                     the {} bytes it reported for the file",
                    self.url, self.len
                ))));
            }
        }
        let body = read_capped(resp, cap, what)?
            .ok_or_else(|| Failure::Permanent(whole_body_refusal(&self.url, None, cap)))?;
        if body.len() as u64 != self.len {
            return Err(Failure::Permanent(std::io::Error::other(format!(
                "the host ignored the Range header and sent {} bytes of {}, which is not the \
                 {} bytes it reported for the file",
                body.len(),
                self.url,
                self.len
            ))));
        }
        if self.policy.trace {
            eprintln!(
                "[http] {} ignored Range: every read is served from its {}-byte body",
                self.url,
                body.len()
            );
        }
        let _ = self.whole.set(body);
        Ok(())
    }

    /// One GET for `[offset, offset+len)` (`len > 0`), classified on failure.
    fn fetch_once(&self, offset: u64, len: u64) -> Result<Vec<u8>, Failure> {
        let end = offset + len - 1; // HTTP ranges are inclusive
        let what = format!("bytes {offset}-{end} of {}", self.url);
        let resp = self
            .request("GET")
            .set("Range", &format!("bytes={offset}-{end}"))
            .call()
            .map_err(|e| classify(e, &what))?;
        // A `200 OK` means the host ignored `Range` and is sending the whole
        // body from offset 0 — never the requested slice. Kept as the file when
        // it is one and fits the cap; refused by name otherwise.
        if resp.status() == 200 {
            self.adopt_whole(resp, &what)?;
            let whole = self.whole.get().expect("adopted above");
            return slice_whole(whole, offset, len, &self.url).map_err(Failure::Permanent);
        }
        if resp.status() != 206 {
            return Err(Failure::Permanent(std::io::Error::other(format!(
                "server answered status {} (expected 206 Partial Content) for {what}; \
                 the host must support HTTP range requests",
                resp.status()
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

    /// [`fetch_once`](Self::fetch_once) with the policy's retries; a read
    /// after the whole body was kept is a slice of it.
    fn fetch(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if let Some(whole) = self.whole.get() {
            return slice_whole(whole, offset, len, &self.url);
        }
        self.with_retries(|| self.fetch_once(offset, len))
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
        if let Some(b) = self.prefix_slice(offset, len) {
            return Ok(b);
        }
        if self.policy.trace && self.whole.get().is_none() {
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
        if let Some(whole) = self.whole.get() {
            return ranges
                .iter()
                .map(|&(o, l)| slice_whole(whole, o, l, &self.url))
                .collect();
        }
        if ranges
            .iter()
            .any(|&(o, l)| l > 0 && self.prefix_slice(o, l).is_some())
        {
            // Serve what the opening read already holds; fetch the rest.
            let rest: Vec<(u64, u64)> = ranges
                .iter()
                .copied()
                .filter(|&(o, l)| l > 0 && self.prefix_slice(o, l).is_none())
                .collect();
            let mut fetched = if rest.is_empty() {
                Vec::new()
            } else {
                self.read_many(&rest)?
            }
            .into_iter();
            return ranges
                .iter()
                .map(|&(o, l)| {
                    if l == 0 {
                        Ok(Vec::new())
                    } else if let Some(b) = self.prefix_slice(o, l) {
                        Ok(b)
                    } else {
                        fetched.next().ok_or_else(|| {
                            std::io::Error::other("read_many left a range unfetched")
                        })
                    }
                })
                .collect();
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
        /// Ignores `Range` and sends the body with no `Content-Length`, ending
        /// it by closing the connection.
        IgnoreRangeUnsized,
        /// A range host whose `HEAD` carries no `Content-Length` (some object
        /// stores): the length must come from a ranged GET.
        HeadWithoutLength,
        /// `HEAD` without a length AND `Range` ignored: the length probe itself
        /// receives the whole file.
        HeadWithoutLengthIgnoreRange,
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
        /// A real range host that answers `401` unless the request carries
        /// `x-token: secret`.
        RequireHeader,
    }

    /// What the server saw: GETs served, and the most it had in flight at once.
    ///
    /// A GET is in flight from the moment its handler counts it until the
    /// moment it starts to answer — not until the handler thread finishes. A
    /// client may send its next request as soon as it has read the answer's
    /// Content-Length bytes, so counting until after the write measured the
    /// handler's tail: a strictly sequential client was sometimes counted
    /// twice (2 for `concurrency_one_is_sequential` in CI on #362; 5 for 4 in
    /// `read_many_overlaps_requests_up_to_the_bound` under load).
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
        let authorized = text
            .lines()
            .any(|l| l.eq_ignore_ascii_case("x-token: secret"));

        let total = data.len();
        let status_only = |stream: &mut std::net::TcpStream, line: &str| {
            let _ = stream.write_all(
                format!("HTTP/1.1 {line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        };
        if matches!(mode, ServerMode::RequireHeader) && !authorized {
            status_only(&mut stream, "401 Unauthorized");
            return;
        }
        if is_head {
            let h = if matches!(
                mode,
                ServerMode::HeadWithoutLength | ServerMode::HeadWithoutLengthIgnoreRange
            ) {
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string()
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                )
            };
            let _ = stream.write_all(h.as_bytes());
            return;
        }
        let n = st.gets.fetch_add(1, Ordering::SeqCst) + 1;
        let now = st.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        st.max_in_flight.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(delay);
        // Out of flight BEFORE the first byte of the answer: the client cannot
        // send its next request until it has read this one, so the decrement
        // happens before that request is counted (see `Stats`).
        st.in_flight.fetch_sub(1, Ordering::SeqCst);
        let ignores_range = matches!(
            mode,
            ServerMode::IgnoreRange
                | ServerMode::IgnoreRangeUnsized
                | ServerMode::HeadWithoutLengthIgnoreRange
        );
        match (mode, range) {
            (ServerMode::Error500, _) => status_only(&mut stream, "500 Internal Server Error"),
            (ServerMode::NotFound, _) => status_only(&mut stream, "404 Not Found"),
            (ServerMode::Unavailable(k), _) if n <= k => {
                status_only(&mut stream, "503 Service Unavailable")
            }
            (ServerMode::FailAt(at), Some(r)) if r.starts_with(&format!("{at}-")) => {
                status_only(&mut stream, "404 Not Found")
            }
            (ServerMode::IgnoreRangeUnsized, _) => {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(data);
            }
            (_, Some(r)) if !ignores_range => {
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
    }

    /// The default policy with quick retries, so failure tests stay fast.
    /// The read-path tests probe with `HEAD` (no opening read), so every
    /// range they ask for reaches the test server.
    fn quick() -> FetchPolicy {
        FetchPolicy {
            backoff_ms: 1,
            open_read: 0,
            ..FetchPolicy::DEFAULT
        }
    }

    /// The default open: one `bytes=0-65535` GET is both the length probe and
    /// the first read; no `HEAD`, and later reads inside it never leave memory.
    #[test]
    fn the_open_reads_first_and_sends_no_head() {
        let data: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
        let (url, stats) = serve_with(data.clone(), ServerMode::HeadWithoutLength, Duration::ZERO);
        let policy = FetchPolicy {
            backoff_ms: 1,
            ..FetchPolicy::DEFAULT
        };
        let r = HttpRangeReader::open_with(&url, policy).unwrap();
        assert_eq!(r.len(), 100_000);
        assert_eq!(stats.gets.load(Ordering::SeqCst), 1, "the open is one GET");
        assert_eq!(r.read_at(1000, 24).unwrap(), &data[1000..1024]);
        assert_eq!(
            r.read_many(&[(0, 16), (60_000, 8)]).unwrap()[1],
            &data[60_000..60_008]
        );
        assert_eq!(
            stats.gets.load(Ordering::SeqCst),
            1,
            "served from the first read"
        );
        assert_eq!(r.read_at(70_000, 4).unwrap(), &data[70_000..70_004]);
        assert_eq!(
            stats.gets.load(Ordering::SeqCst),
            2,
            "past it: one more GET"
        );
        // A file shorter than the first read is read whole by the open.
        let small: Vec<u8> = (0..100u8).collect();
        let (url, stats) = serve_with(small.clone(), ServerMode::HonorRange, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, policy).unwrap();
        assert_eq!(r.len(), 100);
        assert_eq!(r.read_at(90, 10).unwrap(), &small[90..]);
        assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
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
        assert!(!r.serves_whole_body());
    }

    /// Proves the `https://` transport works end-to-end against a real host that
    /// honors Range. Network-dependent, so `#[ignore]`d — CI never touches the
    /// network. Run on demand: `cargo test -p rete-core --features http -- --ignored https`.
    #[test]
    #[ignore = "hits the network (httpbin); run with --ignored"]
    fn reads_ranges_over_https() {
        let url = "https://httpbin.org/range/2048";
        let r = HttpRangeReader::open(url).unwrap();
        assert!(r.len() >= 2048);
        let chunk = r.read_at(100, 16).unwrap();
        assert_eq!(chunk.len(), 16);
    }

    /// G6: a host that ignores `Range` answers `200` with the whole body. It is
    /// never mis-read as the requested slice: under the cap it is kept, and
    /// every read after it — single or batched — is a slice of it, with no
    /// further request.
    #[test]
    fn keeps_the_whole_body_of_a_host_that_ignores_range() {
        let data: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let (url, stats) = serve_with(data.clone(), ServerMode::IgnoreRange, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        assert_eq!(r.len(), 5000);
        assert_eq!(r.read_at(50, 10).unwrap(), &data[50..60]);
        assert!(r.serves_whole_body());
        assert_eq!(r.read_at(4990, 10).unwrap(), &data[4990..5000]);
        let got = r.read_many(&[(0, 4), (3000, 100), (7, 0)]).unwrap();
        assert_eq!(got[0], &data[0..4]);
        assert_eq!(got[1], &data[3000..3100]);
        assert!(got[2].is_empty());
        assert_eq!(
            stats.gets.load(Ordering::SeqCst),
            1,
            "one download, then memory"
        );
        // Bounds still hold against the length the open measured.
        assert!(r.read_at(4999, 2).is_err());
    }

    /// The same for a body without `Content-Length` (ended by closing the
    /// connection): read, measured, kept.
    #[test]
    fn keeps_an_unsized_whole_body_too() {
        let data: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        let (url, stats) = serve_with(data.clone(), ServerMode::IgnoreRangeUnsized, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        assert_eq!(
            r.read_many(&[(10, 5), (2000, 5)]).unwrap()[1],
            &data[2000..2005]
        );
        assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
    }

    /// Above the cap the body is refused by name — permanently, so it is not
    /// downloaded again by a retry — and with a declared length, unread.
    #[test]
    fn refuses_a_whole_body_over_the_cap() {
        let small_cap = FetchPolicy {
            whole_body_cap: 1000,
            ..quick()
        };
        let data: Vec<u8> = (0..=255u8).cycle().take(4000).collect();
        for mode in [ServerMode::IgnoreRange, ServerMode::IgnoreRangeUnsized] {
            let (url, stats) = serve_with(data.clone(), mode, Duration::ZERO);
            let r = HttpRangeReader::open_with(&url, small_cap).unwrap();
            let err = r.read_at(50, 10).unwrap_err().to_string();
            assert!(
                err.contains("ignored the Range header") && err.contains("not kept in memory"),
                "{err}"
            );
            assert_eq!(stats.gets.load(Ordering::SeqCst), 1, "permanent: no retry");
            assert!(!r.serves_whole_body());
            // A batch is refused the same way.
            assert!(r.read_many(&[(0, 1), (2000, 1)]).is_err());
        }
        // The real cap is the browser readers'.
        assert_eq!(FetchPolicy::DEFAULT.whole_body_cap, 256 << 20);
        let msg = whole_body_refusal("u", Some(300 << 20), WHOLE_BODY_CAP).to_string();
        assert!(
            msg.contains("256 MiB") && msg.contains("status 200"),
            "{msg}"
        );
    }

    /// A `HEAD` without a length is answered by a ranged GET's
    /// `Content-Range`; and when that GET is itself a `200`, the open keeps the
    /// file it was sent and no read ever goes out again.
    #[test]
    fn measures_the_length_without_a_head_content_length() {
        let data: Vec<u8> = (0..=255u8).cycle().take(2048).collect();
        let (url, stats) = serve_with(data.clone(), ServerMode::HeadWithoutLength, Duration::ZERO);
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        assert_eq!(r.len(), 2048);
        assert_eq!(r.read_at(100, 8).unwrap(), &data[100..108]);
        assert!(!r.serves_whole_body());
        assert_eq!(stats.gets.load(Ordering::SeqCst), 2, "probe + read");

        let (url, stats) = serve_with(
            data.clone(),
            ServerMode::HeadWithoutLengthIgnoreRange,
            Duration::ZERO,
        );
        let r = HttpRangeReader::open_with(&url, quick()).unwrap();
        assert_eq!(r.len(), 2048);
        assert!(r.serves_whole_body());
        assert_eq!(r.read_at(2000, 48).unwrap(), &data[2000..2048]);
        assert_eq!(
            stats.gets.load(Ordering::SeqCst),
            1,
            "the probe was the file"
        );
    }

    /// Extra headers ride on every request, the length probe included.
    #[test]
    fn sends_the_extra_headers_on_every_request() {
        let data: Vec<u8> = (0..100u8).collect();
        let url = serve(data.clone(), ServerMode::RequireHeader);
        assert!(HttpRangeReader::open_with(&url, quick()).is_err());
        let headers = vec![("x-token".to_string(), "secret".to_string())];
        let r = HttpRangeReader::open_with_headers(&url, headers, quick()).unwrap();
        assert_eq!(r.len(), 100);
        assert_eq!(r.read_many(&[(0, 2), (90, 10)]).unwrap()[1], &data[90..100]);
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
        assert_eq!(content_range_total("bytes 0-0/1000"), Some(1000));
        assert_eq!(content_range_total("bytes 0-0/*"), None);
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
}
