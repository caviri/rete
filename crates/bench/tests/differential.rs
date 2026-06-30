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
<http://ex/a> <http://ex/knows> <http://ex/b> .
<http://ex/b> <http://ex/knows> <http://ex/c> .
<http://ex/c> <http://ex/knows> <http://ex/d> .
<http://ex/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/c> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/d> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/a> <http://ex/age> "30"^^<{XSD}integer> .
<http://ex/b> <http://ex/age> "25"^^<{XSD}integer> .
<http://ex/c> <http://ex/age> "35"^^<{XSD}integer> .
<http://ex/d> <http://ex/age> "30"^^<{XSD}integer> .
<http://ex/blanksubj> <http://ex/has> _:bn .
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
    let (_, sols) = match eval_sparql(rete, q) {
        Ok(r) => r,
        Err(e) => return vec![format!("RETE_ERR: {e}")],
    };
    let mut out: Vec<String> = sols
        .iter()
        .map(|b| {
            let mut parts: Vec<String> = b
                .iter()
                .map(|(k, v)| format!("{k}={}", lexical(v)))
                .collect();
            parts.sort();
            parts.join(";")
        })
        .collect();
    out.sort();
    out
}

/// Oxigraph's solutions in the same canonical form.
fn oxi_rows(store: &Store, q: &str) -> Vec<String> {
    let ev = match SparqlEvaluator::new().parse_query(q) {
        Ok(e) => e,
        Err(e) => return vec![format!("OXI_PARSE_ERR: {e}")],
    };
    let res = match ev.on_store(store).execute() {
        Ok(r) => r,
        Err(e) => return vec![format!("OXI_EXEC_ERR: {e}")],
    };
    let QueryResults::Solutions(sols) = res else {
        return vec!["OXI_NOT_SOLUTIONS".to_string()];
    };
    let mut out = Vec::new();
    for s in sols {
        let s = match s {
            Ok(s) => s,
            Err(e) => return vec![format!("OXI_ROW_ERR: {e}")],
        };
        let mut parts: Vec<String> = s
            .iter()
            .map(|(var, term)| format!("{}={}", var.as_str(), lexical(&term.to_string())))
            .collect();
        parts.sort();
        out.push(parts.join(";"));
    }
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
        // --- casts (the cast_to dispatch) ---
        format!("SELECT (STR(<{XSD}integer>(?o)) AS ?r) WHERE {{ {S} <http://ex/dec> ?o }}"),
        format!("SELECT (STR(<{XSD}double>(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(<{XSD}boolean>(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(<{XSD}string>(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(<{XSD}decimal>(\"2.5\")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        // --- type errors: a function on the wrong type yields no value, so the
        //     projected ?r is simply unbound; both engines must agree on that. ---
        format!("SELECT ?o (ABS(?o) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        // STRLEN/UCASE/SUBSTR/ENCODE_FOR_URI/MD5 on a *numeric* (non-string) literal
        // is a type error → ?r unbound. rete used to operate on the lexical and
        // return a value (the differential caught it); now it type-checks like
        // CONCAT, so both engines agree the result is unbound.
        format!("SELECT (STR(STRLEN(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(UCASE(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(SUBSTR(?o, 1, 2)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(ENCODE_FOR_URI(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        format!("SELECT (STR(MD5(?o)) AS ?r) WHERE {{ {S} <http://ex/num> ?o }}"),
        // CONTAINS/STRSTARTS/STRENDS/REGEX on a non-string → type error → false,
        // so the row is dropped; both engines must return no rows.
        format!("SELECT ?o WHERE {{ {S} <http://ex/num> ?o FILTER(CONTAINS(?o, \"4\")) }}"),
        format!("SELECT ?o WHERE {{ {S} <http://ex/num> ?o FILTER(STRSTARTS(?o, \"4\")) }}"),
        format!("SELECT ?o WHERE {{ {S} <http://ex/num> ?o FILTER(REGEX(?o, \"4\")) }}"),
        // --- string / datetime edge cases ---
        format!("SELECT (STR(STRBEFORE(?o, \"zzz\")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(SUBSTR(?o, 100)) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(CONCAT(\"a\", \"b\", \"c\")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(REPLACE(?o, \"L\", \"_\", \"i\")) AS ?r) WHERE {{ {S} <http://ex/str> ?o }}"),
        format!("SELECT (STR(SECONDS(?o)) AS ?r) WHERE {{ {S} <http://ex/dt> ?o }}"),
        // --- more boolean / term functions ---
        "SELECT ?s WHERE { ?s <http://ex/num> ?o FILTER(?o NOT IN (1, 2, 3)) }".to_string(),
        format!("SELECT ?o WHERE {{ {S} ?p ?o FILTER(SAMETERM(?o, \"Hello World\")) }}"),
        "SELECT ?s WHERE { ?s ?p ?o FILTER(BOUND(?o)) }".to_string(),
        "SELECT ?s WHERE { ?s <http://ex/has> ?o FILTER(ISBLANK(?o)) }".to_string(),
        format!("SELECT ?o WHERE {{ {S} <http://ex/str> ?o FILTER(REGEX(?o, \"hello\", \"i\")) }}"),
    ];
    check(&rete, &store, queries);
}

/// Assert every query in `queries` returns the same result set on rete and
/// Oxigraph, reporting all disagreements at once.
fn check(rete: &Rete, store: &Store, queries: &[String]) {
    let mut failures = Vec::new();
    for q in queries {
        let r = rete_rows(rete, q);
        let o = oxi_rows(store, q);
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

/// Property paths + aggregates over a chain/typed graph — the other two
/// least-covered SPARQL files (`path.rs`, `aggregate.rs`).
#[test]
fn paths_and_aggregates_agree_with_oxigraph() {
    let (rete, store) = engines();
    const K: &str = "<http://ex/knows>";
    const AGE: &str = "<http://ex/age>";
    const A: &str = "<http://ex/a>";
    let queries: &[String] = &[
        // --- property paths ---
        format!("SELECT ?y WHERE {{ {A} {K}+ ?y }}"), // transitive (one-or-more)
        format!("SELECT ?y WHERE {{ {A} {K}* ?y }}"), // zero-or-more (includes self)
        format!("SELECT ?y WHERE {{ {A} {K}? ?y }}"), // zero-or-one
        format!("SELECT ?y WHERE {{ {A} {K}/{K} ?y }}"), // sequence (2 hops)
        format!("SELECT ?x WHERE {{ <http://ex/c> ^{K} ?x }}"), // inverse
        format!("SELECT ?y WHERE {{ {A} ({K}|{AGE}) ?y }}"), // alternative
        format!("SELECT ?y WHERE {{ {A} {K}+/{AGE} ?y }}"), // path then property
        // --- aggregates ---
        "SELECT (COUNT(?p) AS ?n) WHERE { ?p <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> }".to_string(),
        format!("SELECT (SUM(?a) AS ?s) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT (AVG(?a) AS ?v) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT (MIN(?a) AS ?m) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT (MAX(?a) AS ?m) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT (COUNT(DISTINCT ?a) AS ?n) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT ?a (COUNT(?x) AS ?n) WHERE {{ ?x {AGE} ?a }} GROUP BY ?a"),
        format!("SELECT ?a (COUNT(?x) AS ?n) WHERE {{ ?x {AGE} ?a }} GROUP BY ?a HAVING(COUNT(?x) > 1)"),
        // more paths
        format!("SELECT ?y WHERE {{ {A} !{K} ?y }}"),          // negated property set
        format!("SELECT ?y WHERE {{ {A} ({K}/{K})? ?y }}"),    // optional nested sequence
        format!("SELECT ?x ?y WHERE {{ ?x ^{K} ?y }}"),        // inverse over all
        // more aggregates
        format!("SELECT (COUNT(*) AS ?n) WHERE {{ ?x {AGE} ?a }}"),
        // aggregate over an EXPRESSION (not a bare variable) — now supported
        format!("SELECT (SUM(?a * 2) AS ?s) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT (AVG(?a + 10) AS ?v) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT (MIN(?a * 2) AS ?m) WHERE {{ ?x {AGE} ?a }}"),
        format!("SELECT (MAX(?a * ?a) AS ?m) WHERE {{ ?x {AGE} ?a }}"),
        "SELECT ?a ?t (COUNT(?x) AS ?n) WHERE { ?x <http://ex/age> ?a ; <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?t } GROUP BY ?a ?t".to_string(),
    ];
    check(&rete, &store, queries);
}
