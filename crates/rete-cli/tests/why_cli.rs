use std::process::Command;

fn temp_path(name: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rete_why_cli_{}_{}.{}",
        std::process::id(),
        name,
        ext
    ))
}

#[test]
fn why_cli_reports_result_provenance_as_json() {
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

    let why = Command::new(bin)
        .args([
            "why",
            file.to_str().unwrap(),
            "--predicate",
            "<http://ex/knows>",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        why.status.success(),
        "{}",
        String::from_utf8_lossy(&why.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&why.stdout).unwrap();
    assert_eq!(json["pattern"]["predicate"], "<http://ex/knows>");
    assert_eq!(json["results"].as_array().unwrap().len(), 2);

    let first = &json["results"][0];
    assert_eq!(first["terms"]["predicate"], "<http://ex/knows>");
    assert_eq!(first["provenance"]["graph"], "default");
    assert_eq!(first["provenance"]["index_permutation"], "POS");
    assert!(first["provenance"]["index_range"]["len"].as_u64().unwrap() > 0);
    assert!(
        first["provenance"]["dictionary_range"]["len"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(first["provenance"]["tile"]["available"], false);
    assert_eq!(first["provenance"]["tile"]["reason"], "not_materialized");
}
