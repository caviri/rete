//! Build conditions — the **when / by what / how** record of a `.rete` build,
//! stored in the file's kind-7 `BuildInfo` section (see
//! `rete_core::header::SectionKind::BuildInfo`).
//!
//! The Dataset Card answers *what the data is*; this section answers *how this
//! particular file came to be*: a build timestamp, the builder version, the
//! build parameters that shaped the result, and measured cost figures for the
//! card's starter queries. All of that is per-build — two builds of identical
//! data legitimately differ here — so the section lives **outside the content
//! hash** and never perturbs the reproducible blake3 the card folds into.
//!
//! Cost figures come in two kinds, deliberately kept together but labelled
//! apart (issue #153): `bytes`/`requests` are **portable** — a property of the
//! file layout and the query, identical from disk, R2 or GitHub Pages — while
//! `debug_ms` is a **reference measurement from one machine** and is named and
//! contextualized so it cannot be quoted as a property of the file.

use serde::{Deserialize, Serialize};

use rete_core::{eval_query, CountingReader, QueryOutput, RangeReader, Rete};

use super::card::ExampleQuery;

/// Schema version of the build-info JSON (bumped on incompatible change).
pub(crate) const BUILD_INFO_SCHEMA: u8 = 1;

/// The build-conditions record. Every field is `#[serde(default)]`-friendly so
/// newer records keep deserializing in older readers and vice versa.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BuildInfo {
    /// Build-info schema version (this file: [`BUILD_INFO_SCHEMA`]).
    #[serde(default)]
    pub schema: u8,
    /// When the build wrote the file (RFC 3339 UTC, whole seconds). Honors
    /// `SOURCE_DATE_EPOCH` for reproducible pipelines. Distinct from the
    /// card's curated `created`/`source_date`, which describe the *data*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_at: Option<String>,
    /// The binary that wrote the file, e.g. `rete-cli 0.3.2`. The gap this
    /// closes: a misbehaving external file used to carry no way to tell which
    /// `rete` had written it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builder: Option<String>,
    /// The build parameters that shaped the result.
    #[serde(default, skip_serializing_if = "BuildParams::is_empty")]
    pub params: BuildParams,
    /// Measured starter-query costs (only in-memory `rete build` records these;
    /// memory-bounded/merge builds carry no starter queries to measure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_costs: Option<QueryCosts>,
    /// Starter queries this build generated, **ran**, and then refused to ship
    /// because the run showed they were worthless on this very file. Empty on
    /// a healthy build — and the only surviving record of the drop, so the
    /// evidence outlives the terminal the build scrolled past.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_queries: Vec<DroppedQuery>,
}

/// The flags in force for this build. `command` names the CLI path taken.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BuildParams {
    /// CLI entry point: `build`, `build --memory-budget-mb`, `merge`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_pyramid: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub text_index: bool,
    /// `--materialize`: OWL RL/RDFS entailments were baked into the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub materialize: bool,
    /// `--reason`: the coherence verdict was stamped into the card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reason: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pyramid_algo: Option<String>,
    /// `--memory-budget-mb` for the external build (None = in-memory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_budget_mb: Option<u64>,
    /// Section codec the writer used (`zstd` / `none`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// The `CARD_TOP_N` cap in force when the card's lists were derived — the
    /// number `truncated: true` was hinting at without stating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_top_n: Option<u32>,
}

impl BuildParams {
    fn is_empty(&self) -> bool {
        *self == BuildParams::default()
    }
}

/// Measured costs for the card's starter queries, with the context that makes
/// the timing interpretable (engine, transport, one machine).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct QueryCosts {
    /// What produced the numbers — without this the `debug_ms` column is the
    /// misleading form the issue warns about.
    pub context: CostContext,
    /// Per-query figures, in the card's query order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<QueryCost>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CostContext {
    /// Engine that ran the measurement (same binary as `builder`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Transport the reads went through. Build-time measurement runs against
    /// the in-memory image with **logical range reads and no block cache**, so
    /// `bytes`/`requests` are pure layout properties — a block-caching remote
    /// client coalesces requests and rounds bytes up to blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// The caveat, stored where the numbers are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One starter query's measured cost. Each run is **cold**: a fresh lazy open
