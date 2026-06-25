//! Differential correctness oracle: run a battery of SPARQL queries — heavy on
//! the built-in functions (`sparql/expr.rs`) — on both rete and Oxigraph over the
//! same data and assert the result sets agree. The SQLite discipline of "test
//! against a second, independent implementation." Run under coverage
//! (`-p bench`), it drives the expr.rs branches the unit tests miss; run plainly
//! (`cargo test -p bench --test differential`), it's a correctness gate.

use oxigraph::io::RdfFormat;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use rete_core::{eval_sparql, ingest, Rete};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn nt() -> String {
    format!(
        r#"<http://ex/s> <http://ex/str> "Hello World" .
<http://ex/s> <http://ex/num> "42"^^<{XSD}integer> .
<http://ex/s> <http://ex/dec> "-3.5"^^<{XSD}decimal> .
<http://ex/s> <http://ex/dbl> "2.0"^^<{XSD}double> .
<http://ex/s> <http://ex/dt> "2024-03-15T10:30:45"^^<{XSD}dateTime> .
<http://ex/s> <http://ex/fr> "bonjour"@fr .
<http://ex/s> <http://ex/ref> <http://ex/thing> .
<http://ex/t> <http://ex/str> "apple pie" .
<http://ex/t> <http://ex/num> "7"^^<{XSD}integer> .
"#
    )
}

fn engines() -> (Rete, Store) {
    let nt = nt();
    let quads = ingest::parse_statements(&nt, "nt").expect("parse NT");
    let (image, _) = ingest::assemble_dataset(quads, &[]);
    let rete = Rete::open(&image).expect("rete open");
    let store = Store::new().unwrap();
    store
        .load_from_reader(RdfFormat::NTriples, nt.as_bytes())
        .unwrap();
    (rete, store)
}

/// Reduce a term token to a comparable value: a literal's lexical (dropping the
/// `"`…`"` and any datatype/lang — STR()-wrapped queries make most values plain),
/// else the IRI/bnode verbatim. Numbers are canonicalized so format differences
/// (`3.5` vs `3.5E0`, `-3` vs `-3.0`) never create a false mismatch — only a real
/// value difference can fail the comparison.
fn lexical(term: &str) -> String {
    let raw = if let Some(rest) = term.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest).to_string()
    } else {
        term.to_string()
    };
    if let Ok(n) = raw.parse::<f64>() {
        if n.fract() == 0.0 && n.abs() < 1e15 {
            return format!("{}", n as i64);
        }
        return format!("{n}");
    }
    raw
}

/// rete's solutions as a sorted multiset of canonical `var=lexical;…` rows.
fn rete_rows(rete: &Rete, q: &str) -> Vec<String> {
    let (_, sols) = eval_sparql(rete, q).expect("rete query");
    let mut out: Vec<String> = sols
        .iter()
        .map(|b| {
            let mut parts: Vec<String> =
                b.iter().map(|(k, v)| format!("{k}={}", lexical(v))).collect();
            parts.sort();
            parts.join(";")
        })
        .collect();
    out.sort();
    out
}

/// Oxigraph's solutions in the same canonical form.
fn oxi_rows(store: &Store, q: &str) -> Vec<String> {
    let ev = SparqlEvaluator::new().parse_query(q).expect("oxi parse");
    let QueryResults::Solutions(sols) = ev.on_store(store).execute().expect("oxi exec") else {
        panic!("expected solutions for: {q}");
    };
    let mut out: Vec<String> = sols
        .map(|s| {
            let s = s.unwrap();
            let mut parts: Vec<String> = s
                .iter()
                .map(|(var, term)| format!("{}={}", var.as_str(), lexical(&term.to_string())))
                .collect();
            parts.sort();
            parts.join(";")
        })
        .collect();
    out.sort();
    out
}

