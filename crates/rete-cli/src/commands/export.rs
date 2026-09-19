//! The `export` command plus the RDF serialization helpers (Turtle / JSON-LD)
//! shared with the SPARQL CONSTRUCT output and `reason`.

use crate::commands::range_source::{open_local_eager, open_local_ranged};
use crate::commands::render::term_to_json;
use crate::commands::turtle::{Grouping, NamespaceSample, PrefixTable, TurtleWriter, RDF_TYPE};

/// How many statements the namespace prescan looks at before it decides which
/// `@prefix` bindings the document gets.
///
/// The prescan exists so the declarations land in a block at the top of the file
/// instead of being sprinkled through it. It is a *presentation* choice, not a
/// correctness one — a namespace that only shows up after the sample is still
/// abbreviated, its declaration just appears where it is first needed (see
/// `turtle`'s module docs). So this wants to be large enough to see a file's
/// real vocabulary and small enough to be free on a 52 GB input: 100k statements
/// is a few hundred index tiles and the dictionary chunks behind them, and those
/// chunks are exactly the ones the real scan will want next.
const PREFIX_SAMPLE_STATEMENTS: usize = 100_000;

/// At most this many namespaces learned from the data earn a `@prefix` line, on
/// top of the well-known table. Real files use a handful; the cap stops a
/// pathological input from producing a preamble longer than its data.
const MAX_LEARNED_PREFIXES: usize = 32;

/// How many graph slots the prescan looks at. See `sample_namespaces`.
const MAX_SAMPLE_SLOTS: usize = 32;

/// A namespace must appear at least this often in the sample to earn a line.
/// Below it, the `@prefix` line costs more bytes than the abbreviation saves.
const MIN_PREFIX_OCCURRENCES: u64 = 16;

/// Which slice of the dataset `rete export` should write.
///
/// Every field is a **pruning** filter, not a post-hoc row test: they become the
/// triple pattern `Rete::dump_filtered_each` routes on, so exporting one
/// predicate of a 33 GB graph fetches the tiles that predicate lives in and
/// nothing else. See that method for the measured before/after.
#[derive(Default, Clone)]
pub(crate) struct ExportFilter {
    /// `None` = the default graph followed by every named graph (the lossless
    /// N-Quads dump). `Some(None)` = the default graph only. `Some(Some(iri))`
    /// = that named graph only.
    pub graph: Option<Option<String>>,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
}

impl ExportFilter {
    /// The graph slots to write, in order.
    pub(crate) fn slots(&self, rete: &rete_core::Rete) -> Vec<Option<String>> {
        match &self.graph {
            None => std::iter::once(None)
                .chain(rete.graph_names().iter().map(|g| Some((*g).to_string())))
                .collect(),
            Some(None) => vec![None],
            Some(Some(g)) => vec![Some(canonical_graph(rete, g))],
        }
    }

    fn terms(&self) -> (Option<&str>, Option<&str>, Option<&str>) {
        (
            self.subject.as_deref(),
            self.predicate.as_deref(),
            self.object.as_deref(),
        )
    }
}

/// Resolve a user-supplied graph name to the token the file stores. Graph names
/// are canonical N-Triples terms (`<iri>`), but a shell user types the bare IRI;
/// accept either, preferring an exact match. Same rule as the wasm client's
/// `canonical_graph_name`, so `--graph` behaves identically in both.
pub(crate) fn canonical_graph(rete: &rete_core::Rete, name: &str) -> String {
    if rete.graph_names().contains(&name) {
        return name.to_string();
    }
    if name.starts_with('<') || name.starts_with("_:") {
        return name.to_string();
    }
    format!("<{name}>")
}

/// Canonicalize a user-supplied filter term to the N-Triples token the
/// dictionary stores: `<iri>`, `"literal"`, `"lit"@en`, `"lit"^^<dt>`, `_:b`
/// pass through, and a bare IRI gets its angle brackets. A term the dictionary
/// does not know matches nothing — which is a legitimate answer, not an error.
pub(crate) fn canonical_term(term: &str) -> String {
    if term.starts_with('<') || term.starts_with('"') || term.starts_with("_:") {
        term.to_string()
    } else {
        format!("<{term}>")
    }
}

