//! SHACL Core validation command.

use rete_core::{
    validate_shacl, BlockCacheReader, CountingReader, DataGraph, RangeReader, Rete, ReteGraph,
    ShaclShapes, ValidationReport, DEFAULT_BLOCK,
};

use crate::http::HttpRangeReader;
use crate::commands::range_source::open_local;

pub(crate) fn shacl_cmd(
    file: &str,
    shapes_file: &str,
    graph: Option<&str>,
    format: &str,
) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let data = DataGraph::from_rete(&rete, graph);
    let shapes_text = std::fs::read_to_string(shapes_file)?;
    let shapes = ShaclShapes::parse_turtle(&shapes_text)?;
    let report = validate_shacl(&data, &shapes);
    emit(&report, format);
    fail_if_nonconforming(&report)
}

/// Validate a **remote** `.rete` over HTTP, range-reading only what the shapes
/// target: the file is opened lazily and each focus node's values are fetched as
/// routed range reads, so a targeted shape (`sh:targetClass` / `targetNode` /
/// `targetSubjectsOf` / `targetObjectsOf`) never downloads the whole graph.
/// Validates the **default** graph.
pub(crate) fn shacl_url(url: &str, shapes_file: &str, format: &str) -> anyhow::Result<()> {
    // `reader` counts the PHYSICAL HTTP fetches; a read-through block cache above
    // it coalesces SHACL's many small, overlapping range reads (shared dictionary
    // chunks, the type tiles) into a few aligned block fetches. `RETE_BLOCK_KB=0`
    // disables it (one fetch per logical read).
    let reader = std::sync::Arc::new(CountingReader::new(HttpRangeReader::open(url)?));
    let total = reader.len();
    let block_kb: u64 = std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_BLOCK / 1024);
    let rete = if block_kb == 0 {
        Rete::open_ranged_lazy(reader.clone())?
    } else {
        Rete::open_ranged_lazy(std::sync::Arc::new(BlockCacheReader::new(
            reader.clone(),
            block_kb * 1024,
        )))?
    };
    let shapes_text = std::fs::read_to_string(shapes_file)?;
    let shapes = ShaclShapes::parse_turtle(&shapes_text)?;
    let report = validate_shacl(&ReteGraph::new(&rete), &shapes);
    // A failed lazy fetch means a missing focus node or value — the report could
    // wrongly conform. Surface it as an error, never a quiet partial result.
    if rete.index_incomplete() {
        anyhow::bail!(
            "a range request failed while validating {url}; the report could be \
             incomplete — retry"
        );
    }
    emit(&report, format);
    eprintln!(
        "(fetched {} bytes in {} range request(s); file is {} bytes)",
        reader.bytes_read(),
        reader.requests(),
        total
    );
    fail_if_nonconforming(&report)
}

fn emit(report: &ValidationReport, format: &str) {
    match format {
        "json" => println!("{}", report.to_json()),
        "ttl" => print!("{}", report.to_turtle()),
        _ => print_text(report),
    }
}

fn fail_if_nonconforming(report: &ValidationReport) -> anyhow::Result<()> {
    if !report.conforms {
        return Err(crate::NonConformance::new(format!(
            "SHACL validation failed with {} result(s)",
            report.results.len()
        ))
        .into());
    }
    Ok(())
}

fn print_text(report: &ValidationReport) {
    if report.conforms {
        println!("conforms: true");
        return;
    }
    println!("conforms: false ({} result(s))", report.results.len());
    for r in &report.results {
        let path = r
            .result_path
            .as_deref()
            .map(|p| format!(" path={p}"))
            .unwrap_or_default();
        let value = r
            .value_node
            .as_deref()
            .map(|v| format!(" value={v}"))
            .unwrap_or_default();
        println!(
            "  [{}] focus={}{}{} component={}",
            r.severity.iri(),
            r.focus_node,
            path,
            value,
            r.source_constraint_component
        );
        for msg in &r.messages {
            println!("      message: {msg}");
        }
    }
}
