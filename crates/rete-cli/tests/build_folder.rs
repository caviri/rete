//! `rete build DIR`: a folder as input, one named graph per file.

mod common;

use std::path::{Path, PathBuf};

use common::{json, read_plain, rete};

/// A small tree with nested folders, a name that needs percent-encoding, a file
/// with a graph of its own, and things that are not read.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    w(
        "a.ttl",
        "@prefix ex: <http://ex.test/> .\nex:s1 ex:p \"one\" .\n_:b0 ex:p \"bn-a\" .\n",
    );
    w(
        "q.nq",
        "<http://ex.test/s4> <http://ex.test/p> \"four\" <http://ex.test/own> .\n\
         <http://ex.test/s5> <http://ex.test/p> \"five\" .\n",
    );
    w(
        "sub/b.nt",
        "<http://ex.test/s2> <http://ex.test/p> \"two\" .\n_:b0 <http://ex.test/p> \"bn-b\" .\n",
    );
    w(
        "sub/deep/my file.nt",
        "<http://ex.test/s3> <http://ex.test/p> \"three\" .\n",
    );
    w("notes.txt", "not rdf\n");
    w("sub/README.md", "not rdf either\n");
    w(
        ".hidden.ttl",
        "<http://ex.test/h> <http://ex.test/p> \"hidden\" .\n",
    );
    w(
        ".git/x.nt",
        "<http://ex.test/h> <http://ex.test/p> \"hidden dir\" .\n",
    );
    dir
}

/// The files of `tree()` that are read, sorted by relative path.
const READ: [&str; 4] = ["a.ttl", "q.nq", "sub/b.nt", "sub/deep/my file.nt"];

fn build_args(inputs: &[&Path], out: &Path, extra: &[&str]) -> assert_cmd::assert::Assert {
    let mut c = rete();
    c.arg("build");
    for i in inputs {
        c.arg(i);
    }
    c.arg("-o").arg(out).args(["--no-pyramid"]).args(extra);
    c.assert()
}

fn rows(file: &Path, query: &str) -> Vec<String> {
    let v = json(rete().arg("sparql").arg(file).arg(query).arg("--json"));
    let vars: Vec<String> = v["head"]["vars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    v["results"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            vars.iter()
                .map(|n| b[n]["value"].as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect()
}

fn built(dir: &Path, extra: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("out.rete");
    build_args(&[dir], &out, extra).success();
    (out_dir, out)
}

#[test]
fn each_file_is_a_graph_named_after_it() {
    let t = tree();
    let (_o, out) = built(t.path(), &[]);

    let q = "SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s <http://ex.test/p> ?o \
             FILTER(!STRSTARTS(?o, \"bn-\")) } } ORDER BY ?g ?s";
    let mut want: Vec<String> = [
        // a name that needs percent-encoding, two levels down
        "urn:rete:file:sub/deep/my%20file | http://ex.test/s3 | three",
        "urn:rete:file:a | http://ex.test/s1 | one",
        // the triple of q.nq without a graph goes to the file graph
        "urn:rete:file:q | http://ex.test/s5 | five",
        "urn:rete:file:sub/b | http://ex.test/s2 | two",
        // the quad that names its graph keeps it
        "http://ex.test/own | http://ex.test/s4 | four",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    want.sort();
    let mut got = rows(&out, q);
    got.sort();
    assert_eq!(got, want);

    let one = rows(
        &out,
        "SELECT ?s WHERE { GRAPH <urn:rete:file:sub/b> { ?s <http://ex.test/p> \"two\" } }",
    );
    assert_eq!(one, ["http://ex.test/s2"]);
    // nothing is left in the default graph
    let n = rows(&out, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }");
    assert_eq!(n, ["0"]);
}

#[test]
fn blank_nodes_of_different_files_stay_apart() {
    let t = tree();
    let (_o, out) = built(t.path(), &[]);
    let q = "SELECT DISTINCT ?s WHERE { GRAPH ?g { ?s <http://ex.test/p> ?o \
             FILTER(?o IN (\"bn-a\", \"bn-b\")) } }";
    assert_eq!(rows(&out, q).len(), 2);
}

#[test]
fn hidden_and_unknown_files_are_not_read_and_the_note_says_so() {
    let t = tree();
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("o.rete");
    let a = build_args(&[t.path()], &out, &[]).success();
    let err = String::from_utf8_lossy(&a.get_output().stderr).to_string();
    assert!(err.contains("skipped 2 files"), "{err}");
    assert!(
        err.contains("notes.txt") && err.contains("sub/README.md"),
        "{err}"
    );
    assert!(!err.contains(".hidden"), "{err}");
    let all = rows(
        &out,
        "SELECT ?o WHERE { GRAPH ?g { ?s <http://ex.test/p> ?o } FILTER(CONTAINS(?o, \"hidden\")) }",
    );
    assert!(all.is_empty(), "{all:?}");
}

#[test]
fn merge_files_is_the_same_as_listing_the_files() {
    let t = tree();
    let (_o1, merged) = built(t.path(), &["--merge-files"]);
    let listed: Vec<PathBuf> = READ.iter().map(|r| t.path().join(r)).collect();
    let refs: Vec<&Path> = listed.iter().map(|p| p.as_path()).collect();
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("l.rete");
    build_args(&refs, &out, &[]).success();
    assert_eq!(read_plain(&merged), read_plain(&out));
    // and there is a default graph
    let n = rows(&merged, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }");
    assert_ne!(n, ["0"]);
}

#[test]
fn collapse_graphs_on_a_folder_equals_listing_with_collapse() {
    let t = tree();
    let (_o1, a) = built(t.path(), &["--collapse-graphs"]);
    let (_o2, b) = built(t.path(), &["--merge-files", "--collapse-graphs"]);
    let listed: Vec<PathBuf> = READ.iter().map(|r| t.path().join(r)).collect();
    let refs: Vec<&Path> = listed.iter().map(|p| p.as_path()).collect();
    let out_dir = tempfile::tempdir().unwrap();
    let c = out_dir.path().join("c.rete");
    build_args(&refs, &c, &["--collapse-graphs"]).success();
    assert_eq!(read_plain(&a), read_plain(&c));
    assert_eq!(read_plain(&b), read_plain(&c));
    let own = rows(
        &a,
        "SELECT ?o WHERE { ?s <http://ex.test/p> ?o FILTER(?o = \"four\") }",
    );
    assert_eq!(own, ["four"]);
}

#[test]
fn graph_base_sets_the_start_of_the_iri() {
    let t = tree();
    let (_o, out) = built(t.path(), &["--graph-base", "http://g.test/f/"]);
    let r = rows(
        &out,
        "SELECT ?s WHERE { GRAPH <http://g.test/f/sub/b> { ?s <http://ex.test/p> \"two\" } }",
    );
    assert_eq!(r, ["http://ex.test/s2"]);
}

#[test]
fn two_files_with_one_graph_name_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.ttl"),
        "<http://e/s> <http://e/p> <http://e/o> .\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("a.nt"),
        "<http://e/s> <http://e/p> <http://e/o2> .\n",
    )
    .unwrap();
    let out = dir.path().join("..").join("never.rete");
    let a = build_args(&[dir.path()], &out, &[]).failure();
    let err = String::from_utf8_lossy(&a.get_output().stderr).to_string();
    assert!(err.contains("a.ttl") && err.contains("a.nt"), "{err}");
    assert!(err.contains("urn:rete:file:a"), "{err}");
}

#[test]
fn a_folder_with_no_rdf_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap().path().join("o.rete");
    let a = build_args(&[dir.path()], &out, &[]).failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("no RDF files"));

    std::fs::write(dir.path().join("x.txt"), "no").unwrap();
    let a = build_args(&[dir.path()], &out, &[]).failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("no RDF files"));
}

