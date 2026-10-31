//! The inspect group: read-only introspection of a `.rete` file — header
//! (`info`), human overview (`stats`), named graphs (`graphs`), content-hash
//! check (`verify`), the pyramid summary (`summary`), per-predicate totals
//! (`predicates`), and the type-level schema (`schema`). Several of these answer
//! from the summary alone, never touching the triple index.

use crate::commands::range_source::{open_local, LocalRangeReader};
use rete_core::{SummaryView, CODEC_ZSTD};

/// Print the raw file header (section offsets, counts, codec), plus the embedded
/// Dataset Card catalog when the file carries one.
pub(crate) fn info(file: &str) -> anyhow::Result<()> {
    // Header + card are small addressable ranges — read exactly those instead
    // of slurping the file (on a 50 GB graph this is two range reads, not 50 GB).
    let reader = LocalRangeReader::open(file)?;
    let head = rete_core::RangeReader::read_at(&reader, 0, rete_core::HEADER_LEN as u64)?;
    let header = rete_core::Header::from_bytes(&head)?;
    if header.version == rete_core::v06::VERSION {
        print_v06_layout(&reader)?;
    } else {
        println!("{header:#?}");
    }
    // The section directory the header just printed is also where "can I
    // full-text search this?" is decided; fold that into the card the same way
    // `rete card` does, so both surfaces answer it identically.
    let text_index = crate::commands::card::TextIndexSignal::probe(&reader, &header);
    // Same for "can this file merge-join?": it is the header's permutation mask
    // (byte 50, `0` = all six), so it is free here and true of the bytes rather
    // than of anything the card claims.
    let permutations = crate::commands::card::PermutationsSignal::probe(&header);
    println!("permutations: {}", permutations.describe());
    let label_index = crate::commands::card::LabelIndexSignal::probe(&reader, &header);
    if let Some(mut card) = crate::commands::card::load_card_ranged(&reader)? {
        card.observe_text_index(text_index);
        card.observe_label_index(label_index);
        card.observe_permutations(permutations);
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

/// How a file's statements split between the default graph and its named
/// graphs, and where the split came from.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct GraphSplit {
    /// Every statement in the file: the header's `quad_count`.
    pub(crate) statements: u64,
    pub(crate) named_graphs: u64,
    /// Default-graph triples; `None` when they could not be counted cheaply.
    pub(crate) default_triples: Option<u64>,
    pub(crate) source: &'static str,
}

impl GraphSplit {
    pub(crate) fn named_quads(&self) -> Option<u64> {
        self.default_triples
            .map(|d| self.statements.saturating_sub(d))
    }
}

/// Split the header's statement count (default graph + every named graph)
/// into its two parts without reading the named graphs: free when there are
/// none; from the Dataset Card's build-time counts when it carries them and
/// they agree with the header; else by counting the default graph's SPO
/// tiles, which a lazy open faults in (the default graph only).
pub(crate) fn graph_split(
    rete: &rete_core::Rete,
    card: Option<&rete_core::card::DatasetCard>,
) -> GraphSplit {
    let statements = rete.header().quad_count;
    let named_graphs = rete.named_graph_count() as u64;
    let (default_triples, source) = if named_graphs == 0 {
        (Some(statements), "header")
    } else if let Some(c) = card.filter(|c| {
        c.quad_count == statements
            && c.named_graph_count == named_graphs
            && c.triple_count <= statements
    }) {
        (Some(c.triple_count), "dataset card")
    } else if statements <= u64::from(u32::MAX) {
        (
            Some(u64::from(rete.default_index().triple_count())),
            "default-graph index",
        )
    } else {
        (None, "unknown")
    };
    GraphSplit {
        statements,
        named_graphs,
        default_triples,
        source,
    }
}

/// Human-friendly overview: size, counts, graphs, pyramid, top predicates. The
/// per-predicate totals + community count come from the summary alone (the
/// triple index is never read).
pub(crate) fn stats(file: &str) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let h = rete.header();
    let file_len = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    // The card is two small range reads; it carries the build-time split.
    let card = crate::commands::card::load_card_ranged(&LocalRangeReader::open(file)?)
        .ok()
        .flatten();
    let split = graph_split(&rete, card.as_ref());
    let show = |n: Option<u64>| n.map_or_else(|| "unknown".to_string(), |n| n.to_string());
    println!("{file} — {file_len} bytes");
    println!(
        "  statements            : {} (all graphs)",
        split.statements
    );
    println!("  default-graph triples : {}", show(split.default_triples));
    println!("  named graphs          : {}", split.named_graphs);
    if split.named_graphs > 0 {
        println!(
            "  named-graph quads     : {} (split from the {})",
            show(split.named_quads()),
            split.source
        );
    }
    println!("  distinct terms        : {}", h.term_count);
    if h.has_quad_index() {
        println!("  named-graph layout    : quad index (graph-leading permutations, kind 10)");
    }
    println!("  pyramid levels        : {}", h.pyramid_levels);
    if h.has_tile_synopsis() {
        println!("  tile synopsis         : yes (range readers prune tiles by a bound secondary)");
    }
    println!(
        "  index permutations    : {} ({}){}",
        h.perms.len(),
        h.perms.names().join("/"),
        if h.perms.has_merge_orders() {
            ""
        } else {
            " — no sort-merge join"
        }
    );
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

    // Label index: how many entries are searchable by prefix (`rete search`) —
    // the LABEL_INDEX section, else the legacy pyramid-meta block.
    if let Some(li) = rete.label_index_section() {
        println!(
            "  label index: {} labels, {} bytes (LABEL_INDEX — `rete search {} <prefix>`)",
            li.entry_count(),
            li.section_len(),
            file
        );
    }
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
            "  text index: {} bytes (full-text — `rete search {} --contains <word>`){}",
            h.text_index_len,
            file,
            if h.text_index_partial {
                "; some predicates only (section kind 11)"
            } else {
                ""
            }
        );
    }
    Ok(())
}

