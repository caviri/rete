//! `rete` — build, inspect, and query Rete graph files.

mod cypher;
mod http;
mod ntriples;

use clap::{Parser, Subcommand};
use rete_core::{
    build_pyramid_meta, eval_bgp, eval_query, query_predicates, write_dataset, CountingReader,
    DictionaryBuilder, GraphIndexBuilder, PatternTerm, QueryOutput, RangeReader, Rete, SliceReader,
    SummaryView, TriplePattern, CODEC_ZSTD, DEFAULT_TILE_BUDGET,
};

use crate::http::HttpRangeReader;

#[derive(Parser)]
#[command(name = "rete", version, about = "Cloud-native RDF graph files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a `.rete` file from one or more RDF inputs (merged into one file).
    ///
    /// Format is by extension: `.nt`/`.nq`/`.ttl`. Use `-` to read stdin
    /// (defaults to N-Triples). `--format` overrides detection for all inputs.
    /// Example: `cat *.nt | rete build - -o out.rete`.
    Build {
        /// Input files (or `-` for stdin); multiple are merged.
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<String>,
        /// Output `.rete` file.
        #[arg(short, long)]
        output: String,
        /// Force input format for all inputs: nt | nq | ttl.
        #[arg(long, value_parser = ["nt", "nq", "ttl"])]
        format: Option<String>,
    },
    /// Validate that RDF input(s) parse as well-formed N-Triples/N-Quads/Turtle,
    /// without building. Reports counts, or fails with a parse error.
    Validate {
        /// Input files (or `-` for stdin).
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<String>,
        /// Force input format for all inputs: nt | nq | ttl.
        #[arg(long, value_parser = ["nt", "nq", "ttl"])]
        format: Option<String>,
    },
    /// Print the header of a `.rete` file.
    Info {
        /// Path to the `.rete` file.
        file: String,
    },
    /// Human-friendly overview: size, counts, graphs, pyramid, top predicates.
    Stats {
        /// Path to the `.rete` file.
        file: String,
    },
    /// Verify a file's content hash (detects corruption/truncation).
    Verify {
        /// Path to the `.rete` file.
        file: String,
    },
    /// List the named graphs in a dataset.
    Graphs {
        /// Path to the `.rete` file.
        file: String,
    },
    /// Export the dataset. `nq` (default) emits N-Quads (default graph + named
    /// graphs, lossless). `ttl` emits Turtle and `jsonld` expanded JSON-LD — both
    /// serialize the **default graph only** (named graphs are skipped, since
    /// Turtle/JSON-LD have no default-vs-named distinction here).
    Export {
        /// Path to the `.rete` file.
        file: String,
        /// Output format: nq | ttl | jsonld.
        #[arg(long, value_parser = ["nq", "ttl", "jsonld"], default_value = "nq")]
        format: String,
    },
    /// Query a triple pattern. Unspecified positions are variables.
    ///
    /// Terms are matched as canonical N-Triples tokens, e.g.
    /// `--subject '<http://ex/Alice>'` or `--object '"30"'`.
    Query {
        /// Path to the `.rete` file.
        file: String,
        #[arg(short, long)]
        subject: Option<String>,
        #[arg(short, long)]
        predicate: Option<String>,
        #[arg(short, long)]
        object: Option<String>,
    },
    /// Print the pyramid summary graph (community-to-community relations).
    Summary {
        /// Path to the `.rete` file.
        file: String,
    },
    /// Recompute the Louvain communities and expose, per community, its member
    /// subjects and the literal text of its triples — the per-community text
    /// corpus for downstream topic modeling (see `docs/topic-modeling.md`).
    ///
    /// `--json` emits `[{community, size, members:[<iri>…], text:[lexical…]}]`,
    /// the corpus an LDA script consumes (`scripts/lda_topics.py`).
    Communities {
        /// Path to the `.rete` file.
        file: String,
        /// Emit JSON (membership + per-community literal text) instead of text.
        #[arg(long)]
        json: bool,
        /// Dendrogram round to cut at (default: chosen for the tile budget).
        #[arg(long)]
        round: Option<usize>,
        /// Skip communities with fewer than this many members (default 1).
        #[arg(long)]
        min_size: Option<usize>,
        /// Show each community's structural "topic" profile: top rdf:type
        /// classes, top predicates, and top literal words (no ML).
        #[arg(long)]
        profile: bool,
        /// Detect communities using ONLY edges with this predicate IRI (e.g.
        /// `<http://ex/cites>`), giving a criterion-specific partition. Omit to
        /// use all edges. See `docs/multi-criteria.md`.
        #[arg(long)]
        predicate: Option<String>,
    },
    /// Exact per-predicate triple counts, computed from the summary alone
    /// (the triple index is never read).
    Predicates {
        /// Path to the `.rete` file.
        file: String,
    },
    /// Ontology-aware coarse graph: relations between `rdf:type` classes with
    /// instance counts (the dataset's effective schema).
    Schema {
        /// Path to the `.rete` file.
        file: String,
    },
    /// Multi-source transitive reachability over one relation — for each seed,
    /// every node it (transitively) reaches. `--reverse` answers "who reaches the
    /// seed?" (impact analysis). `--parallel` uses the rayon evaluator (one task
    /// per seed; see `docs/BENCHMARK.md` §Parallelism).
    Reach {
        /// Path to the `.rete` file.
        file: String,
        /// Predicate (relation) IRI to traverse, e.g. `<http://ex/dependsOn>`.
        #[arg(long)]
        predicate: String,
        /// Seed node IRI to start from (repeatable).
        #[arg(long = "seed")]
        seeds: Vec<String>,
        /// File with one seed IRI per line (combined with any `--seed`).
        #[arg(long)]
        seeds_file: Option<String>,
        /// Traverse edges in reverse: "who reaches the seed?" (impact analysis).
        #[arg(long)]
        reverse: bool,
        /// Use the parallel (rayon) evaluator — one task per seed.
        #[arg(long)]
        parallel: bool,
        /// Print only the reach-set size per seed, not the members.
        #[arg(long)]
        count: bool,
    },
    /// Evaluate a Basic Graph Pattern. Patterns are separated by ` . ` and terms
    /// by spaces; `?name` is a variable. Terms must not contain spaces.
    ///
    /// Example: `rete bgp g.rete "?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z"`
    Bgp {
        /// Path to the `.rete` file.
        file: String,
        /// The BGP, e.g. "?x <p> ?y . ?y <p> ?z".
        query: String,
    },
    /// Run a prototype OWL RL / RDFS reasoner: materialize entailments and flag
    /// logical inconsistencies ("incoherent points"). Exits non-zero if any
    /// inconsistency is found, so it doubles as a CI coherence check.
    ///
    /// Covers a documented subset (subClassOf/subPropertyOf, domain/range,
    /// inverseOf, symmetric/transitive properties; disjointWith, sameAs vs
    /// differentFrom, functional-property and owl:Nothing clashes) — NOT full
    /// OWL DL. See `docs/reasoning.md`.
    Reason {
        /// Path to the `.rete` file.
        file: String,
        /// Also print the base + inferred graph in the given format.
        #[arg(long)]
        materialize: bool,
        /// Output format for `--materialize`: nq | ttl.
        #[arg(long, value_parser = ["nq", "ttl"], default_value = "nq")]
        format: String,
    },
    /// Run a SPARQL query (SELECT / ASK / CONSTRUCT).
    Sparql {
        /// Path to the `.rete` file.
        file: String,
        /// The SPARQL query.
        query: String,
        /// Emit standard SPARQL Results JSON (SELECT/ASK).
        #[arg(long)]
        json: bool,
    },
    /// Run a read-only **Cypher subset** by translating it to SPARQL.
    ///
    /// Prototype `MATCH … [WHERE …] RETURN … [LIMIT n]`. Bare label/rel/property
    /// names map to `<BASE + name>` (BASE defaults to `http://ex/`, set with
    /// `--base`). Example:
    /// `rete cypher deps.rete "MATCH (a)-[:dependsOn*]->(b) WHERE b = <http://ex/log4x> RETURN a"`.
    Cypher {
        /// Path to the `.rete` file.
        file: String,
        /// The Cypher query.
        query: String,
        /// Base IRI for bare names (default `http://ex/`).
        #[arg(long, default_value = cypher::DEFAULT_BASE)]
        base: String,
        /// Emit standard SPARQL Results JSON.
        #[arg(long)]
        json: bool,
    },
    /// Fetch just the pyramid summary (coarse graph) over HTTP — reads only the
    /// header, dictionary, and summary, skipping the (large) triple index.
    SummaryUrl {
        /// http(s):// URL of a `.rete` file (S3, GitHub, any CDN).
        url: String,
    },
    /// Query a triple pattern over HTTP, fetching only the byte ranges needed.
    QueryUrl {
        /// http(s):// URL of a `.rete` file (host must honor Range requests).
        url: String,
        #[arg(short, long)]
        subject: Option<String>,
        #[arg(short, long)]
        predicate: Option<String>,
        #[arg(short, long)]
        object: Option<String>,
    },
    /// Federate one SPARQL query across several `.rete` sources — local file
    /// paths and/or `http(s)://` URLs (mix allowed) — and merge the results at
    /// the **term (string) level**.
    ///
    /// Each `.rete` has its own dictionary, so integer IDs cannot be merged
    /// across files; federation instead evaluates the query independently on
    /// every source and UNIONS the term-level results. This is correct for
    /// **sharded data where each file yields complete result rows** (e.g.
    /// citations sharded by year). SELECT rows are unioned + deduped; ASK is OR'd
    /// across sources; CONSTRUCT triples are unioned + deduped.
    ///
    /// Routing/pruning (default on): each source's predicate set is read cheaply
    /// from its summary (the triple index is never touched), and a source whose
    /// predicates are disjoint from the query's is skipped. `--no-route` queries
    /// every source.
    ///
    /// LIMITATIONS (honest prototype): this is UNION federation — it does NOT do
    /// cross-file joins (a triple in file A joined with a triple in file B).
    /// Aggregates (COUNT/GROUP BY) and LIMIT are evaluated PER SOURCE then
    /// unioned, so a federated COUNT(*) returns per-source counts, not a global
    /// sum — reduce client-side. See `docs/federation.md`.
    Federate {
        /// `.rete` sources: local file paths and/or `http(s)://` URLs (≥1).
        #[arg(required = true, num_args = 1..)]
        sources: Vec<String>,
        /// The SPARQL query (SELECT / ASK / CONSTRUCT).
        #[arg(long)]
        query: String,
        /// Emit standard SPARQL Results JSON (SELECT/ASK).
        #[arg(long)]
        json: bool,
        /// Disable predicate routing/pruning: query every source.
        #[arg(long)]
        no_route: bool,
    },
    /// Run a SPARQL query over HTTP, range-fetching the file (no full download).
    SparqlUrl {
        /// http(s):// URL of a `.rete` file (host must honor Range requests).
        url: String,
        /// The SPARQL query.
        query: String,
        /// Emit standard SPARQL Results JSON (SELECT/ASK).
        #[arg(long)]
        json: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Build {
            inputs,
            output,
            format,
        } => build(&inputs, &output, format.as_deref()),
        Command::Validate { inputs, format } => validate(&inputs, format.as_deref()),
        Command::Info { file } => info(&file),
        Command::Stats { file } => stats(&file),
        Command::Verify { file } => verify_cmd(&file),
        Command::Graphs { file } => graphs(&file),
        Command::Export { file, format } => export(&file, &format),
        Command::Query {
            file,
            subject,
            predicate,
            object,
        } => query(&file, subject, predicate, object),
        Command::Summary { file } => summary(&file),
        Command::Communities {
            file,
            json,
            round,
            min_size,
            profile,
            predicate,
        } => communities(
            &file,
            json,
            round,
            min_size.unwrap_or(1),
            profile,
            predicate.as_deref(),
        ),
        Command::Predicates { file } => predicates(&file),
        Command::Schema { file } => schema(&file),
        Command::Reach {
            file,
            predicate,
            seeds,
            seeds_file,
            reverse,
            parallel,
            count,
        } => reach(
            &file, &predicate, seeds, seeds_file, reverse, parallel, count,
        ),
        Command::Bgp { file, query } => bgp(&file, &query),
        Command::Reason {
            file,
            materialize,
            format,
        } => reason_cmd(&file, materialize, &format),
        Command::Sparql { file, query, json } => sparql(&file, &query, json),
        Command::Cypher {
            file,
            query,
            base,
            json,
        } => cypher_cmd(&file, &query, &base, json),
        Command::SummaryUrl { url } => summary_url(&url),
        Command::QueryUrl {
            url,
            subject,
            predicate,
            object,
        } => query_url(&url, subject, predicate, object),
        Command::SparqlUrl { url, query, json } => sparql_url(&url, &query, json),
        Command::Federate {
            sources,
            query,
            json,
            no_route,
        } => federate(&sources, &query, json, !no_route),
    }
}

