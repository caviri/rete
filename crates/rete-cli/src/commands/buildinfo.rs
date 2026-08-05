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

/// One starter query's measurement: what it cost, and whether it was worth
/// anything.
pub(crate) struct Measured {
    pub cost: QueryCost,
    /// `None` when the query answered with something. `Some(reason)` when the
    /// run showed the query is worthless on this file — the reason is the text
    /// the build prints and stores in [`DroppedQuery::why`].
    pub useless: Option<String>,
    /// Set with `useless` when the emptiness is a **zero-row** one. Only that
    /// shape can contradict a template's non-emptiness claim; a vacuous row is
    /// not a claim any template makes (`NonEmpty::Aggregate` says in so many
    /// words that the row's values may be unbound).
    pub zero_rows: bool,
}

/// Grade one query's result. Two things make a starter query worthless, and a
/// build can see both:
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
fn grade(out: &QueryOutput) -> (u64, Option<String>, bool) {
    match out {
        QueryOutput::Select(_, rows) if rows.is_empty() => {
            (0, Some("measured 0 rows on the built file".into()), true)
        }
        QueryOutput::Select(_, rows) if rows.iter().all(|b| b.is_empty()) => (
            rows.len() as u64,
            Some(format!(
                "measured {} row(s) binding no variable at all — an aggregate over an empty \
                 solution sequence, which is a row without an answer in it",
                rows.len()
            )),
            false,
        ),
        QueryOutput::Select(_, rows) => (rows.len() as u64, None, false),
        QueryOutput::Ask(false) => (0, Some("measured ASK false on the built file".into()), true),
        QueryOutput::Ask(true) => (1, None, false),
        QueryOutput::Construct(ts) if ts.is_empty() => (
            0,
            Some("constructed no triples on the built file".into()),
            true,
        ),
        QueryOutput::Construct(ts) => (ts.len() as u64, None, false),
        // `QueryOutput` is non-exhaustive; the library emits only the forms
        // above, so anything else is a shape this build cannot vouch for.
        other => (
            0,
            Some(format!("returned an unexpected result form: {other:?}")),
            true,
        ),
    }
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
    let mut out = Vec::with_capacity(queries.len());
    for q in queries {
        let reader = std::sync::Arc::new(CountingReader::new(ImageReader(image.clone())));
        let start = std::time::Instant::now();
        let (rows, useless, zero_rows) = match Rete::open_ranged_lazy(reader.clone()) {
            Ok(rete) => match eval_query(&rete, &q.sparql) {
                Ok(result) => grade(&result),
                Err(e) => (0, Some(format!("failed to run: {e}")), true),
            },
            Err(e) => (0, Some(format!("the built file did not open: {e}")), true),
        };
        let debug_ms = start.elapsed().as_millis() as u64;
        out.push(Measured {
            cost: QueryCost {
                id: q.id.clone(),
                bytes: reader.bytes_read(),
                requests: reader.requests(),
                rows,
                debug_ms,
            },
            useless,
            zero_rows,
        });
    }
    out
}

/// Wrap the kept per-query figures in the context that makes them
/// interpretable.
pub(crate) fn query_costs(queries: Vec<QueryCost>) -> QueryCosts {
    QueryCosts {
        context: CostContext {
            engine: Some(builder_version()),
            transport: Some(
                "local in-memory image; cold lazy open per query; logical range reads, no block cache"
                    .to_string(),
            ),
            note: Some(
                "bytes/requests are properties of file layout + query (portable); debug_ms is one \
                 machine's build-time reference, not a guarantee"
                    .to_string(),
            ),
        },
        queries,
    }
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
            "      query costs (cold; bytes/requests portable, ms = build-machine debug ref):"
        );
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
}
