use std::process::Command;

fn temp_path(name: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rete_progressive_cli_{}_{}.{}",
        std::process::id(),
        name,
        ext
    ))
}

#[test]
fn progressive_cli_answers_exact_count_from_summary_without_index() {
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

    let query = "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?total) WHERE { ?s e:knows ?o }";
    let output = Command::new(bin)
        .args(["progressive", file.to_str().unwrap(), query, "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["progressive"]["stage"], "summary");
    assert_eq!(json["progressive"]["exact"], true);
    assert_eq!(json["progressive"]["reads_index"], false);
    assert_eq!(json["progressive"]["query_shape"], "predicate_count");
    assert_eq!(json["progressive"]["predicate"], "<http://ex/knows>");
    assert!(json["progressive"]["requests"].as_u64().unwrap() >= 1);
    assert!(json["progressive"]["bytes"].as_u64().unwrap() >= 128);
    assert_eq!(json["head"]["vars"], serde_json::json!(["total"]));
    assert_eq!(
        json["results"]["bindings"][0]["total"],
        serde_json::json!({
            "type": "literal",
            "value": "2",
            "datatype": "http://www.w3.org/2001/XMLSchema#integer",
        })
    );

    let total_query = "SELECT (COUNT(*) AS ?total) WHERE { ?s ?p ?o }";
    let total_output = Command::new(bin)
        .args(["progressive", file.to_str().unwrap(), total_query, "--json"])
        .output()
        .unwrap();
    assert!(
        total_output.status.success(),
        "{}",
        String::from_utf8_lossy(&total_output.stderr)
    );

    let total_json: serde_json::Value = serde_json::from_slice(&total_output.stdout).unwrap();
    assert_eq!(total_json["progressive"]["query_shape"], "triple_count");
    assert_eq!(total_json["progressive"]["reads_index"], false);
    assert_eq!(total_json["head"]["vars"], serde_json::json!(["total"]));
    assert_eq!(total_json["results"]["bindings"][0]["total"]["value"], "3");

    let totals_query = "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p";
    let totals_output = Command::new(bin)
        .args([
            "progressive",
            file.to_str().unwrap(),
            totals_query,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        totals_output.status.success(),
        "{}",
        String::from_utf8_lossy(&totals_output.stderr)
    );

    let totals_json: serde_json::Value = serde_json::from_slice(&totals_output.stdout).unwrap();
    assert_eq!(
        totals_json["progressive"]["query_shape"],
        "predicate_totals"
    );
    assert_eq!(totals_json["progressive"]["reads_index"], false);
    assert_eq!(totals_json["head"]["vars"], serde_json::json!(["p", "n"]));
    assert_eq!(
        totals_json["results"]["bindings"][0]["p"]["value"],
        "http://ex/knows"
    );
    assert_eq!(totals_json["results"]["bindings"][0]["n"]["value"], "2");
    assert_eq!(
        totals_json["results"]["bindings"][1]["p"]["value"],
        "http://ex/age"
    );
    assert_eq!(totals_json["results"]["bindings"][1]["n"]["value"], "1");

    let predicates_query = "SELECT DISTINCT ?p WHERE { ?s ?p ?o }";
    let predicates_output = Command::new(bin)
        .args([
            "progressive",
            file.to_str().unwrap(),
            predicates_query,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        predicates_output.status.success(),
        "{}",
        String::from_utf8_lossy(&predicates_output.stderr)
    );

    let predicates_json: serde_json::Value =
        serde_json::from_slice(&predicates_output.stdout).unwrap();
    assert_eq!(
        predicates_json["progressive"]["query_shape"],
        "predicate_list"
    );
    assert_eq!(predicates_json["progressive"]["reads_index"], false);
    assert_eq!(predicates_json["head"]["vars"], serde_json::json!(["p"]));
    assert_eq!(
        predicates_json["results"]["bindings"][0]["p"]["value"],
        "http://ex/knows"
    );
    assert_eq!(
        predicates_json["results"]["bindings"][1]["p"]["value"],
        "http://ex/age"
    );

    let predicate_count_query = "SELECT (COUNT(DISTINCT ?p) AS ?predicateCount) WHERE { ?s ?p ?o }";
    let predicate_count_output = Command::new(bin)
        .args([
            "progressive",
            file.to_str().unwrap(),
            predicate_count_query,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        predicate_count_output.status.success(),
        "{}",
        String::from_utf8_lossy(&predicate_count_output.stderr)
    );

    let predicate_count_json: serde_json::Value =
        serde_json::from_slice(&predicate_count_output.stdout).unwrap();
    assert_eq!(
        predicate_count_json["progressive"]["query_shape"],
        "predicate_distinct_count"
    );
    assert_eq!(predicate_count_json["progressive"]["reads_index"], false);
    assert_eq!(
        predicate_count_json["head"]["vars"],
        serde_json::json!(["predicateCount"])
    );
    assert_eq!(
        predicate_count_json["results"]["bindings"][0]["predicateCount"]["value"],
        "2"
    );

    let any_ask_output = Command::new(bin)
        .args([
            "progressive",
            file.to_str().unwrap(),
            "ASK { ?s ?p ?o }",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        any_ask_output.status.success(),
        "{}",
        String::from_utf8_lossy(&any_ask_output.stderr)
    );

    let any_ask_json: serde_json::Value = serde_json::from_slice(&any_ask_output.stdout).unwrap();
    assert_eq!(any_ask_json["boolean"], true);
    assert_eq!(any_ask_json["progressive"]["query_shape"], "triple_exists");
    assert_eq!(any_ask_json["progressive"]["reads_index"], false);

    std::fs::remove_file(data).ok();
    std::fs::remove_file(file).ok();
}
