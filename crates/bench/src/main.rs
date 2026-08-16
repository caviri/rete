//! Three-way benchmark: **rete (serial)** vs **rete (parallel reach)** vs
//! **Oxigraph**, in one process, on identical data, with warm timings.
//!
//! Two workloads:
//!   1. **SPARQL query latency** — the same queries run on rete's engine
//!      (`eval_query`) and on an in-memory Oxigraph `Store`. rete's SPARQL is
//!      single-threaded; this is the apples-to-apples engine comparison.
//!   2. **Batch transitive reachability** — rete's dedicated batch-reach
//!      (`batch_reach_serial` vs `batch_reach_parallel`, the `parallel` feature)
//!      vs the equivalent on Oxigraph expressed as a `p+` property path per seed.
//!
//! Usage (paths relative to repo root):
//!   cargo run --release -p rete-bench -- <file.rete> <file.nt> [seed_count]
//!   cargo run --release -p rete-bench -- --json <file.rete> <file.nt> [seed_count]
//!
//! Emits Markdown tables by default, or a machine-readable JSON report with
//! `--json`.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use oxigraph::io::RdfFormat;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use rete_core::parallel::batch_reach_parallel;
use rete_core::{
    batch_reach_serial, build_adjacency, eval_query, CountingReader, QueryOutput, Rete, SliceReader,
};
use serde_json::{json, Value};

mod buildmem;
mod lubm;
mod mem;
mod pathread;
mod querymem;

/// Every allocation in this binary (both engines) goes through the counting
/// allocator, so per-query peak-heap numbers are exact, not sampled.
#[global_allocator]
static ALLOC: mem::CountingAlloc = mem::CountingAlloc;

const COAUTHOR: &str = "<http://ex/coauthor>";
const USAGE: &str =
    "usage: rete-bench [--json] <file.rete> <file.nt> [seeds]\n       rete-bench [--json] --lubm [universities]";

#[derive(Clone, Copy, Eq, PartialEq)]
enum OutputFormat {
    Markdown,
    Json,
}

/// Common prefixes, prepended to every query below.
const PREFIXES: &str = "PREFIX cito: <http://purl.org/spar/cito/> \
PREFIX dct: <http://purl.org/dc/terms/> PREFIX ex: <http://ex/> \
PREFIX prism: <http://prismstandard.org/namespaces/basic/2.0/> \
PREFIX foaf: <http://xmlns.com/foaf/0.1/> ";