#[test]
fn format_flag_reads_every_visible_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("data.dat"),
        "<http://e/s> <http://e/p> \"x\" .\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".skip"), "garbage\n").unwrap();
    let (_o, out) = built(dir.path(), &["--format", "nt"]);
    let r = rows(
        &out,
        "SELECT ?o WHERE { GRAPH <urn:rete:file:data.dat> { ?s ?p ?o } }",
    );
    assert_eq!(r, ["x"]);
}

#[test]
fn graph_per_file_on_a_list_of_files() {
    let t = tree();
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("o.rete");
    rete()
        .current_dir(t.path())
        .args([
            "build",
            "a.ttl",
            "sub/b.nt",
            "--graph-per-file",
            "--no-pyramid",
            "-o",
        ])
        .arg(&out)
        .assert()
        .success();
    let r = rows(
        &out,
        "SELECT ?s WHERE { GRAPH <urn:rete:file:sub/b> { ?s <http://ex.test/p> \"two\" } }",
    );
    assert_eq!(r, ["http://ex.test/s2"]);

    // stdin has no name; the two modes exclude each other
    let a = build_args(&[Path::new("-")], &out, &["--graph-per-file"]).failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("stdin"));
    let a = build_args(&[t.path()], &out, &["--graph-per-file", "--merge-files"]).failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("cannot be used"));
}

#[test]
fn graph_base_without_a_graph_per_file_is_refused() {
    let t = tree();
    let out = tempfile::tempdir().unwrap().path().join("o.rete");
    let a = build_args(
        &[t.path()],
        &out,
        &["--merge-files", "--graph-base", "urn:x:"],
    )
    .failure();
    assert!(String::from_utf8_lossy(&a.get_output().stderr).contains("--graph-base"));
}

#[test]
fn two_builds_of_the_same_folder_are_the_same_bytes() {
    let t = tree();
    let (_o1, a) = built(t.path(), &[]);
    let (_o2, b) = built(t.path(), &[]);
    assert_eq!(read_plain(&a), read_plain(&b));
}