/// per query, so the figures are what a stateless remote client would pay to
/// answer just that query.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct QueryCost {
    /// The starter query's stable id (`ov-triples`, …).
    pub id: String,
    /// Bytes fetched (logical range reads), open included. Portable.
    pub bytes: u64,
    /// Logical range requests, open included. Portable.
    pub requests: u64,
    /// Result rows (SELECT rows; 1 for ASK; constructed triples).
    pub rows: u64,
    /// Wall-clock milliseconds on the build machine — a debug reference, NOT a
    /// property of the file. Interpret with `context` and the byte figures
    /// ("N ms for B bytes in R requests" travels; "N ms" does not).
    pub debug_ms: u64,
}

/// A starter query the build measured and then did not ship.
///
/// The generator decides what a card *should* carry from the profile; the
/// build then runs each of those queries against the finished file, so for a
/// carded build emptiness stops being an inference and becomes an observation.
/// An observation of "this answers nothing" is the end of the argument: the
/// query is dropped from the card, and this row is what says so afterwards.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DroppedQuery {
    /// The starter query's stable id (`sp-within`, …).
    pub id: String,
    /// What the run showed, in the words that justify the drop.
    pub why: String,
    /// True when the template behind `id` *claimed* the query could not come
    /// back empty and it did anyway. That is a defect in the generator's static
    /// rule, not a fact about the data — flagged here so it is findable in a
    /// published file long after the build log is gone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub contradicts_claim: bool,
}

impl BuildInfo {
    pub(crate) fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("BuildInfo serializes")
    }

    pub(crate) fn from_json_bytes(b: &[u8]) -> anyhow::Result<Self> {
        serde_json::from_slice(b).map_err(|e| anyhow::anyhow!("malformed build info: {e}"))
    }
}

/// `rete-cli <version>` — the builder identity stamped into every build info.
pub(crate) fn builder_version() -> String {
    format!("rete-cli {}", env!("CARGO_PKG_VERSION"))
}

/// Human name of a section codec id from a file header — the truthful source
/// for the `codec` param is the built image itself, not a compile-time guess.
pub(crate) fn codec_name(codec: u8) -> &'static str {
    match codec {
        rete_core::CODEC_ZSTD => "zstd",
        rete_core::CODEC_NONE => "none",
        _ => "unknown",
    }
}

/// A fresh [`BuildInfo`] carrying the timestamp, builder, and parameters —
/// query costs are measured separately ([`measure_query_costs`]) because they
/// need the finished image.
pub(crate) fn new_build_info(params: BuildParams) -> BuildInfo {
    BuildInfo {
        schema: BUILD_INFO_SCHEMA,
        built_at: Some(now_rfc3339()),
        builder: Some(builder_version()),
        params,
        query_costs: None,
        dropped_queries: Vec::new(),
    }
}

/// RFC 3339 UTC (`2026-08-04T12:34:56Z`), honoring `SOURCE_DATE_EPOCH` so a
/// reproducibility-minded pipeline can pin it. No chrono dependency: days →
/// civil date via the standard Gregorian algorithm.
pub(crate) fn now_rfc3339() -> String {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    epoch_to_rfc3339(secs)
}

/// Format an epoch second as RFC 3339 UTC. (Howard Hinnant's `civil_from_days`.)
fn epoch_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem / 60) % 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// An owning in-memory [`RangeReader`] — `open_ranged_lazy` needs `'static`,
/// which the borrowing `SliceReader` cannot provide.
struct ImageReader(std::sync::Arc<Vec<u8>>);

impl RangeReader for ImageReader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let start = offset as usize;
        let end = start
            .checked_add(len as usize)
            .filter(|&e| e <= self.0.len())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "range out of bounds")
            })?;
        Ok(self.0[start..end].to_vec())
    }
}

