//! Build-memory profiler: walk the `.rete` assembly phases one at a time over a
//! real N-Triples file, snapshotting the **live heap** (via the counting
//! allocator) after each phase plus the high-water mark *during* the heavy
//! phases. The point is to see where peak RAM goes when converting a big graph —
//! and to quantify how much the raw string quads cost once they are redundant
//! with the dictionary.
//!
//! The phases follow `rete build --card --text-index` in its order: the card is
//! derived from the raw quads while they are still resident, the text index is
//! built next to the pyramid (both need the dictionary and the id-triples), and
//! the starter queries are costed last, each one cold against the written file —
//! the step `rete build` takes unless `--no-card-costs` is given, and the one
//! that sets a carded build's peak on large graphs.
//!
//! Run: `cargo run --release -p rete-bench -- --build-mem <file.nt>
//! [--no-card] [--no-card-costs] [--no-text-index]`

use std::time::Instant;

use anyhow::{Context, Result};
use rete_core::{
    build_pyramid_meta_with, card, compute_text_index, ingest, write_dataset_with_metadata,
    DictionaryBuilder, GraphIndexBuilder, RangeReader, Rete, DEFAULT_TILE_BUDGET,
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

/// Which optional phases to profile (all on by default).
#[derive(Debug, Clone, Copy)]
pub struct Phases {
    pub card: bool,
    pub card_costs: bool,
    pub text_index: bool,
}

impl Phases {
    /// From the command line: `--no-card`, `--no-card-costs`, `--no-text-index`.
    pub fn from_args(args: &[String]) -> Self {
        let has = |f: &str| args.iter().any(|a| a == f);
        let card = !has("--no-card");
        Phases {
            card,
            card_costs: card && !has("--no-card-costs"),
            text_index: !has("--no-text-index"),
        }
    }
}

/// The written image as an owned range reader, so each starter query gets a
/// cold lazy open of its own — what `rete build` measures.
struct ImageReader(std::sync::Arc<Vec<u8>>);

impl RangeReader for ImageReader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let (o, l) = (offset as usize, len as usize);
        self.0
            .get(o..o.saturating_add(l))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "range out of bounds")
            })
    }
}

pub fn run(path: &str, phases: Phases) -> Result<()> {
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

    // 4a. Derive the Dataset Card from the raw quads — `rete build --card` does
    // it here, in its metadata callback, because the quads are about to go.
    let dataset_card = if phases.card {
        mem::reset_peak();
        let t = Instant::now();
        let c = card::derive_card(
            &quads,
            dict.term_count() as u64,
            0,
            card::CardInput::default(),
        );
        row(
            &format!("derive card ({} starters)", c.queries.len()),
            mem::live(),
            mem::peak(),
            elapsed(t),
        );
        Some(c)
    } else {
        None
    };

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

    // 5b. The full-text index (`--text-index`): every string literal tokenized
    // into `token → subjects`, while the dictionary and id-triples are both
    // still resident — the same function `rete build` calls.
    let text_index = if phases.text_index {
        mem::reset_peak();
        let t = Instant::now();
        let ti = compute_text_index(&dict, &triples);
        row(
            &format!("build text index ({} MiB)", mem::mib(ti.len())),
            mem::live(),
            mem::peak(),
            elapsed(t),
        );
        ti
    } else {
        Vec::new()
    };

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

    // 7. Serialize the file image, the card's final counts stamped in as the
    // build stamps them (the index has deduplicated the input by now).
    mem::reset_peak();
    let t = Instant::now();
    let blob = dataset_card
        .as_ref()
        .map(|c| {
            let unique = u64::from(index.triple_count());
            c.clone()
                .with_final_counts(ingest::FinalCounts {
                    default_triples: unique,
                    quads: unique,
                })
                .to_json_bytes()
        })
        .unwrap_or_default();
    let bytes =
        write_dataset_with_metadata(&dict, &index, &[], false, &meta, levels, &blob, &text_index);
    let file_len = bytes.len();
    row("write_dataset", mem::live(), mem::peak(), elapsed(t));

    drop((dict, triples, index, meta, text_index, blob));

    // 8. Cost the starter queries: each cold, through a fresh lazy open of the
    // written image, exactly as `rete build` does before it embeds the costs
    // (and drops the queries that come back empty). The phase row is the
    // highest peak of any one query; the heaviest are listed under it.
    if let Some(c) = dataset_card.as_ref().filter(|_| phases.card_costs) {
        let image = std::sync::Arc::new(bytes);
        let base = mem::live();
        let t = Instant::now();
        let mut per: Vec<(usize, f64, String, String)> = Vec::new();
        for q in &c.queries {
            mem::reset_peak();
            let tq = Instant::now();
            let outcome = match Rete::open_ranged_lazy(ImageReader(image.clone())) {
                Ok(rete) => match rete_core::eval_query(&rete, &q.sparql) {
                    Ok(_) => "ok".to_string(),
                    Err(e) => format!("error: {e}"),
                },
                Err(e) => format!("open failed: {e}"),
            };
            per.push((mem::peak(), elapsed(tq), q.id.clone(), outcome));
        }
        let worst = per.iter().map(|p| p.0).max().unwrap_or(base);
        row(
            &format!("card query costs ({} queries)", per.len()),
            mem::live(),
            worst,
            elapsed(t),
        );
        per.sort_by(|a, b| b.0.cmp(&a.0));
        for (peak, ms, id, outcome) in per.iter().take(5) {
            let note = if outcome == "ok" {
                String::new()
            } else {
                format!(" ({outcome})")
            };
            row(&format!("  ↳ {id}{note}"), base, *peak, *ms);
        }
    }

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