/// A matrix of queries spanning operators and complexity, on the citation
/// network. `(operator label, query body)`. Row counts are compared across
/// engines as a cross-engine correctness check; some operators (DESCRIBE) are
/// impl-defined and may legitimately differ.
const QUERIES: &[(&str, &str)] = &[
    // --- forms & basics ---
    ("SELECT count (aggregate)",
     "SELECT (COUNT(?p) AS ?n) WHERE { ?p cito:cites <https://doi.org/10.1038/s41586-021-03819-2> }"),
    ("SELECT DISTINCT",
     "SELECT DISTINCT ?d WHERE { ?p ex:discipline ?d }"),
    ("ASK",
     "ASK { ?p ex:discipline <http://ex/discipline/Physics> }"),
    ("CONSTRUCT",
     "CONSTRUCT { ?a ex:coauthor ?b } WHERE { VALUES ?a { <http://ex/author/1235> } ?a ex:coauthor ?b }"),
    ("DESCRIBE (impl-defined)",
     "DESCRIBE <http://ex/author/1235>"),
    // --- joins & algebra ---
    ("VALUES (inline data)",
     "SELECT ?p WHERE { VALUES ?d { <http://ex/discipline/Biology> <http://ex/discipline/Physics> } ?p ex:discipline ?d }"),
    ("UNION",
     "SELECT ?p WHERE { { ?p ex:discipline <http://ex/discipline/Biology> } UNION { ?p ex:discipline <http://ex/discipline/Chemistry> } }"),
    ("OPTIONAL (left join)",
     "SELECT ?p ?v WHERE { ?p ex:discipline <http://ex/discipline/Biology> OPTIONAL { ?p prism:publicationName ?v } } LIMIT 200"),
    ("MINUS",
     "SELECT ?p WHERE { ?p ex:discipline <http://ex/discipline/Biology> MINUS { ?p dct:subject \"protein\" } }"),
    ("FILTER NOT EXISTS",
     "SELECT ?p WHERE { ?p ex:discipline <http://ex/discipline/Biology> FILTER NOT EXISTS { ?p dct:subject \"protein\" } }"),
    ("3-way join + LIMIT",
     "SELECT ?name ?title WHERE { ?p dct:subject \"protein\" . ?p dct:title ?title . ?p dct:creator ?a . ?a foaf:name ?name } LIMIT 50"),
    // --- filters, bind, functions ---
    ("FILTER REGEX (case-insens.)",
     "SELECT ?p ?t WHERE { ?p dct:title ?t FILTER(REGEX(?t, \"genome\", \"i\")) } LIMIT 200"),
    ("FILTER arith + logical",
     "SELECT ?p ?c WHERE { ?p ex:citationCount ?c FILTER(?c >= 100 && ?c <= 110) } LIMIT 200"),
    ("BIND + SUBSTR + CONCAT",
     "SELECT ?p ?label WHERE { ?p cito:cites <https://doi.org/10.1038/s41586-021-03819-2> . ?p dct:date ?y BIND(CONCAT(SUBSTR(?y,1,3), \"0s\") AS ?label) } LIMIT 200"),
    // --- property paths ---
    ("path sequence a/b",
     "SELECT ?name WHERE { ?p cito:cites <https://doi.org/10.1038/s41586-021-03819-2> . ?p dct:creator/foaf:name ?name } LIMIT 200"),
    ("path inverse ^p (count)",
     "SELECT (COUNT(?x) AS ?n) WHERE { <https://doi.org/10.1038/s41586-021-03819-2> ^cito:cites ?x }"),
    ("path + transitive (count)",
     "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE { <http://ex/author/1235> ex:coauthor+ ?o }"),
    ("path * zero-or-more (count)",
     "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE { <http://ex/author/1235> ex:coauthor* ?o }"),
    // --- aggregation & solution modifiers ---
    ("GROUP BY + ORDER BY",
     "SELECT ?d (COUNT(?p) AS ?n) WHERE { ?p ex:discipline ?d } GROUP BY ?d ORDER BY DESC(?n)"),
    ("GROUP BY + HAVING",
     "SELECT ?d (COUNT(?p) AS ?n) WHERE { ?p ex:discipline ?d } GROUP BY ?d HAVING(COUNT(?p) > 5400)"),
    ("AVG per group",
     "SELECT ?d (AVG(?c) AS ?avg) WHERE { ?p ex:discipline ?d . ?p ex:citationCount ?c } GROUP BY ?d ORDER BY DESC(?avg)"),
    ("MIN/MAX/SUM",
     "SELECT (MIN(?c) AS ?lo) (MAX(?c) AS ?hi) (SUM(?c) AS ?tot) WHERE { ?p ex:citationCount ?c }"),
    ("COUNT(DISTINCT)",
     "SELECT (COUNT(DISTINCT ?v) AS ?n) WHERE { ?p prism:publicationName ?v }"),
    ("ORDER BY + LIMIT + OFFSET",
     "SELECT ?p ?c WHERE { ?p ex:citationCount ?c } ORDER BY DESC(?c) LIMIT 10 OFFSET 50"),
];

/// Truncate an error message to keep the table tidy.
fn short(e: &str) -> String {
    let one = e.replace('\n', " ");
    if one.len() > 60 {
        format!("{}…", &one[..60])
    } else {
        one
    }
}

/// One timing measurement: median, sample standard deviation (the ± spread),
/// and the peak extra heap observed across the repetitions.
struct Measure {
    median_ms: f64,
    sd_ms: f64,
    peak_heap: usize,
}

fn median_sd(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v[v.len() / 2];
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let var = if v.len() > 1 {
        v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (v.len() - 1) as f64
    } else {
        0.0
    };
    (med, var.sqrt())
}

