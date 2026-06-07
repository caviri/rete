//! Honest serial-vs-parallel benchmark for the prototype data-parallel evaluator
//! (`crate::parallel`). Build & run in the dev container:
//!
//! ```text
//! cargo run --release -p rete-core --example parallel_bench --features parallel
//! ```
//!
//! It builds a large synthetic social graph *in-process* (same shape as
//! `scripts/gen_graph.py`: people with `age`/`name` literals and ~5 intra-
//! community `knows` edges each), writes & opens a real `.rete`, then runs each
//! workload serial vs parallel, asserts the results are identical, and prints the
//! speedup. Best-of-N timing after a warm-up. Panics on any RESULTS MISMATCH.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use rete_core::parallel::{
    batch_reach_parallel, count_predicate_parallel, count_predicate_serial,
    count_predicate_tiles_serial, out_degree_parallel, out_degree_serial,
};
use rete_core::{
    batch_reach_serial, build_adjacency, build_dendrogram, build_pyramid_meta,
    choose_round_for_budget, project_graph, tile_by_community, write_dataset, Dictionary,
    DictionaryBuilder, GraphIndexBuilder, Rete, DEFAULT_TILE_BUDGET,
};

/// Deterministic xorshift so the graph is reproducible without an RNG crate.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Build integer triples for `people` people in communities of `comm_size`, with
/// `knows_per` outgoing `knows` edges each (90% intra-community), plus `age` and
/// `name` literals. Mirrors `scripts/gen_graph.py`.
fn gen_triples(
    people: usize,
    knows_per: usize,
    comm_size: usize,
) -> (Dictionary, Vec<(u32, u32, u32)>) {
    let mut db = DictionaryBuilder::new();
    let p = |i: usize| format!("http://ex/p{i}");
    let age = |i: usize| format!("{}", 18 + (i % 60));
    let mut rng = Rng(0x9E3779B97F4A7C15);

    // First pass: observe all terms, recording the knows edges as we draw them.
    for i in 0..people {
        db.observe(&p(i), "http://ex/age", &age(i));
        db.observe(&p(i), "http://ex/name", &format!("Person {i}"));
    }
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for i in 0..people {
        let c = i / comm_size;
        let lo = c * comm_size;
        let hi = ((c + 1) * comm_size).min(people);
        for _ in 0..knows_per {
            let j = if rng.unit() < 0.9 && hi - lo > 1 {
                lo + rng.below(hi - lo)
            } else {
                rng.below(people)
            };
            if j != i {
                db.observe(&p(i), "http://ex/knows", &p(j));
                edges.push((i, j));
            }
        }
    }
    let dict = db.build();

    let mut triples = Vec::new();
    for i in 0..people {
        triples.push(dict.encode(&p(i), "http://ex/age", &age(i)).unwrap());
        triples.push(
            dict.encode(&p(i), "http://ex/name", &format!("Person {i}"))
                .unwrap(),
        );
    }
    for (i, j) in edges {
        triples.push(dict.encode(&p(i), "http://ex/knows", &p(j)).unwrap());
    }
    triples.sort_unstable();
    triples.dedup();
    (dict, triples)
}