#[test]
fn expr_builtins_agree_with_oxigraph() {
    let (rete, store) = engines();
    // Each query is STR()-wrapped for value functions (plain lexical out) or uses
    // FILTER for boolean/comparison functions (row inclusion is the signal).
    const S: &str = "<http://ex/s>";
    let queries: &[String] = &[
        // --- string functions ---
        format!("SELECT (STR(STRLEN(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(UCASE(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(LCASE(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(SUBSTR(?o, 1, 5)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(SUBSTR(?o, 7)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(CONCAT(?o, \"!\")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(STRBEFORE(?o, \" \")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(STRAFTER(?o, \" \")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(REPLACE(?o, \"o\", \"0\")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(ENCODE_FOR_URI(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        // --- hashes ---
        format!("SELECT (STR(MD5(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(SHA1(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(SHA256(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(SHA512(?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        // --- numeric (canonicalized, so decimal/float format is irrelevant) ---
        format!("SELECT (STR(ABS(?o)) AS ?r) WHERE {{ {S} <http://ex/dec> ?o }}"),
        format!("SELECT (STR(CEIL(?o)) AS ?r) WHERE {{ {S} <http://ex/dec> ?o }}"),
        format!("SELECT (STR(FLOOR(?o)) AS ?r) WHERE {{ {S} <http://ex/dec> ?o }}"),
        format!("SELECT (STR(ROUND(?o)) AS ?r) WHERE {{ {S} <http://ex/dec> ?o }}"),
        format!("SELECT (STR(?o + 8) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(?o * 2) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(?o - 100) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        // --- datetime parts ---
        format!("SELECT (STR(YEAR(?o)) AS ?r) WHERE {{ {S} <http://ex/dt> ?o }}"),
        format!("SELECT (STR(MONTH(?o)) AS ?r) WHERE {{ {S} <http://ex/dt> ?o }}"),
        format!("SELECT (STR(DAY(?o)) AS ?r) WHERE {{ {S} <http://ex/dt> ?o }}"),
        format!("SELECT (STR(HOURS(?o)) AS ?r) WHERE {{ {S} <http://ex/dt> ?o }}"),
        format!("SELECT (STR(MINUTES(?o)) AS ?r) WHERE {{ {S} <http://ex/dt> ?o }}"),
        // --- type / term functions ---
        format!("SELECT (STR(LANG(?o)) AS ?r) WHERE {{ {S} <http://ex/fr> ?o }}"),
        format!("SELECT (STR(DATATYPE(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(<http://ex/{{x}}>) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}")
            .replace("{x}", "z"),
        // --- conditionals ---
        format!("SELECT (STR(COALESCE(?missing, ?o)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(IF(STRLEN(?o) > 5, \"big\", \"small\")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        // --- boolean filters (row inclusion is the oracle) ---
        "SELECT ?o WHERE { ?s <http://ex/str> ?o FILTER(CONTAINS(?o, \"pp\")) }".to_string(),
        "SELECT ?o WHERE { ?s <http://ex/str> ?o FILTER(STRSTARTS(?o, \"Hello\")) }".to_string(),
        "SELECT ?o WHERE { ?s <http://ex/str> ?o FILTER(STRENDS(?o, \"pie\")) }".to_string(),
        "SELECT ?o WHERE { ?s <http://ex/str> ?o FILTER(REGEX(?o, \"^[A-Z]\")) }".to_string(),
        "SELECT ?s WHERE { ?s <http://ex/num> ?o FILTER(?o > 10) }".to_string(),
        "SELECT ?s WHERE { ?s <http://ex/num> ?o FILTER(?o >= 7 && ?o <= 42) }".to_string(),
        "SELECT ?o WHERE { ?s ?p ?o FILTER(ISLITERAL(?o)) }".to_string(),
        "SELECT ?o WHERE { ?s ?p ?o FILTER(ISIRI(?o)) }".to_string(),
        "SELECT ?o WHERE { ?s ?p ?o FILTER(ISNUMERIC(?o)) }".to_string(),
        "SELECT ?o WHERE { ?s ?p ?o FILTER(!ISBLANK(?o)) }".to_string(),
        "SELECT ?o WHERE { ?s <http://ex/num> ?o FILTER(?o IN (7, 99)) }".to_string(),
        "SELECT ?o WHERE { ?s <http://ex/fr> ?o FILTER(LANGMATCHES(LANG(?o), \"fr\")) }".to_string(),
        "SELECT ?o WHERE { ?s <http://ex/str> ?o FILTER(true) }".to_string(),
    ];
    let mut failures = Vec::new();
    for q in queries {
        let r = rete_rows(&rete, q);
        let o = oxi_rows(&store, q);
        if r != o {
            failures.push(format!("MISMATCH\n  q: {q}\n  rete: {r:?}\n  oxi:  {o:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} queries disagreed with Oxigraph:\n{}",
        failures.len(),
        queries.len(),
        failures.join("\n")
    );
}