/// `rete export <file> --format <fmt>`: write the graph — or a filtered slice of
/// it — as N-Quads, Turtle, or JSON-LD.
///
/// `sanitize_iris` percent-encodes IRIs that are outside the N-Triples/N-Quads
/// `IRIREF` grammar and RFC 3987 (see `rete_core::iri`), so the dump is
/// something a strict store will actually load. It is **opt-in**: escaping
/// changes the IRI, so a sanitized dump no longer joins against the file it came
/// from. What it changed goes to stderr — stdout is the dump.
pub(crate) fn export(
    file: &str,
    format: &str,
    filter: &ExportFilter,
    sanitize_iris: bool,
    no_prefixes: bool,
    in_memory: bool,
    memory_budget_mb: Option<u64>,
) -> anyhow::Result<()> {
    // Peak RSS is bounded by `--memory-budget-mb` (default 4096): the ranged
    // reader's dictionary chunk cache and index tile cache are capped to a share
    // of it and evict least-recently-used bodies, so a full dump no longer keeps
    // the whole decompressed dictionary resident (the old ~4.3 GB-per-1.5-GB-file
    // floor). `0` means unlimited. `--in-memory` reads the whole file eagerly, so
    // it is unbounded by definition and IGNORES the budget (with a note if one
    // was passed) — both are byte-for-byte the same dump at any budget, since the
    // reader choice and cache cap change only memory, never the scan or the
    // resolved terms. See `range_source` and `Rete::set_memory_budget`.
    let budget: Option<u64> = if in_memory {
        if memory_budget_mb.is_some() {
            eprintln!("note: --in-memory reads the whole file, so --memory-budget-mb is ignored");
        }
        None
    } else {
        match memory_budget_mb.unwrap_or(rete_core::DEFAULT_EXPORT_BUDGET_MB) {
            0 => None, // 0 = unlimited (no eviction), like --in-memory's cap
            mb => Some(mb.saturating_mul(1 << 20)),
        }
    };
    let mut rete = if in_memory {
        open_local_eager(file)?
    } else {
        open_local_ranged(file, budget)?
    };
    let (s, p, o) = filter.terms();
    // One report for the whole dump, so the summary is a single total across
    // every graph slot. `None` when the flag is off: the terms then take the
    // zero-cost `Cow::Borrowed` path and the export is byte-identical to before.
    let mut iris = sanitize_iris.then(rete_core::iri::IriReport::default);
    match format {
        // N-Quads: lossless dump of the selected graph(s).
        // Streamed (dump_filtered_each) so a 100M+ triple file serializes in
        // constant memory instead of materializing every term into a Vec.
        "nq" => {
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut out = std::io::BufWriter::new(stdout.lock());
            for slot in filter.slots(&rete) {
                match &slot {
                    None => rete.dump_filtered_each(None, s, p, o, |s, p, o| {
                        let (s, p, o) = clean(&mut iris, s, p, o);
                        let _ = writeln!(out, "{s} {p} {o} .");
                    }),
                    Some(g) => {
                        // The graph term labels every line of this slot, so it
                        // is sanitized — and therefore counted — ONCE per graph,
                        // not once per quad. The lookup keeps the original
                        // token: it is the file's key, not the dump's text.
                        let label = match iris.as_mut() {
                            Some(r) => r.sanitize(g).into_owned(),
                            None => g.clone(),
                        };
                        rete.dump_filtered_each(Some(g), s, p, o, |s, p, o| {
                            let (s, p, o) = clean(&mut iris, s, p, o);
                            let _ = writeln!(out, "{s} {p} {o} {label} .");
                        });
                        // This slot is done: drop the graph's decoded index so a
                        // many-graph dump holds one graph's tiles at a time, not
                        // all of them (the next floor once the dictionary is
                        // bounded). No borrow of `rete` outlives the dump above.
                        rete.release_named_graph(g);
                    }
                }
            }
            out.flush()?;
            if std::env::var("RETE_OPEN_DEBUG").is_ok() {
                let d = rete.dict_cache_stats();
                eprintln!(
                    "[export] dict cache: decoded_chunks={} decoded_bytes={} evictions={} resident_bytes={} hits={} misses={}",
                    d.decoded_chunks, d.decoded_bytes, d.evictions, d.resident_bytes, d.hits, d.misses,
                );
            }
        }
        // Turtle and TriG: the same streaming, prefix-compressed writer, differing
        // only in whether statements are wrapped in `GRAPH <g> { … }` blocks.
        "ttl" | "trig" => {
            // Turtle has no default-vs-named distinction, so it serializes ONE
            // graph, chosen by the ladder in `select_single_graph`. TriG carries
            // the graph term, so it writes every slot — it is the lossless
            // compact counterpart of N-Quads, and the reason `--format trig`
            // exists.
            let slots: Vec<Option<String>> = if format == "trig" {
                filter.slots(&rete)
            } else {
                vec![select_single_graph(&rete, filter, "Turtle")?]
            };

            // Which prefixes the document declares, learned from a bounded
            // prescan of the very scan about to be written. `--no-prefixes`
            // turns abbreviation off entirely, which is the escape hatch for a
            // consumer that cannot resolve QNames.
            let table = if no_prefixes {
                PrefixTable::empty()
            } else {
                let sample = sample_namespaces(&rete, &slots, s, p, o, sanitize_iris);
                PrefixTable::from_sample(&sample, MAX_LEARNED_PREFIXES, MIN_PREFIX_OCCURRENCES)
            };

            // Subject grouping — writing a subject once and hanging its
            // predicate/object list off it — is only sound if consecutive
            // statements actually arrive grouped by subject. That is a property
            // of the permutation the scan routes to, so it is READ OFF the
            // engine's own plan rather than assumed. When the answer is no, the
            // writer degrades to one statement per line instead of buffering the
            // graph to create the grouping, which is the whole point of this path.
            let grouping = subject_grouping(&rete, &slots, s, p, o);

            match write_turtle_stream(&mut rete, &slots, table, grouping, s, p, o, &mut iris) {
                Ok(()) => {}
                // A closed downstream pipe is how `rete export … | head` ends,
                // not a failure. The nq arm reaches the same outcome by
                // discarding every write error; this one propagates them, so it
                // has to name the benign case rather than inherit it.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
        // JSON-LD has no streaming form here: the expanded serialization is one
        // JSON array, and writing it incrementally would mean hand-rolling the
        // encoder. It keeps the eager path, and the same single-graph ladder as
        // Turtle so `--graph` behaves identically across the two.
        "jsonld" => {
            let g = select_single_graph(&rete, filter, "JSON-LD")?;
            let mut triples = rete.query_in_graph(g.as_deref(), s, p, o);
            if let Some(report) = iris.as_mut() {
                for t in triples.iter_mut() {
                    let (s, p, o) = (
                        report.sanitize(&t.0).into_owned(),
                        report.sanitize(&t.1).into_owned(),
                        report.sanitize(&t.2).into_owned(),
                    );
                    *t = (s, p, o);
                }
            }
            println!("{}", export_jsonld(&triples));
        }
        other => anyhow::bail!("unknown export format: {other}"),
    }
    if let Some(report) = iris.as_ref() {
        crate::commands::iri_report::report_sanitized(report);
    }
    Ok(())
}

/// Sanitize one quad's three terms when the flag is on, or hand them straight
/// back when it is off. Returned as owned `String`s only where a repair
/// happened; `Cow` keeps the untouched (overwhelming) majority allocation-free.
fn clean<'a>(
    report: &mut Option<rete_core::iri::IriReport>,
    s: &'a str,
    p: &'a str,
    o: &'a str,
) -> (
    std::borrow::Cow<'a, str>,
    std::borrow::Cow<'a, str>,
    std::borrow::Cow<'a, str>,
) {
    use std::borrow::Cow;
    match report.as_mut() {
        Some(r) => (r.sanitize(s), r.sanitize(p), r.sanitize(o)),
        None => (Cow::Borrowed(s), Cow::Borrowed(p), Cow::Borrowed(o)),
    }
}

/// Stream one graph selection as Turtle or TriG.
///
/// Split out of `export` so the whole write is one `io::Result`, which lets the
/// caller distinguish a real I/O failure from a downstream pipe closing — and so
/// that every `?` here is a write error rather than a mix of write errors and
/// argument errors.
///
/// Errors from inside `dump_filtered_each`'s callback are parked in `err` rather
/// than returned, because the callback cannot fail the scan; the first failure
/// latches and the remaining statements are skipped, so a full disk stops
/// writing instead of spinning through a 50 GB graph discarding every line.
#[allow(clippy::too_many_arguments)]
fn write_turtle_stream(
    rete: &mut rete_core::Rete,
    slots: &[Option<String>],
    table: PrefixTable,
    grouping: Grouping,
    s: Option<&str>,
    p: Option<&str>,
    o: Option<&str>,
    iris: &mut Option<rete_core::iri::IriReport>,
) -> std::io::Result<()> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut w = TurtleWriter::new(std::io::BufWriter::new(stdout.lock()), table, grouping);
    w.declare_all()?;
    if std::env::var("RETE_OPEN_DEBUG").is_ok() {
        eprintln!(
            "[export] {} prefix binding(s), grouping={grouping:?}",
            w.table().len()
        );
        for (prefix, ns) in w.table().bindings() {
            eprintln!("[export]   {prefix}: {ns}");
        }
    }
    let mut err: std::io::Result<()> = Ok(());
    for slot in slots {
        match slot {
            None => {
                rete.dump_filtered_each(None, s, p, o, |s, p, o| {
                    if err.is_err() {
                        return;
                    }
                    let (s, p, o) = clean(iris, s, p, o);
                    err = w.write_triple(&s, &p, &o);
                });
            }
            Some(g) => {
                // Same rule as the nq arm: the graph term labels the whole block,
                // so it is sanitized — and therefore counted — once per graph
                // rather than once per statement.
                let label = match iris.as_mut() {
                    Some(r) => r.sanitize(g).into_owned(),
                    None => g.clone(),
                };
                w.begin_graph(&label)?;
                rete.dump_filtered_each(Some(g), s, p, o, |s, p, o| {
                    if err.is_err() {
                        return;
                    }
                    let (s, p, o) = clean(iris, s, p, o);
                    err = w.write_triple(&s, &p, &o);
                });
                w.end_graph()?;
                // Drop this graph's decoded index before the next slot, so a
                // many-graph dump holds one graph's tiles at a time.
                rete.release_named_graph(g);
            }
        }
        // Take the error rather than move out of the accumulator: the next slot
        // reuses it.
        std::mem::replace(&mut err, Ok(()))?;
    }
    // `finish` hands the sink back rather than dropping it: a `BufWriter` dropped
    // on the floor swallows the error from its final write, which is how a
    // truncated dump gets mistaken for a complete one.
    w.finish()?.flush()
}