/// Is this source an `http(s)://` URL (vs. a local file path)?
fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// Read a source's predicate IRI set cheaply from its summary — the triple index
/// is never read. Used for routing. A file with no pyramid (no summary) yields
/// `None`, in which case the caller must not prune it (we can't tell).
fn source_predicates(source: &str) -> anyhow::Result<Option<std::collections::BTreeSet<String>>> {
    let view = if is_url(source) {
        let reader = HttpRangeReader::open(source)?;
        SummaryView::open_ranged(&reader)?
    } else {
        let bytes = std::fs::read(source)?;
        let reader = SliceReader::new(&bytes);
        SummaryView::open_ranged(&reader)?
    };
    Ok(view.map(|v| {
        v.predicate_totals()
            .into_iter()
            .map(|(p, _)| p)
            .collect::<std::collections::BTreeSet<String>>()
    }))
}

/// Evaluate `query` against one source (path or URL), returning its result.
fn eval_source(source: &str, query: &str) -> anyhow::Result<QueryOutput> {
    if is_url(source) {
        let reader = HttpRangeReader::open(source)?;
        let rete = Rete::open_ranged(&reader)?;
        eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{source}: {e}"))
    } else {
        let bytes = std::fs::read(source)?;
        let rete = Rete::open(&bytes)?;
        eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{source}: {e}"))
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
fn federate(sources: &[String], query: &str, json: bool, route: bool) -> anyhow::Result<()> {
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
struct MergeAcc {
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
    fn absorb(&mut self, out: QueryOutput) -> usize {
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

    /// Finalize into a single merged [`QueryOutput`]. With no absorbed sources,
    /// defaults to an empty SELECT.
    fn into_output(self) -> QueryOutput {
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

/// Parse Turtle into canonical N-Triples-token triples via oxttl.
fn parse_turtle(text: &str) -> anyhow::Result<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    for r in oxttl::TurtleParser::new().for_reader(text.as_bytes()) {
        let t = r?;
        out.push((
            t.subject.to_string(),
            t.predicate.to_string(),
            t.object.to_string(),
        ));
    }
    Ok(out)
}

/// Read an input source: a file path, or `-` for stdin.
fn read_input(path: &str) -> anyhow::Result<String> {
    if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(std::fs::read_to_string(path)?)
    }
}

/// The parser to use for an input: explicit `--format` wins, else by extension,
/// else (no extension / stdin) N-Triples.
fn input_format(path: &str, override_fmt: Option<&str>) -> &'static str {
    if let Some(f) = override_fmt {
        return match f {
            "nq" => "nq",
            "ttl" => "ttl",
            _ => "nt",
        };
    }
    let p = path.to_ascii_lowercase();
    if p.ends_with(".nq") || p.ends_with(".nquads") {
        "nq"
    } else if p.ends_with(".ttl") || p.ends_with(".turtle") {
        "ttl"
    } else {
        "nt"
    }
}

/// Build a `.rete` from one or more inputs, merged under one shared dictionary.
/// N-Triples/Turtle contribute to the default graph; N-Quads may carry named
/// graphs. The output uses the dataset layout iff any named graph appears (which
/// is byte-identical to the plain triple file when none do).
/// Parse one or more RDF inputs into quads (triples → default graph). Shared by
/// `build` and `validate`. Returns a parse error (which input, what went wrong)
/// if any input is malformed.
fn parse_inputs(inputs: &[String], format: Option<&str>) -> anyhow::Result<Vec<ntriples::RawQuad>> {
    let mut quads: Vec<ntriples::RawQuad> = Vec::new();
    for input in inputs {
        let text = read_input(input)?;
        let parsed: Vec<ntriples::RawQuad> = match input_format(input, format) {
            "nq" => ntriples::parse_quads(&text).map_err(|e| anyhow::anyhow!("{input}: {e}"))?,
            "ttl" => parse_turtle(&text)
                .map_err(|e| anyhow::anyhow!("{input}: {e}"))?
                .into_iter()
                .map(|(s, p, o)| (s, p, o, None))
                .collect(),
            _ => ntriples::parse(&text)
                .map_err(|e| anyhow::anyhow!("{input}: {e}"))?
                .into_iter()
                .map(|(s, p, o)| (s, p, o, None))
                .collect(),
        };
        quads.extend(parsed);
    }
    Ok(quads)
}

/// Parse inputs without building — report triple/quad and named-graph counts, or
/// fail with a clear parse error. The way to check an RDF file (N-Triples /
/// N-Quads / Turtle) is well-formed before ingesting it.
fn validate(inputs: &[String], format: Option<&str>) -> anyhow::Result<()> {
    let quads = parse_inputs(inputs, format)?;
    let named: std::collections::BTreeSet<&String> =
        quads.iter().filter_map(|(_, _, _, g)| g.as_ref()).collect();
    let in_default = quads.iter().filter(|(_, _, _, g)| g.is_none()).count();
    println!(
        "valid: {} statement(s) — {} in the default graph, {} named graph(s)",
        quads.len(),
        in_default,
        named.len()
    );
    Ok(())
}

fn build(inputs: &[String], output: &str, format: Option<&str>) -> anyhow::Result<()> {
    use std::collections::BTreeMap;

    // 1. Parse every input into quads (triples → default graph, `None`).
    let quads = parse_inputs(inputs, format)?;

    // 2. One dictionary over every term in every input.
    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in &quads {
        db.observe(s, p, o);
    }
    let dict = db.build();

    // 3. Split into the default-graph index and one index per named graph.
    let mut default_triples = Vec::new();
    let mut named: BTreeMap<String, Vec<(u32, u32, u32)>> = BTreeMap::new();
    for (s, p, o, g) in &quads {
        let t = dict.encode(s, p, o).expect("observed term");
        match g {
            None => default_triples.push(t),
            Some(graph) => named.entry(graph.clone()).or_default().push(t),
        }
    }
    let has_named = !named.is_empty();

    let mut def = GraphIndexBuilder::new();
    for &t in &default_triples {
        def.push(t);
    }
    let named_indexes: Vec<(String, rete_core::GraphIndex)> = named
        .into_iter()
        .map(|(g, ts)| {
            let mut b = GraphIndexBuilder::new();
            for t in ts {
                b.push(t);
            }
            (g, b.build())
        })
        .collect();

    let (meta, levels) = build_pyramid_meta(&dict, &default_triples, DEFAULT_TILE_BUDGET);
    let bytes = write_dataset(
        &dict,
        &def.build(),
        &named_indexes,
        has_named,
        &meta,
        levels,
    );
    std::fs::write(output, &bytes)?;

    if has_named {
        println!(
            "wrote {output}: {} quads ({} default + {} named graph(s)), {} terms, {} bytes",
            quads.len(),
            default_triples.len(),
            named_indexes.len(),
            dict.term_count(),
            bytes.len()
        );
    } else {
        println!(
            "wrote {output}: {} triples, {} terms, {} pyramid level(s), {} bytes",
            default_triples.len(),
            dict.term_count(),
            levels,
            bytes.len()
        );
    }
    Ok(())
}

fn info(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let header = rete_core::Header::from_bytes(&bytes)?;
    println!("{header:#?}");
    Ok(())
}

fn stats(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let h = rete.header();
    println!("{file} — {} bytes", bytes.len());
    println!("  default-graph triples : {}", h.quad_count);
    println!("  distinct terms        : {}", h.term_count);
    println!("  named graphs          : {}", rete.graph_names().len());
    println!("  pyramid levels        : {}", h.pyramid_levels);
    println!(
        "  compression           : {}",
        if h.block_codec == CODEC_ZSTD {
            "zstd"
        } else {
            "none"
        }
    );

    // Per-predicate totals + community count come from the summary alone.
    let reader = SliceReader::new(&bytes);
    if let Some(view) = SummaryView::open_ranged(&reader)? {
        println!("  communities           : {}", view.community_count());
        let totals = view.predicate_totals();
        println!("  predicates (top {}):", totals.len().min(10));
        for (pred, count) in totals.iter().take(10) {
            println!("    {count:>8}  {pred}");
        }
    }
    Ok(())
}

fn export(file: &str, format: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    match format {
        // N-Quads: lossless dump of the default graph + every named graph.
        "nq" => {
            for (s, p, o) in rete.dump(None) {
                println!("{s} {p} {o} .");
            }
            for g in rete.graph_names() {
                for (s, p, o) in rete.dump(Some(g)) {
                    println!("{s} {p} {o} {g} .");
                }
            }
        }
        // Turtle / JSON-LD are single-graph formats here: emit the default graph.
        "ttl" => print!("{}", export_turtle(&rete.dump(None))),
        "jsonld" => println!("{}", export_jsonld(&rete.dump(None))),
        other => anyhow::bail!("unknown export format: {other}"),
    }
    Ok(())
}

/// Serialize a default-graph triple list (canonical N-Triples tokens) to Turtle.
///
/// The term tokens (`<iri>`, `"lit"`, `"lit"^^<dt>`, `"lit"@lang`, `_:b`) are
/// already valid Turtle term syntax, so they pass through verbatim; we only group
/// statements by subject and abbreviate `rdf:type` to `a` for idiomatic output.
fn export_turtle(triples: &[(String, String, String)]) -> String {
    use std::collections::BTreeMap;

    const RDF_TYPE_IRI: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    // subject → predicate → [objects], all in stable (sorted) order.
    let mut by_subject: BTreeMap<&str, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
    for (s, p, o) in triples {
        by_subject
            .entry(s)
            .or_default()
            .entry(p)
            .or_default()
            .push(o);
    }

    let mut out = String::new();
    for (s, preds) in &by_subject {
        out.push_str(s);
        out.push('\n');
        let pred_count = preds.len();
        for (i, (p, objs)) in preds.iter().enumerate() {
            let pred = if *p == RDF_TYPE_IRI { "a" } else { p };
            let objects = objs.join(" , ");
            let terminator = if i + 1 == pred_count { " ." } else { " ;" };
            out.push_str(&format!("    {pred} {objects}{terminator}\n"));
        }
        out.push('\n');
    }
    out
}

/// Serialize a default-graph triple list to expanded JSON-LD: an array of node
/// objects keyed by `@id`, each predicate mapping to an array of value objects
/// (`{"@id": …}` for IRIs/bnodes, `{"@value": …}` plus `@type`/`@language` for
/// literals). This is the canonical expanded form, valid against the JSON-LD 1.1
/// algorithm with no `@context`.
fn export_jsonld(triples: &[(String, String, String)]) -> String {
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

fn graphs(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let names = rete.graph_names();
    if names.is_empty() {
        println!("(default graph only — no named graphs)");
    } else {
        for n in names {
            println!("{n}");
        }
    }
    Ok(())
}

fn verify_cmd(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    if rete_core::verify(&bytes)? {
        println!("OK — content hash matches");
        Ok(())
    } else {
        anyhow::bail!("FAILED — content hash mismatch (file corrupted or truncated)");
    }
}

fn query(
    file: &str,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let results = rete.query(s.as_deref(), p.as_deref(), o.as_deref());
    for (s, p, o) in &results {
        println!("{s} {p} {o} .");
    }
    eprintln!("{} result(s)", results.len());
    Ok(())
}

fn summary(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let Some(pyr) = rete.pyramid() else {
        eprintln!("file has no pyramid");
        return Ok(());
    };
    let dict = rete.dictionary();
    println!(
        "pyramid round {} — {} communities summarized as {} superedge(s):",
        pyr.round,
        community_count(pyr),
        pyr.summary.len()
    );
    for e in &pyr.summary {
        let pred = dict
            .predicate_term(e.predicate)
            .unwrap_or_else(|| format!("#{}", e.predicate));
        let arrow = if e.s_comm == e.o_comm {
            "(internal)"
        } else {
            "->"
        };
        println!(
            "  C{} {arrow} C{}  via {pred}  x{}",
            e.s_comm, e.o_comm, e.count
        );
    }
    Ok(())
}

fn community_count(pyr: &rete_core::PyramidMeta) -> usize {
    // Tiles are no longer materialized (dropped to shrink the file), so count the
    // distinct communities referenced by the summary superedges instead.
    let mut comms = std::collections::HashSet::new();
    for e in &pyr.summary {
        comms.insert(e.s_comm);
        comms.insert(e.o_comm);
    }
    comms.len()
}

/// One community's membership + literal corpus, ready to serialize.
struct CommunityRecord {
    community: usize,
    members: Vec<String>,
    text: Vec<String>,
    /// Structural "topic" profile (top-K, count desc): rdf:type classes,
    /// predicates, and literal words. Empty unless profiling was requested.
    top_types: Vec<(String, u32)>,
    top_predicates: Vec<(String, u32)>,
    top_terms: Vec<(String, u32)>,
}

/// Top-K `(key, count)` from a frequency map, by count desc then key asc.
fn top_k(counts: std::collections::HashMap<String, u32>, k: usize) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(k);
    v
}

/// Split a literal's lexical value into lowercased content words (≥3 chars, not
/// a common stop word) for the no-ML word profile.
fn content_words(text: &str) -> impl Iterator<Item = String> + '_ {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "are", "was", "were", "has", "have",
        "had", "into", "over", "its", "their", "they", "them", "but", "not", "all", "can", "via",
        "use", "uses", "using", "based", "study", "studies", "between", "which", "such", "these",
        "those", "than", "then", "also", "more", "most", "may", "our", "new",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| w.len() >= 3 && !STOP.contains(&w.as_str()))
}

/// Recompute the Louvain communities from the opened file's index (using the
/// existing public rete-core projection/dendrogram/tiling functions — no format
/// change) and collect, per community: the distinct subject IRIs (members) and
/// the lexical values of all literal objects (the text corpus). Communities with
/// fewer than `min_size` members are dropped.
fn collect_communities(
    rete: &Rete,
    round: Option<usize>,
    min_size: usize,
    profile: bool,
    predicate: Option<&str>,
) -> anyhow::Result<Vec<CommunityRecord>> {
    use std::collections::{HashMap, HashSet};
    let dict = rete.dictionary();
    // A `--predicate` filter restricts community detection to one relation,
    // giving a criterion-specific partition (multi-criteria splitting).
    let ids = match predicate {
        None => rete.match_ids((None, None, None)),
        Some(p) => {
            let pid = dict
                .predicate_id(p)
                .ok_or_else(|| anyhow::anyhow!("predicate not found in graph: {p}"))?;
            rete.match_ids((None, Some(pid), None))
        }
    };
    let g = rete_core::project_graph(dict, &ids);
    let dend = rete_core::build_dendrogram(&g);
    let round = round.unwrap_or_else(|| {
        rete_core::choose_round_for_budget(dict, &ids, &dend, DEFAULT_TILE_BUDGET)
    });
    let tiles = rete_core::tile_by_community(dict, &ids, &dend, round);
    const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let rdf_type_pid = dict.predicate_id(RDF_TYPE);

    let mut out = Vec::new();
    for tile in &tiles {
        // Distinct subjects (members), stable order of first appearance.
        let mut seen = HashSet::new();
        let mut members = Vec::new();
        let mut text = Vec::new();
        // Profile tallies (only filled when `profile`).
        let mut types: HashMap<String, u32> = HashMap::new();
        let mut preds: HashMap<String, u32> = HashMap::new();
        let mut terms: HashMap<String, u32> = HashMap::new();
        for &(s, p, o) in &tile.triples {
            if let Some(term) = dict.subject_term(s) {
                if seen.insert(term.clone()) {
                    members.push(term);
                }
            }
            let obj = dict.object_term(o);
            if let Some(ref obj) = obj {
                if let Some(lex) = literal_lexical(obj) {
                    if profile {
                        for w in content_words(&lex) {
                            *terms.entry(w).or_default() += 1;
                        }
                    }
                    text.push(lex);
                }
            }
            if profile {
                if let Some(pt) = dict.predicate_term(p) {
                    *preds.entry(pt).or_default() += 1;
                }
                if Some(p) == rdf_type_pid {
                    if let Some(ot) = obj {
                        *types.entry(ot).or_default() += 1;
                    }
                }
            }
        }
        if members.len() >= min_size {
            out.push(CommunityRecord {
                community: tile.community,
                members,
                text,
                top_types: if profile { top_k(types, 5) } else { Vec::new() },
                top_predicates: if profile { top_k(preds, 5) } else { Vec::new() },
                top_terms: if profile { top_k(terms, 8) } else { Vec::new() },
            });
        }
    }
    Ok(out)
}

/// `rete communities`: expose per-community membership and literal text. Human
/// form prints one line per community plus sample members; `--json` emits the
/// LDA corpus described in `docs/topic-modeling.md`.
fn communities(
    file: &str,
    json: bool,
    round: Option<usize>,
    min_size: usize,
    profile: bool,
    predicate: Option<&str>,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let records = collect_communities(&rete, round, min_size, profile, predicate)?;

    if json {
        use serde_json::{json, Value};
        let arr: Vec<Value> = records
            .iter()
            .map(|r| {
                let mut obj = json!({
                    "community": r.community,
                    "size": r.members.len(),
                    "members": r.members,
                    "text": r.text,
                });
                if profile {
                    obj["profile"] = json!({
                        "types": r.top_types,
                        "predicates": r.top_predicates,
                        "terms": r.top_terms,
                    });
                }
                obj
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_default()
        );
        return Ok(());
    }

    if records.is_empty() {
        println!("(no communities at this round / min-size)");
        return Ok(());
    }
    let fmt_pairs = |pairs: &[(String, u32)]| {
        pairs
            .iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for r in &records {
        println!(
            "community {}: {} members, {} literals",
            r.community,
            r.members.len(),
            r.text.len()
        );
        if profile {
            // The structural "topic" of the community — no ML needed.
            if !r.top_terms.is_empty() {
                println!("    topic words : {}", fmt_pairs(&r.top_terms));
            }
            if !r.top_types.is_empty() {
                println!("    classes     : {}", fmt_pairs(&r.top_types));
            }
            if !r.top_predicates.is_empty() {
                println!("    predicates  : {}", fmt_pairs(&r.top_predicates));
            }
        }
        for m in r.members.iter().take(5) {
            println!("    {m}");
        }
        if r.members.len() > 5 {
            println!("    … ({} more)", r.members.len() - 5);
        }
    }
    Ok(())
}

fn schema(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;

    let classes = rete_core::schema_classes(&rete);
    if classes.is_empty() {
        println!("(no rdf:type assertions — the data is untyped)");
    } else {
        println!("classes ({} types):", classes.len());
        for (class, count) in &classes {
            println!("  {count:>8}  {class}");
        }
    }

    let summary = rete_core::schema_summary(&rete);
    println!("relations:");
    for (s_class, pred, o_class, count) in &summary {
        println!("  {s_class} --{pred}--> {o_class}  ×{count}");
    }
    Ok(())
}

/// Run the prototype OWL RL / RDFS reasoner over a file's default graph: report
/// the count of newly entailed triples and every detected inconsistency. Exits
/// non-zero (via `anyhow::bail!`) when any inconsistency is found so it can serve
/// as a coherence gate in CI. With `--materialize`, also serialize the base +
/// inferred graph (reusing the same nq/ttl serializers as `export`).
fn reason_cmd(file: &str, materialize: bool, format: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let base = rete.dump(None);

    let result = rete_core::reason(&base);

    println!("inferred {} new triple(s)", result.inferred.len());
    if result.inconsistencies.is_empty() {
        println!("coherent: no inconsistencies found");
    } else {
        println!("{} inconsistency(ies) found:", result.inconsistencies.len());
        for inc in &result.inconsistencies {
            println!("  [{}] {}", inc.kind, inc.detail);
        }
    }

    if materialize {
        let mut all = base.clone();
        all.extend(result.inferred.iter().cloned());
        match format {
            "ttl" => print!("{}", export_turtle(&all)),
            _ => {
                for (s, p, o) in &all {
                    println!("{s} {p} {o} .");
                }
            }
        }
    }

    if !result.inconsistencies.is_empty() {
        anyhow::bail!(
            "{} inconsistency(ies) — graph is incoherent",
            result.inconsistencies.len()
        );
    }
    Ok(())
}

/// `rete reach`: multi-source transitive reachability over one relation. For
/// each seed, the set of nodes it transitively reaches (or, with `--reverse`,
/// that reach it). `--parallel` fans out one rayon task per seed.
#[allow(clippy::too_many_arguments)]
fn reach(
    file: &str,
    predicate: &str,
    mut seeds: Vec<String>,
    seeds_file: Option<String>,
    reverse: bool,
    parallel: bool,
    count: bool,
) -> anyhow::Result<()> {
    use std::collections::HashMap;
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let dict = rete.dictionary();

    if let Some(path) = seeds_file {
        for line in std::fs::read_to_string(&path)?.lines() {
            let t = line.trim();
            if !t.is_empty() && !t.starts_with('#') {
                seeds.push(t.to_string());
            }
        }
    }
    if seeds.is_empty() {
        anyhow::bail!("no seeds given (use --seed <iri> and/or --seeds-file <path>)");
    }

    // Adjacency in unified node space for the chosen direction.
    let adj: HashMap<u32, Vec<u32>> = if reverse {
        let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
        for (s, o) in rete.predicate_pairs(predicate) {
            m.entry(o).or_default().push(s); // who points at o
        }
        m
    } else {
        rete_core::build_adjacency(&rete, predicate)
    };

    // Resolve seed IRIs to node ids; report unknowns rather than silently dropping.
    let mut seed_nodes = Vec::with_capacity(seeds.len());
    for s in &seeds {
        match dict.node_of_term(s) {
            Some(n) => seed_nodes.push(n),
            None => eprintln!("warning: seed not in graph, skipped: {s}"),
        }
    }

    let sets = if parallel {
        rete_core::parallel::batch_reach_parallel(&adj, &seed_nodes)
    } else {
        rete_core::batch_reach_serial(&adj, &seed_nodes)
    };

    let dir = if reverse { "reached-by" } else { "reaches" };
    for (node, set) in seed_nodes.iter().zip(sets.iter()) {
        let seed_term = dict.node_term(*node).unwrap_or_else(|| format!("#{node}"));
        if count {
            println!("{seed_term} {dir} {} node(s)", set.len());
        } else {
            println!("{seed_term} {dir} {} node(s):", set.len());
            for &n in set {
                if let Some(t) = dict.node_term(n) {
                    println!("    {t}");
                }
            }
        }
    }
    eprintln!(
        "({} seed(s), predicate {predicate}, {})",
        seed_nodes.len(),
        if parallel { "parallel" } else { "serial" }
    );
    Ok(())
}

fn predicates(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let reader = SliceReader::new(&bytes);
    match SummaryView::open_ranged(&reader)? {
        Some(view) => {
            println!(
                "{} communities · per-predicate totals (from summary, index not read):",
                view.community_count()
            );
            for (pred, count) in view.predicate_totals() {
                println!("  {count}\t{pred}");
            }
        }
        None => eprintln!("file has no pyramid"),
    }
    Ok(())
}

fn bgp(file: &str, query: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;

    let mut patterns = Vec::new();
    for clause in query.split(" . ") {
        let toks: Vec<&str> = clause.split_whitespace().collect();
        if toks.len() != 3 {
            anyhow::bail!("each pattern needs 3 terms, got: {clause:?}");
        }
        patterns.push(TriplePattern {
            s: PatternTerm::parse(toks[0]),
            p: PatternTerm::parse(toks[1]),
            o: PatternTerm::parse(toks[2]),
        });
    }

    let solutions = eval_bgp(&rete, &patterns);
    for sol in &solutions {
        let row: Vec<String> = sol.iter().map(|(k, v)| format!("?{k}={v}")).collect();
        println!("{}", row.join("  "));
    }
    eprintln!("{} solution(s)", solutions.len());
    Ok(())
}

fn summary_url(url: &str) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    match SummaryView::open_ranged(&reader)? {
        Some(view) => {
            println!(
                "pyramid round {} — {} communities, {} superedge(s):",
                view.round,
                view.community_count(),
                view.summary.len()
            );
            for e in &view.summary {
                let pred = view
                    .predicate_term(e.predicate)
                    .unwrap_or_else(|| format!("#{}", e.predicate));
                let arrow = if e.s_comm == e.o_comm {
                    "(internal)"
                } else {
                    "->"
                };
                println!(
                    "  C{} {arrow} C{}  via {pred}  x{}",
                    e.s_comm, e.o_comm, e.count
                );
            }
            eprintln!(
                "fetched {} of {} bytes in {} range request(s) — index NOT fetched",
                reader.bytes_read(),
                total,
                reader.requests()
            );
        }
        None => eprintln!("file has no pyramid"),
    }
    Ok(())
}

fn query_url(
    url: &str,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    let rete = Rete::open_ranged(&reader)?;
    let results = rete.query(s.as_deref(), p.as_deref(), o.as_deref());
    for (s, p, o) in &results {
        println!("{s} {p} {o} .");
    }
    eprintln!(
        "{} result(s) · fetched {} bytes in {} range request(s) (file is {} bytes)",
        results.len(),
        reader.bytes_read(),
        reader.requests(),
        total
    );
    Ok(())
}

fn sparql(file: &str, query: &str, json: bool) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let result = eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    Ok(())
}

/// Run a read-only Cypher-subset query: translate it to SPARQL (see
/// `cypher.rs`), evaluate with the existing engine, and render like `sparql`.
fn cypher_cmd(file: &str, query: &str, base: &str, json: bool) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let result = cypher::eval_cypher(&rete, query, base).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    Ok(())
}