/// Prefix-search the label index: print the subjects whose label starts with
/// `prefix` (case-insensitive), as `label  <iri>`. Reads the LABEL_INDEX section
/// (or, on an older file, the bounded label block of the pyramid-meta) — no
/// literal scan. `--json` emits a versioned
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
        if !rete.has_label_index() && rete.label_index().is_empty() {
            eprintln!("(this file has no label index — rebuild with `rete build --label-index`)");
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
        // An index of some graphs or predicates finds only their words; say
        // which, so "nothing" is not read as "nowhere in the file".
        if let Some(scope) = rete.text_index_scope().filter(|s| !s.is_default()) {
            eprintln!("(the text index covers: {})", scope.describe());
        }
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
///
/// Streams the hashed sections through a positional reader, so memory stays at
/// one read slice whatever the file size (it used to read the whole file).
pub(crate) fn verify_cmd(file: &str, section: Option<&str>) -> anyhow::Result<()> {
    let reader = LocalRangeReader::open(file)?;
    let head = rete_core::RangeReader::read_at(
        &reader,
        0,
        (rete_core::HEADER_LEN as u64).min(rete_core::RangeReader::len(&reader)),
    )?;
    if rete_core::v06::open::is_v06(&head) {
        let only = match section {
            None => None,
            Some(name) => Some(
                rete_core::v06::kind::ORDER
                    .iter()
                    .copied()
                    .find(|&k| rete_core::v06::kind::name(k) == name)
                    .ok_or_else(|| anyhow::anyhow!("unknown section {name:?}"))?,
            ),
        };
        let bad = rete_core::file::verify_ranged_v06(&reader, &head, only)?;
        if bad.is_empty() {
            println!(
                "OK — {} (format 0x06)",
                match section {
                    Some(s) => format!("section {s} matches its digest"),
                    None => "content hash matches, and every section digest".to_string(),
                }
            );
            return Ok(());
        }
        anyhow::bail!(
            "FAILED — content hash mismatch: the digest of {} does not match (file corrupted \
             or truncated)",
            bad.join(", ")
        );
    }
    if section.is_some() {
        anyhow::bail!("--section needs a format 0x06 file");
    }
    if rete_core::verify_ranged(&reader)? {
        println!("OK — content hash matches");
        Ok(())
    } else {
        anyhow::bail!(
            "FAILED — content hash mismatch, or a section digest mismatch (file corrupted or truncated)"
        );
    }
}

/// Print the pyramid summary — the community super-edge graph, and (for v2 files)
/// the **schema pyramid**: a leveled type histogram (abstract classes at coarse
/// levels, leaves as you zoom in). All read **index-free** from the pyramid-meta
/// via [`SummaryView`]. With `--level k`, print just level `k`'s type histogram.
pub(crate) fn summary(file: &str, level: Option<usize>) -> anyhow::Result<()> {
    // Positional reads of the sections the view needs, not the whole file.
    let reader = LocalRangeReader::open(file)?;
    let Some(view) = SummaryView::open_ranged(&reader)? else {
        // No pyramid: the schema may still have its own section (kind 9).
        match SummaryView::open_schema_ranged(&reader)? {
            Some(view) => {
                if let Some(k) = level {
                    return render_schema_level(&view, k);
                }
                println!("file has no pyramid; its SCHEMA section:");
                render_schema_overview(&view);
            }
            None => eprintln!("file has no pyramid"),
        }
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
    let reader = LocalRangeReader::open(file)?;
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

/// `rete info` for a format 0x06 file: the fixed header, then the root's
/// section table — where every part of the file is, and its digest.
fn print_v06_layout(reader: &LocalRangeReader) -> anyhow::Result<()> {
    use rete_core::v06::{kind, open};
    use rete_core::v06::{F_NAMED, F_QUAD_GRAPH_MAJOR, F_QUOTED, F_TEXT_NAMED};
    let (h, root) = open::read_front(reader)?;
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    // The header's fields by name, as for a 0x05 file (`quad_count: …`).
    println!("format version: 0x06");
    println!("{h:#?}");
    // The 0x05 key scripts read the record's presence from (tests/gate/fixtures.sh).
    println!("build_info_len: {}", h.build_len);
    let flags: Vec<&str> = [
        (F_NAMED, "named graphs"),
        (F_QUOTED, "quoted triples"),
        (F_TEXT_NAMED, "text covers named graphs"),
        (F_QUAD_GRAPH_MAJOR, "quad tiles graph-major"),
    ]
    .into_iter()
    .filter(|&(b, _)| h.flags & b != 0)
    .map(|(_, n)| n)
    .collect();
    println!(
        "format 0x06 — {} bytes, content hash {}",
        h.file_len,
        hex(&h.content_hash)
    );
    println!(
        "  flags        : {}",
        if flags.is_empty() {
            "none".to_string()
        } else {
            flags.join(", ")
        }
    );
    println!(
        "  front        : root {} B at 128, card region {} B (card {} B, record {} B), data at {}",
        h.root_len, h.card_region_len, h.card_len, h.build_len, h.data_offset
    );
    println!("  sections     :");
    for s in &root.sections {
        let a = s.abs(&h);
        println!(
            "    {:<13} {:>14} + {:<12} digest {}",
            kind::name(s.kind),
            a.offset,
            a.len,
            hex(&s.digest)
        );
    }
    Ok(())
}