/// A [`RangeReader`] that refuses to serve more than `budget` bytes.
///
/// The measurement has to actually run the query, and on a remote file that
/// means paying for every byte it reads. `switzerland-fedlex`'s own build
/// record says eight of its ten starter queries read ~1.02 GB each — measuring
/// that card over HTTP unguarded is an 8 GB download nobody asked for. Past the
/// budget the read fails, the query fails with it, and the failure is reported
/// **with the bytes spent**: "costs more than N MB" is itself a usable answer.
pub(crate) struct BudgetReader<R> {
    inner: R,
    budget: u64,
    spent: std::sync::atomic::AtomicU64,
}

impl<R: RangeReader> BudgetReader<R> {
    pub(crate) fn new(inner: R, budget: u64) -> Self {
        Self {
            inner,
            budget,
            spent: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl<R: RangeReader> RangeReader for BudgetReader<R> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        // Charged BEFORE the read, so a single oversized range cannot slip
        // through by being the one that would have crossed the line.
        let spent = self
            .spent
            .fetch_add(len, std::sync::atomic::Ordering::Relaxed)
            + len;
        if spent > self.budget {
            return Err(std::io::Error::other(format!(
                "byte budget exhausted: this query wanted more than {} bytes",
                self.budget
            )));
        }
        self.inner.read_at(offset, len)
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let want: u64 = ranges.iter().map(|&(_, l)| l).sum();
        let spent = self
            .spent
            .fetch_add(want, std::sync::atomic::Ordering::Relaxed)
            + want;
        if spent > self.budget {
            return Err(std::io::Error::other(format!(
                "byte budget exhausted: this query wanted more than {} bytes",
                self.budget
            )));
        }
        self.inner.read_many(ranges)
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }
}

/// What one cold run of one starter query produced: what it cost, whether it
/// finished, and whether it was worth anything.
///
/// `error` and `useless` answer different questions and are both needed.
/// `error` is *did the run complete* — the file did not open, the query failed,
/// a byte budget stopped it — and `cost` then still holds the bytes and
/// requests spent getting that far, which is what makes "costs more than N MB"
/// a usable answer for `card-audit`. `useless` is *is the answer worth
/// shipping*, which is what `rete build` drops on. A run that did not complete
/// is both.
pub(crate) struct Measured {
    pub cost: QueryCost,
    /// Set when the run did not finish. See the type note above.
    pub error: Option<String>,
    /// `None` when the query answered with something. `Some(reason)` when the
    /// run showed the query is worthless on this file — the reason is the text
    /// the build prints and stores in [`DroppedQuery::why`].
    pub useless: Option<String>,
    /// Rows came back, and every one of them binds nothing at all — the
    /// un-grouped aggregate over an empty solution sequence. SPARQL returns
    /// exactly one row there whatever happens, so no row count catches it, and
    /// the row carries no answer. Only a run can see this; a card cannot.
    ///
    /// It is also the one shape of uselessness that contradicts **no**
    /// template: the non-emptiness claims are about row *count*, and
    /// `NonEmpty::Aggregate` says in so many words that the row's values may be
    /// unbound. So `useless.is_some() && !vacuous` is exactly "measured zero
    /// rows", which is what can prove a claim wrong.
    pub vacuous: bool,
}

/// What a run's *result* decides, before its cost is tallied — the output of
/// [`grade`], folded into a [`Measured`] by [`measure_one`].
struct Graded {
    rows: u64,
    error: Option<String>,
    useless: Option<String>,
    vacuous: bool,
}

impl Graded {
    /// A run that never produced a result. It is an error, and — since a query
    /// that cannot be run cannot be shown to answer — it is useless too.
    fn failed(why: String) -> Self {
        Self {
            rows: 0,
            error: Some(why.clone()),
            useless: Some(why),
            vacuous: false,
        }
    }
}

/// Grade one query's result. Two things make a starter query worthless, and a
/// run can see both:
///
/// * **no rows** — the failure the query library exists to prevent ("a starter
///   query that answers nothing is worse than no starter query: the reader
///   concludes the *file* is broken");
/// * **rows that bind nothing** — the un-grouped aggregate over an empty
///   solution sequence. SPARQL returns exactly one row there whatever happens,
///   so no row count can catch it, and the row carries zero information:
///   `sp-bbox` over a file where no subject holds both `wgs:lat` and
///   `wgs:long` returns one row of four unbound variables. Its template says
///   the card "cannot do better" than that — the *measurement* can.
///
/// A `COUNT` is not caught by either: it binds, to 0. See the module notes on
/// `cmp-coverage` in `docs/dataset-cards.md`.
fn grade(out: &QueryOutput) -> Graded {
    let answers = |rows: u64| Graded {
        rows,
        error: None,
        useless: None,
        vacuous: false,
    };
    let empty = |why: &str| Graded {
        rows: 0,
        error: None,
        useless: Some(why.to_string()),
        vacuous: false,
    };
    match out {
        QueryOutput::Select(_, rows) if rows.is_empty() => empty("measured 0 rows on the file"),
        QueryOutput::Select(_, rows) if rows.iter().all(|b| b.is_empty()) => Graded {
            rows: rows.len() as u64,
            error: None,
            useless: Some(format!(
                "measured {} row(s) binding no variable at all — an aggregate over an empty \
                 solution sequence, which is a row without an answer in it",
                rows.len()
            )),
            vacuous: true,
        },
        QueryOutput::Select(_, rows) => answers(rows.len() as u64),
        QueryOutput::Ask(false) => empty("measured ASK false on the file"),
        QueryOutput::Ask(true) => answers(1),
        QueryOutput::Construct(ts) if ts.is_empty() => empty("constructed no triples on the file"),
        QueryOutput::Construct(ts) => answers(ts.len() as u64),
        // `QueryOutput` is non-exhaustive; the library emits only the forms
        // above, so anything else is a shape this run cannot vouch for.
        other => Graded::failed(format!("returned an unexpected result form: {other:?}")),
    }
}

/// **The** measurement, in one place: open the file cold through `reader`, run
/// `q`, tally what it cost, and grade what came back.
///
/// Everything that reports a query cost goes through here — `rete build` when
/// it writes the build record (and when it decides which starter queries the
/// card may keep), `rete card-audit --measure` when it re-measures a file that
/// already exists. Two implementations would drift, and a cost that drifts from
/// the cost a build recorded cannot be compared against it, which is the only
/// thing anyone wants to do with it. The same goes for the grade: the audit
/// reports emptiness with the words the build dropped on.
///
/// `reader` is a fresh counting reader per query on purpose: the figures are
/// what a **stateless** client pays to answer just this query, with no state
/// carried over from the last one. What the reader is wrapped around — an
/// in-memory image, a file handle, an HTTP range client — decides the transport
/// but not the arithmetic.
fn measure_one<R: RangeReader + Send + Sync + 'static>(
    reader: std::sync::Arc<CountingReader<R>>,
    q: &ExampleQuery,
) -> Measured {
    let start = std::time::Instant::now();
    let graded = match Rete::open_ranged_lazy(reader.clone()) {
        Ok(rete) => match eval_query(&rete, &q.sparql) {
            Ok(result) => grade(&result),
            Err(e) => Graded::failed(format!("failed to run: {e}")),
        },
        Err(e) => Graded::failed(format!("the file did not open: {e}")),
    };
    Measured {
        cost: QueryCost {
            id: q.id.clone(),
            bytes: reader.bytes_read(),
            requests: reader.requests(),
            rows: graded.rows,
            debug_ms: start.elapsed().as_millis() as u64,
        },
        error: graded.error,
        useless: graded.useless,
        vacuous: graded.vacuous,
    }
}

