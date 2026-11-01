//! Shared local-or-HTTP range reader for commands that preview or execute
//! bounded reads over either a path or an HTTP(S) `.rete` URL.

use std::fs::File;
use std::io;

use rete_core::{auto_block, BlockCacheReader, RangeReader, Rete};

use crate::http::HttpRangeReader;

pub(crate) fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// The largest file a local open ever reads whole, however much memory is free.
/// Past it the eager open's own cost dominates: decoding every dictionary chunk
/// and permutation of a 1.07 GB file took 12–17 s, where the lazy open answered
/// in 40 ms. This was the fixed threshold before the open became memory-aware.
const MAX_EAGER_BYTES: u64 = 1 << 30; // 1 GiB

/// A file is read whole only when it is at most `1/EAGER_SHARE` of the memory
/// available to the process. An eager open holds the file image, its decoded
/// dictionary and every index permutation, none of it evictable: 6.8× the
/// file on the OpenCitations sample (1.6 GB for a 0.24 GB file), where a
/// 1.07 GB file read whole passed 4 GiB and was killed by a 4 GiB container.
/// A sixteenth keeps that peak under half of what is free and leaves the rest
/// to the query.
const EAGER_SHARE: u64 = 16;

/// The read-whole threshold when the available memory cannot be read (no
/// `/proc/meminfo`, no cgroup files: macOS, Windows). Conservative: the lazy
/// open is never wrong, only sometimes slower on a full scan.
const UNKNOWN_MEMORY_LAZY_ABOVE_BYTES: u64 = 256 << 20; // 256 MiB

/// The largest file a **query command** reads whole. Measured on the
/// OpenCitations sample (223 MB): the eager open cost 0.9–1.2 s and 1.2 GB of
/// RSS on every query, the lazy open 32–305 ms and 21–165 MiB, and the lazy
/// open also won every full scan (81 local jobs: 28.9 s eager, 4.8 s lazy;
/// a 16 MB file: 85 ms eager, 40 ms lazy). Below this the two cost the same:
/// a format `0x06` open reads the first 64 KiB, which on a file this small is
/// most of it.
pub(crate) const QUERY_EAGER_MAX_BYTES: u64 = 1 << 20; // 1 MiB

/// The file size above which a local `.rete` is opened lazily, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LazyThreshold {
    pub(crate) bytes: u64,
    /// What decided it, for `RETE_OPEN_DEBUG`.
    pub(crate) reason: String,
}