/// Pick the one graph a single-graph format (Turtle, JSON-LD) will write.
///
/// Turtle and JSON-LD carry no graph term, so writing several graphs into one
/// document would silently merge them — a lossy operation that looks like a
/// successful one. The rule is therefore "a named graph can be specified;
/// otherwise the default graph", spelled out so the choice is never silent:
///
/// * `--graph <iri>` — exactly that graph. If the file does not have it, fail and
///   name the graphs it does have, because the alternative is an empty dump that
///   is indistinguishable from an empty graph.
/// * `--graph ''` — the default graph, explicitly.
/// * no `--graph`, default graph has content — the default graph, noting on
///   stderr that any named graphs were left out and that `--format trig` keeps
///   them.
/// * no `--graph`, default graph empty, exactly one named graph — that graph. A
///   quads file whose data all lives in one named graph is the ordinary shape of
///   a TriG dump, and refusing it would be pedantry.
/// * no `--graph`, default graph empty, several named graphs — the one genuinely
///   ambiguous case. Fail, list them, and point at both ways out.
///
/// Every branch reports its choice on stderr; stdout stays the dump.
fn select_single_graph(
    rete: &rete_core::Rete,
    filter: &ExportFilter,
    format_label: &str,
) -> anyhow::Result<Option<String>> {
    let names: Vec<String> = rete
        .graph_names()
        .iter()
        .map(|g| (*g).to_string())
        .collect();
    match &filter.graph {
        Some(Some(g)) => {
            let canon = canonical_graph(rete, g);
            if !names.contains(&canon) {
                anyhow::bail!(
                    "no named graph {canon} in this file.\nnamed graphs: {}\n\
                     hint: `--format trig` writes every graph, losslessly.",
                    graph_list(&names)
                );
            }
            eprintln!("note: {format_label} export of named graph {canon}");
            Ok(Some(canon))
        }
        Some(None) => {
            eprintln!("note: {format_label} export of the default graph");
            Ok(None)
        }
        None => {
            let (s, p, o) = filter.terms();
            // One pulled row is enough to know the default graph has content, and
            // `query_iter` stops there — this does not scan the graph to find out.
            if rete.query_iter(None, s, p, o).next().is_some() {
                eprintln!("note: {format_label} export of the default graph");
                if !names.is_empty() {
                    eprintln!(
                        "note: {} named graph(s) are NOT included — {format_label} has no graph \
                         term. Use `--format trig` for a lossless dump, or `--graph <iri>` to \
                         pick one.",
                        names.len()
                    );
                }
                return Ok(None);
            }
            match names.len() {
                0 => {
                    eprintln!("note: {format_label} export of the default graph (which is empty)");
                    Ok(None)
                }
                1 => {
                    eprintln!(
                        "note: the default graph is empty; exporting the only named graph, {}",
                        names[0]
                    );
                    Ok(Some(names[0].clone()))
                }
                _ => anyhow::bail!(
                    "the default graph is empty and this file has {} named graphs, so there is no \
                     single graph to write as {format_label}.\nnamed graphs: {}\n\
                     hint: `--graph <iri>` picks one, or `--format trig` writes them all, \
                     losslessly.",
                    names.len(),
                    graph_list(&names)
                ),
            }
        }
    }
}

