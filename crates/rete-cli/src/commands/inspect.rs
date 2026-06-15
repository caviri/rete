//! The inspect group: read-only introspection of a `.rete` file — header
//! (`info`), human overview (`stats`), named graphs (`graphs`), content-hash
//! check (`verify`), the pyramid summary (`summary`), per-predicate totals
//! (`predicates`), and the type-level schema (`schema`). Several of these answer
//! from the summary alone, never touching the triple index.

use rete_core::{Rete, SliceReader, SummaryView, CODEC_ZSTD};

/// Print the raw file header (section offsets, counts, codec), plus the embedded
/// Dataset Card catalog when the file carries one.
pub(crate) fn info(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let header = rete_core::Header::from_bytes(&bytes)?;
    println!("{header:#?}");
    if let Some(card) = crate::commands::card::load_card(&bytes)? {
        println!();
        println!(
            "{}",
            crate::commands::card::format_card(
                &card,
                &crate::commands::card::hex16(&header.content_hash)
            )
        );
    }
    Ok(())
}

/// Human-friendly overview: size, counts, graphs, pyramid, top predicates. The
/// per-predicate totals + community count come from the summary alone (the
/// triple index is never read).
pub(crate) fn stats(file: &str) -> anyhow::Result<()> {
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

/// List the named graphs in a dataset (or note that there are none).
pub(crate) fn graphs(file: &str) -> anyhow::Result<()> {
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

/// Verify a file's content hash (detects corruption/truncation). Exits non-zero
/// on mismatch.
pub(crate) fn verify_cmd(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    if rete_core::verify(&bytes)? {
        println!("OK — content hash matches");
        Ok(())
    } else {
        anyhow::bail!("FAILED — content hash mismatch (file corrupted or truncated)");
    }
}

/// Print the pyramid summary — the community super-edge graph, and (for v2 files)
/// the **schema pyramid**: a leveled type histogram (abstract classes at coarse
/// levels, leaves as you zoom in). All read **index-free** from the pyramid-meta
/// via [`SummaryView`]. With `--level k`, print just level `k`'s type histogram.
pub(crate) fn summary(file: &str, level: Option<usize>) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let Some(view) = SummaryView::open_ranged(&SliceReader::new(&bytes))? else {
        eprintln!("file has no pyramid");
        return Ok(());
    };

    if let Some(k) = level {
        return render_schema_level(&view, k);
    }

    println!(
        "pyramid round {} — {} communities summarized as {} superedge(s):",
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

    render_schema_overview(&view);
    Ok(())
}

/// Render the whole schema pyramid: one line per semantic level (coarse →
/// abstract, fine → leaves) with the top classes at that level.
pub(crate) fn render_schema_overview(view: &SummaryView) {
    if view.level_count() == 0 {
        return;
    }
    println!(
        "schema pyramid — {} level(s), {} class(es) in the subClassOf hierarchy:",
        view.level_count(),
        view.class_hierarchy.len()
    );
    for (k, lvl) in view.level_rollups.iter().enumerate() {
        let top: Vec<String> = lvl
            .classes
            .iter()
            .take(6)
            .map(|(c, n)| format!("{c}×{n}"))
            .collect();
        let more = if lvl.classes.len() > 6 {
            format!(" … (+{})", lvl.classes.len() - 6)
        } else {
            String::new()
        };
        println!(
            "  level {k} (depth {}, round {}): {}{more}",
            lvl.depth,
            lvl.round,
            top.join(", ")
        );
    }
    let link_total: usize = view.level_links.iter().map(|l| l.links.len()).sum();
    if link_total > 0 {
        println!(
            "  + {link_total} lateral class-relation(s) across levels (use --level k to see them)"
        );
    }
    if !view.descriptors.is_empty() {
        println!(
            "  + {} per-community descriptor(s) for progressive zoom (use the JSON via the API)",
            view.descriptors.len()
        );
    }
}

/// Render one schema-pyramid level's full type histogram (index-free).
fn render_schema_level(view: &SummaryView, k: usize) -> anyhow::Result<()> {
    if view.level_count() == 0 {
        eprintln!("file has no schema pyramid (build it from data with rdf:type)");
        return Ok(());
    }
    let Some(lvl) = view.level_rollup(k) else {
        anyhow::bail!(
            "level {k} out of range — the schema pyramid has {} level(s) (0..{})",
            view.level_count(),
            view.level_count() - 1
        );
    };
    println!(
        "schema pyramid level {k} — depth {} (round {}), {} class(es):",
        lvl.depth,
        lvl.round,
        lvl.classes.len()
    );
    for (class, count) in &lvl.classes {
        println!("  {count:>10}  {class}");
    }
    // The lateral (non-is-a) relations rolled up to this same level, if any.
    if let Some(links) = view.level_links.get(k) {
        if !links.links.is_empty() {
            println!("  relations at this level ({}):", links.links.len());
            for r in &links.links {
                println!(
                    "  {:>10}  {} --{}-> {}",
                    r.count, r.s_class, r.predicate, r.o_class
                );
            }
        }
    }
    Ok(())
}

/// Ontology-aware coarse graph: relations between `rdf:type` classes with
/// instance counts (the dataset's effective schema).
pub(crate) fn schema(file: &str) -> anyhow::Result<()> {
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

/// Exact per-predicate triple counts, computed from the summary alone (the triple
/// index is never read).
pub(crate) fn predicates(file: &str) -> anyhow::Result<()> {
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
