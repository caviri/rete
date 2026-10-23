//! Union federation of one SPARQL query across several `.rete` sources.
//!
//! The query is evaluated on each source independently and the term-level rows
//! are merged: UNION + dedup for SELECT, logical OR for ASK, triple union for
//! CONSTRUCT. It is correct for sharded data where each file yields complete
//! answer rows; it does **not** join across files, and aggregates and `LIMIT`
//! apply per source (a federated `COUNT(*)` is one count per source).
//!
//! This module does no I/O and reads no clock, so every binding (CLI, Python,
//! Java, R, wasm) can use it. A binding supplies its sources through
//! [`FederationSource`]: how a source is opened (a path, a URL, an already-open
//! graph) and when it is dropped stays the binding's decision. [`federate`]
//! evaluates the sources one at a time, so an implementation that opens inside
//! [`FederationSource::eval`] holds one file at a time.

use std::collections::BTreeSet;

use crate::bgp::Binding;
use crate::file::Rete;
use crate::sparql::{eval_query, parse_select, query_predicates_and_form, QueryOutput};

/// One source of a federated query.
pub trait FederationSource {
    /// A label for diagnostics (a path or URL).
    fn name(&self) -> &str;

    /// The predicate IRIs this source holds, read cheaply from its summary, or
    /// `None` when that cannot be told (no summary), in which case the source is
    /// never pruned. The default is `None`.
    fn predicates(&self) -> Result<Option<BTreeSet<String>>, String> {
        Ok(None)
    }

    /// Evaluate `query` against this source.
    fn eval(&self, query: &str) -> Result<QueryOutput, String>;
}

/// An already-open [`Rete`] as a federation source.
pub struct ReteSource<'a> {
    name: &'a str,
    rete: &'a Rete,
}

impl<'a> ReteSource<'a> {
    /// Wrap an open graph; `name` labels it in reports and errors.
    pub fn new(name: &'a str, rete: &'a Rete) -> Self {
        Self { name, rete }
    }
}

impl FederationSource for ReteSource<'_> {
    fn name(&self) -> &str {
        self.name
    }

    /// From the pyramid summary. This faults the pyramid section in on a lazily
    /// opened file; a file without a pyramid yields `None`.
    fn predicates(&self) -> Result<Option<BTreeSet<String>>, String> {
        let dict = self.rete.dictionary();
        Ok(self.rete.pyramid().map(|p| {
            p.summary
                .iter()
                .filter_map(|e| dict.predicate_term(e.predicate))
                .collect()
        }))
    }

    fn eval(&self, query: &str) -> Result<QueryOutput, String> {
        eval_query(self.rete, query).map_err(|e| e.to_string())
    }
}

/// What happened to one source.
#[derive(Debug, Clone)]
pub struct SourceReport {
    /// The source's [`FederationSource::name`].
    pub name: String,
    /// Skipped because its predicate set is disjoint from the query's.
    pub pruned: bool,
    /// New rows (or triples; for ASK, 1 if it answered true) this source added
    /// to the merge. Zero for a pruned source.
    pub contributed: usize,
    /// Set when routing could not read the source's predicates; the source was
    /// then queried unpruned.
    pub routing_error: Option<String>,
}

/// A merged result and the per-source account of how it was produced.
#[derive(Debug, Clone)]
pub struct FederatedOutput {
    /// The merged result.
    pub output: QueryOutput,
    /// One entry per source, in source order.
    pub sources: Vec<SourceReport>,
    /// Query features whose meaning changes when the query runs per source and
    /// the rows are unioned. Empty with fewer than two sources, where the result
    /// is exact.
    pub warnings: Vec<FederationWarning>,
}

/// Which query feature a [`FederationWarning`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FederationWarningKind {
    /// An aggregate or `GROUP BY`.
    Aggregate,
    /// `LIMIT`.
    Limit,
    /// `OFFSET`.
    Offset,
    /// `ORDER BY`.
    OrderBy,
}

impl FederationWarningKind {
    /// `"aggregate"`, `"limit"`, `"offset"` or `"order-by"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aggregate => "aggregate",
            Self::Limit => "limit",
            Self::Offset => "offset",
            Self::OrderBy => "order-by",
        }
    }
}

/// A query feature that is evaluated per source, so the merged result is not
/// what the same query would return over one graph holding all the data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FederationWarning {
    /// The feature concerned.
    pub kind: FederationWarningKind,
    /// One plain-language sentence saying what the caller gets instead.
    pub message: String,
}