/// The lazy-open threshold for this process: `RETE_LOCAL_LAZY_ABOVE_MB` when
/// set (`0` forces lazy for everything), else derived from the memory actually
/// available (see [`available_memory`]) and, when given, the query's memory
/// budget.
pub(crate) fn lazy_threshold(budget: Option<u64>) -> LazyThreshold {
    let env = std::env::var("RETE_LOCAL_LAZY_ABOVE_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());
    // Only probe the memory when it is going to be used.
    let available = if env.is_some() {
        None
    } else {
        available_memory()
    };
    lazy_threshold_from(env, available, budget)
}

/// [`lazy_threshold`] with its inputs explicit (unit-testable).
///
/// - `env_mb`: `RETE_LOCAL_LAZY_ABOVE_MB`, which wins outright.
/// - `available`: bytes the process can still allocate, or `None` when unknown.
/// - `budget`: the query's `--memory-budget-mb` in bytes (`None` = no budget
///   applies): the file must also fit a quarter of it, since none of an eager
///   open's memory can be evicted to honour the budget.
pub(crate) fn lazy_threshold_from(
    env_mb: Option<u64>,
    available: Option<u64>,
    budget: Option<u64>,
) -> LazyThreshold {
    if let Some(mb) = env_mb {
        return LazyThreshold {
            bytes: mb.saturating_mul(1 << 20),
            reason: format!("RETE_LOCAL_LAZY_ABOVE_MB={mb}"),
        };
    }
    let mut bytes = MAX_EAGER_BYTES;
    let mut reason = format!("cap {} MiB", MAX_EAGER_BYTES >> 20);
    let (mem_limit, mem_reason) = match available {
        Some(avail) => (
            avail / EAGER_SHARE,
            format!("1/{EAGER_SHARE} of {} MiB available", avail >> 20),
        ),
        None => (
            UNKNOWN_MEMORY_LAZY_ABOVE_BYTES,
            "available memory unknown".to_string(),
        ),
    };
    if mem_limit < bytes {
        bytes = mem_limit;
        reason = mem_reason;
    }
    if let Some(b) = budget {
        if b / 4 < bytes {
            bytes = b / 4;
            reason = format!("1/4 of the {} MiB memory budget", b >> 20);
        }
    }
    LazyThreshold { bytes, reason }
}

/// Bytes this process can still allocate (`MemAvailable` and every cgroup
/// limit above the process), or `None` where neither can be read. The probe
/// lives in the core ([`rete_core::available_memory`]), which also sizes the
/// default query budget from it.
pub(crate) fn available_memory() -> Option<u64> {
    rete_core::available_memory()
}

/// The byte budget a query command runs under, from its `--memory-budget-mb`
/// (`0` = unlimited, `None`). Omitted, it is the default sized from the memory
/// available ([`rete_core::default_query_budget_mb`]: half of it, 256–4096
/// MiB), or `RETE_QUERY_BUDGET_MB` when that is set. The flag wins over both.
pub(crate) fn query_budget(mb: Option<u64>) -> Option<u64> {
    let mb = mb.unwrap_or_else(|| {
        let mb = rete_core::default_query_budget_mb();
        if std::env::var("RETE_OPEN_DEBUG").is_ok() {
            let why = match std::env::var(rete_core::QUERY_BUDGET_ENV) {
                Ok(v) => format!("{}={}", rete_core::QUERY_BUDGET_ENV, v.trim()),
                Err(_) => match available_memory() {
                    Some(a) => format!("half of {} MiB available, 256–4096", a >> 20),
                    None => "available memory unknown".to_string(),
                },
            };
            eprintln!("[query_budget] default {mb} MiB ({why})");
        }
        mb
    });
    match mb {
        0 => None,
        mb => Some(mb.saturating_mul(1 << 20)),
    }
}

/// [`open_local`] under an explicit memory budget (`None` = unlimited).
///
/// An eager open holds the file image, its decoded dictionary and every index
/// permutation — three to five times the file — and none of that can be
/// evicted. So on top of [`open_local`]'s memory-aware rule a file goes lazy
/// once a quarter of the budget no longer covers it, and the lazy open's
/// caches are capped to the budget. `RETE_LOCAL_LAZY_ABOVE_MB` still overrides
/// the threshold.
pub(crate) fn open_local_budget(path: &str, budget: Option<u64>) -> anyhow::Result<Rete> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let threshold = query_threshold(lazy_threshold(budget));
    debug_open("open_local_budget", path, len, &threshold);
    if len <= threshold.bytes {
        let rete = Rete::open(&std::fs::read(path)?)?;
        // A resident open has no evictable caches to cap, but a query's
        // `COUNT(DISTINCT …)` still spills past the budget's share.
        rete.set_spill_threshold(budget.map(rete_core::spill_threshold_for));
        return Ok(rete);
    }
    open_local_lazy(path, len, budget)
}

/// The query commands' threshold: the memory-aware one, capped at
/// [`QUERY_EAGER_MAX_BYTES`] unless `RETE_LOCAL_LAZY_ABOVE_MB` set it.
fn query_threshold(t: LazyThreshold) -> LazyThreshold {
    if t.reason.starts_with("RETE_LOCAL_LAZY_ABOVE_MB") || t.bytes <= QUERY_EAGER_MAX_BYTES {
        return t;
    }
    LazyThreshold {
        bytes: QUERY_EAGER_MAX_BYTES,
        reason: format!(
            "query commands read whole only up to {} KiB",
            QUERY_EAGER_MAX_BYTES >> 10
        ),
    }
}

