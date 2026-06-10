//! `rete` — build, inspect, and query Rete graph files.

mod commands;
mod cypher;
mod http;

use clap::{Parser, Subcommand};

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
        /// Materialize RDFS/OWL-RL entailments at build time: run the reasoner
        /// over the default graph (subClassOf/subPropertyOf/domain/range,
        /// inverseOf, symmetric/transitive, sameAs) and store the inferred
        /// triples alongside the asserted ones, so they need no query-time
        /// reasoning. Aborts if the graph is logically incoherent.
        #[arg(long)]
        materialize: bool,
        /// Embed a **Dataset Card** (data-catalog metadata) in the file. Auto
        /// fields — triple/term counts, top predicates and classes, vocabularies
        /// — are derived from the data; the curated fields below are optional.
        /// Any card flag opts in; without one the build is cardless. View it with
        /// `rete card` / `rete info`. See `docs/dataset-cards.md`.
        #[arg(long)]
        card: bool,
        /// JSON file of curated card fields (title/description/license/source/
        /// created/example_queries); implies `--card`. Explicit flags override it.
        #[arg(long = "card-file")]
        card_file: Option<String>,
        /// Card title (implies `--card`).
        #[arg(long)]
        title: Option<String>,
        /// Card license, e.g. `CC0-1.0` (implies `--card`).
        #[arg(long)]
        license: Option<String>,
        /// Card source URL (implies `--card`).
        #[arg(long)]
        source: Option<String>,
        /// Card description (implies `--card`).
        #[arg(long)]
        description: Option<String>,
        /// Card creation date, e.g. `2026-06-08` (implies `--card`).
        #[arg(long)]
        created: Option<String>,
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
    /// Print the embedded Dataset Card (data-catalog metadata), if the file has
    /// one — title/license/source, counts, top predicates and classes,
    /// vocabularies, and the content-hash checksum. `--json` emits the raw card.
    Card {
        /// Path to the `.rete` file.
        file: String,
        /// Emit the card as JSON instead of the human catalog view.
        #[arg(long)]
        json: bool,
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
    /// Explain a triple-pattern result: terms, IDs, graph scope, index choice,
    /// and the file byte ranges that were used.
    Why {
        /// Path to the `.rete` file.
        file: String,
        #[arg(short, long)]
        subject: Option<String>,
        #[arg(short, long)]
        predicate: Option<String>,
        #[arg(short, long)]
        object: Option<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
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
    /// Validate a `.rete` graph against SHACL Core shapes (2017 Recommendation).
    ///
    /// Shapes are read from Turtle. The default graph is validated unless
    /// `--graph` names one dataset graph. Exits non-zero when the report is
    /// non-conformant, so it can be used as a CI gate.
    Shacl {
        /// Path to the `.rete` file.
        file: String,
        /// Turtle file containing SHACL shapes.
        #[arg(long)]
        shapes: String,
        /// Validate this named graph instead of the default graph.
        #[arg(long)]
        graph: Option<String>,
        /// Output format: text | json | ttl.
        #[arg(long, value_parser = ["text", "json", "ttl"], default_value = "text")]
        format: String,
    },
    /// Preview the range-read byte cost of running a SPARQL query.
    ///
    /// Reports the cheap summary/overview path, the routed single-pattern path
    /// when applicable, and the current full SPARQL query-open path without
    /// materializing query results.
    Cost {
        /// Local `.rete` file path or http(s) URL.
        source: String,
        /// The SPARQL query to parse and inspect.
        query: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Include planner explanation for the selected access path.
        #[arg(long)]
        explain: bool,
    },
    /// Answer exact summary-safe SPARQL queries without opening the triple index.
    ///
    /// This is the first progressive-query surface: it supports exact
    /// per-predicate `COUNT(*)` and `ASK` shapes from the pyramid summary.
    Progressive {
        /// Local `.rete` file path or http(s) URL.
        source: String,
        /// The SPARQL query to answer from the summary.
        query: String,
        /// Emit SPARQL Results JSON plus progressive metadata.
        #[arg(long)]
        json: bool,
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
    /// Query a triple pattern over HTTP, fetching only the selected permutation
    /// payload after dictionary resolution.
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
            materialize,
            card,
            card_file,
            title,
            license,
            source,
            description,
            created,
        } => commands::build::build(
            &inputs,
            &output,
            format.as_deref(),
            materialize,
            commands::card::CardArgs {
                enabled: card,
                file: card_file,
                title,
                license,
                source,
                description,
                created,
            },
        ),
        Command::Validate { inputs, format } => {
            commands::build::validate(&inputs, format.as_deref())
        }
        Command::Info { file } => commands::inspect::info(&file),
        Command::Stats { file } => commands::inspect::stats(&file),
        Command::Verify { file } => commands::inspect::verify_cmd(&file),
        Command::Card { file, json } => commands::card::card_cmd(&file, json),
        Command::Graphs { file } => commands::inspect::graphs(&file),
        Command::Export { file, format } => commands::export::export(&file, &format),
        Command::Query {
            file,
            subject,
            predicate,
            object,
        } => commands::query::query(&file, subject, predicate, object),
        Command::Why {
            file,
            subject,
            predicate,
            object,
            json,
        } => commands::query::why(&file, subject, predicate, object, json),
        Command::Summary { file } => commands::inspect::summary(&file),
        Command::Communities {
            file,
            json,
            round,
            min_size,
            profile,
            predicate,
        } => commands::communities::communities(
            &file,
            json,
            round,
            min_size.unwrap_or(1),
            profile,
            predicate.as_deref(),
        ),
        Command::Predicates { file } => commands::inspect::predicates(&file),
        Command::Schema { file } => commands::inspect::schema(&file),
        Command::Reach {
            file,
            predicate,
            seeds,
            seeds_file,
            reverse,
            parallel,
            count,
        } => commands::reach::reach(
            &file, &predicate, seeds, seeds_file, reverse, parallel, count,
        ),
        Command::Bgp { file, query } => commands::query::bgp(&file, &query),
        Command::Reason {
            file,
            materialize,
            format,
        } => commands::reason::reason_cmd(&file, materialize, &format),
        Command::Shacl {
            file,
            shapes,
            graph,
            format,
        } => commands::shacl::shacl_cmd(&file, &shapes, graph.as_deref(), &format),
        Command::Cost {
            source,
            query,
            json,
            explain,
        } => commands::cost::cost(&source, &query, json, explain),
        Command::Progressive {
            source,
            query,
            json,
        } => commands::progressive::progressive(&source, &query, json),
        Command::Sparql { file, query, json } => commands::query::sparql(&file, &query, json),
        Command::Cypher {
            file,
            query,
            base,
            json,
        } => commands::query::cypher_cmd(&file, &query, &base, json),
        Command::SummaryUrl { url } => commands::url::summary_url(&url),
        Command::QueryUrl {
            url,
            subject,
            predicate,
            object,
        } => commands::url::query_url(&url, subject, predicate, object),
        Command::SparqlUrl { url, query, json } => commands::url::sparql_url(&url, &query, json),
        Command::Federate {
            sources,
            query,
            json,
            no_route,
        } => commands::federate::federate(&sources, &query, json, !no_route),
    }
}

#[cfg(test)]
mod federate_tests {
    use crate::commands::federate::{source_predicates, MergeAcc};
    use rete_core::{
        build_pyramid_meta, eval_query, query_predicates, write_dataset, Binding,
        DictionaryBuilder, GraphIndexBuilder, QueryOutput, Rete, DEFAULT_TILE_BUDGET,
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