/// The [`FederationWarning`]s a query earns when run over two or more sources.
///
/// [`federate`] calls this for you. Call it directly when you evaluate the
/// sources yourself and merge with [`FederationMerge`]. Only `SELECT` has any:
/// `ASK` is exact under OR, and a `CONSTRUCT` is a union. A query that does not
/// parse as a `SELECT` yields none.
pub fn federation_warnings(query: &str) -> Vec<FederationWarning> {
    let Ok(select) = parse_select(query) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut warn = |kind, message: &str| {
        out.push(FederationWarning {
            kind,
            message: message.to_string(),
        })
    };
    if select.group.is_some() {
        warn(
            FederationWarningKind::Aggregate,
            "aggregates and GROUP BY run per source, so you get one set of groups per source \
             (a COUNT(*) is one count per source, not a total); re-aggregate the rows, or \
             query a single source",
        );
    }
    if select.limit.is_some() {
        warn(
            FederationWarningKind::Limit,
            "LIMIT applies to each source separately, so the merged result can hold more \
             rows than the limit",
        );
    }
    if select.offset > 0 {
        warn(
            FederationWarningKind::Offset,
            "OFFSET skips rows within each source separately, not across the merged result",
        );
    }
    if !select.order.is_empty() {
        warn(
            FederationWarningKind::OrderBy,
            "ORDER BY sorts each source's rows; the merged result is in source order, \
             not globally sorted",
        );
    }
    out
}

/// A federated query failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederateError {
    /// The query did not parse.
    Query(String),
    /// A source failed to evaluate it. No partial result is returned.
    Source {
        /// The failing source's name.
        name: String,
        /// Its error text.
        message: String,
    },
}

impl std::fmt::Display for FederateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query(m) => write!(f, "{m}"),
            Self::Source { name, message } => write!(f, "{name}: {message}"),
        }
    }
}

impl std::error::Error for FederateError {}

/// Run `query` against every source and merge the results.
///
/// With `route` on, a source is skipped when the query pins at least one
/// concrete predicate, the source reports a predicate set, and that set is
/// disjoint from the query's. `on_source` is called after each source is
/// handled (pruned or evaluated), so a caller can print progress or time it.
pub fn federate<S: FederationSource>(
    sources: &[S],
    query: &str,
    route: bool,
    mut on_source: impl FnMut(&SourceReport),
) -> Result<FederatedOutput, FederateError> {
    let (query_preds, form) =
        query_predicates_and_form(query).map_err(|e| FederateError::Query(e.to_string()))?;

    let mut merge = FederationMerge::default();
    let mut reports = Vec::with_capacity(sources.len());
    for source in sources {
        let mut routing_error = None;
        let mut pruned = false;
        if route && !query_preds.is_empty() {
            match source.predicates() {
                Ok(Some(preds)) => pruned = query_preds.is_disjoint(&preds),
                Ok(None) => {}
                Err(e) => routing_error = Some(e),
            }
        }
        let contributed = if pruned {
            0
        } else {
            let out = source
                .eval(query)
                .map_err(|message| FederateError::Source {
                    name: source.name().to_string(),
                    message,
                })?;
            merge.absorb(out)
        };
        let report = SourceReport {
            name: source.name().to_string(),
            pruned,
            contributed,
            routing_error,
        };
        on_source(&report);
        reports.push(report);
    }
    // Every source pruned: no result told the merge its shape, and a false ASK
    // or an empty CONSTRUCT must not come back as an empty SELECT.
    merge.expect_form(form);
    // One source is exact: nothing is split, so nothing is per-source.
    let warnings = if sources.len() > 1 {
        federation_warnings(query)
    } else {
        Vec::new()
    };
    Ok(FederatedOutput {
        output: merge.into_output(),
        sources: reports,
        warnings,
    })
}

