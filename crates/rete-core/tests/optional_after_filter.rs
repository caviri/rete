//! G33: a `FILTER` written before an `OPTIONAL` in the same group scopes over
//! the whole group, so it lowers to a filter ABOVE the left join. The lowering
//! now applies the conjuncts the left side decides on its own below the
//! OPTIONAL, so the OPTIONAL is evaluated for the rows that pass instead of for
//! every row scanned. The rows must be exactly the ones the filter-above plan
//! returns.
//!
//! The oracle is the same pattern wrapped in a sub-select, which lowers to a
//! `Subquery` the filter cannot move into: the left join runs whole, then the
//! filter sees its rows.

use rete_core::{
    eval_sparql, write_file_v05 as write_file, DictionaryBuilder, GraphIndexBuilder, Rete,
};

const P: &str = "PREFIX ex: <http://ex/> ";

/// 240 records: a title on each, a year on all but every 7th, a publisher on
/// every 3rd, a second year on every 11th (a multi-valued optional), tags.
fn data() -> Vec<u8> {
    let mut t: Vec<(String, String, String)> = Vec::new();
    for i in 0..240 {
        let s = format!("<http://ex/d{i:03}>");
        let word = ["alpha", "beta", "gamma", "permafrost"][i % 4];
        t.push((
            s.clone(),
            "<http://ex/title>".into(),
            format!("\"{word} record {i}\""),
        ));
        if i % 7 != 0 {
            t.push((
                s.clone(),
                "<http://ex/year>".into(),
                format!("\"{}\"", 1990 + i % 30),
            ));
        }
        if i % 11 == 0 {
            t.push((s.clone(), "<http://ex/year>".into(), "\"2031\"".into()));
        }
        if i % 3 == 0 {
            t.push((
                s.clone(),
                "<http://ex/publisher>".into(),
                format!("<http://ex/pub{}>", i % 5),
            ));
        }
        if i % 5 == 0 {
            t.push((s, "<http://ex/tag>".into(), "<http://ex/Tagged>".into()));
        }
    }
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &t {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new().with_tile_budget(64);
    for (s, p, o) in &t {
        ib.push(dict.encode(s, p, o).unwrap());
    }
    write_file(&dict, &ib.build(), false, &[], 0)
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    let (_, sols) = eval_sparql(rete, &format!("{P}{q}")).unwrap_or_else(|e| panic!("{q}: {e}"));
    let mut v: Vec<String> = sols.iter().map(|b| format!("{b:?}")).collect();
    v.sort();
    v
}

/// `group` is the pattern before the filter, `filter` the filter body.
fn check(rete: &Rete, vars: &str, group: &str, filter: &str, expect_some: bool) {
    let direct = format!("SELECT {vars} WHERE {{ {group} FILTER({filter}) }}");
    // `group` carries the OPTIONAL: the filter goes after it for the oracle.
    let oracle =
        format!("SELECT {vars} WHERE {{ {{ SELECT * WHERE {{ {group} }} }} FILTER({filter}) }}");
    let got = rows(rete, &direct);
    let want = rows(rete, &oracle);
    assert_eq!(got, want, "{direct}");
    assert_eq!(!got.is_empty(), expect_some, "{direct}: {} rows", got.len());
}

#[test]
fn a_filter_before_an_optional_returns_the_rows_of_the_filter_after_it() {
    let rete = Rete::open(&data()).unwrap();
    let left = "?d ex:title ?t .";
    let opt_year = "OPTIONAL { ?d ex:year ?y }";

    // The OPTIONAL after the filter, in the three positions a filter can take.
    // Pushed: the conjunct reads only ?d / ?t, bound by the left side.
    check(
        &rete,
        "*",
        &format!("{left} FILTER(CONTAINS(?t, \"permafrost\")) {opt_year}"),
        "true",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "CONTAINS(?t, \"permafrost\")",
        true,
    );
    // A row without a year (every 7th) survives; a multi-valued year repeats it.
    check(
        &rete,
        "?d ?t ?y",
        &format!("{left} {opt_year}"),
        "CONTAINS(?t, \"record 14\")",
        true,
    );
    // Stays above: reads ?y, which the OPTIONAL binds.
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "!BOUND(?y)",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "?y > \"2000\"",
        true,
    );
    // Mixed: one conjunct moves, one stays.
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "CONTAINS(?t, \"alpha\") && BOUND(?y)",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "CONTAINS(?t, \"alpha\") && (!BOUND(?y) || ?y > \"2010\")",
        true,
    );
    // A disjunction over the left's variables only, and one mixing in ?y.
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "CONTAINS(?t, \"beta\") || CONTAINS(?t, \"gamma\")",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "CONTAINS(?t, \"beta\") || BOUND(?y)",
        true,
    );
    // EXISTS reads the whole row: it must stay where it was.
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "EXISTS { ?d ex:tag ?g } && CONTAINS(?t, \"record\")",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "NOT EXISTS { ?d ex:publisher ?q }",
        true,
    );
    // A filter that errors on some rows (a number from a string) rejects them
    // the same way before and after.
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "?t + 1 > 0 || CONTAINS(?t, \"gamma\")",
        true,
    );
    // A filter nothing passes.
    check(
        &rete,
        "*",
        &format!("{left} {opt_year}"),
        "CONTAINS(?t, \"no such word\")",
        false,
    );

    // A multi-pattern OPTIONAL, one with its own filter, and a chain of two.
    check(
        &rete,
        "*",
        &format!("{left} OPTIONAL {{ ?d ex:year ?y . ?d ex:publisher ?p }}"),
        "CONTAINS(?t, \"permafrost\")",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} OPTIONAL {{ ?d ex:year ?y FILTER(?y > \"2005\") }}"),
        "CONTAINS(?t, \"alpha\")",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} {opt_year} OPTIONAL {{ ?d ex:publisher ?p }}"),
        "CONTAINS(?t, \"gamma\") && (!BOUND(?p) || ?y > \"2000\")",
        true,
    );
    check(
        &rete,
        "*",
        &format!("{left} OPTIONAL {{ ?d ex:publisher ?p }} OPTIONAL {{ ?d ex:year ?y }}"),
        "CONTAINS(?t, \"permafrost\")",
        true,
    );
    // The left side is a join, the filter on a variable of its second half.
    check(
        &rete,
        "*",
        &format!("{left} ?d ex:tag ?g . {opt_year}"),
        "CONTAINS(?t, \"record 1\") && ?g = ex:Tagged",
        true,
    );
    // The optional side shares no variable with the filter's.
    check(
        &rete,
        "*",
        &format!("{left} OPTIONAL {{ ?e ex:publisher ?p }}"),
        "CONTAINS(?t, \"permafrost\") && ?d = ex:d003",
        true,
    );
}

/// With a LIMIT the rows may differ in WHICH rows, never in how many or in
/// whether they are among the unlimited answer.
#[test]
fn a_limit_after_a_filter_and_an_optional_returns_rows_of_the_full_answer() {
    let rete = Rete::open(&data()).unwrap();
    let body = "?d ex:title ?t FILTER(CONTAINS(?t, \"permafrost\")) OPTIONAL { ?d ex:year ?y }";
    let all = rows(&rete, &format!("SELECT * WHERE {{ {body} }}"));
    for limit in [1, 5, 25, 200] {
        let got = rows(&rete, &format!("SELECT * WHERE {{ {body} }} LIMIT {limit}"));
        assert_eq!(got.len(), limit.min(all.len()), "LIMIT {limit}");
        assert!(got.iter().all(|r| all.contains(r)), "LIMIT {limit}");
    }
}