/// Run SPARQL against a `.rete` over HTTP(S), fetching only the byte ranges the
/// open needs (header, dictionary, index, pyramid) — never a full download.
fn sparql_url(url: &str, query: &str, json: bool) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    let rete = Rete::open_ranged(&reader)?;
    let result = eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    eprintln!(
        "(fetched {} bytes in {} range request(s); file is {} bytes)",
        reader.bytes_read(),
        reader.requests(),
        total
    );
    Ok(())
}

/// Print a query result: SPARQL Results JSON when `json`, else a readable form.
fn print_query_output(result: &QueryOutput, json: bool) {
    if json {
        println!("{}", results_json(result));
        return;
    }
    match result {
        QueryOutput::Ask(b) => println!("{b}"),
        QueryOutput::Construct(triples) => {
            for (s, p, o) in triples {
                println!("{s} {p} {o} .");
            }
            eprintln!("{} triple(s)", triples.len());
        }
        QueryOutput::Select(project, solutions) => {
            for sol in solutions {
                let keys: Vec<&String> = if project.is_empty() {
                    sol.keys().collect()
                } else {
                    project.iter().collect()
                };
                let row: Vec<String> = keys
                    .iter()
                    .map(|k| format!("?{k}={}", sol.get(*k).map(String::as_str).unwrap_or("")))
                    .collect();
                println!("{}", row.join("  "));
            }
            eprintln!("{} solution(s)", solutions.len());
        }
    }
}