/// Open a local `.rete` for a **query command** (`sparql`, `query`, `bgp`,
/// `why`, `cypher`, `search`, `stats`, `graphs`, `schema`, `shacl`): lazily,
/// unless the file is at most [`QUERY_EAGER_MAX_BYTES`]. A query touches the
/// directories and the payload it routes to; reading every dictionary chunk
/// and every permutation first is what made a query on a 223 MB file take a
/// second and 1.2 GB. Commands that walk the whole graph (`communities`,
/// `reach`, `reason`, `serve`) keep [`open_local`].
pub(crate) fn open_local_query(path: &str) -> anyhow::Result<Rete> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let threshold = query_threshold(lazy_threshold(None));
    debug_open("open_local_query", path, len, &threshold);
    if len <= threshold.bytes {
        return Ok(Rete::open(&std::fs::read(path)?)?);
    }
    open_local_lazy(path, len, None)
}

/// `RETE_OPEN_DEBUG`: say which way an open went and what decided it.
fn debug_open(who: &str, path: &str, len: u64, threshold: &LazyThreshold) {
    if std::env::var("RETE_OPEN_DEBUG").is_ok() {
        eprintln!(
            "[{who}] {path}: len={len} threshold={} ({}) -> {}",
            threshold.bytes,
            threshold.reason,
            if len > threshold.bytes {
                "LAZY"
            } else {
                "EAGER"
            }
        );
    }
}

/// Open a local `.rete`, reading it whole only when that is actually cheaper.
///
/// `Rete::open` takes a full file image and decodes every dictionary chunk up
/// front, so peak memory is the file plus its *decompressed* dictionary. That is
/// fine for ordinary graphs and hopeless for one carrying embedded media: a
/// 23.5 GB file whose dictionary is 23.4 GB of base64 images needs ~55 GB to
/// open, and dies, even though answering a query touches a handful of chunks.
///
/// Above the threshold we therefore go through the same lazy path the HTTP
/// reader uses — `Rete::open_ranged_lazy` over a positional-read file handle —
/// which reads the header, the section directories and the index, then faults
/// dictionary chunks in on demand. Small files keep the read-it-all path, which
/// stays faster when the whole graph is going to be touched anyway.
///
/// The threshold follows the memory actually available ([`lazy_threshold`]):
/// a file is read whole only when it is at most a sixteenth of what the
/// process can still allocate, and never past 1 GiB. Before, a fixed 1 GiB sent a
/// 1.07 GB file down the eager path inside a 4 GiB container, which killed
/// every default-open command after ~12 s; lazy, the same command took 40 ms
/// and 13 MiB.
///
/// The `BlockCacheReader` is not optional here. The engine re-reads the same
/// spans (a chunk directory during binary search, the header) many times over,
/// and a bare file handle re-does every one of them: a point lookup on a 23.5 GB
/// file read 15.9 GB and still had not finished. Block-aligned caching makes the
/// repeats free, which is what `sparql-url` has always done for the remote path.
pub(crate) fn open_local(path: &str) -> anyhow::Result<Rete> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let threshold = lazy_threshold(None);
    debug_open("open_local", path, len, &threshold);
    if len <= threshold.bytes {
        return Ok(Rete::open(&std::fs::read(path)?)?);
    }
    open_local_lazy(path, len, None)
}

