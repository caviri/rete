//! Jelly conformance suite: every `pos_*` case must decode to exactly the
//! expected statements, every `neg_*` case must be refused.
//!
//! Cases are `<version>/<category>/<case>/in.jelly` plus `out*.nt|nq` for the
//! positive ones. The vendored subset lives in `tests/jelly-conformance`; set
//! `RETE_JELLY_CONFORMANCE` to a jelly-protobuf `test/rdf` checkout (its
//! `v1_1/from_jelly` and `v1_2/from_jelly` directories) to run all of it.
//!
//! Two kinds of case are refused on purpose, whatever the suite says: streams
//! of RDF Messages (`messages`, optional in the specification) and generalized
//! RDF (`*_generalized`), which rete's term model has no place for.

use rete_core::ingest::{parse_jelly_bytes, parse_statements, RawQuad};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn root() -> (PathBuf, bool) {
    match std::env::var("RETE_JELLY_CONFORMANCE") {
        Ok(p) if !p.is_empty() => (PathBuf::from(p), true),
        _ => (
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/jelly-conformance"),
            false,
        ),
    }
}

/// Rename blank nodes in order of first appearance, so two statement lists that
/// differ only by a bijection of labels compare equal.
fn canonical(quads: &[RawQuad]) -> Vec<[String; 4]> {
    let mut map: HashMap<String, usize> = HashMap::new();
    let mut term = |t: &str| -> String {
        if t.starts_with('"') {
            return normal_literal(t);
        }
        let mut out = String::new();
        let mut rest = t;
        while let Some(i) = rest.find("_:") {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 2..];
            let end = tail
                .find(|c: char| c.is_whitespace() || c == '>' || c == ')')
                .unwrap_or(tail.len());
            let n = map.len();
            let id = *map.entry(tail[..end].to_string()).or_insert(n);
            out.push_str(&format!("_:b{id}"));
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    };
    quads
        .iter()
        .map(|(s, p, o, g)| {
            [
                term(s),
                term(p),
                term(o),
                g.as_deref().map(&mut term).unwrap_or_default(),
            ]
        })
        .collect()
}

/// The same literal spelled one way: rete's N-Quads reader keeps the escapes of
/// the source text, so `"a\tb"` and a raw tab are two spellings of one term.
fn normal_literal(t: &str) -> String {
    let close = t.rfind('"').unwrap();
    let (body, tail) = (&t[1..close], &t[close + 1..]);
    let mut lex = String::new();
    let mut it = body.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            lex.push(c);
            continue;
        }
        match it.next().unwrap() {
            't' => lex.push('\t'),
            'n' => lex.push('\n'),
            'r' => lex.push('\r'),
            'b' => lex.push('\u{8}'),
            'f' => lex.push('\u{c}'),
            e @ ('u' | 'U') => {
                let width = if e == 'u' { 4 } else { 8 };
                let hex: String = it.by_ref().take(width).collect();
                lex.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
            }
            other => lex.push(other),
        }
    }
    let tail = if tail.starts_with('@') {
        tail.to_ascii_lowercase()
    } else {
        tail.to_string()
    };
    format!("{lex:?}{tail}")
}

fn expected(dir: &Path) -> Vec<RawQuad> {
    let mut outs: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            n.starts_with("out") && (n.ends_with(".nt") || n.ends_with(".nq"))
        })
        .collect();
    outs.sort();
    let mut all = Vec::new();
    for o in outs {
        let text = std::fs::read_to_string(&o).unwrap();
        let fmt = if o.extension().unwrap() == "nq" {
            "nq"
        } else {
            "nt"
        };
        all.extend(parse_statements(&text, fmt).unwrap_or_else(|e| panic!("{}: {e}", o.display())));
    }
    all
}

#[test]
fn conformance_suite() {
    let (root, full) = root();
    let mut ran = 0;
    let mut failures = Vec::new();
    for ver in ["v1_1", "v1_2"] {
        let base = if full {
            root.join(ver).join("from_jelly")
        } else {
            root.join(ver)
        };
        let Ok(cats) = std::fs::read_dir(&base) else {
            continue;
        };
        for cat in cats {
            let cat = cat.unwrap().path();
            if !cat.is_dir() {
                continue;
            }
            let cat_name = cat.file_name().unwrap().to_string_lossy().into_owned();
            let refused = cat_name == "messages" || cat_name.contains("generalized");
            for case in std::fs::read_dir(&cat).unwrap() {
                let case = case.unwrap().path();
                let name = format!(
                    "{ver}/{cat_name}/{}",
                    case.file_name().unwrap().to_string_lossy()
                );
                let input = std::fs::read(case.join("in.jelly")).unwrap();
                let got = parse_jelly_bytes(&input);
                ran += 1;
                let positive = name.contains("/pos_");
                match (&got, positive && !refused) {
                    (Err(_), false) => {}
                    (Ok(_), false) => failures.push(format!("{name}: should have been refused")),
                    (Err(e), true) => failures.push(format!("{name}: refused: {e}")),
                    (Ok(q), true) => {
                        if canonical(q) != canonical(&expected(&case)) {
                            failures.push(format!("{name}: statements differ"));
                        }
                    }
                }
            }
        }
    }
    assert!(
        ran > 0,
        "no conformance cases found under {}",
        root.display()
    );
    assert!(
        failures.is_empty(),
        "{} of {ran} cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("jelly conformance: {ran} cases passed");
}
