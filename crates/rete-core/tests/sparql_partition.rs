//! Running one query as chunks split by a variable (`partition_plan` +
//! `eval_query_partitioned`): the chunks' rows together must be exactly the
//! query's rows, and every split that could not promise that is refused.

use rete_core::{
    eval_query, eval_query_partitioned, partition_plan, write_file, DictionaryBuilder,
    GraphIndexBuilder, PartitionPlan, QueryOpts, QueryOutput, Rete, SparqlError,
};

const P: &str = "PREFIX : <http://ex.org/> ";

/// 30 works: every third cites :X, every third :Y; each has a title and an
/// author (one of four); only the even ones have a year.
fn graph() -> Vec<u8> {
    let mut triples: Vec<(String, String, String)> = Vec::new();
    let ex = |s: &str| format!("<http://ex.org/{s}>");
    for i in 0..30 {
        let w = ex(&format!("w{i}"));
        let t = |p: &str, o: String| (w.clone(), ex(p), o);
        triples.push((
            w.clone(),
            "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".into(),
            ex("Work"),
        ));
        triples.push(t("title", format!("\"Title {i}\"")));
        triples.push(t("author", ex(&format!("a{}", i % 4))));
        match i % 3 {
            0 => triples.push(t("cites", ex("X"))),
            1 => triples.push(t("cites", ex("Y"))),
            _ => {}
        }
        if i % 2 == 0 {
            triples.push(t(
                "year",
                format!(
                    "\"{}\"^^<http://www.w3.org/2001/XMLSchema#integer>",
                    1990 + i
                ),
            ));
        }
    }
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &triples {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new();
    for (s, p, o) in &triples {
        ib.push(dict.encode(s, p, o).unwrap());
    }
    write_file(&dict, &ib.build(), false, &[], 0)
}

fn rows(out: QueryOutput) -> Vec<String> {
    match out {
        QueryOutput::Select(_, rows) => rows.into_iter().map(|r| format!("{r:?}")).collect(),
        QueryOutput::Construct(triples) => triples.into_iter().map(|t| format!("{t:?}")).collect(),
        _ => panic!("not a SELECT or CONSTRUCT"),
    }
}

fn keys(rete: &Rete, plan: &PartitionPlan) -> Vec<String> {
    match eval_query(rete, &plan.keys_query).unwrap() {
        QueryOutput::Select(_, rows) => rows.into_iter().map(|r| r[&plan.var].clone()).collect(),
        _ => panic!("the keys query is a SELECT"),
    }
}

/// The query run as chunks of `size` keys, merged the way a caller must:
/// in key order, deduplicated when the plan says so, then OFFSET/LIMIT.
fn chunked(rete: &Rete, q: &str, var: &str, size: usize) -> (PartitionPlan, Vec<String>) {
    let plan = partition_plan(rete, q, var).unwrap();
    let mut out: Vec<String> = Vec::new();
    for chunk in keys(rete, &plan).chunks(size) {
        let (o, _) = eval_query_partitioned(rete, q, QueryOpts::default(), var, chunk).unwrap();
        for row in rows(o) {
            if !plan.dedupe || !out.contains(&row) {
                out.push(row);
            }
        }
    }
    let out = out
        .into_iter()
        .skip(plan.offset)
        .take(plan.limit.unwrap_or(usize::MAX))
        .collect();
    (plan, out)
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn chunks_together_are_the_query() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    for (q, var) in [
        ("SELECT ?w ?t WHERE { ?w :cites :X ; :title ?t }", "w"),
        // OPTIONAL details: rows without a year stay.
        (
            "SELECT ?w ?t ?y WHERE { ?w :cites :X ; :title ?t OPTIONAL { ?w :year ?y } }",
            "?w",
        ),
        // FILTER and BIND are per row.
        (
            "SELECT ?w ?u WHERE { ?w a :Work ; :title ?t BIND(UCASE(?t) AS ?u) FILTER(?w != :w3) }",
            "w",
        ),
        // Grouped by the split variable: each group lies in one chunk.
        (
            "SELECT ?w (COUNT(?a) AS ?n) WHERE { ?w :author ?a ; :cites ?c } GROUP BY ?w",
            "w",
        ),
        // A bag: no DISTINCT, duplicates kept exactly.
        ("SELECT ?a WHERE { ?w :cites ?c ; :author ?a }", "w"),
        // No pattern binds ?a with constants alone: keys from the WHERE clause.
        ("SELECT ?w ?a WHERE { ?w :author ?a }", "a"),
        // A query-level VALUES clause.
        ("SELECT ?w ?c WHERE { ?w :cites ?c } VALUES ?c { :X }", "w"),
    ] {
        let q = format!("{P}{q}");
        let whole = sorted(rows(eval_query(&rete, &q).unwrap()));
        for size in [1, 4, 100] {
            let (_, parts) = chunked(&rete, &q, var, size);
            assert_eq!(sorted(parts), whole, "{q} in chunks of {size}");
        }
    }
}

#[test]
fn distinct_without_the_variable_and_construct_are_deduplicated() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    // Two works can share an author: the same ?a comes from several chunks.
    let q = format!("{P}SELECT DISTINCT ?a WHERE {{ ?w :cites :X ; :author ?a }}");
    let (plan, parts) = chunked(&rete, &q, "w", 2);
    assert!(plan.dedupe);
    assert_eq!(sorted(parts), sorted(rows(eval_query(&rete, &q).unwrap())));

    // Projecting the variable keeps rows of different chunks apart.
    let q = format!("{P}SELECT DISTINCT ?w ?a WHERE {{ ?w :cites :X ; :author ?a }}");
    assert!(!partition_plan(&rete, &q, "w").unwrap().dedupe);

    let q = format!("{P}CONSTRUCT {{ :all :cited ?c }} WHERE {{ ?w :cites ?c }}");
    let (plan, parts) = chunked(&rete, &q, "w", 3);
    assert!(plan.dedupe);
    assert_eq!(plan.form, "construct");
    assert_eq!(sorted(parts), sorted(rows(eval_query(&rete, &q).unwrap())));
}