/// Open a local `.rete` through the lazy RANGED reader **regardless of size** —
/// the header, section directories and index up front, then dictionary chunks
/// and tiles faulted in on demand over a positional-read file handle.
///
/// This is what `rete export` uses by default. The motivation is peak RSS: the
/// eager path (`Rete::open` over the whole file image, see [`open_local`]) holds
/// the entire file resident plus its decompressed dictionary and all six index
/// permutations, so export RSS scaled ~3 GB per GB of file — 6 GB for a 1.53 GB
/// graph, and a 52 GB crossref/datacite would need ~150 GB and OOM the 50 GB
/// Docker VM. The ranged reader never holds the raw file image or the unused
/// permutations, so a filtered export (one predicate/graph) is bounded to the
/// tiles it touches. A *full* dump still faults every dictionary chunk and those
/// stay resident (the decompressed dictionary is the floor either way), so the
/// win is largest for slices; but even a full dump avoids the raw-image and
/// spare-permutation overhead the eager open pays on top of that floor.
///
/// `--in-memory` (→ [`open_local_eager`]) forces the old whole-file load back,
/// which is faster for small files that fit in RAM.
///
/// `budget` is the total `--memory-budget-mb` in bytes (`None` = unlimited): it
/// caps the block cache proportionally here and the dictionary/tile caches on
/// the opened `Rete`, bounding peak RSS to roughly the budget (plan §5.2).
pub(crate) fn open_local_ranged(path: &str, budget: Option<u64>) -> anyhow::Result<Rete> {
    let len = std::fs::metadata(path)?.len();
    if std::env::var("RETE_OPEN_DEBUG").is_ok() {
        eprintln!("[open_local_ranged] {path}: len={len} -> LAZY");
    }
    open_local_lazy(path, len, budget)
}

/// Force the whole file resident: read the entire image and decode every
/// dictionary chunk and index permutation up front (`Rete::open`). Needs RAM
/// roughly equal to the file size; the opt-in `rete export --in-memory` path.
pub(crate) fn open_local_eager(path: &str) -> anyhow::Result<Rete> {
    if std::env::var("RETE_OPEN_DEBUG").is_ok() {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        eprintln!("[open_local_eager] {path}: len={len} -> EAGER (whole file)");
    }
    Ok(Rete::open(&std::fs::read(path)?)?)
}

/// The lazy open shared by [`open_local`] (above its size threshold) and
/// [`open_local_ranged`] (always): a positional-read file handle behind a
/// block-aligned cache, then `Rete::open_ranged_lazy`.
fn open_local_lazy(path: &str, len: u64, budget: Option<u64>) -> anyhow::Result<Rete> {
    let reader = std::sync::Arc::new(LocalRangeReader::open(path)?);
    // `RETE_BLOCK_KB` wins (0 disables), else auto-tune by length — same knob
    // and heuristic as the URL commands, so local and remote behave alike.
    let block: u64 = match std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(kb) => kb * 1024,
        None => auto_block(len),
    };
    // A finite budget caps the raw-block LRU too (its share of the split); an
    // unlimited budget keeps the reader's default cap.
    let block_cap = rete_core::split_memory_budget(budget.unwrap_or(u64::MAX)).block_cache;
    let rete = if block == 0 {
        Rete::open_ranged_lazy(reader)?
    } else {
        let mut bc = BlockCacheReader::new(reader, block);
        if block_cap != u64::MAX {
            bc = bc.with_cache_cap(block_cap);
        }
        Rete::open_ranged_lazy(std::sync::Arc::new(bc))?
    };
    // Cap the dictionary chunk cache and the index tile cache to the rest of the
    // budget (no-op when unlimited).
    rete.set_memory_budget(budget);
    Ok(rete)
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

    // Both of these must be forwarded. Without them the trait defaults apply —
    // a sequential `read_many` and a concurrency of 1 — and every batch the
    // engine hands the HTTP reader goes out one range at a time: in 1,091 of
    // 1,095 measured remote runs the round trips on the critical path equalled
    // the request count, so remote time was requests × RTT.
    fn read_many(&self, ranges: &[(u64, u64)]) -> io::Result<Vec<Vec<u8>>> {
        match self {
            Self::Local(r) => r.read_many(ranges),
            Self::Http(r) => r.read_many(ranges),
        }
    }

    fn concurrency(&self) -> usize {
        match self {
            Self::Local(r) => r.concurrency(),
            Self::Http(r) => r.concurrency(),
        }
    }
}

pub(crate) struct LocalRangeReader {
    file: File,
    len: u64,
}

impl LocalRangeReader {
    pub(crate) fn open(path: &str) -> anyhow::Result<Self> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;