/// The graph names for a diagnostic, capped so a file with thousands of graphs
/// does not turn one error message into a screenful.
fn graph_list(names: &[String]) -> String {
    const SHOWN: usize = 12;
    let mut s = names
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        s.push_str(&format!(", ... ({} more)", names.len() - SHOWN));
    }
    s
}

/// Will consecutive statements of this scan share a subject?
///
/// Asked of the engine rather than assumed. `dump_filtered_each` streams in the
/// **routed permutation's** order, and which permutation that is depends on which
/// of `s`/`p`/`o` are bound. `ScanPlan::permutation` reports the choice before a
/// single tile is fetched, and its `name()` — `"SPO"`, `"POS"`, … — spells out
/// the column order directly.
///
/// The test is then: every column the permutation sorts on *before* the subject
/// must be bound. A bound column is one constant for the whole scan, so it cannot
/// separate two statements about the same subject; an unbound one can and will.
/// Concretely: all three unbound routes to SPO and is grouped; a bound predicate
/// routes to POS and is **not** (it is sorted by object, then subject); a bound
/// object routes to OSP and *is*, because the object is constant and the next
/// column is the subject.
///
/// Reading the order out of `name()` keeps this change inside `rete-cli`. The
/// same order is available as `IndexPermutation::roles()`, but that is
/// `pub(crate)` to `rete-core`, and widening it would pull a serialization change
/// into the crate whose wasm artifacts are provenance-checked — a
/// disproportionate blast radius for three characters of routing metadata.
fn subject_grouping(
    rete: &rete_core::Rete,
    slots: &[Option<String>],
    s: Option<&str>,
    p: Option<&str>,
    o: Option<&str>,
) -> Grouping {
    // Routing depends only on which components are bound, which is the same for
    // every slot, so the first slot that yields a plan answers for all of them.
    // (A slot yields none when the graph is absent or a bound term is unknown —
    // then there is nothing to write and the answer does not matter.)
    let Some(scan) = slots
        .iter()
        .find_map(|slot| rete.dump_plan(slot.as_deref(), s, p, o).scan)
    else {
        return Grouping::BySubject;
    };
    for col in scan.permutation.name().chars() {
        match col {
            'S' => return Grouping::BySubject,
            'P' if p.is_some() => continue,
            'O' if o.is_some() => continue,
            _ => return Grouping::PerStatement,
        }
    }
    Grouping::PerStatement
}

