//! Build-memory profiler: walk the `.rete` assembly phases one at a time over a
//! real N-Triples file, snapshotting the **live heap** (via the counting
//! allocator) after each phase plus the high-water mark *during* the heavy
//! phases. The point is to see where peak RAM goes when converting a big graph —
//! and to quantify how much the raw string quads cost once they are redundant
//! with the dictionary.
//!
//! Run: `cargo run --release -p rete-bench -- --build-mem <file.nt>`

use std::time::Instant;

use anyhow::{Context, Result};
use rete_core::{
    build_pyramid_meta_with, ingest, write_dataset, DictionaryBuilder, GraphIndexBuilder,
    DEFAULT_TILE_BUDGET,
};

use crate::mem;

fn row(phase: &str, live: usize, peak: usize, ms: f64) {
    println!(
        "| {phase:<34} | {:>10} | {:>10} | {:>8.0} |",
        mem::mib(live),
        mem::mib(peak),
        ms
    );
}

pub fn run(path: &str) -> Result<()> {
    println!("# Build-memory profile: `{path}`\n");
    println!("Live heap after each phase and the high-water mark reached *during* it,");
    println!("from the counting allocator (exact, not sampled).\n");
    println!("| Phase | live heap MiB | peak MiB | ms |");
    println!("|---|--:|--:|--:|");

    // 1. Stream-parse the file → Vec<RawQuad> (every term an owned String, heavily
    //    duplicated) without ever materializing the whole text — the path
    //    `rete build` now takes for an N-Triples/N-Quads file.
    mem::reset_peak();
    let t = Instant::now();
    let file = std::fs::File::open(path).with_context(|| format!("open {path}"))?;
    let cap = file
        .metadata()
        .map(|m| (m.len() / 64) as usize)
        .unwrap_or(0);
    let quads = ingest::parse_reader(std::io::BufReader::new(file), "nt", cap)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let n = quads.len();
    row(
        "stream-parse file → quads",
        mem::live(),
        mem::peak(),
        elapsed(t),
    );

    // 3. Build the dictionary (interns every unique term once).
    mem::reset_peak();
    let t = Instant::now();
    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in &quads {
        db.observe(s, p, o);
    }
    let dict = db.build();
    row("build dictionary", mem::live(), mem::peak(), elapsed(t));

    // 4. Encode quads → id-triples (Vec<(u32,u32,u32)>).
    mem::reset_peak();
    let t = Instant::now();
    let triples: Vec<(u32, u32, u32)> = quads
        .iter()
        .map(|(s, p, o, _)| dict.encode(s, p, o).expect("observed term"))
        .collect();
    row("encode → id-triples", mem::live(), mem::peak(), elapsed(t));

    // 4b. The raw string quads are now redundant with the dictionary + id-triples.
    // Measure what they were costing by dropping them here.
    let live_before = mem::live();
    drop(quads);
    let freed = live_before.saturating_sub(mem::live());
    row("DROP raw string quads", mem::live(), mem::peak(), 0.0);
    println!(
        "| → freeing the string quads released **{} MiB** | | | |",
        mem::mib(freed)
    );

    // 5. Build the pyramid (project graph, Louvain dendrogram, summary, tiles,
    //    plus the query_stats / characteristic-set / label-index blocks).
    mem::reset_peak();
    let t = Instant::now();
    let (meta, levels) = build_pyramid_meta_with(&dict, &triples, DEFAULT_TILE_BUDGET, None);
    row(
        "build pyramid + stat blocks",
        mem::live(),
        mem::peak(),
        elapsed(t),
    );

    // 6. Build the permutation index. `GraphIndexBuilder::new()` is the default
    // set, so this is whatever `rete build` writes without `--permutations` —
    // six orders, not the three this row claimed for two format generations.
    mem::reset_peak();
    let t = Instant::now();
    let mut ib = GraphIndexBuilder::new();
    for &tr in &triples {
        ib.push(tr);
    }
    let index = ib.build();
    let perms = index.perms();
    row(
        &format!("build index ({} perms)", perms.len()),
        mem::live(),
        mem::peak(),
        elapsed(t),
    );

    // 7. Serialize the file image.
    mem::reset_peak();
    let t = Instant::now();
    let bytes = write_dataset(&dict, &index, &[], false, &meta, levels);
    let file_len = bytes.len();
    row("write_dataset", mem::live(), mem::peak(), elapsed(t));

    drop((dict, triples, index, meta, bytes));
    println!(
        "\n{n} triples → {} MiB file. Process peak RSS (`VmHWM`): {}.\n",
        mem::mib(file_len),
        mem::vm_hwm_kb()
            .map(|kb| format!("{:.0} MiB", kb as f64 / 1024.0))
            .unwrap_or_else(|| "n/a".into())
    );
    Ok(())
}

fn elapsed(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
