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

/// rete's solutions with every term kept whole (datatype and language tag
/// included), in projection order, one `var=term` list per row, sorted unless
/// `ordered`. Unbound variables are left out, so a type error (unbound) and a
/// value never look alike.
fn rete_terms(rete: &Rete, q: &str, ordered: bool) -> Vec<String> {
    let (vars, sols) = match eval_sparql(rete, q) {
        Ok(r) => r,
        Err(e) => return vec![format!("RETE_ERR: {e}")],
    };
    let mut out: Vec<String> = sols
        .iter()
        .map(|b| {
            vars.iter()
                .filter_map(|v| b.get(v).map(|t| format!("{v}={t}")))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    if !ordered {
        out.sort();
    }
    out
}

/// Oxigraph's solutions in the same whole-term form.
fn oxi_terms(store: &Store, q: &str, ordered: bool) -> Vec<String> {
    let ev = match SparqlEvaluator::new().parse_query(q) {
        Ok(e) => e,
        Err(e) => return vec![format!("OXI_PARSE_ERR: {e}")],
    };
    let QueryResults::Solutions(sols) = ev.on_store(store).execute().unwrap() else {
        return vec!["OXI_NOT_SOLUTIONS".to_string()];
    };
    let vars: Vec<String> = sols
        .variables()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();
    let mut out = Vec::new();
    for s in sols {
        let s = s.unwrap();
        out.push(
            vars.iter()
                .filter_map(|v| s.get(v.as_str()).map(|t| format!("{v}={t}")))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if !ordered {
        out.sort();
    }
    out
}

/// `=`, `!=`, `<`, `<=`, `>`, `>=`, `IN`, ORDER BY, MIN / MAX and SUM / AVG
/// over the operand matrix of SPARQL 1.1 §17.3: simple literals vs
/// `xsd:string` vs the numeric types vs language-tagged literals vs unknown
/// datatypes, with NaN, INF, -0, leading zeros and exponents. Every value is
/// compared WHOLE (datatype included) and an error stays an unbound variable,
/// so this is an exact agreement check, not the lexical one above.
///
/// Where Oxigraph departs from the spec the pair is kept out of the columns
/// it gets wrong (and pinned by `rete-core`'s `sparql::value` unit tests
/// instead): Oxigraph makes `NaN < 1` an error (op:numeric-less-than says
/// false); `NaN <= NaN`, `<a> <= <a>` and `"x"^^ex:t <= "x"^^ex:t` true (the
/// first is false, the others type errors); `false < true` an error
/// (op:boolean-less-than says true); and a zoned and an unzoned date/time less
/// than 14 hours apart unequal, where the DAWG test `open-world/date-2`
/// requires an error (no implicit timezone, so the answer is indeterminate).
#[test]
fn comparison_matrix_agrees_with_oxigraph() {
    // Canonical lexical forms only in the DATA: Oxigraph stores numbers by
    // value (`"-3"^^xsd:int` comes back as `"-3"^^xsd:integer`, `"2.5E0"` as
    // `"2.5"`), rete keeps the term as written. Non-canonical forms are
    // exercised in the VALUES matrix, whose terms are never echoed.
    let nt = format!(
        r#"<http://ex/s1> <http://ex/label> "a"@EN .
<http://ex/s2> <http://ex/label> "a"@en .
<http://ex/s3> <http://ex/label> "a" .
<http://ex/s4> <http://ex/label> "b"@fr .
<http://ex/s1> <http://ex/v> "10" .
<http://ex/s2> <http://ex/v> "9" .
<http://ex/s3> <http://ex/v> "NaN" .
<http://ex/s4> <http://ex/v> "01" .
<http://ex/s1> <http://ex/n> "10"^^<{XSD}integer> .
<http://ex/s2> <http://ex/n> "9.5"^^<{XSD}decimal> .
<http://ex/s3> <http://ex/n> "2.5"^^<{XSD}double> .
<http://ex/s4> <http://ex/n> "-3"^^<{XSD}integer> .
<http://ex/s1> <http://ex/d> "2020-01-02"^^<{XSD}date> .
<http://ex/s2> <http://ex/d> "2019-12-31"^^<{XSD}date> .
<http://ex/s3> <http://ex/d> "2020-01-01"^^<{XSD}date> .
"#
    );
    let quads = ingest::parse_statements(&nt, "nt").expect("parse NT");
    let (image, _) = ingest::assemble_dataset(quads, &[]);
    let rete = Rete::open(&image).expect("rete open");
    let store = Store::new().unwrap();
    store
        .load_from_reader(RdfFormat::NTriples, nt.as_bytes())
        .unwrap();

    let p = format!("PREFIX xsd: <{XSD}> PREFIX ex: <http://ex/> ");
    // Pairs compared under every operator.
    let all = r#"
        ("s01" "Nan" "Nan") ("s02" "NaN" "NaN") ("s03" "nan" "nan") ("s04" "1.0" "1")
        ("s05" "01" "1") ("s06" "inf" "Infinity") ("s07" "abc" "abc") ("s08" "1e3" "1000")
        ("s09" "a" "b") ("s10" "10" "9") ("s11" "" "") ("s12" "x" "x"^^xsd:string)
        ("s13" "01"^^xsd:string "1")
        ("n01" 1 1.0) ("n02" 1 "01"^^xsd:integer) ("n03" 1 1e0) ("n04" "1"^^xsd:int 1)
        ("n05" 1e3 1000) ("n06" "-0"^^xsd:double "0"^^xsd:double) ("n07" "-0"^^xsd:integer 0.0)
        ("n08" "INF"^^xsd:double "INF"^^xsd:float) ("n09" "-INF"^^xsd:double "INF"^^xsd:double)
        ("n10" 9007199254740993 9007199254740992) ("n11" "0.1"^^xsd:float "0.1"^^xsd:double)
        ("n12" "0.5"^^xsd:float 0.5e0) ("n13" 1 1.5) ("n14" 2 1.5e0) ("n15" 0.30 0.3)
        ("n16" "+1"^^xsd:unsignedByte 1) ("n17" "1e3"^^xsd:double 999.9)
        ("x01" "1" 1) ("x02" "1" 1.0) ("x03" 1 "1"@en) ("x04" "1" <http://ex/1>)
        ("x05" "true"^^xsd:boolean "true")
        ("l01" "a"@en "a"@en) ("l02" "a"@en "a"@fr) ("l03" "a"@en "a") ("l04" "a"@en "b"@en)
        ("l05" "b"@en "a"@en)
        ("u02" "a"^^ex:t "b"^^ex:t) ("u03" "a"^^ex:t "a")
        ("u04" "a"^^ex:t <http://ex/a>) ("u05" "a"^^ex:t 1)
        ("u07" "abc"^^xsd:integer 1)
        ("u08" "1e3"^^xsd:integer 1000) ("u09" "a"^^ex:t "a"@en)
        ("i01" <http://ex/a> <http://ex/b>) ("i02" <http://ex/a> "http://ex/a")
        ("t01" "2020-01-01T00:00:00Z"^^xsd:dateTime "2020-01-01T01:00:00+01:00"^^xsd:dateTime)
        ("t03" "2020-01-01T00:00:00Z"^^xsd:dateTime "2020-01-02T15:00:00"^^xsd:dateTime)
        ("t04" "2020-01-01T00:00:00.50Z"^^xsd:dateTime "2020-01-01T00:00:00.5Z"^^xsd:dateTime)
        ("t05" "2019-12-31"^^xsd:date "2020-01-01"^^xsd:date) ("t06" "2020"^^xsd:gYear "2021"^^xsd:gYear)
        ("t07" "2020-01-01T00:00:00"^^xsd:dateTime "2020-01-01"^^xsd:date)
        ("t08" "PT60M"^^xsd:duration "PT1H"^^xsd:duration)
        ("t09" "P1M"^^xsd:duration "P30D"^^xsd:duration)
        ("t10" "P1M"^^xsd:duration "P32D"^^xsd:duration)
        ("t11" "P1D"^^xsd:dayTimeDuration "PT24H"^^xsd:dayTimeDuration)
        ("t12" "2020-01-01T00:00:00"^^xsd:dateTime "2020")
    "#;
    // Pairs whose ORDER Oxigraph gets differently from the spec: `=` / `!=` only.
    let eq_only = r#"
        ("e01" "NaN"^^xsd:double "NaN"^^xsd:double) ("e02" "NaN"^^xsd:double 1)
        ("e03" "NaN"^^xsd:float "NaN"^^xsd:double) ("e04" <http://ex/a> <http://ex/a>)
        ("e05" "true"^^xsd:boolean "1"^^xsd:boolean) ("e06" "false"^^xsd:boolean "true"^^xsd:boolean)
        ("e07" "a"^^ex:t "a"^^ex:t) ("e08" "abc"^^xsd:integer "abc"^^xsd:integer)
    "#;
    let queries: Vec<(String, bool)> = vec![
        (
            format!(
                "{p}SELECT ?id ?eq ?ne ?lt ?le ?gt ?ge WHERE {{ VALUES (?id ?a ?b) {{ {all} }} \
                 BIND(?a = ?b AS ?eq) BIND(?a != ?b AS ?ne) BIND(?a < ?b AS ?lt) \
                 BIND(?a <= ?b AS ?le) BIND(?a > ?b AS ?gt) BIND(?a >= ?b AS ?ge) }}"
            ),
            false,
        ),
        (
            format!(
                "{p}SELECT ?id ?eq ?ne WHERE {{ VALUES (?id ?a ?b) {{ {eq_only} }} \
                 BIND(?a = ?b AS ?eq) BIND(?a != ?b AS ?ne) }}"
            ),
            false,
        ),
        // FILTER drops false and error alike; `!` drops only false.
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(?v = \"10\") }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(?v = 10) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(?v != ?v) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(?v > 5) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(!(?v > 5)) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(?v > \"5\") }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:n ?n FILTER(?n > 5) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:n ?n FILTER(?n = \"10\") }}"), false),
        (
            format!("{p}SELECT ?s WHERE {{ ?s ex:d ?d FILTER(?d >= \"2020-01-01\"^^xsd:date) }}"),
            false,
        ),
        // Language tags compare case-insensitively (Oxigraph lower-cases on load).
        (format!("{p}SELECT ?s WHERE {{ ?s ex:label ?l FILTER(?l = \"a\"@en) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:label ?l FILTER(?l != \"a\"@en) }}"), false),
        // IN / NOT IN use `=`.
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(?v IN (10, \"9\")) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:v ?v FILTER(?v NOT IN (10, \"9\")) }}"), false),
        (format!("{p}SELECT ?s WHERE {{ ?s ex:n ?n FILTER(?n IN (\"10\", 9.5)) }}"), false),
        // ORDER BY within one kind (between kinds the order is
        // implementation-defined, §15.1): strings by code point, numbers by
        // value, dates by value, labels with and without tags interleaved.
        (format!("{p}SELECT ?v WHERE {{ ?s ex:v ?v }} ORDER BY ?v"), true),
        (format!("{p}SELECT ?v WHERE {{ ?s ex:v ?v }} ORDER BY DESC(?v)"), true),
        (format!("{p}SELECT ?n WHERE {{ ?s ex:n ?n }} ORDER BY ?n"), true),
        (format!("{p}SELECT ?d WHERE {{ ?s ex:d ?d }} ORDER BY DESC(?d)"), true),
        (
            format!("{p}SELECT ?x WHERE {{ VALUES ?x {{ \"b\" \"a\"@en \"c\"@fr \"a\" }} }} ORDER BY ?x"),
            true,
        ),
        (
            format!("{p}SELECT ?x WHERE {{ VALUES ?x {{ \"z\" <http://ex/b> <http://ex/a> }} }} ORDER BY ?x"),
            true,
        ),
        // MIN / MAX follow ORDER BY; SUM / AVG take numbers only.
        (format!("{p}SELECT (MIN(?v) AS ?lo) (MAX(?v) AS ?hi) WHERE {{ ?s ex:v ?v }}"), false),
        (format!("{p}SELECT (MIN(?n) AS ?lo) (MAX(?n) AS ?hi) WHERE {{ ?s ex:n ?n }}"), false),
        (format!("{p}SELECT (MIN(?d) AS ?lo) (MAX(?d) AS ?hi) WHERE {{ ?s ex:d ?d }}"), false),
        (format!("{p}SELECT (SUM(?v) AS ?sum) WHERE {{ ?s ex:v ?v }}"), false),
        (
            format!("{p}SELECT (AVG(?v) AS ?avg) WHERE {{ ?s ex:v ?v FILTER(?v != \"NaN\") }}"),
            false,
        ),
        // Numeric functions and arithmetic take numbers only.
        (format!("{p}SELECT ?s (ABS(?v) AS ?a) WHERE {{ ?s ex:v ?v }}"), false),
        (format!("{p}SELECT ?s (?v + 1 AS ?a) WHERE {{ ?s ex:v ?v }}"), false),
        (format!("{p}SELECT ?s (STR(?n + 1) AS ?a) WHERE {{ ?s ex:n ?n }}"), false),
    ];
    let mut failures = Vec::new();
    for (q, ordered) in &queries {
        let r = rete_terms(&rete, q, *ordered);
        let o = oxi_terms(&store, q, *ordered);
        if r != o {
            let rs: std::collections::BTreeSet<_> = r.iter().collect();
            let os: std::collections::BTreeSet<_> = o.iter().collect();
            let only_r: Vec<_> = rs.difference(&os).collect();
            let only_o: Vec<_> = os.difference(&rs).collect();
            failures.push(format!(
                "MISMATCH\n  q: {q}\n  rete only: {only_r:?}\n  oxi only:  {only_o:?}\n  rete: {r:?}\n  oxi:  {o:?}"
            ));
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