/// Measure a starter query cold through a caller-supplied transport. The
/// closure is called once per query and must hand back a **fresh** reader —
/// a new file handle, a new HTTP client — so no query is warmed by the last.
pub(crate) fn measure_query<R, F>(open: F, q: &ExampleQuery) -> anyhow::Result<Measured>
where
    R: RangeReader + Send + Sync + 'static,
    F: FnOnce() -> anyhow::Result<R>,
{
    Ok(measure_one(
        std::sync::Arc::new(CountingReader::new(open()?)),
        q,
    ))
}

/// Measure every starter query **cold** against the finished image: a fresh
/// lazy ranged open per query (what a stateless remote client pays), counting
/// logical range reads. `bytes`/`requests` are deterministic properties of
/// layout + query; `debug_ms` is this machine's reference timing.
///
/// The image measured here lacks the build-info section itself, but that does
/// not skew the figures: inserting it only shifts later sections by a constant,
/// and logical reads are section-relative, so sizes and counts are invariant.
///
/// The run also **grades** each answer ([`Measured::useless`]). That costs
/// nothing extra — the query had to run to be costed — and it is the whole
/// point: for a carded build, "does this query answer?" is measured, not
/// reasoned about.
pub(crate) fn measure_query_costs(
    image: std::sync::Arc<Vec<u8>>,
    queries: &[ExampleQuery],
) -> Vec<Measured> {
    queries
        .iter()
        .map(|q| {
            measure_one(
                std::sync::Arc::new(CountingReader::new(ImageReader(image.clone()))),
                q,
            )
        })
        .collect()
}