/// Run `f` `reps` times; report median ± sd timing plus the peak heap the
/// repetitions allocated beyond what was live when they started.
fn bench<F: FnMut() -> usize>(reps: usize, mut f: F) -> (Measure, usize) {
    let baseline = mem::live();
    mem::reset_peak();
    let mut times = Vec::with_capacity(reps);
    let mut last = 0;
    for _ in 0..reps {
        let t = Instant::now();
        last = f();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let (median_ms, sd_ms) = median_sd(times);
    let peak_heap = mem::peak().saturating_sub(baseline);
    (
        Measure {
            median_ms,
            sd_ms,
            peak_heap,
        },
        last,
    )
}

/// `2.41 ±0.05`-style cell.
fn pm(m: &Measure) -> String {
    format!("{:.2} ±{:.2}", m.median_ms, m.sd_ms)
}

/// Human byte size for the lazy-fetch column.
fn fmt_bytes(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else if b >= 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}

/// rete result size, or an error string (e.g. an unsupported operator).
fn rete_try(rete: &Rete, q: &str) -> Result<usize, String> {
    match eval_query(rete, q) {
        Ok(QueryOutput::Select(_, rows)) => Ok(rows.len()),
        Ok(QueryOutput::Ask(b)) => Ok(b as usize),
        Ok(QueryOutput::Construct(t)) => Ok(t.len()),
        Ok(_) => Err("unsupported query result kind".to_owned()),
        Err(e) => Err(format!("{e}")),
    }
}

/// Oxigraph result size, or an error string.
fn oxi_try(store: &Store, q: &str) -> Result<usize, String> {
    let ev = SparqlEvaluator::new()
        .parse_query(q)
        .map_err(|e| e.to_string())?;
    let res = ev.on_store(store).execute().map_err(|e| e.to_string())?;
    Ok(match res {
        QueryResults::Solutions(solutions) => solutions.filter(|s| s.is_ok()).count(),
        QueryResults::Boolean(b) => b as usize,
        QueryResults::Graph(triples) => triples.filter(|t| t.is_ok()).count(),
    })
}

fn parse_args() -> Result<(OutputFormat, Option<usize>, String, String, usize)> {
    let mut format = OutputFormat::Markdown;
    let mut lubm = false;
    let mut args = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--json" => format = OutputFormat::Json,
            "--markdown" => format = OutputFormat::Markdown,
            "--lubm" => lubm = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => args.push(arg),
        }
    }
    if lubm {
        let universities = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
        return Ok((format, Some(universities), String::new(), String::new(), 0));
    }

    let rete_path = args.first().cloned().context(USAGE)?;
    let nt_path = args.get(1).cloned().context(USAGE)?;
    let seed_count = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(300);
    Ok((format, None, rete_path, nt_path, seed_count))
}

