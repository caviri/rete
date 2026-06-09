use std::process::Command;

fn temp_path(name: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rete_shacl_cli_{}_{}.{}",
        std::process::id(),
        name,
        ext
    ))
}

#[test]
fn shacl_cli_reports_conformance_and_violations() {
    let bin = env!("CARGO_BIN_EXE_rete");
    let data = temp_path("data", "nt");
    let file = temp_path("data", "rete");
    let ok_shapes = temp_path("ok", "ttl");
    let bad_shapes = temp_path("bad", "ttl");

    std::fs::write(
        &data,
        r#"<http://ex/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/alice> <http://ex/email> "alice@example.org" .
"#,
    )
    .unwrap();
    let shape = |min: u8| {
        format!(
            r#"@prefix ex: <http://ex/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [
    sh:path ex:email ;
    sh:minCount {min}
  ] .
"#
        )
    };
    std::fs::write(&ok_shapes, shape(1)).unwrap();
    std::fs::write(&bad_shapes, shape(2)).unwrap();

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

    let ok = Command::new(bin)
        .args([
            "shacl",
            file.to_str().unwrap(),
            "--shapes",
            ok_shapes.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(String::from_utf8_lossy(&ok.stdout).contains("\"conforms\": true"));

    let bad = Command::new(bin)
        .args([
            "shacl",
            file.to_str().unwrap(),
            "--shapes",
            bad_shapes.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    let stdout = String::from_utf8_lossy(&bad.stdout);
    assert!(stdout.contains("\"conforms\": false"));
    assert!(stdout.contains("MinCountConstraintComponent"));

    std::fs::remove_file(data).ok();
    std::fs::remove_file(file).ok();
    std::fs::remove_file(ok_shapes).ok();
    std::fs::remove_file(bad_shapes).ok();
}