/// Wrap the kept per-query figures in the context that makes them
/// interpretable.
pub(crate) fn query_costs(queries: Vec<QueryCost>) -> QueryCosts {
    QueryCosts {
        context: cost_context(IN_MEMORY_TRANSPORT),
        queries,
    }
}

/// How `rete build` reads the file it has just written.
pub(crate) const IN_MEMORY_TRANSPORT: &str =
    "local in-memory image; cold lazy open per query; logical range reads, no block cache";

/// The caveat that travels with every cost figure — stored in the file, and
/// printed by anything that shows one. One string, so the two cannot diverge.
pub(crate) const COST_NOTE: &str =
    "bytes/requests are properties of file layout + query (portable); debug_ms is one machine's \
     reference timing, not a guarantee";

/// The context every set of cost figures travels with. `transport` is the one
/// part a caller varies, and it is not optional: "1.4 MB in 76 requests" means
/// nothing without knowing what did the requesting.
pub(crate) fn cost_context(transport: &str) -> CostContext {
    CostContext {
        engine: Some(builder_version()),
        transport: Some(transport.to_string()),
        note: Some(COST_NOTE.to_string()),
    }
}

/// How big a chunk the streaming rewriter moves at a time. 4 MiB is enough to
/// keep a spinning disk streaming and small enough that peak RSS stays flat
/// whatever the file weighs.
const REWRITE_CHUNK: usize = 4 << 20;

/// Write `info` into `path`'s build-info section, **streaming** — the file is
/// never held in memory, so a 17 GB `.rete` costs [`REWRITE_CHUNK`] of RAM and
/// one pass of I/O rather than 34 GB of `Vec`. (The command that calls this
/// still has to *run* the queries first, and that is where its memory goes; the
/// copy below is the cheap half.)
///
/// Why a rewrite at all: the section sits immediately after the card, near the
/// front of the file, so growing it shifts every byte behind it. The identity
/// is preserved (build info is outside the content hash — see
/// [`rete_core::plan_build_info`]) but the bytes are not, which is the trade
/// this function exists to make explicit rather than hide.
///
/// Crash-safety: the new image is built beside the target and renamed over it
/// only once it is complete and fsynced, so an interrupted run leaves the
/// original intact.
///
/// Returns the new file length.
pub(crate) fn write_build_info_streaming(
    path: &std::path::Path,
    info: &[u8],
) -> anyhow::Result<u64> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut src = std::fs::File::open(path)?;
    let file_len = src.metadata()?.len();
    let mut head = vec![0u8; rete_core::format::HEADER_LEN];
    src.read_exact(&mut head)?;
    let before = rete_core::format::Header::from_bytes(&head)?;
    let plan = rete_core::plan_build_info(&head, file_len, info.len() as u64)?;

    let tmp = path.with_extension("rete.costs-tmp");
    {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        out.write_all(&plan.header)?;
        // The `[HEADER_LEN, insert)` prefix is the card, which is KBs; the
        // `[tail_start, len)` tail is the rest of the file, which is not.
        let mut copy = |from: u64, to: u64, out: &mut dyn Write| -> anyhow::Result<()> {
            src.seek(SeekFrom::Start(from))?;
            let mut left = to - from;
            let mut buf = vec![0u8; REWRITE_CHUNK.min(left.max(1) as usize)];
            while left > 0 {
                let n = buf.len().min(left as usize);
                src.read_exact(&mut buf[..n])?;
                out.write_all(&buf[..n])?;
                left -= n as u64;
            }
            Ok(())
        };
        copy(rete_core::format::HEADER_LEN as u64, plan.insert, &mut out)?;
        out.write_all(info)?;
        copy(plan.tail_start, file_len, &mut out)?;
        out.flush()?;
        out.into_inner()
            .map_err(|e| anyhow::anyhow!("flushing {}: {e}", tmp.display()))?
            .sync_all()?;
    }
    drop(src);

    // Read the new header back before committing: the section must be there,
    // and the content hash — the file's identity — must be the byte-for-byte
    // same 16 bytes it was. A rewrite that moved it is a rewrite that produced
    // a different file, and the caller was promised it would not.
    let mut written_head = vec![0u8; rete_core::format::HEADER_LEN];
    std::fs::File::open(&tmp)?.read_exact(&mut written_head)?;
    let after = rete_core::format::Header::from_bytes(&written_head)
        .map_err(|e| anyhow::anyhow!("the rewritten file has no readable header: {e}"))?;
    if after.content_hash != before.content_hash {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("refusing to commit: the rewrite changed the content hash");
    }
    if after.build_info_len != info.len() as u64 {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("refusing to commit: the rewritten build-info section is the wrong length");
    }
    std::fs::rename(&tmp, path)?;
    Ok(plan.new_len)
}