/// Best-of-`reps` wall time of `f` (after one warm-up call).
fn best_of<T>(reps: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut out = f(); // warm-up (also the value we return)
    let mut best = Duration::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        out = f();
        best = best.min(t.elapsed());
    }
    (best, out)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn report(name: &str, serial: Duration, parallel: Duration, matched: bool) {
    let speedup = serial.as_secs_f64() / parallel.as_secs_f64();
    println!("\n## {name}");
    println!("  serial   : {:>9.3} ms", ms(serial));
    println!("  parallel : {:>9.3} ms", ms(parallel));
    println!("  speedup  : {speedup:>9.2}x");
    if matched {
        println!("  RESULTS MATCH");
    } else {
        println!("  RESULTS MISMATCH");
        panic!("benchmark integrity violation: {name} serial != parallel");
    }
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    println!("=== rete parallel evaluator benchmark ===");
    println!("available_parallelism (cores seen): {cores}");
    println!("rayon threads: {}", rayon::current_num_threads());

    // ~20k people * (age+name+~5 knows) ≈ 140k triples. Bump to scale.
    let people: usize = std::env::var("BENCH_PEOPLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let knows_per = 5;
    let comm_size = 100;

    println!(
        "\nbuilding graph: {people} people, {knows_per} knows/each, communities of {comm_size}"
    );
    let t0 = Instant::now();
    let (dict, triples) = gen_triples(people, knows_per, comm_size);
    println!(
        "generated {} triples in {:.0} ms",
        triples.len(),
        ms(t0.elapsed())
    );

    // Build a real .rete and open it (exercises the full engine path).
    let t0 = Instant::now();
    let mut def = GraphIndexBuilder::new();
    for &t in &triples {
        def.push(t);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &triples, DEFAULT_TILE_BUDGET);
    let bytes = write_dataset(&dict, &def.build(), &[], false, &meta, levels);
    let rete = Rete::open(&bytes).expect("open .rete");
    println!(
        "built + opened .rete ({} bytes, {} pyramid levels) in {:.0} ms",
        bytes.len(),
        levels,
        ms(t0.elapsed())
    );

    // Tiles for the per-community workloads.
    let g = project_graph(&dict, &triples);
    let dend = build_dendrogram(&g);
    let round = choose_round_for_budget(&dict, &triples, &dend, DEFAULT_TILE_BUDGET);
    let tiles = tile_by_community(&dict, &triples, &dend, round);
    println!("tiled into {} communities at round {round}", tiles.len());

    let knows_pid = dict.predicate_id("http://ex/knows").unwrap();

    // -------------------------------------------------------------------
    // Workload A: cheap single predicate count (expect parallel to LOSE —
    // the scan is already fast; tiling + rayon overhead dominates).
    // -------------------------------------------------------------------
    let (s, sv) = best_of(20, || count_predicate_serial(&rete, "http://ex/knows"));
    let (pn, pv) = best_of(20, || count_predicate_parallel(&tiles, knows_pid));
    // Cross-check the tiled serial sum equals the index count too.
    assert_eq!(sv, count_predicate_tiles_serial(&tiles, knows_pid));
    report(
        "Workload A — predicate count (cheap scan; parallel expected to lose)",
        s,
        pn,
        sv == pv,
    );
    println!("  (count = {sv} knows edges)");

    // -------------------------------------------------------------------
    // Workload B: per-subject out-degree distribution (heavier, decomposable;
    // BTreeMap merge as harmonize).
    // -------------------------------------------------------------------
    let (s, sv) = best_of(10, || out_degree_serial(&tiles));
    let (pn, pv) = best_of(10, || out_degree_parallel(&tiles));
    report(
        "Workload B — per-subject out-degree (per-community + BTreeMap merge)",
        s,
        pn,
        sv == pv,
    );
    println!("  (distinct subjects = {})", sv.len());

    // -------------------------------------------------------------------
    // Workload C: batch of independent reachability queries (embarrassingly
    // parallel; the clearest win — multi-source impact analysis).
    // -------------------------------------------------------------------
    let adj = build_adjacency(&rete, "http://ex/knows");
    // Pick a spread of seeds across the graph.
    let n_seeds = 512.min(people);
    let stride = (people / n_seeds).max(1);
    let seeds: Vec<u32> = (0..people)
        .step_by(stride)
        .filter_map(|i| dict.node_of_term(&format!("http://ex/p{i}")))
        .take(n_seeds)
        .collect();
    println!("\nbatch reachability over {} seeds", seeds.len());
    let (s, sv) = best_of(5, || batch_reach_serial(&adj, &seeds));
    let (pn, pv) = best_of(5, || batch_reach_parallel(&adj, &seeds));
    let total_reached: usize = sv.iter().map(BTreeSet::len).sum();
    report(
        "Workload C — batch reachability (embarrassingly parallel)",
        s,
        pn,
        sv == pv,
    );
    println!(
        "  (sum of per-seed reach set sizes = {total_reached}, avg {:.0}/seed)",
        total_reached as f64 / seeds.len().max(1) as f64
    );

    println!("\n=== done; all RESULTS MATCH ===");
}
