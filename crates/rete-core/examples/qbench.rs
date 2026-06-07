//! Self-contained query micro-benchmark for the lazy/range-scan engine work.
//!
//! Generates a deterministic citation-shaped dataset in memory (the same shape
//! the Oxigraph comparison in docs/BENCHMARK.md uses — cito:cites, ex:discipline,
//! ex:citationCount, dct:*, foaf:name, ex:coauthor) and times every query in the
//! benchmark matrix on rete's engine. No external data, no Oxigraph — so it can
//! be run on two source revisions (before/after) to isolate an engine change.
//!
//!   cargo run --release --example qbench [num_papers]

use std::time::Instant;

use rete_core::{eval_query, DictionaryBuilder, GraphIndexBuilder, QueryOutput, Rete};

const DOI: &str = "<https://doi.org/10.1038/s41586-021-03819-2>";
const PREFIXES: &str = "PREFIX cito: <http://purl.org/spar/cito/> \
PREFIX dct: <http://purl.org/dc/terms/> PREFIX ex: <http://ex/> \
PREFIX prism: <http://prismstandard.org/namespaces/basic/2.0/> \
PREFIX foaf: <http://xmlns.com/foaf/0.1/> ";

const DISCIPLINES: &[&str] = &[
    "Physics",
    "Biology",
    "Chemistry",
    "Geology",
    "Medicine",
    "CS",
];
const SUBJECTS: &[&str] = &["protein", "genome", "quantum", "climate", "neuron"];
const XSD_INT: &str = "<http://www.w3.org/2001/XMLSchema#integer>";

/// Build a deterministic dataset of `papers` papers + their authors.
fn generate(papers: usize) -> Vec<(String, String, String)> {
    let authors = (papers / 5).max(1);
    let mut t: Vec<(String, String, String)> = Vec::new();
    let mut push =
        |s: &str, p: &str, o: &str| t.push((s.to_string(), p.to_string(), o.to_string()));

    for i in 0..papers {
        let paper = format!("<http://ex/paper/{i}>");
        // Two thirds of papers cite the AlphaFold DOI.
        if i % 3 != 0 {
            push(&paper, "<http://purl.org/spar/cito/cites>", DOI);
        }
        let disc = DISCIPLINES[i % DISCIPLINES.len()];
        push(
            &paper,
            "<http://ex/discipline>",
            &format!("<http://ex/discipline/{disc}>"),
        );
        push(
            &paper,
            "<http://ex/citationCount>",
            &format!("\"{}\"^^{XSD_INT}", i % 250),
        );
        push(
            &paper,
            "<http://prismstandard.org/namespaces/basic/2.0/publicationName>",
            &format!("\"Journal {}\"", i % 40),
        );
        push(
            &paper,
            "<http://purl.org/dc/terms/subject>",
            &format!("\"{}\"", SUBJECTS[i % SUBJECTS.len()]),
        );
        push(
            &paper,
            "<http://purl.org/dc/terms/title>",
            &format!("\"A study of {} number {i}\"", SUBJECTS[i % SUBJECTS.len()]),
        );
        push(
            &paper,
            "<http://purl.org/dc/terms/date>",
            &format!("\"20{:02}-01-01\"", 5 + (i % 15)),
        );
        let author = i % authors;
        push(
            &paper,
            "<http://purl.org/dc/terms/creator>",
            &format!("<http://ex/author/{author}>"),
        );
    }
    for a in 0..authors {
        push(
            &format!("<http://ex/author/{a}>"),
            "<http://xmlns.com/foaf/0.1/name>",
            &format!("\"Author {a}\""),
        );
        // A coauthorship ring so `coauthor+`/`coauthor*` and `^cites` have edges.
        for k in 1..=3 {
            push(
                &format!("<http://ex/author/{a}>"),
                "<http://ex/coauthor>",
                &format!("<http://ex/author/{}>", (a + k) % authors),
            );
        }
    }
    t
}

