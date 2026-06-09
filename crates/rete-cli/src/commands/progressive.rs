//! First progressive-query surface: answer exact summary-safe SPARQL shapes from
//! the pyramid summary without opening the triple index.

use rete_core::{
    summary_query_shape, Binding, CountingReader, QueryOutput, RangeReader, SummaryQueryShape,
    SummaryView,
};

use crate::commands::range_source::RangedSourceReader;
use crate::commands::render::{print_query_output, query_output_json};

const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

pub(crate) fn progressive(source: &str, query: &str, json: bool) -> anyhow::Result<()> {
    let shape =
        summary_query_shape(query).map_err(|e| anyhow::anyhow!("SPARQL parse error: {e}"))?;
    let Some(shape) = shape else {
        anyhow::bail!(
            "query is not exactly answerable from the summary; use `rete cost --explain` to inspect it or `rete sparql` for full-index evaluation"
        );
    };

    let reader = CountingReader::new(RangedSourceReader::open(source)?);
    let total = reader.len();
    let Some(view) = SummaryView::open_ranged(&reader)? else {
        anyhow::bail!("file has no pyramid summary");
    };
    let result = summary_result(&view, &shape);

    if json {
        let mut body = query_output_json(&result);
        body.as_object_mut()
            .expect("SPARQL result JSON root is an object")
            .insert(
                "progressive".into(),
                progressive_meta(&shape, &view, &reader, total),
            );
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        print_query_output(&result, false);
        eprintln!(
            "(summary exact; fetched {} of {} bytes in {} range request(s); index NOT fetched)",
            reader.bytes_read(),
            total,
            reader.requests()
        );
    }

    Ok(())
}

fn summary_result(view: &SummaryView, shape: &SummaryQueryShape) -> QueryOutput {
    match shape {
        SummaryQueryShape::PredicateCount {
            predicate,
            variable,
        } => {
            let mut binding = Binding::new();
            binding.insert(
                variable.clone(),
                format!("\"{}\"^^<{}>", view.predicate_total(predicate), XSD_INTEGER),
            );
            QueryOutput::Select(vec![variable.clone()], vec![binding])
        }
        SummaryQueryShape::TripleCount { variable } => {
            let mut binding = Binding::new();
            binding.insert(
                variable.clone(),
                format!("\"{}\"^^<{}>", summary_total(view), XSD_INTEGER),
            );
            QueryOutput::Select(vec![variable.clone()], vec![binding])
        }
        SummaryQueryShape::PredicateTotals {
            predicate_variable,
            count_variable,
        } => {
            let rows = view
                .predicate_totals()
                .into_iter()
                .map(|(predicate, count)| {
                    let mut binding = Binding::new();
                    binding.insert(predicate_variable.clone(), predicate);
                    binding.insert(
                        count_variable.clone(),
                        format!("\"{}\"^^<{}>", count, XSD_INTEGER),
                    );
                    binding
                })
                .collect();
            QueryOutput::Select(
                vec![predicate_variable.clone(), count_variable.clone()],
                rows,
            )
        }
        SummaryQueryShape::PredicateList { variable } => {
            let rows = view
                .predicate_totals()
                .into_iter()
                .map(|(predicate, _)| {
                    let mut binding = Binding::new();
                    binding.insert(variable.clone(), predicate);
                    binding
                })
                .collect();
            QueryOutput::Select(vec![variable.clone()], rows)
        }
        SummaryQueryShape::PredicateDistinctCount { variable } => {
            let mut binding = Binding::new();
            binding.insert(
                variable.clone(),
                format!("\"{}\"^^<{}>", predicate_count(view), XSD_INTEGER),
            );
            QueryOutput::Select(vec![variable.clone()], vec![binding])
        }
        SummaryQueryShape::TripleExists => QueryOutput::Ask(summary_total(view) > 0),
        SummaryQueryShape::PredicateExists { predicate } => {
            QueryOutput::Ask(view.predicate_total(predicate) > 0)
        }
    }
}

fn progressive_meta(
    shape: &SummaryQueryShape,
    view: &SummaryView,
    reader: &CountingReader<RangedSourceReader>,
    file_bytes: u64,
) -> serde_json::Value {
    match shape {
        SummaryQueryShape::PredicateCount { predicate, .. } => serde_json::json!({
            "stage": "summary",
            "exact": true,
            "reads_index": false,
            "query_shape": "predicate_count",
            "predicate": predicate,
            "value": view.predicate_total(predicate),
            "bytes": reader.bytes_read(),
            "requests": reader.requests(),
            "file_bytes": file_bytes,
        }),
        SummaryQueryShape::TripleCount { .. } => serde_json::json!({
            "stage": "summary",
            "exact": true,
            "reads_index": false,
            "query_shape": "triple_count",
            "value": summary_total(view),
            "bytes": reader.bytes_read(),
            "requests": reader.requests(),
            "file_bytes": file_bytes,
        }),
        SummaryQueryShape::PredicateTotals { .. } => serde_json::json!({
            "stage": "summary",
            "exact": true,
            "reads_index": false,
            "query_shape": "predicate_totals",
            "value": view.predicate_totals(),
            "bytes": reader.bytes_read(),
            "requests": reader.requests(),
            "file_bytes": file_bytes,
        }),
        SummaryQueryShape::PredicateList { .. } => serde_json::json!({
            "stage": "summary",
            "exact": true,
            "reads_index": false,
            "query_shape": "predicate_list",
            "value": predicate_list(view),
            "bytes": reader.bytes_read(),
            "requests": reader.requests(),
            "file_bytes": file_bytes,
        }),
        SummaryQueryShape::PredicateDistinctCount { .. } => serde_json::json!({
            "stage": "summary",
            "exact": true,
            "reads_index": false,
            "query_shape": "predicate_distinct_count",
            "value": predicate_count(view),
            "bytes": reader.bytes_read(),
            "requests": reader.requests(),
            "file_bytes": file_bytes,
        }),
        SummaryQueryShape::TripleExists => serde_json::json!({
            "stage": "summary",
            "exact": true,
            "reads_index": false,
            "query_shape": "triple_exists",
            "value": summary_total(view) > 0,
            "bytes": reader.bytes_read(),
            "requests": reader.requests(),
            "file_bytes": file_bytes,
        }),
        SummaryQueryShape::PredicateExists { predicate } => serde_json::json!({
            "stage": "summary",
            "exact": true,
            "reads_index": false,
            "query_shape": "predicate_exists",
            "predicate": predicate,
            "value": view.predicate_total(predicate) > 0,
            "bytes": reader.bytes_read(),
            "requests": reader.requests(),
            "file_bytes": file_bytes,
        }),
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
