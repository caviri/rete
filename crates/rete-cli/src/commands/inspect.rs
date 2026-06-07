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

/// Print the pyramid summary graph (community-to-community super-edges).
pub(crate) fn summary(file: &str) -> anyhow::Result<()> {
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

/// Count the distinct communities referenced by the summary super-edges. Tiles
/// are no longer materialized (dropped to shrink the file), so there is no tile
/// list to count — the super-edge endpoints are the source of truth.
fn community_count(pyr: &rete_core::PyramidMeta) -> usize {
    let mut comms = std::collections::HashSet::new();
    for e in &pyr.summary {
        comms.insert(e.s_comm);
        comms.insert(e.o_comm);
    }
    comms.len()
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
