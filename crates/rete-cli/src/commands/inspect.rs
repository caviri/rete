//! The inspect group: read-only introspection of a `.rete` file — header
//! (`info`), human overview (`stats`), named graphs (`graphs`), content-hash
//! check (`verify`), the pyramid summary (`summary`), per-predicate totals
//! (`predicates`), and the type-level schema (`schema`). Several of these answer
//! from the summary alone, never touching the triple index.

use crate::commands::range_source::{open_local, LocalRangeReader};
use rete_core::{SliceReader, SummaryView, CODEC_ZSTD};

/// Print the raw file header (section offsets, counts, codec), plus the embedded
/// Dataset Card catalog when the file carries one.
pub(crate) fn info(file: &str) -> anyhow::Result<()> {
    // Header + card are small addressable ranges — read exactly those instead
    // of slurping the file (on a 50 GB graph this is two range reads, not 50 GB).
    let reader = LocalRangeReader::open(file)?;
    let head = rete_core::RangeReader::read_at(&reader, 0, rete_core::HEADER_LEN as u64)?;
    let header = rete_core::Header::from_bytes(&head)?;
    println!("{header:#?}");
    if let Some(card) = crate::commands::card::load_card_ranged(&reader)? {
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
    let rete = open_local(file)?;
    let h = rete.header();
    let file_len = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    println!("{file} — {file_len} bytes");
    println!("  default-graph triples : {}", h.quad_count);
    println!("  distinct terms        : {}", h.term_count);
    println!("  named graphs          : {}", rete.graph_names().len());
    println!("  pyramid levels        : {}", h.pyramid_levels);
    if h.has_tile_synopsis() {
        println!("  tile synopsis         : yes (range readers prune tiles by a bound secondary)");
    }
    println!(
        "  compression           : {}",
        if h.block_codec == CODEC_ZSTD {
            "zstd"
        } else {
            "none"
        }
    );

    // Per-predicate totals + community count come from the summary alone — read
    // through a range reader so this stays cheap on a multi-GB file.
    let reader = LocalRangeReader::open(file)?;
    if let Some(view) = SummaryView::open_ranged(&reader)? {
        println!("  communities           : {}", view.community_count());
        let totals = view.predicate_totals();
        println!("  predicates (top {}):", totals.len().min(10));
        for (pred, count) in totals.iter().take(10) {
            println!("    {count:>8}  {pred}");
        }
    }

    // Per-predicate planner stats (query_stats block) — distinct subjects/objects,
    // multiplicities, and the functional / inverse-functional hints the cost-based
    // planner uses. Absent on files built before the block existed.
    let pstats = rete.predicate_stats();
    if !pstats.is_empty() {
        let dict = rete.dictionary();
        let mut sorted: Vec<_> = pstats.iter().collect();
        sorted.sort_by(|a, b| b.count.cmp(&a.count));
        println!(
            "  planner stats (query_stats, {} predicates):",
            pstats.len()
        );
        for s in sorted.iter().take(10) {
            let term = dict
                .predicate_term(s.predicate)
                .unwrap_or_else(|| format!("#{}", s.predicate));
            let f = if s.max_objects_per_subject == 1 {
                " · functional"
            } else {
                ""
            };
            let inv = if s.max_subjects_per_object == 1 {
                " · inverse-functional"
            } else {
                ""
            };
            println!(
                "    {:>8}  {term}  ({} subj, {} obj; ≤{}/subj, ≤{}/obj{f}{inv})",
                s.count,
                s.distinct_subjects,
                s.distinct_objects,
                s.max_objects_per_subject,
                s.max_subjects_per_object,
            );
        }
    }

    // Entity shapes (characteristic sets): the most common predicate-combinations.
    let shapes = rete.char_sets();
    if !shapes.is_empty() {
        let dict = rete.dictionary();
        let local = |iri: &str| -> String {
            iri.trim_end_matches('>')
                .rsplit(['/', '#'])
                .next()
                .unwrap_or(iri)
                .to_string()
        };
        println!(
            "  entity shapes (characteristic sets, top {}):",
            shapes.len().min(8)
        );
        for c in shapes.iter().take(8) {
            let preds: Vec<String> = c
                .predicates
                .iter()
                .map(|&p| {
                    dict.predicate_term(p)
                        .map(|t| local(&t))
                        .unwrap_or_else(|| format!("#{p}"))
                })
                .collect();
            println!("    {:>8} subj  {{{}}}", c.subjects, preds.join(", "));
        }
    }

    // Label index: how many entries are searchable by prefix (`rete search`).
    let labels = rete.label_index();
    if !labels.is_empty() {
        println!(
            "  label index: {} labels (prefix-searchable — `rete search {} <prefix>`)",
            labels.len(),
            file
        );
    }

    // Full-text index (TEXT_INDEX section): word/CONTAINS search over literals.
    if rete.has_text_index() {
        println!(
            "  text index: {} bytes (full-text — `rete search {} --contains <word>`)",
            h.text_index_len, file
        );
    }
    Ok(())
}

/// Prefix-search the label index: print the subjects whose label starts with
/// `prefix` (case-insensitive), as `label  <iri>`. Reads the bounded label-index
/// block in the pyramid-meta — no literal scan. `--json` emits a versioned
/// `{schemaVersion, matches:[{label, subject}]}` envelope.
pub(crate) fn search(file: &str, prefix: &str, limit: usize, json: bool) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let hits = rete.prefix_search(prefix, limit);
    if json {
        let items: Vec<String> = hits
            .iter()
            .map(|(label, iri)| {
                format!(
                    "{{\"label\":{},\"subject\":{}}}",
                    json_str(label),
                    json_str(iri)
                )
            })
            .collect();
        println!(
            "{{\"schemaVersion\":{},\"matches\":[{}]}}",
            crate::JSON_SCHEMA_VERSION,
            items.join(",")
        );
        return Ok(());
    }
    if hits.is_empty() {
        if rete.label_index().is_empty() {
            eprintln!("(this file has no label index — rebuild with a recent `rete build`)");
        } else {
            println!("(no labels match \"{prefix}\")");
        }
        return Ok(());
    }
    for (label, iri) in &hits {
        println!("{label}\t{iri}");
    }
    Ok(())
}

