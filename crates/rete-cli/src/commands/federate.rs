//! The `federate` command: UNION federation of one SPARQL query across several
//! `.rete` sources (local paths and/or http(s) URLs). The routing and the
//! term-level merge live in `rete_core::federate`; this file opens each source
//! and prints.

use std::collections::BTreeSet;
use std::time::Instant;

use rete_core::{eval_query, FederationSource, QueryOutput, SummaryView};

use crate::commands::range_source::{open_local, query_budget, RangedSourceReader};
use crate::commands::render::print_query_output;
use crate::commands::url::open_url_lazy;

/// Is this source an `http(s)://` URL (vs. a local file path)?
fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// Read a source's predicate IRI set cheaply from its summary — the triple index
/// is never read. Used for routing. A file with no pyramid (no summary) yields
/// `None`, in which case the caller must not prune it (we can't tell).
pub(crate) fn source_predicates(source: &str) -> anyhow::Result<Option<BTreeSet<String>>> {
    // Ranged for both: a local file is read by positional reads too, never
    // slurped whole just to route.
    let reader = RangedSourceReader::open(source)?;
    let view = SummaryView::open_ranged(&reader)?;
    Ok(view.map(|v| v.predicate_totals().into_iter().map(|(p, _)| p).collect()))
}

/// Evaluate `query` against one source (path or URL), returning its result.
/// Each source gets the HTTP `ServiceClient`, so a federated query may itself
/// carry `SERVICE` blocks — N `.rete` files and a live SPARQL endpoint in one
/// query.
fn eval_source(source: &str, query: &str) -> anyhow::Result<QueryOutput> {
    let mut rete = if is_url(source) {
        // The same lazy open as `sparql-url`: the eager `Rete::open_ranged`
        // decodes the whole dictionary and index directory up front, which
        // OOM-killed a 4 GiB container on a 55-63 GB source.
        let budget = query_budget(None);
        open_url_lazy(source, budget)?.0
    } else {
        open_local(source)?
    };
    rete.set_service_client(Box::new(super::service_http::HttpServiceClient));
    let out = eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{source}: {e}"))?;
    // Lazy tile fetches fail out-of-band; a partial answer must be an error,
    // never quietly fewer rows (same contract as `sparql-url`).
    if rete.index_incomplete() {
        anyhow::bail!(
            "{source}: a range request failed while streaming index tiles; \
             results would be incomplete — retry"
        );
    }
    Ok(out)
}

/// A path or URL, opened only when it is evaluated and dropped right after, so
/// at most one source is resident at a time.
struct CliSource<'a>(&'a str);

impl FederationSource for CliSource<'_> {
    fn name(&self) -> &str {
        self.0
    }

    fn predicates(&self) -> Result<Option<BTreeSet<String>>, String> {
        source_predicates(self.0).map_err(|e| e.to_string())
    }

    fn eval(&self, query: &str) -> Result<QueryOutput, String> {
        eval_source(self.0, query).map_err(|e| e.to_string())
    }
}

/// `rete federate`: run one SPARQL query across several `.rete` sources (local
/// paths and/or http(s) URLs) and merge the term-level results.
///
/// - **Routing** (`route`): skip any source whose predicate set is disjoint from
///   the query's concrete predicates (read from each summary; index untouched).
/// - **Merge**: SELECT → union + dedup rows (stable order); ASK → logical OR;
///   CONSTRUCT → union + dedup triples.
///
/// This is UNION federation (no cross-file joins); aggregates/LIMIT are per
/// source then unioned. Per-source diagnostics go to stderr.
pub(crate) fn federate(
    sources: &[String],
    query: &str,
    json: bool,
    route: bool,
) -> anyhow::Result<()> {
    let srcs: Vec<CliSource> = sources.iter().map(|s| CliSource(s)).collect();

    let mut last = Instant::now();
    let got = rete_core::federate(&srcs, query, route, |r| {
        if let Some(e) = &r.routing_error {
            eprintln!("warning: {}: routing skipped ({e})", r.name);
        }
        if !r.pruned {
            eprintln!(
                "  {}: {} row(s) in {:.1?}",
                r.name,
                r.contributed,
                last.elapsed()
            );
        }
        last = Instant::now();
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    print_query_output(&got.output, json);

    let pruned: Vec<&str> = got
        .sources
        .iter()
        .filter(|s| s.pruned)
        .map(|s| s.name.as_str())
        .collect();
    eprintln!(
        "federated {} source(s): {} queried, {} pruned (routing {}); {} merged result(s)",
        sources.len(),
        sources.len() - pruned.len(),
        pruned.len(),
        if route { "on" } else { "off" },
        match &got.output {
            QueryOutput::Select(_, rows) => rows.len(),
            QueryOutput::Ask(_) => 1,
            QueryOutput::Construct(ts) => ts.len(),
            _ => 0,
        }
    );
    if !pruned.is_empty() {
        eprintln!("  pruned (predicate-disjoint): {}", pruned.join(", "));
    }
    Ok(())
}