#[test]
fn limit_and_offset_apply_once_to_the_merged_rows() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    let q = format!("{P}SELECT ?w WHERE {{ ?w a :Work }} LIMIT 5 OFFSET 2");
    let (plan, parts) = chunked(&rete, &q, "w", 3);
    assert_eq!((plan.offset, plan.limit), (2, Some(5)));
    assert_eq!(parts.len(), 5);
    // A chunk never applies them itself: all of its keys' rows come back.
    let all = keys(&rete, &plan);
    let (one, _) =
        eval_query_partitioned(&rete, &q, QueryOpts::default(), "w", &all[..10]).unwrap();
    assert_eq!(rows(one).len(), 10);
}

#[test]
fn the_keys_query_is_one_constant_pattern() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    let q = format!("{P}SELECT ?w ?t WHERE {{ ?w :title ?t ; :cites :X }}");
    let plan = partition_plan(&rete, &q, "w").unwrap();
    assert_eq!(plan.keys_from, "pattern");
    assert!(
        plan.keys_query.contains("<http://ex.org/cites>"),
        "{}",
        plan.keys_query
    );
    assert!(!plan.keys_query.contains("title"), "{}", plan.keys_query);
    assert_eq!(keys(&rete, &plan).len(), 10);

    // Patterns with another variable, under OPTIONAL, or inside a sub-SELECT
    // never become the keys query.
    let q = format!(
        "{P}SELECT ?w WHERE {{ ?w :title ?t OPTIONAL {{ ?w :cites :X }} \
         {{ SELECT ?w WHERE {{ ?w :cites :Y }} }} }}"
    );
    let plan = partition_plan(&rete, &q, "w").unwrap();
    assert_eq!(plan.keys_from, "where", "{}", plan.keys_query);
}

