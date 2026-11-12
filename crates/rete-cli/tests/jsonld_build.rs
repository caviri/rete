//! `rete build` of JSON-LD and YAML-LD input: by extension and by `--format`,
//! against the same data built from TriG, plus remote contexts (refused by
//! default, `--context IRI=FILE`, `--fetch-contexts`) and the memory-bounded
//! build's refusal.

mod common;

use common::rete;
use std::io::{Read as _, Write as _};

const JSONLD: &str = r#"{
  "@context": {
    "ex": "http://example.org/",
    "name": "ex:name",
    "age": {"@id": "ex:age", "@type": "http://www.w3.org/2001/XMLSchema#integer"},
    "label": {"@id": "ex:label", "@language": "en"},
    "knows": {"@id": "ex:knows", "@type": "@id"}
  },
  "@graph": [
    {"@id": "ex:alice", "name": "Alice", "age": "42", "label": "Alice", "knows": "ex:bob"},
    {"@id": "ex:g1", "@graph": [{"@id": "ex:bob", "name": "Bob"}]}
  ]
}"#;

const YAMLLD: &str = r#""@context":
  ex: http://example.org/
  name: ex:name
  age: {"@id": "ex:age", "@type": "http://www.w3.org/2001/XMLSchema#integer"}
  label: {"@id": "ex:label", "@language": en}
  knows: {"@id": "ex:knows", "@type": "@id"}
"@graph":
  - "@id": ex:alice
    name: Alice
    age: "42"
    label: Alice
    knows: ex:bob
  - "@id": ex:g1
    "@graph":
      - "@id": ex:bob
        name: Bob
"#;

const TRIG: &str = r#"@prefix ex: <http://example.org/> .
ex:alice ex:name "Alice" ;
  ex:age "42"^^<http://www.w3.org/2001/XMLSchema#integer> ;
  ex:label "Alice"@en ;
  ex:knows ex:bob .
ex:g1 { ex:bob ex:name "Bob" . }
"#;