/// Full-text search (`rete search --contains <word>…`): subjects whose literals
/// contain every word (AND, whole-word, case-insensitive), optionally also a word
/// starting with `--contains-prefix`. Answers from the TEXT_INDEX section.
pub(crate) fn search_contains(
    file: &str,
    words: &[String],
    prefix: Option<&str>,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    if !rete.has_text_index() {
        if json {
            println!(
                "{{\"schemaVersion\":{},\"matches\":[]}}",
                crate::JSON_SCHEMA_VERSION
            );
        } else {
            eprintln!("(this file has no text index — rebuild with `rete build --text-index`)");
        }
        return Ok(());
    }
    let word_refs: Vec<&str> = words.iter().map(String::as_str).collect();
    let hits = rete.text_search(&word_refs, prefix, limit);
    if json {
        let items: Vec<String> = hits
            .iter()
            .map(|iri| format!("{{\"subject\":{}}}", json_str(iri)))
            .collect();
        println!(
            "{{\"schemaVersion\":{},\"matches\":[{}]}}",
            crate::JSON_SCHEMA_VERSION,
            items.join(",")
        );
        return Ok(());
    }
    if hits.is_empty() {
        let mut terms: Vec<String> = words.to_vec();
        if let Some(p) = prefix {
            terms.push(format!("{p}*"));
        }
        println!("(no entities contain {})", terms.join(" + "));
        return Ok(());
    }
    for iri in &hits {
        println!("{iri}");
    }
    Ok(())
}

/// Minimal JSON string escaping for the `--json` output.
pub(crate) fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// List the named graphs in a dataset (or note that there are none).
pub(crate) fn graphs(file: &str) -> anyhow::Result<()> {
    let rete = open_local(file)?;
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
    let rete = open_local(file)?;

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