/// Learn the document's namespaces from a bounded prefix of the scan.
///
/// Pulls at most [`PREFIX_SAMPLE_STATEMENTS`] statements, split across the graph
/// slots so a TriG dump whose graphs use different vocabularies sees all of them.
/// `query_iter` is the lazy pull form of the same routed scan, so `take` really
/// does stop early: this costs the tiles and dictionary chunks of the sample and
/// nothing more, and those are the ones the real scan is about to want anyway.
///
/// With `--sanitize-iris` the sample is sanitized too, through a throwaway report
/// — the namespaces that matter are the ones that will actually be written, and
/// counting the repairs twice would make the user-facing summary wrong.
fn sample_namespaces(
    rete: &rete_core::Rete,
    slots: &[Option<String>],
    s: Option<&str>,
    p: Option<&str>,
    o: Option<&str>,
    sanitize_iris: bool,
) -> NamespaceSample {
    let mut sample = NamespaceSample::default();
    let mut scratch = sanitize_iris.then(rete_core::iri::IriReport::default);
    // Spread the budget across slots, but over at most MAX_SAMPLE_SLOTS of them.
    // Each slot costs a routed scan to set up, and a file can have half a million
    // named graphs (switzerland-fedlex has 497,905) — dividing the budget by that
    // would turn a bounded sample into half a million one-row scans. A few dozen
    // graphs is already far more vocabulary than a prefix table can hold.
    let n = slots.len().min(MAX_SAMPLE_SLOTS).max(1);
    let per_slot = PREFIX_SAMPLE_STATEMENTS.div_ceil(n);
    for slot in slots.iter().take(MAX_SAMPLE_SLOTS) {
        for (ts, tp, to) in rete.query_iter(slot.as_deref(), s, p, o).take(per_slot) {
            // `rdf:type` is always written as the keyword `a`, so observing it
            // must not be what earns `rdf:` a declaration line nothing uses.
            let tp = (tp != RDF_TYPE).then_some(tp);
            match scratch.as_mut() {
                Some(r) => {
                    sample.observe(&r.sanitize(&ts));
                    if let Some(tp) = &tp {
                        sample.observe(&r.sanitize(tp));
                    }
                    sample.observe(&r.sanitize(&to));
                }
                None => {
                    sample.observe(&ts);
                    if let Some(tp) = &tp {
                        sample.observe(tp);
                    }
                    sample.observe(&to);
                }
            }
        }
    }
    sample
}

