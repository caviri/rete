//! Byte-cost preview for SPARQL over a `.rete` source. This does not evaluate
//! the query; it opens the source through the same range-reader paths used by
//! `summary-url` and `sparql-url` and reports the observed request/byte budget.

use rete_core::{
    query_predicates, routed_triple_pattern, summary_query_shape, CountingReader, Header,
    RangeReader, Rete, RoutedTriplePattern, SummaryQueryShape, SummaryView, HEADER_LEN,
};

use crate::commands::range_source::{is_url, RangedSourceReader};

const HEADER_LEN_U64: u64 = HEADER_LEN as u64;

#[derive(Clone, Copy)]
struct AccessCost {
    available: bool,
    bytes: u64,
    requests: u64,
    reads_index: bool,
}

/// Preview the range-read cost for a SPARQL query over a local file or HTTP(S)
/// `.rete` URL. The current SPARQL engine opens the full query view before
/// evaluation, so this reports that path plus the cheaper summary-only overview
/// path that progressive clients can use for routing/overview.
pub(crate) fn cost(source: &str, query: &str, json: bool, explain: bool) -> anyhow::Result<()> {
    let query_preds =
        query_predicates(query).map_err(|e| anyhow::anyhow!("SPARQL parse error: {e}"))?;
    let summary_shape =
        summary_query_shape(query).map_err(|e| anyhow::anyhow!("SPARQL parse error: {e}"))?;
    let routed_shape =
        routed_triple_pattern(query).map_err(|e| anyhow::anyhow!("SPARQL parse error: {e}"))?;
    let header = read_header(source)?;
    let source_kind = if is_url(source) { "url" } else { "local" };
    let summary = measure_summary(source)?;
    let routed = measure_routed_pattern(source, routed_shape.as_ref())?;
    let full = measure_full(source)?;
    let lazy = measure_lazy_open(source)?;
    // Tiled (v0.2) files evaluate SPARQL-over-URL with lazy tile faulting: the
    // open budget below plus one range request per index tile the query's
    // scans actually touch. Pre-tiling files fetch the index whole.
    let engine_access = if header.version >= 2 {
        "lazy-tiles"
    } else {
        "full-index"
    };
    let summary_answer = summary_answer_json(summary.view.as_ref(), summary_shape.as_ref());
    let explain_plan = explain_json(summary_shape.as_ref(), &summary_answer, &routed);

    if json {
        let mut body = serde_json::json!({
            "schemaVersion": crate::JSON_SCHEMA_VERSION,
            "source": source,
            "source_kind": source_kind,
            "file_bytes": full.file_bytes,
            "current_engine_access": engine_access,
            "query_predicates": query_preds,
            "summary_answer": summary_answer,
            "summary_overview": {
                "available": summary.access.available,
                "bytes": summary.access.bytes,
                "requests": summary.access.requests,
                "reads_index": summary.access.reads_index,
            },
            "routed_pattern_open": routed_json(&routed),
            "lazy_query_open": {
                "available": lazy.available,
                "bytes": lazy.bytes,
                "requests": lazy.requests,
                "reads_index": lazy.reads_index,
                "note": "tile directories only; index tiles fault in per scan",
            },
            "full_query_open": {
                "available": full.available,
                "bytes": full.bytes,
                "requests": full.requests,
                "reads_index": full.reads_index,
            },
            "sections": section_json(&header),
        });
        if explain {
            body.as_object_mut()
                .expect("cost JSON root is an object")
                .insert("explain".into(), explain_plan);
        }
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        println!("query cost preview");
        println!("  source: {source}");
        println!("  source kind: {source_kind}");
        println!("  file bytes: {}", full.file_bytes);
        println!("  current engine access: {engine_access}");
        if query_preds.is_empty() {
            println!("  query predicates: (none pinned; predicate routing cannot prune)");
        } else {
            println!(
                "  query predicates: {}",
                query_preds.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
        print_summary_answer(&summary_answer);
        if explain {
            print_explain(&explain_plan);
        }
        print_access("summary overview", summary.access);
        print_routed_pattern(&routed);
        print_access("lazy query open", lazy);
        print_access(
            "full query open",
            AccessCost {
                available: full.available,
                bytes: full.bytes,
                requests: full.requests,
                reads_index: full.reads_index,
            },
        );
        println!(
            "  index section: offset {} · len {}",
            header.root_dir_offset, header.root_dir_len
        );
        if header.version >= 2 {
            println!(
                "  note: SPARQL evaluation opens lazily (tile directories only) and \
                 range-fetches index tiles as the query touches them; the full \
                 query-open figure is the eager upper bound."
            );
        } else {
            println!(
                "  note: pre-tiling (v0.1) file — SPARQL evaluation uses the full \
                 query-open path; the summary path is the cheaper overview budget."
            );
        }
    }
    Ok(())
}

fn print_access(label: &str, cost: AccessCost) {
    let available = if cost.available {
        ""
    } else {
        " (summary unavailable)"
    };
    let index = if cost.reads_index {
        "reads index"
    } else {
        "skips index"
    };
    println!(
        "  {label}: {} bytes in {} range request(s) · {index}{available}",
        cost.bytes, cost.requests
    );
}

struct FullCost {
    available: bool,
    file_bytes: u64,
    bytes: u64,
    requests: u64,
    reads_index: bool,
}

struct SummaryCost {
    access: AccessCost,
    view: Option<SummaryView>,
}

struct RoutedPatternCost {
    access: AccessCost,
    pattern: Option<RoutedTriplePattern>,
    reason: Option<&'static str>,
}

fn measure_summary(source: &str) -> anyhow::Result<SummaryCost> {
    let reader = CountingReader::new(RangedSourceReader::open(source)?);
    let view = SummaryView::open_ranged(&reader)?;
    Ok(SummaryCost {
        access: AccessCost {
            available: view.is_some(),
            bytes: reader.bytes_read(),
            requests: reader.requests(),
            reads_index: false,
        },
        view,
    })
}

fn measure_full(source: &str) -> anyhow::Result<FullCost> {
    let reader = CountingReader::new(RangedSourceReader::open(source)?);
    let file_bytes = reader.len();
    let _rete = Rete::open_ranged(&reader)?;
    Ok(FullCost {
        available: true,
        file_bytes,
        bytes: reader.bytes_read(),
        requests: reader.requests(),
        reads_index: true,
    })
}

/// The lazy SPARQL open budget (what `sparql-url` actually pays up front on a
/// tiled file): header + dictionary + pyramid + tile directories, no tiles.
fn measure_lazy_open(source: &str) -> anyhow::Result<AccessCost> {
    let reader = std::sync::Arc::new(CountingReader::new(RangedSourceReader::open(source)?));
    let _rete = Rete::open_ranged_lazy(reader.clone())?;
    Ok(AccessCost {
        available: true,
        bytes: reader.bytes_read(),
        requests: reader.requests(),
        reads_index: true, // the (small) tile directories live in the index region
    })
}

fn measure_routed_pattern(
    source: &str,
    pattern: Option<&RoutedTriplePattern>,
) -> anyhow::Result<RoutedPatternCost> {
    let Some(pattern) = pattern else {
        return Ok(RoutedPatternCost {
            access: AccessCost {
                available: false,
                bytes: 0,
                requests: 0,
                reads_index: false,
            },
            pattern: None,
            reason: Some("query is not a single default-graph triple pattern"),
        });
    };

    let reader = CountingReader::new(RangedSourceReader::open(source)?);
    let reads_index = Rete::route_pattern_ranged(
        &reader,
        pattern.subject.as_deref(),
        pattern.predicate.as_deref(),
        pattern.object.as_deref(),
    )?;
    Ok(RoutedPatternCost {
        access: AccessCost {
            available: true,
            bytes: reader.bytes_read(),
            requests: reader.requests(),
            reads_index,
        },
        pattern: Some(pattern.clone()),
        reason: None,
    })
}

fn read_header(source: &str) -> anyhow::Result<Header> {
    let reader = RangedSourceReader::open(source)?;
    let head = reader.read_at(0, HEADER_LEN_U64)?;
    Ok(Header::from_bytes(&head)?)
}

fn section_json(header: &Header) -> serde_json::Value {
    serde_json::json!({
        "header": { "offset": 0, "len": HEADER_LEN_U64 },
        "metadata": { "offset": header.metadata_offset, "len": header.metadata_len },
        "dictionary": { "offset": header.dictionary_offset, "len": header.dictionary_len },
        "index": { "offset": header.root_dir_offset, "len": header.root_dir_len },
        "pyramid_meta": { "offset": header.pyramid_meta_offset, "len": header.pyramid_meta_len },
        "named_graphs": { "offset": header.named_graphs_offset, "len": header.named_graphs_len },
    })
}

fn summary_answer_json(
    view: Option<&SummaryView>,
    shape: Option<&SummaryQueryShape>,
) -> serde_json::Value {
    match (view, shape) {
        (Some(view), Some(SummaryQueryShape::PredicateCount { predicate, .. })) => {
            serde_json::json!({
                "available": true,
                "kind": "predicate_count",
                "predicate": predicate,
                "value": view.predicate_total(predicate),
                "reads_index": false,
            })
        }
        (Some(view), Some(SummaryQueryShape::TripleCount { .. })) => {
            serde_json::json!({
                "available": true,
                "kind": "triple_count",
                "value": summary_total(view),
                "reads_index": false,
            })
        }
        (Some(view), Some(SummaryQueryShape::PredicateTotals { .. })) => {
            serde_json::json!({
                "available": true,
                "kind": "predicate_totals",
                "value": view.predicate_totals(),
                "reads_index": false,
            })
        }
        (Some(view), Some(SummaryQueryShape::PredicateList { .. })) => {
            serde_json::json!({
                "available": true,
                "kind": "predicate_list",
                "value": predicate_list(view),
                "reads_index": false,
            })
        }
        (Some(view), Some(SummaryQueryShape::PredicateDistinctCount { .. })) => {
            serde_json::json!({
                "available": true,
                "kind": "predicate_distinct_count",
                "value": predicate_count(view),
                "reads_index": false,
            })
        }
        (Some(view), Some(SummaryQueryShape::TripleExists)) => {
            serde_json::json!({
                "available": true,
                "kind": "triple_exists",
                "value": summary_total(view) > 0,
                "reads_index": false,
            })
        }
        (Some(view), Some(SummaryQueryShape::PredicateExists { predicate })) => {
            serde_json::json!({
                "available": true,
                "kind": "predicate_exists",
                "predicate": predicate,
                "value": view.predicate_total(predicate) > 0,
                "reads_index": false,
            })
        }
        (Some(_), Some(_)) => {
            serde_json::json!({
                "available": false,
                "kind": "unknown_summary_shape",
                "reason": "summary query shape is not supported by this CLI build",
                "reads_index": false,
            })
        }
        (None, Some(shape)) => {
            serde_json::json!({
                "available": false,
                "kind": shape_kind(shape),
                "reason": "file has no pyramid summary",
                "reads_index": false,
            })
        }
        (_, None) => {
            serde_json::json!({
                "available": false,
                "kind": "requires_index",
                "reason": "query shape is not exactly answerable from summary predicate totals",
                "reads_index": false,
            })
        }
    }
}

fn routed_json(cost: &RoutedPatternCost) -> serde_json::Value {
    if cost.access.available {
        let pattern = cost.pattern.as_ref().expect("available routed pattern");
        serde_json::json!({
            "available": true,
            "bytes": cost.access.bytes,
            "requests": cost.access.requests,
            "reads_index": cost.access.reads_index,
            "index_access": "single-permutation",
            "pattern": {
                "subject": pattern.subject,
                "predicate": pattern.predicate,
                "object": pattern.object,
            },
        })
    } else {
        serde_json::json!({
            "available": false,
            "bytes": 0,
            "requests": 0,
            "reads_index": false,
            "reason": cost.reason.unwrap_or("not routable"),
        })
    }
}

fn print_routed_pattern(cost: &RoutedPatternCost) {
    if cost.access.available {
        print_access("routed pattern open", cost.access);
    } else {
        println!(
            "  routed pattern open: unavailable ({})",
            cost.reason.unwrap_or("not routable")
        );
    }
}

fn explain_json(
    shape: Option<&SummaryQueryShape>,
    summary_answer: &serde_json::Value,
    routed: &RoutedPatternCost,
) -> serde_json::Value {
    let summary_exact = summary_answer["available"].as_bool() == Some(true);
    let query_shape = shape.map(shape_kind).unwrap_or("requires_index");
    let planned_access = if summary_exact {
        "summary-only"
    } else if routed.access.available {
        "routed-pattern"
    } else {
        "full-index"
    };
    let reason = match (shape, summary_exact) {
        (Some(SummaryQueryShape::PredicateCount { .. }), true) => {
            "COUNT(*) over one unbound predicate triple pattern is exact from summary predicate totals"
        }
        (Some(SummaryQueryShape::TripleCount { .. }), true) => {
            "COUNT(*) over one fully unbound triple pattern is exact from summary totals"
        }
        (Some(SummaryQueryShape::PredicateTotals { .. }), true) => {
            "GROUP BY predicate COUNT(*) over one fully unbound triple pattern is exact from summary predicate totals"
        }
        (Some(SummaryQueryShape::PredicateList { .. }), true) => {
            "DISTINCT predicate projection over one fully unbound triple pattern is exact from summary predicate totals"
        }
        (Some(SummaryQueryShape::PredicateDistinctCount { .. }), true) => {
            "COUNT(DISTINCT ?p) over one fully unbound triple pattern is exact from summary predicate totals"
        }
        (Some(SummaryQueryShape::TripleExists), true) => {
            "ASK over one fully unbound triple pattern is exact from summary totals"
        }
        (Some(SummaryQueryShape::PredicateExists { .. }), true) => {
            "ASK over one unbound predicate triple pattern is exact from summary predicate totals"
        }
        (_, false) => summary_answer["reason"]
            .as_str()
            .unwrap_or("query requires the full index"),
        _ => "query requires the full index",
    };
    let reason = if !summary_exact && routed.access.available {
        "single default-graph triple pattern can fetch one selected permutation section instead of the full index container"
    } else {
        reason
    };

    serde_json::json!({
        "query_shape": query_shape,
        "summary_exact": summary_exact,
        "planned_access": planned_access,
        "current_engine_access": "full-index",
        "current_engine_reads_index": true,
        "reason": reason,
    })
}

fn print_summary_answer(answer: &serde_json::Value) {
    if answer["available"].as_bool() == Some(true) {
        let kind = answer["kind"].as_str().unwrap_or("summary");
        let predicate = answer["predicate"].as_str().unwrap_or("");
        let value = &answer["value"];
        println!("  summary exact answer: {kind} {predicate} = {value}");
    } else {
        let reason = answer["reason"].as_str().unwrap_or("requires index");
        println!("  summary exact answer: unavailable ({reason})");
    }
}

fn print_explain(explain: &serde_json::Value) {
    println!("  explain:");
    println!(
        "    query shape: {}",
        explain["query_shape"].as_str().unwrap_or("unknown")
    );
    println!(
        "    summary exact: {}",
        explain["summary_exact"].as_bool().unwrap_or(false)
    );
    println!(
        "    planned access: {}",
        explain["planned_access"].as_str().unwrap_or("full-index")
    );
    println!(
        "    current engine reads index: {}",
        explain["current_engine_reads_index"]
            .as_bool()
            .unwrap_or(true)
    );
    println!(
        "    reason: {}",
        explain["reason"]
            .as_str()
            .unwrap_or("query requires the full index")
    );
}

fn shape_kind(shape: &SummaryQueryShape) -> &'static str {
    match shape {
        SummaryQueryShape::PredicateCount { .. } => "predicate_count",
        SummaryQueryShape::TripleCount { .. } => "triple_count",
        SummaryQueryShape::PredicateTotals { .. } => "predicate_totals",
        SummaryQueryShape::PredicateList { .. } => "predicate_list",
        SummaryQueryShape::PredicateDistinctCount { .. } => "predicate_distinct_count",
        SummaryQueryShape::TripleExists => "triple_exists",
        SummaryQueryShape::PredicateExists { .. } => "predicate_exists",
        _ => "unknown_summary_shape",
    }
}

fn summary_total(view: &SummaryView) -> u32 {
    view.summary.iter().map(|edge| edge.count).sum()
}

fn predicate_list(view: &SummaryView) -> Vec<String> {
    view.predicate_totals()
        .into_iter()
        .map(|(predicate, _)| predicate)
        .collect()
}

fn predicate_count(view: &SummaryView) -> usize {
    view.predicate_totals().len()
}