#[test]
fn the_memory_bounded_build_writes_the_same_graphs() {
    let t = tree();
    let (_o1, ram) = built(t.path(), &[]);
    let (_o2, ext) = built(t.path(), &["--memory-budget-mb", "64"]);
    let q = "SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s <http://ex.test/p> ?o \
             FILTER(!STRSTARTS(?o, \"bn-\")) } } ORDER BY ?g ?s";
    let a = rows(&ram, q);
    assert_eq!(a.len(), 5);
    assert_eq!(a, rows(&ext, q));
    let bn = "SELECT DISTINCT ?s WHERE { GRAPH ?g { ?s <http://ex.test/p> ?o \
              FILTER(?o IN (\"bn-a\", \"bn-b\")) } }";
    assert_eq!(rows(&ext, bn).len(), 2);
}

/// JSON-LD and YAML-LD files in a folder are read by their extension and each
/// gets its graph, with a named graph of the document kept as it is.
#[test]
fn json_ld_and_yaml_ld_files_in_a_folder_are_graphs_too() {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| std::fs::write(dir.path().join(rel), text).unwrap();
    w(
        "t.ttl",
        "<http://ex.test/t> <http://ex.test/p> \"turtle\" .\n",
    );
    w(
        "j.jsonld",
        r#"{"@context": {"p": "http://ex.test/p"},
            "@graph": [
              {"@id": "http://ex.test/j", "p": "json"},
              {"@id": "http://ex.test/own", "@graph": [{"@id": "http://ex.test/k", "p": "kept"}]}
            ]}"#,
    );
    w(
        "y.yamlld",
        "\"@context\":\n  p: http://ex.test/p\n\"@id\": http://ex.test/y\np: yaml\n",
    );
    // Not read: the extension does not say JSON-LD.
    w("plain.json", "{\"a\": 1}\n");
    let (_keep, out) = built(dir.path(), &[]);
    assert_eq!(
        rows(
            &out,
            "SELECT ?g ?o WHERE { GRAPH ?g { ?s <http://ex.test/p> ?o } } ORDER BY ?g ?o"
        ),
        [
            "http://ex.test/own | kept",
            "urn:rete:file:j | json",
            "urn:rete:file:t | turtle",
            "urn:rete:file:y | yaml",
        ]
    );
}

/// Two files that both write `_:b0`, as a plain list: two nodes, in memory and
/// under a memory budget; one node with `--shared-blank-nodes`.
#[test]
fn listed_files_keep_their_blank_nodes_apart_unless_shared() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.nt");
    let b = dir.path().join("b.ttl");
    std::fs::write(
        &a,
        "_:b0 <http://ex.test/name> \"Alice\" .\n_:b0 <http://ex.test/knows> _:b1 .\n",
    )
    .unwrap();
    std::fs::write(&b, "_:b0 <http://ex.test/name> \"Bob\" .\n").unwrap();
    let subjects = "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s <http://ex.test/name> ?o }";
    // A node keeps its identity inside its file.
    let same_node = "SELECT ?o WHERE { ?s <http://ex.test/name> ?o ; <http://ex.test/knows> ?x }";
    for extra in [&[][..], &["--memory-budget-mb", "64"][..]] {
        let out = dir.path().join("apart.rete");
        build_args(&[&a, &b], &out, extra).success();
        assert_eq!(rows(&out, subjects), ["2"], "{extra:?}");
        assert_eq!(rows(&out, same_node), ["Alice"], "{extra:?}");
        let out = dir.path().join("shared.rete");
        let mut with = extra.to_vec();
        with.push("--shared-blank-nodes");
        build_args(&[&a, &b], &out, &with).success();
        assert_eq!(rows(&out, subjects), ["1"], "{with:?}");
    }
    // One input: nothing to keep apart, the build is the same with the flag.
    let one = dir.path().join("one.rete");
    let one_shared = dir.path().join("one-shared.rete");
    build_args(&[&a], &one, &[]).success();
    build_args(&[&a], &one_shared, &["--shared-blank-nodes"]).success();
    assert_eq!(read_plain(&one), read_plain(&one_shared));
}

/// A folder read with `--merge-files`, and one with `--collapse-graphs`, keep
/// the blank nodes of their files apart too.
#[test]
fn a_merged_folder_keeps_blank_nodes_apart() {
    let dir = tree();
    let nodes = "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ?p ?o FILTER(isBlank(?s)) }";
    for flag in ["--merge-files", "--collapse-graphs"] {
        let (_keep, out) = built(dir.path(), &[flag]);
        assert_eq!(rows(&out, nodes), ["2"], "{flag}");
        let (_keep, out) = built(dir.path(), &[flag, "--shared-blank-nodes"]);
        assert_eq!(rows(&out, nodes), ["1"], "{flag} shared");
    }
    // Graph per file, shared labels: the one node is in two graphs.
    let (_keep, out) = built(dir.path(), &["--shared-blank-nodes"]);
    assert_eq!(
        rows(
            &out,
            "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o FILTER(isBlank(?s)) } }"
        ),
        ["1"]
    );
}
