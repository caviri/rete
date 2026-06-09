use std::process::Command;

fn temp_path(name: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rete_cost_cli_{}_{}.{}",
        std::process::id(),
        name,
        ext
    ))
}

#[test]
fn cost_cli_reports_summary_and_full_open_byte_preview_as_json() {
    let bin = env!("CARGO_BIN_EXE_rete");
    let data = temp_path("data", "nt");
    let file = temp_path("data", "rete");

    std::fs::write(
        &data,
        r#"<http://ex/Alice> <http://ex/knows> <http://ex/Bob> .
<http://ex/Bob> <http://ex/knows> <http://ex/Alice> .
<http://ex/Alice> <http://ex/age> "30" .
"#,
    )
    .unwrap();

    let build = Command::new(bin)
        .args([
            "build",
            data.to_str().unwrap(),
            "-o",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let query = "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }";
    let cost = Command::new(bin)
        .args(["cost", file.to_str().unwrap(), query, "--json"])
        .output()
        .unwrap();
    assert!(
        cost.status.success(),
        "{}",
        String::from_utf8_lossy(&cost.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&cost.stdout).unwrap();
    assert_eq!(json["source_kind"], "local");
    assert_eq!(json["current_engine_access"], "full-index");
    assert_eq!(
        json["query_predicates"].as_array().unwrap(),
        &[serde_json::json!("<http://ex/knows>")]
    );
    assert_eq!(json["summary_overview"]["reads_index"], false);
    assert_eq!(json["full_query_open"]["reads_index"], true);
    assert_eq!(json["routed_pattern_open"]["available"], true);
    assert_eq!(
        json["routed_pattern_open"]["index_access"],
        "single-permutation"
    );
    assert_eq!(json["routed_pattern_open"]["reads_index"], true);
    assert!(
        json["routed_pattern_open"]["bytes"].as_u64().unwrap()
            < json["full_query_open"]["bytes"].as_u64().unwrap()
    );
    assert!(json["summary_overview"]["requests"].as_u64().unwrap() >= 1);
    assert!(
        json["full_query_open"]["bytes"].as_u64().unwrap()
            >= json["summary_overview"]["bytes"].as_u64().unwrap()
    );
    assert!(
        json["file_bytes"].as_u64().unwrap() >= json["full_query_open"]["bytes"].as_u64().unwrap()
    );

    let count_query = "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }";
    let count_cost = Command::new(bin)
        .args(["cost", file.to_str().unwrap(), count_query, "--json"])
        .output()
        .unwrap();
    assert!(
        count_cost.status.success(),
        "{}",
        String::from_utf8_lossy(&count_cost.stderr)
    );
    let count_json: serde_json::Value = serde_json::from_slice(&count_cost.stdout).unwrap();
    assert_eq!(count_json["summary_answer"]["available"], true);
    assert_eq!(count_json["summary_answer"]["kind"], "predicate_count");
    assert_eq!(
        count_json["summary_answer"]["predicate"],
        "<http://ex/knows>"
    );
    assert_eq!(count_json["summary_answer"]["value"], 2);
    assert_eq!(count_json["summary_answer"]["reads_index"], false);

    let explained = Command::new(bin)
        .args([
            "cost",
            file.to_str().unwrap(),
            count_query,
            "--json",
            "--explain",
        ])
        .output()
        .unwrap();
    assert!(
        explained.status.success(),
        "{}",
        String::from_utf8_lossy(&explained.stderr)
    );
    let explained_json: serde_json::Value = serde_json::from_slice(&explained.stdout).unwrap();
    assert_eq!(explained_json["explain"]["query_shape"], "predicate_count");
    assert_eq!(explained_json["explain"]["summary_exact"], true);
    assert_eq!(explained_json["explain"]["planned_access"], "summary-only");
    assert_eq!(
        explained_json["explain"]["current_engine_reads_index"],
        true
    );

    let totals_query = "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p";
    let totals_cost = Command::new(bin)
        .args([
            "cost",
            file.to_str().unwrap(),
            totals_query,
            "--json",
            "--explain",
        ])
        .output()
        .unwrap();
    assert!(
        totals_cost.status.success(),
        "{}",
        String::from_utf8_lossy(&totals_cost.stderr)
    );
    let totals_json: serde_json::Value = serde_json::from_slice(&totals_cost.stdout).unwrap();
    assert_eq!(totals_json["summary_answer"]["available"], true);
    assert_eq!(totals_json["summary_answer"]["kind"], "predicate_totals");
    assert_eq!(
        totals_json["summary_answer"]["value"][0][0],
        "<http://ex/knows>"
    );
    assert_eq!(totals_json["summary_answer"]["value"][0][1], 2);
    assert_eq!(
        totals_json["summary_answer"]["value"][1][0],
        "<http://ex/age>"
    );
    assert_eq!(totals_json["summary_answer"]["value"][1][1], 1);
    assert_eq!(totals_json["summary_answer"]["reads_index"], false);
    assert_eq!(totals_json["explain"]["query_shape"], "predicate_totals");
    assert_eq!(totals_json["explain"]["planned_access"], "summary-only");

    let predicates_query = "SELECT DISTINCT ?p WHERE { ?s ?p ?o }";
    let predicates_cost = Command::new(bin)
        .args([
            "cost",
            file.to_str().unwrap(),
            predicates_query,
            "--json",
            "--explain",
        ])
        .output()
        .unwrap();
    assert!(
        predicates_cost.status.success(),
        "{}",
        String::from_utf8_lossy(&predicates_cost.stderr)
    );
    let predicates_json: serde_json::Value =
        serde_json::from_slice(&predicates_cost.stdout).unwrap();
    assert_eq!(predicates_json["summary_answer"]["available"], true);
    assert_eq!(predicates_json["summary_answer"]["kind"], "predicate_list");
    assert_eq!(
        predicates_json["summary_answer"]["value"],
        serde_json::json!(["<http://ex/knows>", "<http://ex/age>"])
    );
    assert_eq!(predicates_json["summary_answer"]["reads_index"], false);
    assert_eq!(predicates_json["explain"]["query_shape"], "predicate_list");
    assert_eq!(predicates_json["explain"]["planned_access"], "summary-only");

    let predicate_count_query = "SELECT (COUNT(DISTINCT ?p) AS ?predicateCount) WHERE { ?s ?p ?o }";
    let predicate_count_cost = Command::new(bin)
        .args([
            "cost",
            file.to_str().unwrap(),
            predicate_count_query,
            "--json",
            "--explain",
        ])
        .output()
        .unwrap();
    assert!(
        predicate_count_cost.status.success(),
        "{}",
        String::from_utf8_lossy(&predicate_count_cost.stderr)
    );
    let predicate_count_json: serde_json::Value =
        serde_json::from_slice(&predicate_count_cost.stdout).unwrap();
    assert_eq!(predicate_count_json["summary_answer"]["available"], true);
    assert_eq!(
        predicate_count_json["summary_answer"]["kind"],
        "predicate_distinct_count"
    );
    assert_eq!(predicate_count_json["summary_answer"]["value"], 2);
    assert_eq!(predicate_count_json["summary_answer"]["reads_index"], false);
    assert_eq!(
        predicate_count_json["explain"]["query_shape"],
        "predicate_distinct_count"
    );
    assert_eq!(
        predicate_count_json["explain"]["planned_access"],
        "summary-only"
    );

    let ask_any = Command::new(bin)
        .args([
            "cost",
            file.to_str().unwrap(),
            "ASK { ?s ?p ?o }",
            "--json",
            "--explain",
        ])
        .output()
        .unwrap();
    assert!(
        ask_any.status.success(),
        "{}",
        String::from_utf8_lossy(&ask_any.stderr)
    );
    let ask_any_json: serde_json::Value = serde_json::from_slice(&ask_any.stdout).unwrap();
    assert_eq!(ask_any_json["summary_answer"]["available"], true);
    assert_eq!(ask_any_json["summary_answer"]["kind"], "triple_exists");
    assert_eq!(ask_any_json["summary_answer"]["value"], true);
    assert_eq!(ask_any_json["explain"]["query_shape"], "triple_exists");
    assert_eq!(ask_any_json["explain"]["planned_access"], "summary-only");

    std::fs::remove_file(data).ok();
    std::fs::remove_file(file).ok();
}
