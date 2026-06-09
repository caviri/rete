//! SHACL Core validation command.

use rete_core::{validate_shacl, DataGraph, Rete, ShaclShapes, ValidationReport};

pub(crate) fn shacl_cmd(
    file: &str,
    shapes_file: &str,
    graph: Option<&str>,
    format: &str,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let data = DataGraph::from_rete(&rete, graph);
    let shapes_text = std::fs::read_to_string(shapes_file)?;
    let shapes = ShaclShapes::parse_turtle(&shapes_text)?;
    let report = validate_shacl(&data, &shapes);

    match format {
        "json" => println!("{}", report.to_json()),
        "ttl" => print!("{}", report.to_turtle()),
        _ => print_text(&report),
    }

    if !report.conforms {
        anyhow::bail!(
            "SHACL validation failed with {} result(s)",
            report.results.len()
        );
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