fn main() -> Result<()> {
    // Build-memory profiler: `--build-mem <file.nt>` (separate from the query
    // benchmark; walks the assembly phases snapshotting the live heap).
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = raw.iter().position(|a| a == "--build-mem") {
        let path = raw
            .get(i + 1)
            .context("--build-mem needs a <file.nt> path")?;
        return buildmem::run(path);
    }
    // Query-path memory profiler: `--query-mem <file.rete> "<sparql>"`.
    if let Some(i) = raw.iter().position(|a| a == "--query-mem") {
        let path = raw
            .get(i + 1)
            .context("--query-mem needs <file.rete> \"<sparql>\"")?;
        let query = raw.get(i + 2).context("--query-mem needs a SPARQL query")?;
        return querymem::run(path, query);
    }
    // Warm, in-process safe property-path profiler with decoder counters.
    if let Some(i) = raw.iter().position(|a| a == "--path-read") {
        let path = raw
            .get(i + 1)
            .context("--path-read needs a <file.rete> path")?;
        let samples = raw
            .get(i + 2)
            .and_then(|value| value.parse().ok())
            .unwrap_or(15);
        return pathread::run(path, samples);
    }

    let (format, lubm_universities, rete_path, nt_path, seed_count) = parse_args()?;
    if let Some(universities) = lubm_universities {
        return lubm::run(format == OutputFormat::Json, universities);
    }
    let reps = 5;
    let reach_reps = 3;

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    // ---- Load both engines (one-time cost + memory, reported separately) ----
    let heap0 = mem::live();
    let bytes = std::fs::read(&rete_path).with_context(|| format!("read {rete_path}"))?;
    // provenance so a run is reproducible: content hash of the queried .rete and
    // the repo commit (best-effort `git`, else $GITHUB_SHA, else "unknown").
    let rete_sha256 = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&bytes))
    };
    let git_commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| option_env!("GITHUB_SHA").map(|s| s.chars().take(7).collect()))
        .unwrap_or_else(|| "unknown".to_string());
    let t = Instant::now();
    let rete = Rete::open(&bytes).context("Rete::open")?;
    let rete_open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let rete_heap = mem::live().saturating_sub(heap0);

    let heap1 = mem::live();
    let store = Store::new().context("Store::new")?;
    let t = Instant::now();
    store
        .load_from_reader(RdfFormat::NTriples, BufReader::new(File::open(&nt_path)?))
        .context("oxigraph load")?;
    let oxi_load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let oxi_len = store.len()?;
    let oxi_heap = mem::live().saturating_sub(heap1);

    // A 'static copy of the file image for the LAZY (range-read) engine, which
    // needs an owned reader. The lazy path faults only the index tiles + dictionary
    // chunks a query touches — the same code a browser/remote client runs over HTTP
    // range reads — so a `CountingReader` over it measures exactly the bytes that
    // query would fetch. Leaked once; the OS reclaims it at process exit.
    let lazy_bytes: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());

    if format == OutputFormat::Markdown {
        println!("# Benchmark: rete vs rete (parallel) vs Oxigraph\n");
        println!(
            "Data: `{rete_path}` ({} bytes) / `{nt_path}` · Oxigraph store: {oxi_len} triples · \
             {threads} logical cores · median ±sd of {reps} warm runs.\n\n\
             _commit `{git_commit}` · .rete sha256 `{}`_\n",
            bytes.len(),
            &rete_sha256[..16]
        );

        println!("## Load / open (one-time)\n");
        println!("| Engine | Step | Time | Resident heap after load |");
        println!("|---|---|--:|--:|");
        println!(
            "| rete | `Rete::open` (mmap-style, indexes already built) | {rete_open_ms:.1} ms | {} MiB (file image + parsed sections) |",
            mem::mib(rete_heap)
        );
        println!(
            "| Oxigraph | bulk-load N-Triples + index in memory | {oxi_load_ms:.0} ms | {} MiB |",
            mem::mib(oxi_heap)
        );
        if let Some(kb) = mem::vm_hwm_kb() {
            println!(
                "\nProcess peak RSS after both loads (`VmHWM`): {:.1} MiB.",
                kb as f64 / 1024.0
            );
        }
        println!();
    }

    // ---- Workload 1: SPARQL operator-coverage matrix ----
    if format == OutputFormat::Markdown {
        println!("## SPARQL operators & complexity (single-threaded engines)\n");
        println!(
            "Each query on three engines: rete **eager** (`Rete::open`, whole file in memory), \
             rete **lazy** (`Rete::open_ranged_lazy`, a fresh cold open + range reads per rep — \
             the path a browser/remote client runs), and **Oxigraph** (in-memory). `lazy fetched` \
             is the bytes that one query pulled from the file image; `rows` cross-checks result \
             sizes. Times are median ±sd.\n"
        );
        println!("| Operator / form | rete eager ms | rete lazy ms (cold) | lazy fetched | Oxigraph ms | eager vs oxi | rows (r/o) | ✓ |");
        println!("|---|--:|--:|--:|--:|--:|--:|:--:|");
    }
    let mut agree = 0;
    let mut total = 0;
    let mut query_rows: Vec<Value> = Vec::new();
    // Hard failures — a rete error, or a row-count disagreement on a query whose
    // result is NOT implementation-defined — make the bench exit non-zero, so a
    // correctness regression fails CI instead of just printing a lower tally.
    let mut hard_mismatch: Vec<String> = Vec::new();
    for (name, body) in QUERIES {
        let q = format!("{PREFIXES}{body}");
        let r0 = rete_try(&rete, &q);
        let o0 = oxi_try(&store, &q);
        total += 1;
        match (&r0, &o0) {
            (Ok(rr), Ok(or)) => {
                let (rete_m, _) = bench(reps, || rete_try(&rete, &q).unwrap_or(0));
                // Lazy (range-read) path: a fresh COLD open + eval each rep, so the
                // time is what a one-shot remote query pays (open the directories,
                // fault only the touched tiles, eval); `lazy_fetch` is the bytes that
                // one query actually pulled from the file image.
                let lazy_fetch = {
                    let reader =
                        std::sync::Arc::new(CountingReader::new(SliceReader::new(lazy_bytes)));
                    let r = Rete::open_ranged_lazy(reader.clone()).expect("open_ranged_lazy");
                    let n = rete_try(&r, &q).unwrap_or(0);
                    debug_assert_eq!(n, *rr, "lazy must agree with eager for {name}");
                    reader.bytes_read()
                };
                let (lazy_m, _) = bench(reps, || {
                    let reader =
                        std::sync::Arc::new(CountingReader::new(SliceReader::new(lazy_bytes)));
                    let r = Rete::open_ranged_lazy(reader.clone()).expect("open_ranged_lazy");
                    rete_try(&r, &q).unwrap_or(0)
                });
                let (oxi_m, _) = bench(reps, || oxi_try(&store, &q).unwrap_or(0));
                let speedup = oxi_m.median_ms / rete_m.median_ms;
                let ok = rr == or;
                if ok {
                    agree += 1;
                } else if !name.contains("impl-defined") {
                    hard_mismatch.push(format!("{name}: rete {rr} rows vs oxigraph {or}"));
                }
                query_rows.push(json!({
                    "name": name,
                    "query": body,
                    "rete_ms": rete_m.median_ms,
                    "rete_ms_sd": rete_m.sd_ms,
                    "rete_peak_heap_bytes": rete_m.peak_heap,
                    "rete_lazy_ms": lazy_m.median_ms,
                    "rete_lazy_ms_sd": lazy_m.sd_ms,
                    "lazy_bytes_fetched": lazy_fetch,
                    "oxigraph_ms": oxi_m.median_ms,
                    "oxigraph_ms_sd": oxi_m.sd_ms,
                    "oxigraph_peak_heap_bytes": oxi_m.peak_heap,
                    "speedup": speedup,
                    "rete_rows": rr,
                    "oxigraph_rows": or,
                    "agree": ok,
                    "implementation_defined": name.contains("impl-defined"),
                }));
                if format == OutputFormat::Markdown {
                    println!(
                        "| {name} | {} | {} | {} | {} | {speedup:.1}× | {rr} / {or} | {} |",
                        pm(&rete_m),
                        pm(&lazy_m),
                        fmt_bytes(lazy_fetch),
                        pm(&oxi_m),
                        if ok { "✓" } else { "✗" }
                    );
                }
            }
            (Err(e), _) => {
                hard_mismatch.push(format!("{name}: rete error: {}", short(e)));
                query_rows.push(json!({
                    "name": name,
                    "query": body,
                    "rete_error": short(e),
                }));
                if format == OutputFormat::Markdown {
                    println!("| {name} | _rete: {}_ | — | — | — | — |", short(e));
                }
            }
            (_, Err(e)) => {
                query_rows.push(json!({
                    "name": name,
                    "query": body,
                    "oxigraph_error": short(e),
                }));
                if format == OutputFormat::Markdown {
                    println!("| {name} | — | _oxi: {}_ | — | — | — |", short(e));
                }
            }
        }
    }
    if format == OutputFormat::Markdown {
        println!("\n{agree}/{total} queries returned identical row counts on both engines.\n");
    }
    ensure!(
        hard_mismatch.is_empty(),
        "cross-engine correctness regression:\n  {}",
        hard_mismatch.join("\n  ")
    );

    // ---- Workload 2: batch transitive reachability ----
    // Seeds: first `seed_count` distinct subjects of the coauthor relation.
    let dict = rete.dictionary();
    let pairs = rete.predicate_pairs(COAUTHOR);
    let mut seen = BTreeSet::new();
    let mut seed_nodes: Vec<u32> = Vec::new();
    for (s, _) in &pairs {
        if seen.insert(*s) {
            seed_nodes.push(*s);
            if seed_nodes.len() >= seed_count {
                break;
            }
        }
    }
    let seed_iris: Vec<String> = seed_nodes
        .iter()
        .filter_map(|n| dict.node_term(*n))
        .collect();

    let adj = build_adjacency(&rete, COAUTHOR);
    let (serial_m, total_serial) = bench(reach_reps, || {
        let sets = batch_reach_serial(&adj, &seed_nodes);
        sets.iter().map(BTreeSet::len).sum()
    });
    let (par_m, total_par) = bench(reach_reps, || {
        let sets = batch_reach_parallel(&adj, &seed_nodes);
        sets.iter().map(BTreeSet::len).sum()
    });
    let (serial_ms, par_ms) = (serial_m.median_ms, par_m.median_ms);
    // The parallel reach must reproduce the serial result exactly — assert it
    // rather than just claiming it in prose.
    ensure!(
        total_serial == total_par,
        "parallel reach disagrees with serial: {total_par} vs {total_serial} nodes"
    );

    // Oxigraph: same transitive closure expressed as a property path, per seed.
    let (oxi_reach_m, total_oxi) = bench(reach_reps, || {
        let mut total = 0usize;
        for iri in &seed_iris {
            let q = format!(
                "PREFIX ex: <http://ex/> SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE {{ {iri} ex:coauthor+ ?o }}"
            );
            // Read the single COUNT cell so the work is actually performed.
            if let QueryResults::Solutions(mut sols) = SparqlEvaluator::new()
                .parse_query(&q)
                .expect("parse")
                .on_store(&store)
                .execute()
                .expect("exec")
            {
                if let Some(Ok(sol)) = sols.next() {
                    if let Some(v) = sol.get("n") {
                        // Parse the COUNT(DISTINCT ?o) integer literal
                        // (`"42"^^xsd:integer`) so the total is the real reach
                        // count, comparable to rete's — not a meaningless length.
                        let s = v.to_string();
                        let lex = s.trim_start_matches('"').split('"').next().unwrap_or(&s);
                        total += lex.parse::<usize>().unwrap_or(0);
                    }
                }
            }
        }
        total
    });
    let oxi_reach_ms = oxi_reach_m.median_ms;

    if format == OutputFormat::Markdown {
        println!(
            "## Batch transitive reachability — `coauthor+` from {} seeds\n",
            seed_nodes.len()
        );
        println!(
            "rete reached {total_serial} nodes total (serial), {total_par} (parallel); \
             the two agree. Oxigraph touched {total_oxi} result cells.\n"
        );
        println!("| Engine / mode | Time | vs rete-serial |");
        println!("|---|--:|--:|");
        println!(
            "| rete — `batch_reach_serial` (1 core) | {serial_ms:.1} ±{:.1} ms | 1.0× |",
            serial_m.sd_ms
        );
        println!(
            "| rete — `batch_reach_parallel` ({threads} cores) | {par_ms:.1} ±{:.1} ms | {:.1}× |",
            par_m.sd_ms,
            serial_ms / par_ms
        );
        println!(
            "| Oxigraph — `coauthor+` property path, per seed | {oxi_reach_ms:.0} ±{:.0} ms | {:.1}× |",
            oxi_reach_m.sd_ms,
            serial_ms / oxi_reach_ms
        );
        println!();
        println!(
            "_rete's reach is a purpose-built BFS over a prebuilt adjacency map; Oxigraph evaluates a \
             general SPARQL property path. Different abstraction levels — read it as \"a dedicated \
             graph primitive vs. a general SPARQL engine,\" not a like-for-like core comparison._"
        );
    } else {
        let report = json!({
            "schema_version": 2,
            "tool": "rete-bench",
            "inputs": {
                "rete_path": rete_path,
                "nt_path": nt_path,
                "rete_bytes": bytes.len(),
                "rete_sha256": rete_sha256,
                "oxigraph_triples": oxi_len,
            },
            "environment": {
                "logical_cores": threads,
                "query_repetitions": reps,
                "reach_repetitions": reach_reps,
                "git_commit": git_commit,
            },
            "load_open": {
                "rete_ms": rete_open_ms,
                "oxigraph_ms": oxi_load_ms,
                "rete_heap_bytes": rete_heap,
                "oxigraph_heap_bytes": oxi_heap,
                "process_peak_rss_kb": mem::vm_hwm_kb(),
            },
            "queries": query_rows,
            "query_agreement": {
                "agree": agree,
                "total": total,
            },
            "reachability": {
                "predicate": COAUTHOR,
                "seed_count": seed_nodes.len(),
                "rete_serial_ms": serial_ms,
                "rete_serial_ms_sd": serial_m.sd_ms,
                "rete_parallel_ms": par_ms,
                "rete_parallel_ms_sd": par_m.sd_ms,
                "oxigraph_ms": oxi_reach_ms,
                "oxigraph_ms_sd": oxi_reach_m.sd_ms,
                "rete_serial_total": total_serial,
                "rete_parallel_total": total_par,
                "oxigraph_total": total_oxi,
                "parallel_speedup_vs_serial": serial_ms / par_ms,
                "oxigraph_vs_rete_serial": serial_ms / oxi_reach_ms,
            }
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }

    Ok(())
}