/// Term-level merge accumulator: unions SELECT rows (deduped), ORs ASK results
/// and unions CONSTRUCT triples (deduped), all in stable insertion order. The
/// output kind is fixed by the first absorbed result.
///
/// [`federate`] is built on this. Use it directly when the caller evaluates the
/// sources itself, for example a browser that fetches each one asynchronously.
#[derive(Default)]
pub struct FederationMerge {
    kind: Option<OutKind>,
    select_vars: Vec<String>,
    select_rows: Vec<Binding>,
    select_seen: BTreeSet<String>,
    ask_any: bool,
    construct: Vec<(String, String, String)>,
    construct_seen: BTreeSet<(String, String, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum OutKind {
    Select,
    Ask,
    Construct,
}

impl FederationMerge {
    /// Name the query form (`"ask"`, `"construct"`, anything else is SELECT)
    /// for a merge that absorbed nothing, so an empty result keeps its shape.
    /// No effect once a source has answered.
    pub(crate) fn expect_form(&mut self, form: &str) {
        self.kind.get_or_insert(match form {
            "ask" => OutKind::Ask,
            "construct" | "describe" => OutKind::Construct,
            _ => OutKind::Select,
        });
    }

    /// Fold one source's result in; return how many *new* rows/triples it added
    /// (for ASK: 1 if it answered true, else 0).
    pub fn absorb(&mut self, out: QueryOutput) -> usize {
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
        }
    }

    /// Finalize into one merged [`QueryOutput`]. With nothing absorbed, an empty
    /// SELECT.
    pub fn into_output(self) -> QueryOutput {
        match self.kind {
            Some(OutKind::Ask) => QueryOutput::Ask(self.ask_any),
            Some(OutKind::Construct) => QueryOutput::Construct(self.construct),
            Some(OutKind::Select) | None => QueryOutput::Select(self.select_vars, self.select_rows),
        }
    }
}

/// A canonical, order-independent string key for a SELECT solution row over the
/// given variable order, used to dedup identical rows across sources.
fn row_key(vars: &[String], row: &Binding) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        build_pyramid_meta, write_dataset, DictionaryBuilder, GraphIndexBuilder,
        DEFAULT_TILE_BUDGET,
    };

    const CITES: &str = "<http://purl.org/spar/cito/cites>";
    const DATE: &str = "<http://purl.org/dc/terms/date>";

    fn build(triples: &[(&str, &str, &str)]) -> Rete {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<(u32, u32, u32)> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let mut ib = GraphIndexBuilder::new();
        for &t in &ids {
            ib.push(t);
        }
        let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
        let bytes = write_dataset(&dict, &ib.build(), &[], false, &meta, levels);
        Rete::open(&bytes).unwrap()
    }

    /// A source that fails, to prove an error is not swallowed into fewer rows.
    struct Failing;
    impl FederationSource for Failing {
        fn name(&self) -> &str {
            "failing"
        }
        fn eval(&self, _: &str) -> Result<QueryOutput, String> {
            Err("range request failed".into())
        }
    }

    fn two_shards() -> (Rete, Rete) {
        let a = build(&[
            ("<http://a/1>", CITES, "<http://target>"),
            ("<http://shared>", CITES, "<http://target>"),
        ]);
        let b = build(&[
            ("<http://b/1>", CITES, "<http://target>"),
            ("<http://shared>", CITES, "<http://target>"),
        ]);
        (a, b)
    }

    #[test]
    fn select_unions_and_dedups_across_sources() {
        let (a, b) = two_shards();
        let srcs = [ReteSource::new("a", &a), ReteSource::new("b", &b)];
        let q = format!("SELECT ?c WHERE {{ ?c {CITES} <http://target> }}");
        let got = federate(&srcs, &q, true, |_| {}).unwrap();
        let QueryOutput::Select(_, rows) = got.output else {
            panic!("expected SELECT")
        };
        let cs: BTreeSet<_> = rows.iter().map(|r| r["c"].clone()).collect();
        assert_eq!(cs.len(), 3, "rows: {rows:?}");
        // a adds 2, b adds only its own (the shared row is a duplicate).
        assert_eq!(got.sources[0].contributed, 2);
        assert_eq!(got.sources[1].contributed, 1);
    }

    #[test]
    fn routing_prunes_a_predicate_disjoint_source() {
        let a = build(&[("<http://a/1>", CITES, "<http://target>")]);
        let b = build(&[("<http://b/1>", DATE, "\"2021\"")]);
        let srcs = [ReteSource::new("a", &a), ReteSource::new("b", &b)];
        let q = format!("SELECT ?c WHERE {{ ?c {CITES} <http://target> }}");

        let mut seen = Vec::new();
        let on = federate(&srcs, &q, true, |r| seen.push((r.name.clone(), r.pruned))).unwrap();
        assert_eq!(seen, [("a".into(), false), ("b".into(), true)]);
        assert!(on.sources[1].pruned);

        let off = federate(&srcs, &q, false, |_| {}).unwrap();
        assert!(off.sources.iter().all(|s| !s.pruned));
    }

    #[test]
    fn a_fully_pruned_query_keeps_its_shape() {
        let a = build(&[("<http://a/1>", DATE, "\"2021\"")]);
        let srcs = [ReteSource::new("a", &a)];
        let ask = format!("ASK {{ ?s {CITES} ?o }}");
        let got = federate(&srcs, &ask, true, |_| {}).unwrap();
        assert!(got.sources[0].pruned);
        assert!(matches!(got.output, QueryOutput::Ask(false)));
        let con = format!("CONSTRUCT {{ ?s {CITES} ?o }} WHERE {{ ?s {CITES} ?o }}");
        let got = federate(&srcs, &con, true, |_| {}).unwrap();
        assert!(matches!(got.output, QueryOutput::Construct(ref t) if t.is_empty()));
    }

    #[test]
    fn a_variable_predicate_query_prunes_nothing() {
        let a = build(&[("<http://a/1>", CITES, "<http://target>")]);
        let b = build(&[("<http://b/1>", DATE, "\"2021\"")]);
        let srcs = [ReteSource::new("a", &a), ReteSource::new("b", &b)];
        let got = federate(&srcs, "SELECT ?s WHERE { ?s ?p ?o }", true, |_| {}).unwrap();
        assert!(got.sources.iter().all(|s| !s.pruned));
    }

    #[test]
    fn ask_is_or_and_construct_unions() {
        let (a, b) = two_shards();
        let srcs = [ReteSource::new("a", &a), ReteSource::new("b", &b)];

        let ask = format!("ASK {{ <http://b/1> {CITES} <http://target> }}");
        let got = federate(&srcs, &ask, true, |_| {}).unwrap();
        assert!(matches!(got.output, QueryOutput::Ask(true)));

        let con = format!(
            "CONSTRUCT {{ ?c {CITES} <http://target> }} WHERE {{ ?c {CITES} <http://target> }}"
        );
        let got = federate(&srcs, &con, true, |_| {}).unwrap();
        let QueryOutput::Construct(ts) = got.output else {
            panic!("expected CONSTRUCT")
        };
        assert_eq!(ts.len(), 3);
    }

    #[test]
    fn a_failing_source_is_an_error_not_fewer_rows() {
        let (a, _) = two_shards();
        let ok = ReteSource::new("a", &a);
        let q = format!("SELECT ?c WHERE {{ ?c {CITES} <http://target> }}");
        // Two different source types cannot share a slice, so drive the failing
        // one on its own: the contract is the error, with the source named.
        let err = federate(&[Failing], &q, false, |_| {}).unwrap_err();
        assert_eq!(
            err,
            FederateError::Source {
                name: "failing".into(),
                message: "range request failed".into()
            }
        );
        assert!(federate(&[ok], &q, false, |_| {}).is_ok());
    }

    fn kinds(w: &[FederationWarning]) -> Vec<FederationWarningKind> {
        w.iter().map(|w| w.kind).collect()
    }

    #[test]
    fn an_aggregate_over_two_sources_warns_and_stays_per_source() {
        let (a, b) = two_shards();
        let srcs = [ReteSource::new("a", &a), ReteSource::new("b", &b)];
        let q = format!("SELECT (COUNT(*) AS ?n) WHERE {{ ?c {CITES} <http://target> }}");
        let got = federate(&srcs, &q, true, |_| {}).unwrap();
        assert_eq!(kinds(&got.warnings), [FederationWarningKind::Aggregate]);
        assert!(got.warnings[0].message.contains("one count per source"));
        // Both shards hold two citations, so the rows are 2 and 2 (deduped to 1
        // row, "2"): the merge is not a total of 4. That is what the warning is for.
        let QueryOutput::Select(_, rows) = got.output else {
            panic!("expected SELECT")
        };
        assert!(rows.iter().all(|r| !r["n"].contains('4')), "rows: {rows:?}");
    }

    #[test]
    fn limit_offset_and_order_by_each_warn() {
        let (a, b) = two_shards();
        let srcs = [ReteSource::new("a", &a), ReteSource::new("b", &b)];
        let q = format!(
            "SELECT ?c WHERE {{ ?c {CITES} <http://target> }} ORDER BY ?c LIMIT 1 OFFSET 1"
        );
        let got = federate(&srcs, &q, true, |_| {}).unwrap();
        assert_eq!(
            kinds(&got.warnings),
            [
                FederationWarningKind::Limit,
                FederationWarningKind::Offset,
                FederationWarningKind::OrderBy
            ]
        );
    }

    #[test]
    fn plain_queries_and_single_sources_do_not_warn() {
        let (a, b) = two_shards();
        let two = [ReteSource::new("a", &a), ReteSource::new("b", &b)];
        // DISTINCT is exact: the merge dedups. ASK and CONSTRUCT have none.
        for q in [
            format!("SELECT DISTINCT ?c WHERE {{ ?c {CITES} <http://target> }}"),
            format!("ASK {{ ?c {CITES} <http://target> }}"),
        ] {
            assert!(federate(&two, &q, true, |_| {})
                .unwrap()
                .warnings
                .is_empty());
        }
        // One source is exact even for an aggregate.
        let one = [ReteSource::new("a", &a)];
        let agg = format!("SELECT (COUNT(*) AS ?n) WHERE {{ ?c {CITES} <http://target> }}");
        assert!(federate(&one, &agg, true, |_| {})
            .unwrap()
            .warnings
            .is_empty());
        // The standalone function does not know the source count; it still reports.
        assert_eq!(
            kinds(&federation_warnings(&agg)),
            [FederationWarningKind::Aggregate]
        );
    }

    #[test]
    fn a_bad_query_is_reported_before_any_source_runs() {
        let (a, _) = two_shards();
        let err =
            federate(&[ReteSource::new("a", &a)], "SELECT WHERE {", true, |_| {}).unwrap_err();
        assert!(matches!(err, FederateError::Query(_)), "{err:?}");
    }
}
