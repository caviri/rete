//! `rete` — build, inspect, and query Rete graph files.

mod commands;
mod cypher;
mod http;

use clap::{CommandFactory, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) const JSON_SCHEMA_VERSION: u8 = 1;

#[derive(Parser)]
#[command(name = "rete", version, about = "Cloud-native RDF graph files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate shell completions and a man page for release archives.
    #[command(hide = true)]
    Generate {
        /// Directory that receives rete.bash, _rete, rete.fish, rete.ps1, and rete.1.
        #[arg(long, value_name = "DIR")]
        output: PathBuf,
    },
    /// Build a `.rete` file from one or more RDF inputs (merged into one file).
    ///
    /// Format is by extension: `.nt`/`.nq`/`.ttl`/`.trig`, plus
    /// `.rdf`/`.owl`/`.rdfxml` (RDF/XML — how most OWL ontologies ship). Any input
    /// may be **gzipped** (`.ttl.gz`, `.trig.gz`, …) — compression is detected from
    /// the bytes and decompressed while streaming, so a dump never has to be
    /// expanded to disk first. Use `-` to read stdin (defaults to N-Triples).
    /// `--format` overrides detection for all inputs.
    /// Example: `cat *.nt | rete build - -o out.rete`.
    Build {
        /// Input files (or `-` for stdin); multiple are merged.
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<String>,
        /// Output `.rete` file.
        #[arg(short, long)]
        output: String,
        /// Force input format for all inputs: nt | nq | ttl | trig | rdfxml.
        #[arg(long, value_parser = ["nt", "nq", "ttl", "trig", "rdfxml"])]
        format: Option<String>,
        /// Fold every named graph into the **default graph**, dropping the graph
        /// term. Dumps that put all their data inside named graphs — TriG
        /// exports like SemOpenAlex, most Wikibase and GraphDB dumps — otherwise
        /// answer `?s ?p ?o` with nothing and build an empty pyramid, because in
        /// SPARQL the default graph is not the union of the named ones. It is
        /// also what makes such an input eligible for `--memory-budget-mb`,
        /// which writes default-graph files only.
        #[arg(long = "collapse-graphs")]
        collapse_graphs: bool,
        /// Materialize RDFS/OWL-RL entailments at build time: run the reasoner
        /// over the default graph (subClassOf/subPropertyOf/domain/range,
        /// inverseOf, symmetric/transitive, sameAs) and store the inferred
        /// triples alongside the asserted ones, so they need no query-time
        /// reasoning. Aborts if the graph is logically incoherent.
        #[arg(long)]
        materialize: bool,
        /// Run the OWL RL / RDFS reasoner at build time and stamp the coherence
        /// verdict into the Dataset Card (implies `--card`), so a remote reader
        /// learns the graph's coherence from the index-free card with no compute.
        /// Unlike `--materialize`, this does NOT abort an incoherent graph — it
        /// records `coherent: false` honestly. Combine with `--materialize` to also
        /// bake the inferred triples in. Verify later with `rete reason --verify-card`.
        #[arg(long)]
        reason: bool,
        /// Skip the community pyramid — no pyramid section is written. SPARQL /
        /// SHACL / triple / reachability queries don't use it, so the file stays
        /// fully queryable and is markedly smaller (the pyramid is the largest
        /// section on dense graphs). Only community / summary / progressive
        /// queries need the pyramid.
        #[arg(long = "no-pyramid")]
        no_pyramid: bool,
        /// Community algorithm for the pyramid: `louvain` (default — topological
        /// modularity, single-threaded, byte-identical) or `types` (partition by
        /// `rdf:type` — deterministic, parallelizable, self-naming communities, and
        /// it still emits the planner's `query_stats`; falls back to louvain when
        /// the graph is untyped). `types` makes a pyramid feasible on graphs too
        /// large for the single-threaded Louvain build.
        #[arg(long = "pyramid-algo", value_parser = ["louvain", "types"], default_value = "louvain")]
        pyramid_algo: String,
        /// Build a **full-text index** over the literals: every string-literal
        /// word maps to the subjects that carry it, so `rete search --contains
        /// <word>` finds entities by content with no scan. Adds a `TextIndex`
        /// section (off by default; the index can be large on text-heavy graphs).
        #[arg(long = "text-index")]
        text_index: bool,
        /// Override the predicate that types subjects with classes for the schema
        /// pyramid (default: `rdf:type`, else auto-detected). Use it where
        /// `rdf:type` is structural noise — e.g. Wikidata's `wdt:P31`:
        /// `--type-predicate http://www.wikidata.org/prop/direct/P31`.
        #[arg(long = "type-predicate")]
        type_predicate: Option<String>,
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
        /// Skip measuring the starter queries' cost figures (bytes / range
        /// requests / reference timing) into the build-info section. Card
        /// builds measure them by default; each starter query is run once,
        /// cold, against the finished image.
        #[arg(long = "no-card-costs")]
        no_card_costs: bool,
        /// **Memory-bounded external build**: assemble the file within roughly
        /// this many MiB of RAM by cutting the input into disk-spilled chunks
        /// and merging them (the budget decides the number of chunks and the
        /// external-sort run sizes). For graphs too large for the in-RAM build.
        /// Output is byte-identical to a standard `--no-pyramid` build.
        /// Limits: N-Triples/N-Quads/Turtle/TriG only (gzipped or not — RDF/XML
        /// is the one syntax that must be converted first), default graph only
        /// (see `--collapse-graphs`), implies `--no-pyramid`, and excludes
        /// `--text-index`/`--materialize`/`--reason`.
        #[arg(long = "memory-budget-mb")]
        memory_budget_mb: Option<u64>,
        /// Directory for the external build's spill files (default: alongside
        /// the output file). Needs free space on the order of the input size.
        #[arg(long = "tmp-dir")]
        tmp_dir: Option<String>,
    },
    /// Validate that RDF input(s) parse as well-formed N-Triples/N-Quads/Turtle/
    /// RDF-XML, without building. Reports counts, or fails with a parse error.
    Validate {
        /// Input files (or `-` for stdin).
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<String>,
        /// Force input format for all inputs: nt | nq | ttl | rdfxml.
        #[arg(long, value_parser = ["nt", "nq", "ttl", "rdfxml"])]
        format: Option<String>,
    },
    /// Estimate a build's output size, wall time and spill **before** running it.
    ///
    /// Streams the input exactly as `rete build` would — parsing every statement,
    /// counting distinct terms with HyperLogLog — but writes nothing. Use
    /// `--sample-mb` to read only a leading slice and extrapolate, which turns a
    /// multi-hour question ("will this 110 GB conversion fit?") into a minute.
    Estimate {
        /// Input files (N-Triples / N-Quads / Turtle / TriG, gzipped or not).
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<String>,
        /// Force input format for all inputs: nt | nq | ttl | trig.
        #[arg(long, value_parser = ["nt", "nq", "ttl", "trig"])]
        format: Option<String>,
        /// Read only this many MiB and extrapolate (default: read everything).
        #[arg(long)]
        sample_mb: Option<u64>,
        /// The `--memory-budget-mb` the real build would use (default 4096).
        #[arg(long)]
        memory_budget_mb: Option<u64>,
        /// Project the size of a `--no-pyramid` build (what the huge graphs use).
        #[arg(long)]
        no_pyramid: bool,
    },
    /// Fold several `.rete` files into one, without going back through text.
    ///
    /// Consolidating a sharded dataset used to mean rebuilding from the original
    /// source — re-running the converter and re-parsing every line. This reads
    /// the SHARDS instead: dictionary-encoded and compressed, roughly a quarter
    /// of the bytes, and no RDF parsing at all.
    ///
    /// It does NOT skip the sorting. The dictionary is HDT-style (`shared` /
    /// subject-only / object-only), so a term that is subject-only in one shard
    /// and object-only in another becomes shared in the merge and changes ID
    /// section — the shards' orderings do not survive the remap, and every
    /// permutation is rebuilt. Memory stays bounded: inputs are opened lazily and
    /// the builder spills under `--memory-budget-mb`.
    Merge {
        /// Input `.rete` files.
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<String>,
        /// Output `.rete` file.
        #[arg(short, long)]
        output: String,
        /// Resident memory budget in MiB (default 4096); the builder spills past it.
        #[arg(long, default_value_t = 4096)]
        memory_budget_mb: u64,
        /// Directory for spill files (default: next to the output).
        #[arg(long)]
        tmp_dir: Option<String>,
        /// Embed a Dataset Card in the merged file.
        #[arg(long)]
        card: bool,
        /// Read the card's curated fields from a JSON file.
        #[arg(long)]
        card_file: Option<String>,
        /// Card title.
        #[arg(long)]
        title: Option<String>,
        /// Card licence.
        #[arg(long)]
        license: Option<String>,
        /// Card source URL.
        #[arg(long)]
        source: Option<String>,
        /// Card description.
        #[arg(long)]
        description: Option<String>,
        /// Card creation date (ISO-8601).
        #[arg(long)]
        created: Option<String>,
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
    /// Search a `.rete` for entities. Two modes:
    ///
    /// • **Label prefix** (default): the subjects whose label starts with
    ///   `prefix` (case-insensitive), from the bounded label-index block in the
    ///   pyramid-meta — no literal scan, the fast path for autocomplete.
    ///
    /// • **Full-text** (`--contains <word>…`): subjects whose literals contain
    ///   **every** given word (whole-word, case-insensitive — AND), from the
    ///   TEXT_INDEX section (`build --text-index`). `--contains-prefix einst`
    ///   additionally requires a word starting with `einst`. On a remote file
    ///   only the queried posting lists are fetched, not the whole index.
    Search {
        /// Path to the `.rete` file.
        file: String,
        /// Case-insensitive label prefix (empty matches the first `--limit`).
        #[arg(default_value = "")]
        prefix: String,
        /// Full-text: require each WORD to appear in the subject's literals (AND).
        #[arg(long = "contains", num_args = 1.., value_name = "WORD")]
        contains: Vec<String>,
        /// Full-text: also require a literal word starting with this prefix.
        #[arg(long = "contains-prefix", value_name = "PREFIX")]
        contains_prefix: Option<String>,
        /// Maximum number of matches to print.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Emit `{schemaVersion:1,matches:[…]}` JSON. Prefix matches contain
        /// `label` + `subject`; full-text matches contain `subject`.
        #[arg(long)]
        json: bool,
    },
    /// Search a **remote** `.rete` over HTTP range reads — the counterpart of
    /// `rete search`, with the same two modes and the same output.
    ///
    /// Opens for **search only**: the header and the subject halves of the
    /// dictionary, then the TEXT_INDEX token table on the first `--contains`
    /// and one range request per posting list and per dictionary chunk holding
    /// a hit. The index tile directories a SPARQL open must fetch to route, and
    /// the object-only dictionary directory that dominates a normal open on a
    /// literal-heavy graph, are both skipped — so this is by far the cheapest
    /// way to find an entity by its text in a file you have not downloaded.
    /// Bare (label-prefix) mode faults the pyramid instead.
    SearchUrl {
        /// http(s):// URL of a `.rete` file (host must honor Range requests).
        url: String,
        /// Case-insensitive label prefix (empty matches the first `--limit`).
        #[arg(default_value = "")]
        prefix: String,
        /// Full-text: require each WORD to appear in the subject's literals (AND).
        #[arg(long = "contains", num_args = 1.., value_name = "WORD")]
        contains: Vec<String>,
        /// Full-text: also require a literal word starting with this prefix.
        #[arg(long = "contains-prefix", value_name = "PREFIX")]
        contains_prefix: Option<String>,
        /// Maximum number of matches to print.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Emit `{schemaVersion:1,matches:[…]}` JSON, as `search` does.
        #[arg(long)]
        json: bool,
    },
    /// Print the embedded Dataset Card (data-catalog metadata), if the file has
    /// one — title/license/source, counts, top predicates and classes,
    /// vocabularies, the content-hash checksum, and (when present) the build
    /// record: when it was built, by which `rete`, with which flags, and what
    /// the starter queries cost. `--json` emits the raw card; `--format jsonld`
    /// projects it to JSON-LD (VoID + schema.org + PROV-O — already RDF);
    /// `--format croissant` emits the honestly-mappable Croissant subset.
    Card {
        /// Path to the `.rete` file.
        file: String,
        /// Emit the card as JSON instead of the human catalog view.
        #[arg(long)]
        json: bool,
        /// Output format: json | jsonld | croissant (default: human text).
        #[arg(long, value_parser = ["json", "jsonld", "croissant"])]
        format: Option<String>,
        /// The file's sha256 (hex), for `--format croissant`: Croissant requires
        /// an md5/sha256 on every FileObject and a file cannot carry its own —
        /// supply it from outside (`sha256sum file.rete`) for a fully
        /// validator-clean document.
        #[arg(long)]
        sha256: Option<String>,
    },
    /// Fetch just the embedded Dataset Card over HTTP — reads only the header and
    /// the metadata + build-info range (the index-free CARD tier), never the
    /// dictionary or index. The cold-start self-description, fetched in two
    /// small range requests.
    CardUrl {
        /// http(s):// URL of a `.rete` file (host must honor Range requests).
        url: String,
        /// Emit the card as JSON instead of the human catalog view.
        #[arg(long)]
        json: bool,
        /// Output format: json | jsonld | croissant (default: human text).
        #[arg(long, value_parser = ["json", "jsonld", "croissant"])]
        format: Option<String>,
        /// The file's sha256 (hex), for `--format croissant` (see `rete card`).
        #[arg(long)]
        sha256: Option<String>,
    },
    /// Audit the starter queries a Dataset Card **already ships**: which of them
    /// still answer on the file that carries them, which provably return
    /// nothing, and which no card can decide. Reads the CARD tier only (two
    /// range requests), so a published multi-GB file costs tens of KB to check
    /// — the point being that you do not have to re-card a catalog to find out
    /// which of its files greet a newcomer with zero rows.
    ///
    /// `--measure` runs the queries instead of reasoning about them, reporting
    /// rows, bytes and range requests per query — the figures a build records
    /// in its `query_costs`, measured the same way, so the two are comparable.
    /// That costs real reads (`--only`/`--max-mb` bound them), and it settles
    /// the templates a card cannot decide at all.
    CardAudit {
        /// A `.rete` file (local path or `http(s)://` URL), or a card JSON
        /// document from `rete card --json` / `rete card-url --json` (`-` for
        /// stdin). `--measure` needs the file itself.
        path: String,
        /// Emit one JSON object with every finding instead of the text table.
        #[arg(long)]
        json: bool,
        /// Run each starter query cold and report what it cost. Local path =
        /// free but only as honest as your copy; `http(s)://` = what a reader
        /// really pays. The output names which it was, either way.
        #[arg(long)]
        measure: bool,
        /// Measure only these query ids (repeatable, or comma-separated).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Abandon a query once it has asked for this many MB (0 = no cap;
        /// fractions allowed). A remote measurement is a download; this is the
        /// leash — and "costs more than N MB" is itself an answer.
        #[arg(long, default_value_t = 0.0)]
        max_mb: f64,
        /// Record the measurement in the file's build-info section. Implies
        /// `--measure`; local files only. Build info sits **outside** the
        /// content hash, so the file keeps its identity — but the section is
        /// near the front, so the file is rewritten end to end to make room.
        #[arg(long)]
        write_costs: bool,
        /// Let `--write-costs` proceed even though a starter query measured
        /// zero rows.
        #[arg(long, requires = "write_costs")]
        allow_empty: bool,
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
    /// Rebuild a `.rete`'s pyramid in place, reading triples straight from the
    /// file (no `export | build` N-Quads round-trip). Use to add a schema
    /// pyramid to a file built before it existed.
    Repyramid {
        /// Path to the input `.rete` file.
        file: String,
        /// Output path for the rebuilt `.rete`.
        #[arg(short, long)]
        output: String,
        /// Override the type predicate for the schema pyramid (e.g. Wikidata's
        /// `wdt:P31`); same semantics as `build --type-predicate`.
        #[arg(long = "type-predicate")]
        type_predicate: Option<String>,
        /// Community algorithm for the rebuilt pyramid: `louvain` or `types`
        /// (same semantics as `build --pyramid-algo`).
        #[arg(long = "pyramid-algo", value_parser = ["louvain", "types"], default_value = "louvain")]
        pyramid_algo: String,
        /// Build a full-text (word/CONTAINS) index over the literals as a
        /// TEXT_INDEX section; same semantics as `build --text-index`.
        #[arg(long = "text-index")]
        text_index: bool,
        /// Embed a **Dataset Card** in the rebuilt file (same flags as `build`).
        #[arg(long)]
        card: bool,
        /// JSON file of curated card fields; implies `--card`.
        #[arg(long = "card-file")]
        card_file: Option<String>,
        /// Card title (implies `--card`).
        #[arg(long)]
        title: Option<String>,
        /// Card license (implies `--card`).
        #[arg(long)]
        license: Option<String>,
        /// Card source URL (implies `--card`).
        #[arg(long)]
        source: Option<String>,
        /// Card description (implies `--card`).
        #[arg(long)]
        description: Option<String>,
        /// Card creation date (implies `--card`).
        #[arg(long)]
        created: Option<String>,
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
    /// Print the pyramid summary: the community super-edge graph plus the schema
    /// pyramid (a leveled `rdf:type` histogram — abstract classes at coarse
    /// levels, leaves as you zoom in), read index-free from the pyramid-meta.
    Summary {
        /// Path to the `.rete` file.
        file: String,
        /// Print only the schema-pyramid type histogram at this semantic level
        /// (0 = coarsest/most abstract).
        #[arg(long)]
        level: Option<usize>,
    },
    /// Recompute the Louvain communities and expose, per community, its member
    /// subjects and the literal text of its triples — the per-community text
    /// corpus for downstream topic modeling (see `docs/topic-modeling.md`).
    ///
    /// `--json` emits `{schemaVersion:1,communities:[{community, size,
    /// members:[<iri>…], text:[lexical…]}]}`, the corpus consumed by
    /// `scripts/lda_topics.py`.
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
        /// The BGP, e.g. `?x <p> ?y . ?y <p> ?z`.
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
        /// Path to a local `.rete` file (omit when using `--url`).
        file: Option<String>,
        /// Read a remote `.rete` over HTTP range requests instead of a local file.
        #[arg(long, conflicts_with = "file")]
        url: Option<String>,
        /// Also print the base + inferred graph in the given format.
        #[arg(long)]
        materialize: bool,
        /// Output format for `--materialize`: nq | ttl.
        #[arg(long, value_parser = ["nq", "ttl"], default_value = "nq")]
        format: String,
        /// Coherence-gate mode: print one verdict line, exit non-zero on any
        /// incoherent point (suppresses `--materialize` output). For CI.
        #[arg(long)]
        check: bool,
        /// Verify the file's baked coherence card (from `rete build --reason`)
        /// against a fresh reasoning run — guards against drift and a stale ruleset.
        #[arg(long = "verify-card")]
        verify_card: bool,
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
    /// Validate a **remote** `.rete` over HTTP against SHACL shapes, range-reading
    /// only what the shapes target. The file is opened lazily and each focus
    /// node's values are fetched as routed range reads, so a targeted shape never
    /// downloads the whole graph. Validates the default graph.
    ShaclUrl {
        /// http(s):// URL of a `.rete` file (host must honor Range requests).
        url: String,
        /// Turtle file containing SHACL shapes.
        #[arg(long)]
        shapes: String,
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
        /// OWL 2 QL entailment: rewrite the query against the ontology so the
        /// answer includes `rdfs:subClassOf`-entailed solutions, computed over
        /// the raw data with no materialization (opt-in; off = exact-match).
        #[arg(long)]
        entail: bool,
    },
    /// Serve a `.rete` as a live SPARQL 1.1 Protocol endpoint — queries AND
    /// SPARQL Update. The base file is never mutated: updates append to a
    /// journal (`<file>.changes`) and the merged state is queryable
    /// immediately; `GET /snapshot.rete` downloads it as a fresh `.rete`
    /// (publish = upload the snapshot, delete the journal). Other rete
    /// clients can federate against it with `SERVICE <http://host:port/sparql>`.
    Serve {
        /// Path to the `.rete` file — or a `.rete-manifest.json` (`.json`),
        /// serving that manifest's visible fold — to serve.
        file: String,
        /// Address to bind. Loopback by default — bind 0.0.0.0 deliberately,
        /// and set --token when you do.
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: String,
        /// Bearer token required for updates (queries stay open).
        #[arg(long)]
        token: Option<String>,
        /// Journal path override (default: `<file>.changes`).
        #[arg(long)]
        journal: Option<String>,
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
        /// OWL 2 QL entailment (see `sparql --entail`): reason over the ontology
        /// while reading only the bytes the rewritten query touches.
        #[arg(long)]
        entail: bool,
    },
    /// Explain a triple-pattern result over a **remote** `.rete` (HTTP range):
    /// which permutation, section, and byte ranges answer it — fetching only the
    /// routed tiles. The remote counterpart of `rete why`.
    WhyUrl {
        /// http(s):// URL of a `.rete` file (host must honor Range requests).
        url: String,
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
    /// Manage a **manifest**: a writable logical graph made of immutable
    /// `.rete` segments — an ordered log of additions and tombstone deletions
    /// that many sessions can grow independently, queried as ONE graph, and
    /// folded back into a single `.rete` with `compact`. `rete serve` accepts
    /// a manifest too (live SPARQL Update over the fold); `seal` then turns
    /// its journal into fresh segments.
    Manifest {
        #[command(subcommand)]
        command: ManifestCommand,
    },
}

#[derive(Subcommand)]
enum ManifestCommand {
    /// Start a manifest whose log holds one base segment.
    Init {
        /// Path of the manifest to create (conventionally `<name>.rete-manifest.json`).
        manifest: String,
        /// The base `.rete` segment (path relative to the manifest, or http(s):// URL).
        base: String,
        /// Logical graph name (default: the manifest's file stem).
        #[arg(long)]
        name: Option<String>,
    },
    /// Append one log entry — a built segment and/or a tombstone file — and
    /// bump the generation. This is how independent sessions contribute.
    Add {
        /// Path to the manifest.
        manifest: String,
        /// A `.rete` of quads this entry adds.
        #[arg(long)]
        adds: Option<String>,
        /// A `.rete` of quads this entry deletes (a tombstone segment).
        #[arg(long)]
        dels: Option<String>,
    },
    /// Show the log and verify every segment against its `{size, blake3_16}` pin.
    Status {
        /// Path to the manifest.
        manifest: String,
        /// Also run the full fold and report the visible quad count.
        #[arg(long)]
        count: bool,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run SPARQL over the fold as ONE graph: joins across segments resolve
    /// (pattern-level, unlike `federate`'s per-source UNION).
    Query {
        /// Path to the manifest.
        manifest: String,
        /// The SPARQL query (SELECT / ASK / CONSTRUCT).
        query: String,
        /// Emit standard SPARQL Results JSON (SELECT/ASK).
        #[arg(long)]
        json: bool,
        /// OWL 2 QL entailment (see `sparql --entail`).
        #[arg(long)]
        entail: bool,
    },
    /// Checkpoint a `rete serve` journal: net its `+`/`-` changes, build them
    /// as an adds segment + a tombstone segment, append one log entry, and
    /// truncate the journal. Stop the server first (single-writer journal).
    Seal {
        /// Path to the manifest.
        manifest: String,
        /// Journal path override (default: `<manifest>.changes`, matching `rete serve`).
        #[arg(long)]
        journal: Option<String>,
        /// Directory for the new segment files (default: the manifest's directory).
        #[arg(long)]
        dir: Option<String>,
    },
    /// Fold the whole log into ONE fresh `.rete` and reset the manifest to a
    /// single entry. Superseded segments are left on disk.
    Compact {
        /// Path to the manifest.
        manifest: String,
        /// Output path for the compacted `.rete` (default: `<name>-g<gen>-<hash>.rete`
        /// next to the manifest).
        #[arg(short, long)]
        output: Option<String>,
        /// Skip the pyramid build (faster; the file loses semantic-zoom summaries).
        #[arg(long)]
        no_pyramid: bool,
    },
}

#[derive(Debug)]
enum CliError {
    Usage(clap::Error),
    Runtime(anyhow::Error),
    NonConformance(anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct NonConformance(String);

impl NonConformance {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(CliError::Usage(error)) => {
            let code = error.exit_code();
            let _ = error.print();
            ExitCode::from(code as u8)
        }
        Err(CliError::Runtime(error)) => {
            eprintln!("Error: {error:#}");
            ExitCode::from(1)
        }
        Err(CliError::NonConformance(error)) => {
            eprintln!("Error: {error:#}");
            ExitCode::from(3)
        }
    }
}

fn run() -> Result<ExitCode, CliError> {
    let cli = Cli::try_parse().map_err(CliError::Usage)?;
    match dispatch(cli.command) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(error) if error.downcast_ref::<NonConformance>().is_some() => {
            Err(CliError::NonConformance(error))
        }
        Err(error) => Err(CliError::Runtime(error)),
    }
}

fn dispatch(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Generate { output } => generate_release_artifacts(&output),
        Command::Build {
            inputs,
            output,
            format,
            collapse_graphs,
            materialize,
            reason,
            no_pyramid,
            pyramid_algo,
            text_index,
            type_predicate,
            card,
            card_file,
            title,
            license,
            source,
            description,
            created,
            no_card_costs,
            memory_budget_mb,
            tmp_dir,
        } => {
            let card_args = commands::card::CardArgs {
                enabled: card,
                file: card_file,
                title,
                license,
                source,
                description,
                created,
            };
            if let Some(mb) = memory_budget_mb {
                commands::build::build_external_cmd(
                    &inputs,
                    &output,
                    format.as_deref(),
                    mb,
                    tmp_dir.as_deref(),
                    materialize,
                    reason,
                    text_index,
                    collapse_graphs,
                    card_args,
                )
            } else {
                commands::build::build(
                    &inputs,
                    &output,
                    format.as_deref(),
                    materialize,
                    no_pyramid,
                    reason,
                    rete_core::PyramidAlgo::from_cli(&pyramid_algo).unwrap_or_default(),
                    text_index,
                    type_predicate.as_deref(),
                    collapse_graphs,
                    card_args,
                    no_card_costs,
                )
            }
        }
        Command::Validate { inputs, format } => {
            commands::build::validate(&inputs, format.as_deref())
        }
        Command::Estimate {
            inputs,
            format,
            sample_mb,
            memory_budget_mb,
            no_pyramid,
        } => commands::estimate::estimate(
            &inputs,
            format.as_deref(),
            sample_mb,
            memory_budget_mb,
            no_pyramid,
        ),
        Command::Merge {
            inputs,
            output,
            memory_budget_mb,
            tmp_dir,
            card,
            card_file,
            title,
            license,
            source,
            description,
            created,
        } => commands::merge::merge_cmd(
            &inputs,
            &output,
            memory_budget_mb,
            tmp_dir.as_deref(),
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
        Command::Info { file } => commands::inspect::info(&file),
        Command::Stats { file } => commands::inspect::stats(&file),
        Command::Verify { file } => commands::inspect::verify_cmd(&file),
        Command::Search {
            file,
            prefix,
            contains,
            contains_prefix,
            limit,
            json,
        } => {
            if contains.is_empty() && contains_prefix.is_none() {
                commands::inspect::search(&file, &prefix, limit, json)
            } else {
                commands::inspect::search_contains(
                    &file,
                    &contains,
                    contains_prefix.as_deref(),
                    limit,
                    json,
                )
            }
        }
        Command::SearchUrl {
            url,
            prefix,
            contains,
            contains_prefix,
            limit,
            json,
        } => commands::url::search_url(
            &url,
            &prefix,
            &contains,
            contains_prefix.as_deref(),
            limit,
            json,
        ),
        Command::Card {
            file,
            json,
            format,
            sha256,
        } => commands::card::card_cmd(&file, json, format.as_deref(), sha256.as_deref()),
        Command::CardUrl {
            url,
            json,
            format,
            sha256,
        } => commands::url::card_url(&url, json, format.as_deref(), sha256.as_deref()),
        Command::CardAudit {
            path,
            json,
            measure,
            only,
            max_mb,
            write_costs,
            allow_empty,
        } => commands::card::card_audit_cmd(
            &path,
            &commands::card::AuditOptions {
                json,
                measure: measure || write_costs,
                only,
                max_mb,
                write_costs,
                allow_empty,
            },
        ),
        Command::Graphs { file } => commands::inspect::graphs(&file),
        Command::Export { file, format } => commands::export::export(&file, &format),
        Command::Repyramid {
            file,
            output,
            type_predicate,
            pyramid_algo,
            text_index,
            card,
            card_file,
            title,
            license,
            source,
            description,
            created,
        } => commands::build::repyramid(
            &file,
            &output,
            text_index,
            type_predicate.as_deref(),
            rete_core::PyramidAlgo::from_cli(&pyramid_algo).unwrap_or_default(),
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
        Command::Summary { file, level } => commands::inspect::summary(&file, level),
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
            url,
            materialize,
            format,
            check,
            verify_card,
        } => commands::reason::reason_cmd(
            file.as_deref(),
            url.as_deref(),
            materialize,
            &format,
            check,
            verify_card,
        ),
        Command::Shacl {
            file,
            shapes,
            graph,
            format,
        } => commands::shacl::shacl_cmd(&file, &shapes, graph.as_deref(), &format),
        Command::ShaclUrl {
            url,
            shapes,
            format,
        } => commands::shacl::shacl_url(&url, &shapes, &format),
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
        Command::Sparql {
            file,
            query,
            json,
            entail,
        } => commands::query::sparql(&file, &query, json, entail),
        Command::Serve {
            file,
            bind,
            token,
            journal,
        } => commands::serve::serve(&file, &bind, token.as_deref(), journal.as_deref()),
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
        Command::SparqlUrl {
            url,
            query,
            json,
            entail,
        } => commands::url::sparql_url(&url, &query, json, entail),
        Command::WhyUrl {
            url,
            subject,
            predicate,
            object,
            json,
        } => commands::url::why_url(&url, subject, predicate, object, json),
        Command::Federate {
            sources,
            query,
            json,
            no_route,
        } => commands::federate::federate(&sources, &query, json, !no_route),
        Command::Manifest { command } => match command {
            ManifestCommand::Init {
                manifest,
                base,
                name,
            } => commands::manifest::init(&manifest, &base, name.as_deref()),
            ManifestCommand::Add {
                manifest,
                adds,
                dels,
            } => commands::manifest::add(&manifest, adds.as_deref(), dels.as_deref()),
            ManifestCommand::Status {
                manifest,
                count,
                json,
            } => commands::manifest::status(&manifest, count, json),
            ManifestCommand::Query {
                manifest,
                query,
                json,
                entail,
            } => commands::manifest::query(&manifest, &query, json, entail),
            ManifestCommand::Seal {
                manifest,
                journal,
                dir,
            } => commands::manifest::seal(&manifest, journal.as_deref(), dir.as_deref()),
            ManifestCommand::Compact {
                manifest,
                output,
                no_pyramid,
            } => commands::manifest::compact(&manifest, output.as_deref(), no_pyramid),
        },
    }
}

fn generate_release_artifacts(output: &Path) -> anyhow::Result<()> {
    use clap_complete::Shell;
    use std::fs::File;

    std::fs::create_dir_all(output)?;
    for (shell, name) in [
        (Shell::Bash, "rete.bash"),
        (Shell::Zsh, "_rete"),
        (Shell::Fish, "rete.fish"),
        (Shell::PowerShell, "rete.ps1"),
    ] {
        let mut command = Cli::command();
        let mut file = File::create(output.join(name))?;
        clap_complete::generate(shell, &mut command, "rete", &mut file);
    }

    let mut man = File::create(output.join("rete.1"))?;
    clap_mangen::Man::new(Cli::command()).render(&mut man)?;
    Ok(())
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