/// Serialize a triple list to Turtle in memory — the eager wrapper around the
/// streaming [`TurtleWriter`], for a caller that already holds every triple.
///
/// `rete export --format ttl` does **not** come through here; it streams. This is
/// for `rete reason --materialize --format ttl`, whose input is an inferred
/// closure that is a `Vec` by construction, so there is nothing to stream from.
/// Routing it through the same writer means the tool has one Turtle serializer
/// rather than two that can drift apart.
///
/// The list is sorted first: subject grouping needs grouped input, and a
/// reasoner emits in derivation order.
pub(crate) fn export_turtle(triples: &[(String, String, String)]) -> String {
    let mut sorted: Vec<&(String, String, String)> = triples.iter().collect();
    sorted.sort();
    let mut sample = NamespaceSample::default();
    for (s, p, o) in &sorted {
        sample.observe(s);
        if p != RDF_TYPE {
            sample.observe(p);
        }
        sample.observe(o);
    }
    let table = PrefixTable::from_sample(&sample, MAX_LEARNED_PREFIXES, MIN_PREFIX_OCCURRENCES);
    let mut w = TurtleWriter::new(Vec::new(), table, Grouping::BySubject);
    // The sink is a `Vec<u8>`, so none of these writes can fail.
    let _ = w.declare_all();
    for (s, p, o) in sorted {
        let _ = w.write_triple(s, p, o);
    }
    String::from_utf8(w.finish().unwrap_or_default()).unwrap_or_default()
}

/// Serialize a default-graph triple list to expanded JSON-LD: an array of node
/// objects keyed by `@id`, each predicate mapping to an array of value objects
/// (`{"@id": …}` for IRIs/bnodes, `{"@value": …}` plus `@type`/`@language` for
/// literals). This is the canonical expanded form, valid against the JSON-LD 1.1
/// algorithm with no `@context`.
pub(crate) fn export_jsonld(triples: &[(String, String, String)]) -> String {
    use serde_json::{json, Map, Value};
    use std::collections::BTreeMap;

    // subject id → predicate iri → [value objects], stable (sorted) order.
    let mut nodes: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();
    for (s, p, o) in triples {
        let id = node_id(s);
        let pred = p
            .strip_prefix('<')
            .and_then(|x| x.strip_suffix('>'))
            .unwrap_or(p)
            .to_string();
        nodes
            .entry(id)
            .or_default()
            .entry(pred)
            .or_default()
            .push(object_to_jsonld(o));
    }

    let arr: Vec<Value> = nodes
        .into_iter()
        .map(|(id, preds)| {
            let mut obj = Map::new();
            obj.insert("@id".into(), json!(id));
            for (pred, vals) in preds {
                obj.insert(pred, Value::Array(vals));
            }
            Value::Object(obj)
        })
        .collect();
    serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_default()
}