#[test]
fn a_values_list_on_the_variable_is_the_keys() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    // A list of entities found elsewhere (another graph), the details asked
    // here: the keys are the list, read from nowhere, and :w99 — not in this
    // graph — is a key with an empty chunk.
    for q in [
        "SELECT ?w ?t ?y WHERE { VALUES ?w { :w0 :w3 :w6 :w7 :w99 } ?w :title ?t OPTIONAL { ?w :year ?y } }",
        // At the end of the query, binding two variables per row.
        "SELECT ?w ?c ?t WHERE { ?w :title ?t ; :cites ?c } VALUES (?w ?c) { (:w0 :X) (:w1 :Y) (:w3 :X) (:w3 :Y) }",
    ] {
        let q = format!("{P}{q}");
        let plan = partition_plan(&rete, &q, "w").unwrap();
        assert_eq!(plan.keys_from, "values", "{q}");
        assert!(!plan.keys_query.contains("title"), "{}", plan.keys_query);
        let whole = sorted(rows(eval_query(&rete, &q).unwrap()));
        for size in [1, 2, 100] {
            let (_, parts) = chunked(&rete, &q, "w", size);
            assert_eq!(sorted(parts), whole, "{q} in chunks of {size}");
        }
    }
    let q =
        format!("{P}SELECT ?w ?t WHERE {{ VALUES ?w {{ :w0 :w3 :w6 :w7 :w99 }} ?w :title ?t }}");
    assert_eq!(
        keys(&rete, &partition_plan(&rete, &q, "w").unwrap()).len(),
        5
    );

    // Keys from elsewhere that the list lacks still answer exactly: only the
    // ones the list holds have rows.
    let other = [
        "<http://ex.org/w0>".to_string(),
        "<http://ex.org/w1>".to_string(),
    ];
    let (out, _) = eval_query_partitioned(&rete, &q, QueryOpts::default(), "w", &other).unwrap();
    assert_eq!(rows(out).len(), 1);

    // An UNDEF in the list: not every row binds ?w, so the list is not the keys.
    let q = format!(
        "{P}SELECT ?w ?c WHERE {{ ?w :cites ?c }} VALUES (?w ?c) {{ (:w0 UNDEF) (UNDEF :Y) }}"
    );
    assert_ne!(partition_plan(&rete, &q, "w").unwrap().keys_from, "values");
}

#[test]
fn splits_that_would_change_the_answer_are_refused() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    for (q, var, why) in [
        ("ASK { ?w :cites :X }", "w", "one boolean"),
        ("DESCRIBE ?w WHERE { ?w :cites :X }", "w", "DESCRIBE"),
        (
            "SELECT ?w WHERE { ?w :cites :X } ORDER BY ?w",
            "w",
            "ORDER BY",
        ),
        (
            "SELECT (COUNT(?w) AS ?n) WHERE { ?w :cites :X }",
            "w",
            "aggregate",
        ),
        (
            "SELECT ?a (COUNT(?w) AS ?n) WHERE { ?w :author ?a } GROUP BY ?a",
            "w",
            "aggregate",
        ),
        (
            "SELECT ?w ?y WHERE { ?w a :Work OPTIONAL { ?w :year ?y } }",
            "y",
            "unbound",
        ),
        (
            "SELECT ?w ?c WHERE { { ?w :cites ?c } UNION { ?w :title ?t } }",
            "c",
            "unbound",
        ),
        (
            "SELECT ?n WHERE { { SELECT (COUNT(?w) AS ?n) WHERE { ?w a :Work } } }",
            "w",
            "unbound",
        ),
        (
            "SELECT ?w (STR(?w) AS ?s) WHERE { ?w a :Work }",
            "s",
            "select expression",
        ),
        (
            "SELECT ?w WHERE { ?w a :Work }",
            "not a var",
            "not a variable name",
        ),
    ] {
        let q = format!("{P}{q}");
        match partition_plan(&rete, &q, var) {
            Err(SparqlError::Partition(m)) => assert!(m.contains(why), "{q}: {m}"),
            other => panic!("{q}: expected a refusal, got {other:?}"),
        }
    }
    // A chunk is never run on a query its plan refuses.
    let q = format!("{P}SELECT ?w WHERE {{ ?w :cites :X }} ORDER BY ?w");
    let keys = ["<http://ex.org/w0>".to_string()];
    assert!(matches!(
        eval_query_partitioned(&rete, &q, QueryOpts::default(), "w", &keys),
        Err(SparqlError::Partition(_))
    ));
}
