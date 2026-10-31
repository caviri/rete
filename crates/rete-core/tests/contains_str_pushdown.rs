//! `FILTER(CONTAINS(STR(?o), …))` and the text index: the index holds the words
//! of literals only, but `STR` makes an IRI, a blank node or a quoted triple a
//! searchable string too. Before the fix the pushdown pruned with the index
//! anyway and dropped every row whose `?o` was not a literal (2 rows on a plain
//! file, 1 with a text index). Now a needle through `STR` uses the index only
//! where `?o` is provably a literal — the filter says so (`isLiteral`,
//! `LANG`, `DATATYPE`), or the pattern's predicate has literal objects only —
//! and every query here returns exactly the rows of the same file without a
//! text index.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::{
    eval_query, Header, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader,
    TextIndexGraphs, TextIndexScope,
};

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

fn quad(s: &str, p: &str, o: &str, g: Option<&str>) -> RawQuad {
    (iri(s), iri(p), o.to_string(), g.map(iri))
}

/// `label` has literal objects only; `mixed` has literals, IRIs and blank
/// nodes; `link` IRIs only; `note` literals in named graphs only. Several IRIs
/// and blank-node labels contain the words the literals contain.
fn quads() -> Vec<RawQuad> {
    vec![
        // The reproduction: one IRI and one literal object, both containing "Thing".
        quad("e1", "kind", "<http://ex/Thing>", None),
        quad("e2", "kind", "\"a Thing literal\"", None),
        quad("e1", "label", "\"Glucose metabolism\"@en", None),
        quad("e2", "label", "\"Benzene ring\"", None),
        quad(
            "e3",
            "label",
            "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            None,
        ),
        quad("e3", "mixed", "\"glucose in a literal\"", None),
        quad("e4", "mixed", "<http://ex/glucose/page>", None),
        quad("e5", "mixed", "_:glucose1", None),
        quad("e6", "mixed", "_:b7", None),
        quad("e7", "link", "<http://ex/benzene>", None),
        quad("e7", "link", "<http://ex/Thing>", None),
        quad("p1", "note", "\"created glucose record\"", Some("g1")),
        quad("p1", "mixed", "<http://ex/created/glucose>", Some("g1")),
        quad("p2", "note", "\"benzene modified\"@de", Some("g2")),
        quad("p2", "mixed", "_:created9", Some("g2")),
        quad("e1", "label", "\"Thing label in a graph\"", Some("g2")),
    ]
}

fn build(layout: GraphLayout, scope: Option<TextIndexScope>) -> Vec<u8> {
    build_quads(quads(), layout, scope)
}