const QUERIES: &[(&str, &str)] = &[
    ("SELECT count (aggregate)", "SELECT (COUNT(?p) AS ?n) WHERE { ?p cito:cites <https://doi.org/10.1038/s41586-021-03819-2> }"),
    ("SELECT DISTINCT", "SELECT DISTINCT ?d WHERE { ?p ex:discipline ?d }"),
    ("ASK", "ASK { ?p ex:discipline <http://ex/discipline/Physics> }"),
    ("CONSTRUCT", "CONSTRUCT { ?a ex:coauthor ?b } WHERE { VALUES ?a { <http://ex/author/123> } ?a ex:coauthor ?b }"),
    ("DESCRIBE", "DESCRIBE <http://ex/author/123>"),
    ("VALUES", "SELECT ?p WHERE { VALUES ?d { <http://ex/discipline/Biology> <http://ex/discipline/Physics> } ?p ex:discipline ?d }"),
    ("UNION", "SELECT ?p WHERE { { ?p ex:discipline <http://ex/discipline/Biology> } UNION { ?p ex:discipline <http://ex/discipline/Chemistry> } }"),
    ("OPTIONAL", "SELECT ?p ?v WHERE { ?p ex:discipline <http://ex/discipline/Biology> OPTIONAL { ?p prism:publicationName ?v } } LIMIT 200"),
    ("MINUS", "SELECT ?p WHERE { ?p ex:discipline <http://ex/discipline/Biology> MINUS { ?p dct:subject \"protein\" } }"),
    ("FILTER NOT EXISTS", "SELECT ?p WHERE { ?p ex:discipline <http://ex/discipline/Biology> FILTER NOT EXISTS { ?p dct:subject \"protein\" } }"),
    ("3-way join + LIMIT", "SELECT ?name ?title WHERE { ?p dct:subject \"protein\" . ?p dct:title ?title . ?p dct:creator ?a . ?a foaf:name ?name } LIMIT 50"),
    ("FILTER REGEX + LIMIT", "SELECT ?p ?t WHERE { ?p dct:title ?t FILTER(REGEX(?t, \"genome\", \"i\")) } LIMIT 200"),
    ("FILTER arith + LIMIT", "SELECT ?p ?c WHERE { ?p ex:citationCount ?c FILTER(?c >= 100 && ?c <= 110) } LIMIT 200"),
    ("BIND + SUBSTR + LIMIT", "SELECT ?p ?label WHERE { ?p cito:cites <https://doi.org/10.1038/s41586-021-03819-2> . ?p dct:date ?y BIND(CONCAT(SUBSTR(?y,1,3), \"0s\") AS ?label) } LIMIT 200"),
    ("path a/b + LIMIT", "SELECT ?name WHERE { ?p cito:cites <https://doi.org/10.1038/s41586-021-03819-2> . ?p dct:creator/foaf:name ?name } LIMIT 200"),
    ("path inverse ^p (count)", "SELECT (COUNT(?x) AS ?n) WHERE { <https://doi.org/10.1038/s41586-021-03819-2> ^cito:cites ?x }"),
    ("path + transitive (count)", "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE { <http://ex/author/123> ex:coauthor+ ?o }"),
    ("path * (count)", "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE { <http://ex/author/123> ex:coauthor* ?o }"),
    ("GROUP BY + ORDER BY", "SELECT ?d (COUNT(?p) AS ?n) WHERE { ?p ex:discipline ?d } GROUP BY ?d ORDER BY DESC(?n)"),
    ("GROUP BY + HAVING", "SELECT ?d (COUNT(?p) AS ?n) WHERE { ?p ex:discipline ?d } GROUP BY ?d HAVING(COUNT(?p) > 100)"),
    ("AVG per group", "SELECT ?d (AVG(?c) AS ?avg) WHERE { ?p ex:discipline ?d . ?p ex:citationCount ?c } GROUP BY ?d ORDER BY DESC(?avg)"),
    ("MIN/MAX/SUM", "SELECT (MIN(?c) AS ?lo) (MAX(?c) AS ?hi) (SUM(?c) AS ?tot) WHERE { ?p ex:citationCount ?c }"),
    ("COUNT(DISTINCT)", "SELECT (COUNT(DISTINCT ?v) AS ?n) WHERE { ?p prism:publicationName ?v }"),
    ("ORDER BY + LIMIT + OFFSET", "SELECT ?p ?c WHERE { ?p ex:citationCount ?c } ORDER BY DESC(?c) LIMIT 10 OFFSET 50"),
];

fn rows(rete: &Rete, q: &str) -> usize {
    match eval_query(rete, q) {
        Ok(QueryOutput::Select(_, r)) => r.len(),
        Ok(QueryOutput::Ask(b)) => b as usize,
        Ok(QueryOutput::Construct(t)) => t.len(),
        Err(_) => usize::MAX,
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let papers: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000);
    let triples = generate(papers);

    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &triples {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new();
    for (s, p, o) in &triples {
        if let Some(t) = dict.encode(s, p, o) {
            ib.push(t);
        }
    }
    let bytes = rete_core::write_file(&dict, &ib.build(), false, &[], 0);
    let rete = Rete::open(&bytes).expect("open");

    eprintln!("dataset: {} triples ({papers} papers)\n", triples.len());
    println!("| Query | rows | median ms |");
    println!("|---|--:|--:|");
    for (name, body) in QUERIES {
        let q = format!("{PREFIXES}{body}");
        let n = rows(&rete, &q); // warm up + correctness anchor
        let reps = 7;
        let mut times = Vec::with_capacity(reps);
        for _ in 0..reps {
            let t = Instant::now();
            let _ = rows(&rete, &q);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("| {name} | {n} | {:.3} |", median(times));
    }
}