/// The statements of a .rete as sorted N-Quads lines.
fn exported(file: &std::path::Path) -> Vec<String> {
    let out = rete()
        .arg("export")
        .arg(file)
        .args(["--format", "nq"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

fn build(dir: &std::path::Path, input: &str, text: &str, extra: &[&str]) -> std::path::PathBuf {
    let src = dir.join(input);
    std::fs::write(&src, text).unwrap();
    let out = dir.join(format!("{input}.rete"));
    rete()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(extra)
        .assert()
        .success();
    out
}

#[test]
fn json_ld_and_yaml_ld_build_the_same_graph_as_trig() {
    let dir = tempfile::tempdir().unwrap();
    let expected = exported(&build(dir.path(), "g.trig", TRIG, &[]));
    assert_eq!(expected.len(), 5, "{expected:?}");
    assert!(expected
        .iter()
        .any(|l| l.ends_with("<http://example.org/g1> .")));

    // by extension
    assert_eq!(
        exported(&build(dir.path(), "g.jsonld", JSONLD, &[])),
        expected
    );
    assert_eq!(
        exported(&build(dir.path(), "g.yamlld", YAMLLD, &[])),
        expected
    );
    // `.json` / `.yaml` are not detected: they need the format named
    assert_eq!(
        exported(&build(
            dir.path(),
            "g.json",
            JSONLD,
            &["--format", "jsonld"]
        )),
        expected
    );
    assert_eq!(
        exported(&build(dir.path(), "g.yml", YAMLLD, &["--format", "yamlld"])),
        expected
    );

    // and the result answers queries
    let built = dir.path().join("g.jsonld.rete");
    rete()
        .arg("query")
        .arg(&built)
        .args(["--predicate", "<http://example.org/knows>"])
        .assert()
        .success()
        .stdout(predicates::str::contains("example.org/bob"));
}

#[test]
fn collapse_graphs_folds_json_ld_named_graphs() {
    let dir = tempfile::tempdir().unwrap();
    let built = build(dir.path(), "g.jsonld", JSONLD, &["--collapse-graphs"]);
    let lines = exported(&built);
    assert_eq!(lines.len(), 5);
    assert!(lines.iter().all(|l| !l.contains("g1>")), "{lines:?}");
}

#[test]
fn blank_node_labels_do_not_collide_across_files() {
    let dir = tempfile::tempdir().unwrap();
    let one = r#"{"@context": {"ex": "http://example.org/"}, "@id": "_:b0", "ex:p": "one"}"#;
    let two = r#"{"@context": {"ex": "http://example.org/"}, "@id": "_:b0", "ex:p": "two"}"#;
    std::fs::write(dir.path().join("a.jsonld"), one).unwrap();
    std::fs::write(dir.path().join("b.jsonld"), two).unwrap();
    let out = dir.path().join("ab.rete");
    rete()
        .arg("build")
        .arg(dir.path().join("a.jsonld"))
        .arg(dir.path().join("b.jsonld"))
        .arg("-o")
        .arg(&out)
        .assert()
        .success();
    let lines = exported(&out);
    let subject = |l: &str| l.split(' ').next().unwrap().to_string();
    assert_eq!(lines.len(), 2);
    assert_ne!(subject(&lines[0]), subject(&lines[1]), "{lines:?}");
}

const REMOTE: &str =
    r#"{"@context": "http://ctx.example/c.jsonld", "@id": "http://example.org/a", "name": "x"}"#;
const CONTEXT: &str = r#"{"@context": {"name": "http://example.org/name"}}"#;
const REMOTE_QUAD: &str = r#"<http://example.org/a> <http://example.org/name> "x" ."#;

#[test]
fn a_remote_context_is_refused_naming_the_iri_and_the_ways_out() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("r.jsonld");
    std::fs::write(&src, REMOTE).unwrap();
    let out = rete()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.path().join("r.rete"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("http://ctx.example/c.jsonld"), "{err}");
    assert!(
        err.contains("--context") && err.contains("--fetch-contexts"),
        "{err}"
    );
}

#[test]
fn context_maps_an_iri_to_a_local_file() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = dir.path().join("c.jsonld");
    std::fs::write(&ctx, CONTEXT).unwrap();
    let mapping = format!("http://ctx.example/c.jsonld={}", ctx.display());
    let built = build(dir.path(), "r.jsonld", REMOTE, &["--context", &mapping]);
    assert_eq!(exported(&built), vec![REMOTE_QUAD.to_string()]);

    // `validate` takes the same flag
    let src = dir.path().join("r.jsonld");
    rete()
        .arg("validate")
        .arg(&src)
        .args(["--context", &mapping])
        .assert()
        .success();
    // a malformed mapping is a usage error
    rete()
        .arg("validate")
        .arg(&src)
        .args(["--context", "no-equals-sign"])
        .assert()
        .failure();
}

#[test]
fn fetch_contexts_gets_the_context_over_http() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/c.jsonld", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/ld+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{CONTEXT}",
                CONTEXT.len()
            );
            let _ = stream.write_all(head.as_bytes());
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let doc = format!(r#"{{"@context": "{url}", "@id": "http://example.org/a", "name": "x"}}"#);
    let built = build(dir.path(), "f.jsonld", &doc, &["--fetch-contexts"]);
    assert_eq!(exported(&built), vec![REMOTE_QUAD.to_string()]);
}

#[test]
fn the_memory_bounded_build_refuses_json_ld_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("g.jsonld");
    std::fs::write(&src, JSONLD).unwrap();
    let out = rete()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.path().join("g.rete"))
        .args(["--memory-budget-mb", "64"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("JSON-LD") && err.contains("jsonld"), "{err}");
}

#[test]
fn a_malformed_document_is_refused_with_the_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bad.jsonld");
    std::fs::write(&src, "{\"@context\": ").unwrap();
    let out = rete().arg("validate").arg(&src).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("bad.jsonld") && err.contains("json-ld"),
        "{err}"
    );
}