fn build_quads(q: Vec<RawQuad>, layout: GraphLayout, scope: Option<TextIndexScope>) -> Vec<u8> {
    assemble_dataset_with_layout(
        q,
        false,
        scope,
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        layout,
        |_, _| Vec::<u8>::new(),
    )
    .0
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    let q = format!("PREFIX ex: <http://ex/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> {q}");
    match eval_query(rete, &q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

fn scope(graphs: TextIndexGraphs, exclude: &[&str]) -> TextIndexScope {
    let none: [&str; 0] = [];
    TextIndexScope::from_options(graphs, none, exclude.iter()).unwrap()
}

fn scopes() -> Vec<(&'static str, TextIndexScope)> {
    use TextIndexGraphs::{All, Default};
    vec![
        ("default", TextIndexScope::default()),
        ("all graphs", scope(All, &[])),
        ("exclude kind", scope(Default, &["http://ex/kind"])),
        ("exclude kind, all graphs", scope(All, &["http://ex/kind"])),
    ]
}

/// Records every range a lazy open reads.
#[derive(Clone)]
struct Recorded {
    data: Arc<Vec<u8>>,
    reads: Arc<Mutex<Vec<(u64, u64)>>>,
}

impl Recorded {
    fn new(data: Vec<u8>) -> Self {
        Recorded {
            data: Arc::new(data),
            reads: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn take(&self) -> Vec<(u64, u64)> {
        std::mem::take(&mut *self.reads.lock().unwrap())
    }
}

impl RangeReader for Recorded {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push((offset, len));
        SliceReader::new(&self.data).read_at(offset, len)
    }
}

#[test]
fn str_over_an_iri_object_keeps_its_row() {
    let q = "SELECT ?s WHERE { ?s ex:kind ?o FILTER(CONTAINS(STR(?o), \"Thing\")) }";
    for layout in [GraphLayout::PerGraph, GraphLayout::Quad] {
        let plain = Rete::open(&build(layout, None)).unwrap();
        let want = rows(&plain, q);
        assert_eq!(want.len(), 2, "the IRI <…Thing> and the literal: {want:?}");
        let bytes = build(layout, Some(TextIndexScope::default()));
        assert_eq!(
            rows(&Rete::open(&bytes).unwrap(), q),
            want,
            "{layout:?} resident"
        );
        let lazy = Rete::open_ranged_lazy_with(Recorded::new(bytes), rete_core::HEADER_LEN as u64).unwrap();
        assert_eq!(rows(&lazy, q), want, "{layout:?} lazy");
    }
}

/// Every STR shape, with and without a proof that `?o` is a literal, over IRI,
/// literal and blank-node objects, in the default graph, a `UNION` with
/// `GRAPH ?g`, and `GRAPH <g>`: always the plain file's rows.
#[test]
fn every_str_shape_answers_like_a_file_without_an_index() {
    let mut queries: Vec<String> = Vec::new();
    for w in [
        "glucose", "Thing", "thing", "benzene", "created", "b7", "ex/", "42",
    ] {
        let lw = w.to_lowercase();
        for (pat, extra) in [
            // A literal-only predicate: proven by the data.
            ("?s ex:label ?o", ""),
            // A mixed predicate: needs the filter's proof, else scans.
            ("?s ex:mixed ?o", ""),
            ("?s ex:mixed ?o", "FILTER(isLiteral(?o))"),
            ("?s ex:mixed ?o", "FILTER(LANG(?o) = \"\")"),
            ("?s ex:mixed ?o", "FILTER(DATATYPE(?o) = xsd:string)"),
            ("?s ex:link ?o", ""),
            ("?s ex:kind ?o", ""),
            // A variable predicate.
            ("?s ?p ?o", ""),
            ("?s ?p ?o", "FILTER(isLiteral(?o))"),
            ("?s ?p ?o", "FILTER(langMatches(LANG(?o), \"en\"))"),
            ("?s ?p ?o", "FILTER(isLiteral(?o) || isIRI(?o))"),
            ("?s ?p ?o", "FILTER(?p NOT IN (ex:kind))"),
            (
                "?s ?p ?o",
                "FILTER(isLiteral(?o)) FILTER(?p NOT IN (ex:kind))",
            ),
        ] {
            for hay in ["STR(?o)", "LCASE(STR(?o))", "UCASE(STR(?o))"] {
                let needle = if hay.starts_with("UCASE") {
                    w.to_uppercase()
                } else if hay.starts_with("LCASE") {
                    lw.clone()
                } else {
                    w.to_string()
                };
                let f = format!("FILTER(CONTAINS({hay}, \"{needle}\")) {extra}");
                queries.push(format!("SELECT ?s ?o WHERE {{ {pat} {f} }}"));
                queries.push(format!(
                    "SELECT ?s ?o ?g WHERE {{ {{ {pat} }} UNION {{ GRAPH ?g {{ {pat} }} }} {f} }}"
                ));
                queries.push(format!(
                    "SELECT ?s ?o WHERE {{ GRAPH ex:g1 {{ {pat} }} {f} }}"
                ));
            }
        }
        // Inside GRAPH, with the proof in a nested filter.
        queries.push(format!(
            "SELECT ?s ?o ?g WHERE {{ GRAPH ?g {{ ?s ?p ?o FILTER(isLiteral(?o)) }} FILTER(CONTAINS(LCASE(STR(?o)), \"{lw}\")) }}"
        ));
    }
    for layout in [GraphLayout::PerGraph, GraphLayout::Quad] {
        let plain = Rete::open(&build(layout, None)).unwrap();
        let expected: Vec<Vec<String>> = queries.iter().map(|q| rows(&plain, q)).collect();
        assert!(
            expected.iter().filter(|r| !r.is_empty()).count() > queries.len() / 3,
            "the fixture should answer many queries"
        );
        for (name, sc) in scopes() {
            let bytes = build(layout, Some(sc));
            let resident = Rete::open(&bytes).unwrap();
            let lazy = Rete::open_ranged_lazy_with(Recorded::new(bytes.clone()), rete_core::HEADER_LEN as u64).unwrap();
            for (q, want) in queries.iter().zip(&expected) {
                assert_eq!(&rows(&resident, q), want, "{layout:?} {name} resident: {q}");
                assert_eq!(&rows(&lazy, q), want, "{layout:?} {name} lazy: {q}");
            }
        }
    }
}

/// The byte range of the text index's postings (after the token table): a
/// pushdown reads a posting list, a declined one never does.
fn postings(bytes: &[u8]) -> (u64, u64) {
    let h = Header::from_bytes(bytes).unwrap();
    let (off, len) = h.text_index_payload();
    let tt = Rete::open(bytes)
        .unwrap()
        .text_index_token_table_len()
        .unwrap();
    (off + tt, len - tt)
}

/// Where `?o` is provably a literal the index is still used; where it is not,
/// the query scans.
#[test]
fn the_index_is_used_exactly_where_the_object_is_proven_literal() {
    for layout in [GraphLayout::PerGraph, GraphLayout::Quad] {
        let bytes = build(layout, Some(TextIndexScope::default()));
        let plain = Rete::open(&build(layout, None)).unwrap();
        let (off, len) = postings(&bytes);
        for (q, uses) in [
            // The data proves it: `label` has literal objects only.
            ("SELECT ?s WHERE { ?s ex:label ?o FILTER(CONTAINS(LCASE(STR(?o)), \"glucose\")) }", true),
            // The filter proves it.
            ("SELECT ?s WHERE { ?s ex:mixed ?o FILTER(isLiteral(?o)) FILTER(CONTAINS(STR(?o), \"glucose\")) }", true),
            ("SELECT ?s WHERE { ?s ?p ?o FILTER(isLiteral(?o) && CONTAINS(LCASE(STR(?o)), \"glucose\")) }", true),
            ("SELECT ?s WHERE { ?s ex:mixed ?o FILTER(LANG(?o) != \"de\") FILTER(CONTAINS(STR(?o), \"glucose\")) }", true),
            // No STR: an IRI is a type error for CONTAINS, nothing to prove.
            ("SELECT ?s WHERE { ?s ex:mixed ?o FILTER(CONTAINS(?o, \"glucose\")) }", true),
            // Nothing proves it: an IRI or a blank node can match.
            ("SELECT ?s WHERE { ?s ex:mixed ?o FILTER(CONTAINS(STR(?o), \"glucose\")) }", false),
            ("SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(LCASE(STR(?o)), \"glucose\")) }", false),
            ("SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(STR(?o), \"glucose\") && (isLiteral(?o) || isBlank(?o))) }", false),
        ] {
            let rec = Recorded::new(bytes.clone());
            let rete = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
            rec.take();
            let got = rows(&rete, q);
            assert_eq!(got, rows(&plain, q), "{layout:?}: {q}");
            assert!(!got.is_empty(), "{q}");
            let touched = rec
                .take()
                .iter()
                .any(|&(o, l)| o < off + len && off < o + l);
            assert_eq!(touched, uses, "{layout:?} index used = {uses}: {q}");
        }
    }
}

/// A deterministic xorshift, so the needles are the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A small graph mixing literals, IRIs and blank nodes under shared words, and
/// random needles cut from its terms (and a few from nowhere): the pushdown
/// and the scan agree on every one, for every shape and scope.
#[test]
fn pushdown_and_scan_agree_on_random_needles() {
    let words = [
        "alpha",
        "beta",
        "gamma",
        "delta",
        "glucose",
        "thing",
        "x1",
        "Ünïcode",
    ];
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut q: Vec<RawQuad> = Vec::new();
    for i in 0..120 {
        let w1 = words[rng.below(words.len())];
        let w2 = words[rng.below(words.len())];
        let s = format!("s{i}");
        let obj = match rng.below(5) {
            0 => format!("\"{w1} {w2} text {i}\""),
            1 => format!("\"{w1}-{w2}\"@en"),
            2 => format!("<http://ex/{w1}/{w2}/{i}>"),
            3 => format!("_:{w1}{i}"),
            _ => format!("\"{i}\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
        };
        let p = ["p", "q", "lit"][rng.below(3)];
        // `lit` keeps literal objects only, so the data proves it.
        let obj = if p == "lit" && !obj.starts_with('"') {
            format!("\"{w1} {i}\"")
        } else {
            obj
        };
        let g = match rng.below(4) {
            0 => Some(format!("g{}", rng.below(3))),
            _ => None,
        };
        q.push(quad(&s, p, &obj, g.as_deref()));
    }
    let mut needles: Vec<String> = Vec::new();
    for _ in 0..60 {
        let (_, _, o, _) = &q[rng.below(q.len())];
        let chars: Vec<char> = o.chars().collect();
        let a = rng.below(chars.len());
        let b = (a + 1 + rng.below(6)).min(chars.len());
        let cut: String = chars[a..b]
            .iter()
            .filter(|c| **c != '"' && **c != '\\')
            .collect();
        if !cut.is_empty() {
            needles.push(cut);
        }
    }
    needles.extend(["zzz", "ex/", "_:", "http", "1"].map(String::from));

    let shapes = |n: &str| -> Vec<String> {
        let n = n.replace('\\', "\\\\");
        vec![
            format!("SELECT ?s ?o WHERE {{ ?s ex:lit ?o FILTER(CONTAINS(STR(?o), \"{n}\")) }}"),
            format!("SELECT ?s ?o WHERE {{ ?s ex:p ?o FILTER(CONTAINS(STR(?o), \"{n}\")) }}"),
            format!("SELECT ?s ?o WHERE {{ ?s ?p ?o FILTER(CONTAINS(LCASE(STR(?o)), \"{}\")) }}", n.to_lowercase()),
            format!("SELECT ?s ?o WHERE {{ ?s ?p ?o FILTER(isLiteral(?o)) FILTER(CONTAINS(STR(?o), \"{n}\")) }}"),
            format!("SELECT ?s ?o WHERE {{ ?s ?p ?o FILTER(CONTAINS(?o, \"{n}\")) }}"),
            format!("SELECT ?s ?o ?g WHERE {{ {{ ?s ?p ?o }} UNION {{ GRAPH ?g {{ ?s ?p ?o }} }} FILTER(CONTAINS(STR(?o), \"{n}\")) }}"),
            format!("SELECT ?s ?o ?g WHERE {{ {{ ?s ex:lit ?o }} UNION {{ GRAPH ?g {{ ?s ex:lit ?o }} }} FILTER(CONTAINS(STR(?o), \"{n}\")) }}"),
        ]
    };
    for layout in [GraphLayout::PerGraph, GraphLayout::Quad] {
        let plain = Rete::open(&build_quads(q.clone(), layout, None)).unwrap();
        for (name, sc) in [
            ("default", TextIndexScope::default()),
            ("all graphs", scope(TextIndexGraphs::All, &[])),
            (
                "exclude q, all graphs",
                scope(TextIndexGraphs::All, &["http://ex/q"]),
            ),
        ] {
            let bytes = build_quads(q.clone(), layout, Some(sc));
            let resident = Rete::open(&bytes).unwrap();
            let lazy = Rete::open_ranged_lazy_with(Recorded::new(bytes.clone()), rete_core::HEADER_LEN as u64).unwrap();
            for n in &needles {
                for query in shapes(n) {
                    let want = rows(&plain, &query);
                    assert_eq!(
                        rows(&resident, &query),
                        want,
                        "{layout:?} {name} resident: {query}"
                    );
                    assert_eq!(rows(&lazy, &query), want, "{layout:?} {name} lazy: {query}");
                }
            }
        }
    }
}

/// The literal object ids found by two binary searches per section, on a lazy
/// open of a dictionary of many chunks, are exactly the objects whose term is a
/// literal.
#[test]
fn literal_object_ranges_match_every_term() {
    let mut q: Vec<RawQuad> = Vec::new();
    for i in 0..6000 {
        q.push(quad(
            &format!("s{i}"),
            "p",
            &format!("\"literal number {i:05} with padding to fill chunks\""),
            None,
        ));
        q.push(quad(
            &format!("s{i}"),
            "q",
            &format!("<http://ex/object/{i:05}/with/a/long/iri>"),
            None,
        ));
        q.push(quad(&format!("s{i}"), "r", &format!("_:blank{i}"), None));
        // Shared terms: subjects that are objects too.
        q.push(quad(
            &format!("s{i}"),
            "next",
            &iri(&format!("s{}", (i + 1) % 6000)),
            None,
        ));
    }
    let bytes = build_quads(q, GraphLayout::PerGraph, None);
    let lazy = Rete::open_ranged_lazy_with(Recorded::new(bytes.clone()), rete_core::HEADER_LEN as u64).unwrap();
    let dict = lazy.dictionary();
    let lits = dict.literal_objects().expect("readable");
    let n_objects = dict.shared_count() + dict.object_only_count();
    let mut literals = 0;
    for o in 1..=n_objects {
        let term = dict.object_term(o).unwrap();
        assert_eq!(
            lits.contains(o),
            term.starts_with('"'),
            "object {o}: {term}"
        );
        literals += usize::from(term.starts_with('"'));
    }
    assert_eq!(literals, 6000);
    // The resident open agrees.
    assert_eq!(
        Rete::open(&bytes).unwrap().dictionary().literal_objects(),
        Some(lits)
    );
}