/// Render a query result as SPARQL Results JSON (W3C
/// `application/sparql-results+json`), pretty-printed.
fn results_json(result: &QueryOutput) -> String {
    use serde_json::{json, Map, Value};
    let v = match result {
        QueryOutput::Ask(b) => json!({ "head": {}, "boolean": b }),
        QueryOutput::Select(project, solutions) => {
            // Variable order: the projection, else the union of solution keys.
            let mut vars: Vec<String> = project.clone();
            if vars.is_empty() {
                let mut seen = std::collections::BTreeSet::new();
                for s in solutions {
                    for k in s.keys() {
                        if seen.insert(k.clone()) {
                            vars.push(k.clone());
                        }
                    }
                }
            }
            let bindings: Vec<Value> = solutions
                .iter()
                .map(|s| {
                    let mut obj = Map::new();
                    for v in &vars {
                        if let Some(term) = s.get(v) {
                            obj.insert(v.clone(), term_to_json(term));
                        }
                    }
                    Value::Object(obj)
                })
                .collect();
            json!({ "head": { "vars": vars }, "results": { "bindings": bindings } })
        }
        // CONSTRUCT isn't a results-set; emit the triples as JSON for convenience.
        QueryOutput::Construct(triples) => {
            let arr: Vec<Value> = triples.iter().map(|(s, p, o)| json!([s, p, o])).collect();
            json!({ "triples": arr })
        }
    };
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Classify an N-Triples term token into a SPARQL-JSON RDF term object.
fn term_to_json(token: &str) -> serde_json::Value {
    use serde_json::json;
    if let Some(iri) = token.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return json!({ "type": "uri", "value": iri });
    }
    if let Some(b) = token.strip_prefix("_:") {
        return json!({ "type": "bnode", "value": b });
    }
    if token.starts_with('"') {
        // Closing quote (honoring \" escapes), then optional ^^<dt> / @lang.
        let bytes = token.as_bytes();
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => break,
                _ => i += 1,
            }
        }
        // The JSON `value` is the *unescaped* lexical form; the token carries
        // N-Triples escapes (\", \n, \\, \uXXXX …) that must be resolved first,
        // or serde would re-escape them and emit a doubly-escaped string.
        let value = unescape_nt(&token[1..i.min(token.len())]);
        let rest = token.get(i + 1..).unwrap_or("");
        if let Some(dt) = rest.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
            return json!({ "type": "literal", "value": value, "datatype": dt });
        }
        if let Some(lang) = rest.strip_prefix('@') {
            return json!({ "type": "literal", "value": value, "xml:lang": lang });
        }
        return json!({ "type": "literal", "value": value });
    }
    json!({ "type": "literal", "value": token })
}