    #[test]
    fn env_override_wins_over_everything() {
        let t = lazy_threshold_from(Some(0), Some(64 * GIB), Some(4096 * MIB));
        assert_eq!(t.bytes, 0, "0 forces lazy");
        let t = lazy_threshold_from(Some(5000), Some(GIB), Some(MIB));
        assert_eq!(t.bytes, 5000 * MIB, "the override is not clamped");
        assert!(
            t.reason.contains("RETE_LOCAL_LAZY_ABOVE_MB"),
            "{}",
            t.reason
        );
    }

    #[test]
    fn threshold_follows_available_memory() {
        // A 4 GiB container with ~3.8 GiB free: a sixteenth, well under 1 GiB —
        // so the 1.07 GB OpenCitations file opens lazily (it was eager, and
        // killed, under the fixed 1 GiB rule).
        let avail = 3800 * MIB;
        let t = lazy_threshold_from(None, Some(avail), None);
        assert_eq!(t.bytes, avail / 16);
        assert!(1_072_399_312 > t.bytes);
        // ...while its 0.17–0.25 GB siblings are a small enough fraction.
        assert!(t.bytes > 250_000_000 / 2, "{t:?}");
        // A roomy machine is still capped at 1 GiB.
        let t = lazy_threshold_from(None, Some(48 * GIB), None);
        assert_eq!(t.bytes, GIB);
        assert!(t.reason.contains("cap"), "{}", t.reason);
    }

    /// Query commands read a file whole only when it is tiny, however much
    /// memory there is; the environment override still wins.
    #[test]
    fn query_commands_open_lazily_above_a_mebibyte() {
        let t = query_threshold(lazy_threshold_from(None, Some(48 * GIB), None));
        assert_eq!(t.bytes, QUERY_EAGER_MAX_BYTES);
        let t = query_threshold(lazy_threshold_from(None, Some(4 * MIB), None));
        assert_eq!(t.bytes, MIB / 4, "a tighter memory rule is kept");
        let t = query_threshold(lazy_threshold_from(Some(300), Some(48 * GIB), None));
        assert_eq!(t.bytes, 300 * MIB, "RETE_LOCAL_LAZY_ABOVE_MB wins");
    }

    #[test]
    fn unknown_memory_falls_back_conservatively() {
        let t = lazy_threshold_from(None, None, None);
        assert_eq!(t.bytes, UNKNOWN_MEMORY_LAZY_ABOVE_BYTES);
        assert!(t.reason.contains("unknown"), "{}", t.reason);
    }

    #[test]
    fn budget_quarter_still_applies() {
        // 2048 MiB budget: a quarter is 512 MiB, tighter than 1/16 of 48 GiB.
        let t = lazy_threshold_from(None, Some(48 * GIB), Some(2048 * MIB));
        assert_eq!(t.bytes, 512 * MIB);
        assert!(t.reason.contains("budget"), "{}", t.reason);
        // Memory tighter than the budget: memory decides.
        let t = lazy_threshold_from(None, Some(GIB), Some(4096 * MIB));
        assert_eq!(t.bytes, GIB / 16);
    }

    /// On the Linux this suite runs on (the dev container), the probe finds
    /// something; elsewhere `None` is the documented answer.
    #[test]
    fn available_memory_is_readable_on_linux() {
        if cfg!(target_os = "linux") {
            let a = available_memory().expect("Linux exposes MemAvailable");
            assert!(a > 0);
        }
    }

    /// The URL source must hand the engine's batches to the HTTP reader, not
    /// fall back to the trait's one-at-a-time default.
    #[test]
    fn ranged_source_forwards_batches_and_concurrency() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        std::fs::write(&path, &data).unwrap();
        let r = RangedSourceReader::open(path.to_str().unwrap()).unwrap();
        assert_eq!(r.concurrency(), 1, "a local file reports no fan-out");
        let got = r.read_many(&[(10, 5), (4000, 96), (0, 0)]).unwrap();
        assert_eq!(got[0], &data[10..15]);
        assert_eq!(got[1], &data[4000..4096]);
        assert!(got[2].is_empty());
    }
}