/// The JSON-LD `@id` string for a subject/IRI-or-bnode token: the bare IRI for
/// `<iri>`, the `_:b` token verbatim for a blank node.
fn node_id(token: &str) -> String {
    token
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .map(str::to_string)
        .unwrap_or_else(|| token.to_string())
}

/// Classify an object token into a JSON-LD value object (`@id` for IRIs/bnodes,
/// `@value` + optional `@type`/`@language` for literals). Reuses `term_to_json`'s
/// classification so escaping/datatype/lang handling stays consistent.
fn object_to_jsonld(token: &str) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    let t = term_to_json(token);
    match t["type"].as_str() {
        Some("uri") => json!({ "@id": t["value"] }),
        Some("bnode") => json!({ "@id": format!("_:{}", t["value"].as_str().unwrap_or("")) }),
        _ => {
            let mut obj = Map::new();
            obj.insert("@value".into(), t["value"].clone());
            if let Some(dt) = t.get("datatype") {
                obj.insert("@type".into(), dt.clone());
            }
            if let Some(lang) = t.get("xml:lang") {
                obj.insert("@language".into(), lang.clone());
            }
            Value::Object(obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_triples() -> Vec<(String, String, String)> {
        vec![
            (
                "<http://ex/Alice>".into(),
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".into(),
                "<http://ex/Person>".into(),
            ),
            (
                "<http://ex/Alice>".into(),
                "<http://ex/age>".into(),
                "\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>".into(),
            ),
            (
                "<http://ex/Alice>".into(),
                "<http://ex/label>".into(),
                "\"héllo \\\"quote\\\"\"@en".into(),
            ),
            (
                "<http://ex/Alice>".into(),
                "<http://ex/knows>".into(),
                "_:b0".into(),
            ),
        ]
    }

    #[test]
    fn turtle_export_groups_and_abbreviates() {
        let ttl = export_turtle(&sample_triples());
        // One subject block; predicates in sorted order, `rdf:type` shown as `a`.
        assert!(ttl.contains("<http://ex/Alice>\n"), "got:\n{ttl}");
        assert_eq!(
            ttl.matches("<http://ex/Alice>").count(),
            1,
            "the subject is written once, not once per statement:\n{ttl}"
        );
        assert!(ttl.contains("a <http://ex/Person>"), "got:\n{ttl}");
        // The datatype IRI abbreviates; the lexical form is untouched.
        assert!(
            ttl.contains("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> ."),
            "got:\n{ttl}"
        );
        assert!(ttl.contains("\"30\"^^xsd:integer"), "got:\n{ttl}");
        // `rdf:type` became `a`, so the rdf prefix is never needed — and so is
        // never declared.
        assert!(!ttl.contains("@prefix rdf:"), "got:\n{ttl}");
        // Lang tag + escaped quote preserved exactly.
        assert!(ttl.contains("\"héllo \\\"quote\\\"\"@en"), "got:\n{ttl}");
        // Blank node passes through; the block ends with ` .`.
        assert!(ttl.contains("_:b0"));
        assert!(ttl.trim_end().ends_with(" ."));
    }

    #[test]
    fn jsonld_export_expanded_shape() {
        let v: serde_json::Value = serde_json::from_str(&export_jsonld(&sample_triples())).unwrap();
        let node = &v[0];
        assert_eq!(node["@id"], "http://ex/Alice");
        // IRI object → {"@id": …}; rdf:type is a normal predicate IRI (not @type).
        assert_eq!(
            node["http://www.w3.org/1999/02/22-rdf-syntax-ns#type"][0]["@id"],
            "http://ex/Person"
        );
        // Typed literal → @value + @type, with the unescaped lexical form.
        let age = &node["http://ex/age"][0];
        assert_eq!(age["@value"], "30");
        assert_eq!(age["@type"], "http://www.w3.org/2001/XMLSchema#integer");
        // Lang-tagged literal → @value + @language; escapes resolved to chars.
        let label = &node["http://ex/label"][0];
        assert_eq!(label["@value"], "héllo \"quote\"");
        assert_eq!(label["@language"], "en");
        // Blank node object → {"@id": "_:b0"}.
        assert_eq!(node["http://ex/knows"][0]["@id"], "_:b0");
    }
}
