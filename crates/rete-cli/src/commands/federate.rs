//! The `federate` command: UNION federation of one SPARQL query across several
//! `.rete` sources (local paths and/or http(s) URLs), with summary-based routing
//! and a term-level merge of the per-source results.

use rete_core::{eval_query, query_predicates, QueryOutput, Rete, SummaryView};

use crate::commands::range_source::{open_local_for_query, RangedSourceReader};
use crate::commands::render::print_query_output;
use crate::http::HttpRangeReader;

/// Is this source an `http(s)://` URL (vs. a local file path)?
fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// Read a source's predicate IRI set cheaply from its summary — the triple index
/// is never read. Used for routing. A file with no pyramid (no summary) yields
/// `None`, in which case the caller must not prune it (we can't tell).
pub(crate) fn source_predicates(
    source: &str,
) -> anyhow::Result<Option<std::collections::BTreeSet<String>>> {
    let reader = RangedSourceReader::open(source)?;
    let view = SummaryView::open_ranged(&reader)?;
    Ok(view.map(|v| {
        v.predicate_totals()
            .into_iter()
            .map(|(p, _)| p)
            .collect::<std::collections::BTreeSet<String>>()
    }))
}

/// Evaluate `query` against one source (path or URL), returning its result.
/// Each source gets the HTTP `ServiceClient`, so a federated query may itself
/// carry `SERVICE` blocks — N `.rete` files and a live SPARQL endpoint in one
/// query.
fn eval_source(source: &str, query: &str) -> anyhow::Result<QueryOutput> {
    let mut rete = if is_url(source) {
        let reader = HttpRangeReader::open(source)?;
        Rete::open_ranged(&reader)?
    } else {
        open_local_for_query(source)?
    };
    rete.set_service_client(Box::new(super::service_http::HttpServiceClient));
    eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{source}: {e}"))
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
    use std::collections::BTreeSet;
    use std::time::Instant;

    // The query's concrete predicates drive routing. An empty set (every pattern
    // uses a variable predicate) means we cannot prune on predicates.
    let query_preds: BTreeSet<String> =
        query_predicates(query).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Decide which sources to query. A source is pruned only when routing is on,
    // the query pins ≥1 predicate, the source exposes a predicate set, and that
    // set is disjoint from the query's predicates.
    let mut queried: Vec<&String> = Vec::new();
    let mut skipped: Vec<&String> = Vec::new();
    for source in sources {
        let mut prune = false;
        if route && !query_preds.is_empty() {
            match source_predicates(source) {
                Ok(Some(preds)) => {
                    prune = query_preds.is_disjoint(&preds);
                }
                Ok(None) => {} // no summary → can't tell → keep it
                Err(e) => eprintln!("warning: {source}: routing skipped ({e})"),
            }
        }
        if prune {
            skipped.push(source);
        } else {
            queried.push(source);
        }
    }

    // Evaluate each queried source and merge (union + dedup) into one output.
    let mut acc = MergeAcc::default();
    for source in &queried {
        let start = Instant::now();
        let out = eval_source(source, query)?;
        let contributed = acc.absorb(out);
        eprintln!(
            "  {source}: {contributed} row(s) in {:.1?}",
            start.elapsed()
        );
    }
    let result = acc.into_output();

    print_query_output(&result, json);

    eprintln!(
        "federated {} source(s): {} queried, {} pruned (routing {}); {} merged result(s)",
        sources.len(),
        queried.len(),
        skipped.len(),
        if route { "on" } else { "off" },
        match &result {
            QueryOutput::Select(_, rows) => rows.len(),
            QueryOutput::Ask(_) => 1,
            QueryOutput::Construct(ts) => ts.len(),
            _ => 0,
        }
    );
    if !skipped.is_empty() {
        eprintln!("  pruned (predicate-disjoint): {}", join_sources(&skipped));
    }
    Ok(())
}

/// Term-level merge accumulator for federation: unions SELECT rows (deduped),
/// OR's ASK results, and unions CONSTRUCT triples (deduped), all in stable
/// insertion order. The output kind is fixed by the first absorbed result.
#[derive(Default)]
pub(crate) struct MergeAcc {
    kind: Option<OutKind>,
    select_vars: Vec<String>,
    select_rows: Vec<rete_core::Binding>,
    select_seen: std::collections::BTreeSet<String>,
    ask_any: bool,
    construct: Vec<(String, String, String)>,
    construct_seen: std::collections::BTreeSet<(String, String, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum OutKind {
    Select,
    Ask,
    Construct,
}

impl MergeAcc {
    /// Fold one source's result in; return how many *new* rows/triples it added
    /// (for ASK: 1 if it answered true, else 0).
    pub(crate) fn absorb(&mut self, out: QueryOutput) -> usize {
        match out {
            QueryOutput::Select(vars, rows) => {
                self.kind.get_or_insert(OutKind::Select);
                if self.select_vars.is_empty() {
                    self.select_vars = vars;
                }
                let before = self.select_rows.len();
                for row in rows {
                    // Canonical key over the projected vars so identical rows
                    // dedup across sources regardless of map iteration order.
                    if self.select_seen.insert(row_key(&self.select_vars, &row)) {
                        self.select_rows.push(row);
                    }
                }
                self.select_rows.len() - before
            }
            QueryOutput::Ask(b) => {
                self.kind.get_or_insert(OutKind::Ask);
                self.ask_any |= b;
                usize::from(b)
            }
            QueryOutput::Construct(triples) => {
                self.kind.get_or_insert(OutKind::Construct);
                let before = self.construct.len();
                for t in triples {
                    if self.construct_seen.insert(t.clone()) {
                        self.construct.push(t);
                    }
                }
                self.construct.len() - before
            }
            _ => {
                eprintln!("warning: query result kind is not supported by this CLI build");
                0
            }
        }
    }

    /// Finalize into a single merged [`QueryOutput`]. With no absorbed sources,
    /// defaults to an empty SELECT.
    pub(crate) fn into_output(self) -> QueryOutput {
        match self.kind {
            Some(OutKind::Ask) => QueryOutput::Ask(self.ask_any),
            Some(OutKind::Construct) => QueryOutput::Construct(self.construct),
            Some(OutKind::Select) | None => QueryOutput::Select(self.select_vars, self.select_rows),
        }
    }
}

/// A canonical, order-independent string key for a SELECT solution row over the
/// given variable order — used to dedup identical rows across sources.
fn row_key(vars: &[String], row: &rete_core::Binding) -> String {
    if vars.is_empty() {
        // SELECT * : key over all bindings in sorted (Binding is a BTreeMap) order.
        row.iter()
            .map(|(k, v)| format!("{k}\u{1}{v}"))
            .collect::<Vec<_>>()
            .join("\u{2}")
    } else {
        vars.iter()
            .map(|v| row.get(v).map(String::as_str).unwrap_or("").to_string())
            .collect::<Vec<_>>()
            .join("\u{2}")
    }
}

/// Join source labels for a diagnostic line.
fn join_sources(sources: &[&String]) -> String {
    sources
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