/// Render the build info for the human `rete card` catalog view.
pub(crate) fn format_build_info(info: &BuildInfo) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "  build:");
    let opt = |out: &mut String, label: &str, v: &Option<String>| {
        if let Some(v) = v {
            let _ = writeln!(out, "      {label:<11}: {v}");
        }
    };
    opt(&mut out, "built at", &info.built_at);
    opt(&mut out, "builder", &info.builder);
    let p = &info.params;
    if !p.is_empty() {
        let mut flags: Vec<String> = Vec::new();
        if let Some(c) = &p.command {
            flags.push(c.clone());
        }
        if let Some(f) = &p.format {
            flags.push(format!("--format {f}"));
        }
        if p.no_pyramid {
            flags.push("--no-pyramid".into());
        }
        if p.text_index {
            flags.push("--text-index".into());
        }
        if p.materialize {
            flags.push("--materialize".into());
        }
        if p.reason {
            flags.push("--reason".into());
        }
        if let Some(a) = &p.pyramid_algo {
            flags.push(format!("--pyramid-algo {a}"));
        }
        if let Some(mb) = p.memory_budget_mb {
            flags.push(format!("--memory-budget-mb {mb}"));
        }
        let _ = writeln!(out, "      params     : {}", flags.join(" "));
        if let Some(c) = &p.codec {
            let _ = writeln!(out, "      codec      : {c}");
        }
        if let Some(n) = p.card_top_n {
            let _ = writeln!(out, "      card top-N : {n}");
        }
    }
    if let Some(costs) = &info.query_costs {
        let _ = writeln!(
            out,
            "      query costs (cold; bytes/requests portable, ms = one machine's debug ref):"
        );
        // The transport belongs WITH the numbers: "1.4 MB in 76 requests" is
        // only a cost once you know what did the requesting, and these figures
        // can now come either from the build (an in-memory image) or from a
        // later `card-audit --measure` (a file handle, or HTTP).
        if let Some(t) = &costs.context.transport {
            let _ = writeln!(out, "        measured over: {t}");
        }
        for c in &costs.queries {
            let _ = writeln!(
                out,
                "        {:<14} {:>9} B in {:>2} req · {:>5} row(s) · {} ms",
                c.id, c.bytes, c.requests, c.rows, c.debug_ms
            );
        }
    }
    if !info.dropped_queries.is_empty() {
        let _ = writeln!(
            out,
            "      starter queries generated, measured, and NOT shipped:"
        );
        for d in &info.dropped_queries {
            let _ = writeln!(
                out,
                "        {:<14} {}{}",
                d.id,
                d.why,
                if d.contradicts_claim {
                    " [generator defect: its template claimed this could not happen]"
                } else {
                    ""
                }
            );
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_epochs() {
        assert_eq!(epoch_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(1_785_801_600), "2026-08-04T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(86_399), "1970-01-01T23:59:59Z");
    }

    #[test]
    fn build_info_round_trips_and_omits_absent_fields() {
        let info = new_build_info(BuildParams {
            command: Some("build".into()),
            no_pyramid: true,
            codec: Some(codec_name(rete_core::CODEC_ZSTD).into()),
            card_top_n: Some(100),
            ..Default::default()
        });
        let bytes = info.to_json_bytes();
        let back = BuildInfo::from_json_bytes(&bytes).unwrap();
        assert_eq!(info, back);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("built_at"));
        assert!(text.contains("rete-cli"));
        assert!(text.contains("no_pyramid"));
        // Absent options are omitted, keeping the section small.
        assert!(!text.contains("memory_budget_mb"));
        assert!(!text.contains("query_costs"));
        assert!(!text.contains("text_index"));
    }

    #[test]
    fn empty_build_info_parses_from_empty_object() {
        // Forward-compat: an empty or minimal JSON object still deserializes.
        let b = BuildInfo::from_json_bytes(b"{}").unwrap();
        assert!(b.built_at.is_none() && b.builder.is_none());
        assert!(b.params.is_empty());
    }

    /// The grade is read by two callers that ask different questions of it, so
    /// every result shape is pinned against **both**.
    ///
    /// `rete build` drops on `useless` and then asks "was this a *zero-row*
    /// emptiness?" — the only shape that can contradict a template's
    /// non-emptiness claim — which it reads as `useless && !vacuous`.
    /// `rete card-audit` reports an `outcome` and reads `error` to tell a run
    /// that could not finish from one that finished with nothing. Those two
    /// readings must stay consistent with each other and with the shapes below,
    /// because a starter query silently kept (or silently dropped) is exactly
    /// the failure both features exist to prevent.
    #[test]
    fn every_result_shape_grades_the_same_way_for_both_callers() {
        use rete_core::Binding;

        let unbound = Binding::default();
        let bound = Binding::from([("s".to_string(), "http://x/a".to_string())]);
        let vars = vec!["s".to_string()];
        let triple = || ("s".to_string(), "p".to_string(), "o".to_string());

        // output, rows, useless, vacuous, error
        let cases = [
            (
                QueryOutput::Select(vars.clone(), vec![]),
                0,
                true,
                false,
                false,
            ),
            (
                QueryOutput::Select(vars.clone(), vec![unbound.clone()]),
                1,
                true,
                true,
                false,
            ),
            (
                QueryOutput::Select(vars.clone(), vec![bound.clone(), unbound.clone()]),
                2,
                false,
                false,
                false,
            ),
            (
                QueryOutput::Select(vars.clone(), vec![bound]),
                1,
                false,
                false,
                false,
            ),
            (QueryOutput::Ask(false), 0, true, false, false),
            (QueryOutput::Ask(true), 1, false, false, false),
            (QueryOutput::Construct(vec![]), 0, true, false, false),
            (
                QueryOutput::Construct(vec![triple()]),
                1,
                false,
                false,
                false,
            ),
        ];

        for (out, rows, useless, vacuous, error) in cases {
            let g = grade(&out);
            let what = format!("{out:?}");
            assert_eq!(g.rows, rows, "rows for {what}");
            assert_eq!(g.useless.is_some(), useless, "useless for {what}");
            assert_eq!(g.vacuous, vacuous, "vacuous for {what}");
            assert_eq!(g.error.is_some(), error, "error for {what}");
            // What `build.rs` reads as "measured zero rows".
            assert_eq!(
                g.useless.is_some() && !g.vacuous,
                useless && !vacuous,
                "zero-row emptiness for {what}"
            );
        }

        // A run that never produced a result is both: nothing to ship, and a
        // failure the audit has to name rather than report as an honest zero.
        let f = Graded::failed("the file did not open: nope".into());
        assert_eq!(f.rows, 0);
        assert_eq!(f.error.as_deref(), f.useless.as_deref());
        assert!(
            !f.vacuous,
            "a failure is a zero-row emptiness, not a vacuous one"
        );
    }
}