/// If `token` is a literal (`"…"`, `"…"^^<dt>`, or `"…"@lang`), return its
/// **lexical value** with N-Triples escapes resolved (the bare string, no quotes,
/// datatype, or language tag). Returns `None` for IRIs and blank nodes. This is
/// the text a topic model consumes — the same quote/`^^`/`@` stripping that
/// `term_to_json` performs for the literal case.
fn literal_lexical(token: &str) -> Option<String> {
    if !token.starts_with('"') {
        return None;
    }
    // Find the closing quote, honoring \" escapes.
    let bytes = token.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => break,
            _ => i += 1,
        }
    }
    Some(unescape_nt(&token[1..i.min(token.len())]))
}

/// Resolve the N-Triples escape sequences in a literal's body to actual chars.
fn unescape_nt(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string(); // the overwhelmingly common fast path
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let unicode = |chars: &mut std::str::Chars, n: usize, out: &mut String| {
            let hex: String = chars.take(n).collect();
            match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                Some(ch) => out.push(ch),
                None => out.push('\u{FFFD}'),
            }
        };
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{08}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{0C}'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('\\') => out.push('\\'),
            Some('u') => unicode(&mut chars, 4, &mut out),
            Some('U') => unicode(&mut chars, 8, &mut out),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod federate_tests {
    use super::*;
    use rete_core::{
        build_pyramid_meta, write_dataset, Binding, DictionaryBuilder, GraphIndexBuilder, Rete,
        DEFAULT_TILE_BUDGET,
    };

    /// Build an in-memory `.rete` from `(s, p, o)` N-Triples-token triples.
    fn build_bytes(triples: &[(&str, &str, &str)]) -> Vec<u8> {
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
        write_dataset(&dict, &ib.build(), &[], false, &meta, levels)
    }

    /// Write a built `.rete` to a temp path and return it.
    fn temp_rete(name: &str, triples: &[(&str, &str, &str)]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("rete_fed_{name}_{}.rete", std::process::id()));
        std::fs::write(&path, build_bytes(triples)).unwrap();
        path
    }

    const CITES: &str = "<http://purl.org/spar/cito/cites>";
    const DATE: &str = "<http://purl.org/dc/terms/date>";

    /// SELECT federation across two shards: union + dedup of term-level rows.
    /// File A and B share one citing IRI (must dedup) and each have a unique one.
    #[test]
    fn federate_select_unions_and_dedups() {
        let a = temp_rete(
            "a",
            &[
                ("<http://a/1>", CITES, "<http://target>"),
                ("<http://shared>", CITES, "<http://target>"),
            ],
        );
        let b = temp_rete(
            "b",
            &[
                ("<http://b/1>", CITES, "<http://target>"),
                ("<http://shared>", CITES, "<http://target>"),
            ],
        );

        let q = format!("SELECT ?citing WHERE {{ ?citing {CITES} <http://target> }}");

        let mut acc = MergeAcc::default();
        for src in [&a, &b] {
            let bytes = std::fs::read(src).unwrap();
            let rete = Rete::open(&bytes).unwrap();
            acc.absorb(eval_query(&rete, &q).unwrap());
        }
        let out = acc.into_output();
        let QueryOutput::Select(_, rows) = out else {
            panic!("expected SELECT")
        };
        // 3 distinct citing IRIs: a/1, b/1, shared (the duplicate is deduped).
        let citings: std::collections::BTreeSet<String> = rows
            .iter()
            .map(|r| r.get("citing").cloned().unwrap_or_default())
            .collect();
        assert_eq!(citings.len(), 3, "rows: {rows:?}");
        assert!(citings.contains("<http://shared>"));
        assert!(citings.contains("<http://a/1>"));
        assert!(citings.contains("<http://b/1>"));

        std::fs::remove_file(&a).ok();
        std::fs::remove_file(&b).ok();
    }

    /// Routing prunes a source whose predicate set is disjoint from the query's.
    /// File A uses `cites`; file B uses only `date`. A `cites` query must keep A
    /// and prune B.
    #[test]
    fn routing_prunes_predicate_disjoint_source() {
        let a = temp_rete("ra", &[("<http://a/1>", CITES, "<http://target>")]);
        let b = temp_rete("rb", &[("<http://b/1>", DATE, "\"2021\"")]);

        let q = format!("SELECT ?x WHERE {{ ?x {CITES} <http://target> }}");
        let query_preds = query_predicates(&q).unwrap();
        assert!(query_preds.contains(CITES));

        let preds_a = source_predicates(a.to_str().unwrap()).unwrap().unwrap();
        let preds_b = source_predicates(b.to_str().unwrap()).unwrap().unwrap();
        // A shares the query predicate → kept; B is disjoint → pruned.
        assert!(!query_preds.is_disjoint(&preds_a));
        assert!(query_preds.is_disjoint(&preds_b));

        std::fs::remove_file(&a).ok();
        std::fs::remove_file(&b).ok();
    }

    /// ASK federation is a logical OR across sources.
    #[test]
    fn federate_ask_is_or() {
        let mut acc = MergeAcc::default();
        acc.absorb(QueryOutput::Ask(false));
        acc.absorb(QueryOutput::Ask(true));
        assert!(matches!(acc.into_output(), QueryOutput::Ask(true)));

        let mut acc2 = MergeAcc::default();
        acc2.absorb(QueryOutput::Ask(false));
        acc2.absorb(QueryOutput::Ask(false));
        assert!(matches!(acc2.into_output(), QueryOutput::Ask(false)));
    }

    /// CONSTRUCT federation unions + dedups triples.
    #[test]
    fn federate_construct_unions_and_dedups() {
        let dup = (
            "<http://s>".to_string(),
            "<http://p>".to_string(),
            "<http://o>".to_string(),
        );
        let uniq = (
            "<http://s2>".to_string(),
            "<http://p>".to_string(),
            "<http://o2>".to_string(),
        );
        let mut acc = MergeAcc::default();
        acc.absorb(QueryOutput::Construct(vec![dup.clone()]));
        acc.absorb(QueryOutput::Construct(vec![dup.clone(), uniq.clone()]));
        let QueryOutput::Construct(ts) = acc.into_output() else {
            panic!("expected CONSTRUCT")
        };
        assert_eq!(ts.len(), 2);
        assert!(ts.contains(&dup));
        assert!(ts.contains(&uniq));
    }

    /// `query_predicates` extracts BGP and property-path predicates; a variable
    /// predicate contributes nothing (so routing cannot prune on it).
    #[test]
    fn query_predicates_extraction() {
        let p = query_predicates(&format!(
            "SELECT ?x WHERE {{ ?x {CITES} ?y . ?y {DATE} ?d }}"
        ))
        .unwrap();
        assert!(p.contains(CITES) && p.contains(DATE));

        let var = query_predicates("SELECT ?x ?p WHERE { ?x ?p ?y }").unwrap();
        assert!(var.is_empty());

        // A `+` property path still surfaces its predicate IRI.
        let path = query_predicates(&format!("SELECT ?x WHERE {{ ?x {CITES}+ ?y }}")).unwrap();
        assert!(path.contains(CITES));
    }

    /// Row order is stable: first source's rows precede the second's new rows.
    #[test]
    fn select_merge_is_stable() {
        let mut a = Binding::new();
        a.insert("v".into(), "<http://1>".into());
        let mut b = Binding::new();
        b.insert("v".into(), "<http://2>".into());
        let mut acc = MergeAcc::default();
        acc.absorb(QueryOutput::Select(vec!["v".into()], vec![a.clone()]));
        acc.absorb(QueryOutput::Select(
            vec!["v".into()],
            vec![a.clone(), b.clone()],
        ));
        let QueryOutput::Select(_, rows) = acc.into_output() else {
            panic!()
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("v").unwrap(), "<http://1>");
        assert_eq!(rows[1].get("v").unwrap(), "<http://2>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rete_core::Binding;
    use serde_json::json;

    #[test]
    fn term_json_classification() {
        assert_eq!(
            term_to_json("<http://ex/a>"),
            json!({"type":"uri","value":"http://ex/a"})
        );
        assert_eq!(term_to_json("_:b0"), json!({"type":"bnode","value":"b0"}));
        assert_eq!(
            term_to_json("\"plain\""),
            json!({"type":"literal","value":"plain"})
        );
        assert_eq!(
            term_to_json("\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            json!({"type":"literal","value":"30",
                   "datatype":"http://www.w3.org/2001/XMLSchema#integer"})
        );
        assert_eq!(
            term_to_json("\"hi\"@en"),
            json!({"type":"literal","value":"hi","xml:lang":"en"})
        );
        // Escapes in the literal body are resolved to their actual characters,
        // so serde emits a singly-escaped JSON string (not a doubly-escaped one).
        assert_eq!(
            term_to_json(r#""he said \"hi\"\nbye""#),
            json!({"type":"literal","value":"he said \"hi\"\nbye"})
        );
        assert_eq!(
            term_to_json(r#""tab\there\\end""#),
            json!({"type":"literal","value":"tab\there\\end"})
        );
        // \u escape → actual code point. The token body is the 6 ASCII chars
        // backslash-u-0-0-E-9; the decoded value is the single char U+00E9 (é).
        assert_eq!(
            term_to_json("\"caf\\u00E9\""),
            json!({"type":"literal","value":"caf\u{E9}"})
        );
        // An escaped quote must not be mistaken for the closing quote that
        // precedes a datatype tag.
        assert_eq!(
            term_to_json(r#""a\"b"^^<http://ex/dt>"#),
            json!({"type":"literal","value":"a\"b","datatype":"http://ex/dt"})
        );
    }

    #[test]
    fn literal_lexical_extraction() {
        // Plain literal → bare lexical value.
        assert_eq!(
            literal_lexical("\"hello world\"").as_deref(),
            Some("hello world")
        );
        // Datatype and language tags are stripped, value only.
        assert_eq!(
            literal_lexical("\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>").as_deref(),
            Some("30")
        );
        assert_eq!(
            literal_lexical("\"bonjour\"@fr").as_deref(),
            Some("bonjour")
        );
        // Escapes are resolved; an escaped quote is not the closing quote.
        assert_eq!(
            literal_lexical(r#""he said \"hi\"\nbye""#).as_deref(),
            Some("he said \"hi\"\nbye")
        );
        // IRIs and blank nodes are not literals.
        assert_eq!(literal_lexical("<http://ex/a>"), None);
        assert_eq!(literal_lexical("_:b0"), None);
    }

    #[test]
    fn select_results_json_shape() {
        let mut b = Binding::new();
        b.insert("p".into(), "<http://ex/Alice>".into());
        let out = QueryOutput::Select(vec!["p".into()], vec![b]);
        let v: serde_json::Value = serde_json::from_str(&results_json(&out)).unwrap();
        assert_eq!(v["head"]["vars"][0], "p");
        assert_eq!(v["results"]["bindings"][0]["p"]["value"], "http://ex/Alice");
    }

    #[test]
    fn ask_results_json() {
        let v: serde_json::Value =
            serde_json::from_str(&results_json(&QueryOutput::Ask(true))).unwrap();
        assert_eq!(v["boolean"], true);
    }

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
        // One subject block, predicates sorted, `rdf:type` shown as `a`.
        assert!(ttl.starts_with("<http://ex/Alice>\n"));
        assert!(ttl.contains("    a <http://ex/Person>"), "got:\n{ttl}");
        // Datatype literal passes through verbatim (valid Turtle term syntax).
        assert!(
            ttl.contains("\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            "got:\n{ttl}"
        );
        // Lang tag + escaped quote preserved exactly.
        assert!(ttl.contains("\"héllo \\\"quote\\\"\"@en"), "got:\n{ttl}");
        // Blank node passes through; statement list ends with ` .`.
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

    #[test]
    fn turtle_parse_abbreviations() {
        let ttl = "@prefix ex: <http://ex/> .\nex:A ex:knows ex:B , ex:C .";
        let t = parse_turtle(ttl).unwrap();
        assert_eq!(t.len(), 2);
        assert!(t.contains(&(
            "<http://ex/A>".into(),
            "<http://ex/knows>".into(),
            "<http://ex/B>".into()
        )));
    }
}
