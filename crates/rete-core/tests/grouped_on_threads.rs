//! A grouped query folded on several threads (`sparql::par_agg`) answers
//! exactly as the single fold does.
//!
//! The fixture has the shape that made it matter: thousands of small named
//! graphs (three per publication), cut into 1 KiB tiles so that the scan that
//! drives a pattern spans many of them. Every query is evaluated with one
//! thread and with four, and the two answers must be equal, row for row and in
//! the same order. `sharded_queries` says which path a query took, so a query
//! this is meant to split cannot quietly stop being split, and one that must
//! not be split is seen to stay whole.
//!
//! Only meaningful with the `parallel` feature (`cargo test --workspace`
//! turns it on through the CLI); without it both runs take the single fold
//! and the test still checks the lowering change (a FILTER moved below the
//! BINDs it does not read).

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::query::{eval_query, set_query_threads, sharded_queries, QueryOutput};
use rete_core::write_opts::{set_write_options, write_options};
use rete_core::{PermSet, PyramidAlgo, Rete};

const N: usize = 1_500;
const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#integer";

fn iri(s: &str) -> String {
    format!("<{s}>")
}

/// `N` publications on three hosts. Each has a head graph (its type), an
/// assertion graph (a score, a relation), and a provenance graph (an evidence
/// grade, and for two publications in three the grade's label). A few triples
/// sit in the default graph.
fn fixture() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    for i in 0..N {
        let host = ["alpha.example", "beta.example", "gamma.example"][i % 3];
        let base = format!("http://{host}/np/{i:05}");
        let np = iri(&base);
        let head = Some(iri(&format!("{base}#Head")));
        let assertion = iri(&format!("{base}#assertion"));
        let prov = iri(&format!("{base}#provenance"));
        q.push((
            np.clone(),
            iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            iri("http://ex/Nanopublication"),
            head.clone(),
        ));
        q.push((
            np.clone(),
            iri("http://ex/hasAssertion"),
            assertion.clone(),
            head,
        ));
        let ga = Some(assertion.clone());
        q.push((
            iri(&format!("http://ex/e/{}", (i * 7) % 500)),
            iri("http://ex/score"),
            format!("\"{}\"^^<{XSD_INT}>", i % 41),
            ga.clone(),
        ));
        q.push((
            iri(&format!("http://ex/e/{}", (i * 7) % 500)),
            iri(&format!("http://ex/rel{}", i % 4)),
            iri(&format!("http://ex/o/{}", (i * 13) % 997)),
            ga,
        ));
        let gp = Some(prov);
        let grade = iri(&format!("http://ex/grade/{}", i % 5));
        q.push((
            assertion.clone(),
            iri("http://ex/evidence"),
            grade.clone(),
            gp.clone(),
        ));
        if i % 3 != 0 {
            q.push((
                grade,
                iri("http://www.w3.org/2000/01/rdf-schema#label"),
                format!("\"grade {}\"", i % 5),
                gp.clone(),
            ));
        }
        q.push((
            assertion,
            iri("http://ex/source"),
            iri(&format!(
                "http://{}/v{}/src#{}",
                ["rdf.one.example", "rdf.two.example"][i % 2],
                i % 3,
                i % 11
            )),
            gp,
        ));
        if i % 50 == 0 {
            q.push((
                np,
                iri("http://ex/featured"),
                format!("\"{}\"^^<{XSD_INT}>", i / 50),
                None,
            ));
        }
    }
    q
}

fn image(layout: GraphLayout) -> Vec<u8> {
    let mut o = write_options();
    o.tile_budget = 1024;
    set_write_options(o);
    assemble_dataset_with_layout(
        fixture(),
        false,
        false,
        None,
        PyramidAlgo::Louvain,
        PermSet::CORE,
        layout,
        |_, _| Vec::<u8>::new(),
    )
    .0
}

fn file() -> Rete {
    Rete::open(&image(GraphLayout::Quad)).unwrap()
}

/// A reader whose reads count as round trips (it does not say it is local).
struct Far(Vec<u8>);

impl rete_core::RangeReader for Far {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let (a, b) = (offset as usize, (offset + len) as usize);
        self.0
            .get(a..b)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::other("read past the end"))
    }
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(vars, rows) => std::iter::once(format!("{vars:?}"))
            .chain(rows.into_iter().map(|b| format!("{b:?}")))
            .collect(),
        other => panic!("unexpected output {other:?}"),
    }
}

const P: &str = "PREFIX ex: <http://ex/> \
                 PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ";

/// `(query, split)`: `split` says the query is one `par_agg` folds on threads.
fn queries() -> Vec<(&'static str, bool)> {
    vec![
        // A BIND over every row of a scan under GRAPH ?g, grouped by it.
        (
            "SELECT ?ns (COUNT(?np) AS ?n) WHERE { GRAPH ?h { ?np a ex:Nanopublication } \
             BIND(REPLACE(STR(?np), \"[^/]*$\", \"\") AS ?ns) } GROUP BY ?ns ORDER BY DESC(?n)",
            true,
        ),
        // A FILTER and two BINDs; the filter reads neither of them.
        (
            "SELECT ?release ?src (COUNT(*) AS ?n) WHERE { GRAPH ?pv { ?x ex:source ?s } \
             FILTER(STRSTARTS(STR(?s), \"http://rdf.one.example/v\")) \
             BIND(STRBEFORE(STRAFTER(STR(?s), \"http://rdf.one.example/\"), \"/\") AS ?release) \
             BIND(REPLACE(STR(?s), \"^.*[/#]\", \"\") AS ?src) } \
             GROUP BY ?release ?src ORDER BY ?release DESC(?n) ?src",
            true,
        ),
        // A FILTER that reads a BIND's variable stays above it.
        (
            "SELECT ?host (COUNT(*) AS ?n) WHERE { GRAPH ?h { ?np a ex:Nanopublication } \
             BIND(STRBEFORE(STRAFTER(STR(?np), \"//\"), \"/\") AS ?host) \
             FILTER(?host != \"beta.example\") } GROUP BY ?host ORDER BY ?host",
            true,
        ),
        // The graph of each row looked up for an OPTIONAL.
        (
            "SELECT ?e ?label (COUNT(*) AS ?n) WHERE { GRAPH ?pv { ?x ex:evidence ?e } \
             OPTIONAL { GRAPH ?pv { ?e rdfs:label ?label } } } \
             GROUP BY ?e ?label ORDER BY ?e ?label",
            true,
        ),
        // The same as a required join.
        (
            "SELECT ?label (COUNT(*) AS ?n) WHERE { GRAPH ?pv { ?x ex:evidence ?e } \
             GRAPH ?pv { ?e rdfs:label ?label } } GROUP BY ?label ORDER BY ?label",
            true,
        ),
        // UNION: both branches are scans.
        (
            "SELECT ?via (COUNT(*) AS ?n) (SUM(IF(?via = \"score\", 1, 0)) AS ?scores) WHERE { \
             { GRAPH ?g { ?s ex:score ?v } BIND(\"score\" AS ?via) } UNION \
             { GRAPH ?g { ?s ex:rel0 ?v } BIND(\"rel\" AS ?via) } } GROUP BY ?via ORDER BY ?via",
            true,
        ),
        // UNION with a branch that cannot be split (two patterns): slice 0 runs it.
        (
            "SELECT (COUNT(*) AS ?n) WHERE { { GRAPH ?g { ?s ex:score ?v } } UNION \
             { ?np ex:featured ?k . ?np ex:featured ?k2 } }",
            true,
        ),
        // Every mergeable aggregate, on numbers and on strings.
        (
            "SELECT ?s (COUNT(?v) AS ?n) (SUM(?v) AS ?sum) (AVG(?v) AS ?avg) (MIN(?v) AS ?min) \
             (MAX(?v) AS ?max) (SAMPLE(?v) AS ?any) WHERE { GRAPH ?g { ?s ex:score ?v } } \
             GROUP BY ?s ORDER BY ?s",
            true,
        ),
        (
            "SELECT ?e (GROUP_CONCAT(?l; SEPARATOR=\"|\") AS ?all) (MIN(?l) AS ?lo) WHERE { \
             GRAPH ?pv { ?e rdfs:label ?l } } GROUP BY ?e ORDER BY ?e",
            true,
        ),
        // An aggregate over an expression, HAVING, and a slice.
        (
            "SELECT ?s (SUM(?v * 2) AS ?twice) WHERE { GRAPH ?g { ?s ex:score ?v } } \
             GROUP BY ?s HAVING (SUM(?v * 2) > 100) ORDER BY DESC(?twice) ?s LIMIT 20",
            true,
        ),
        // No GROUP BY: one group, also when no row matches.
        (
            "SELECT (COUNT(*) AS ?n) (MAX(?v) AS ?m) WHERE { GRAPH ?g { ?s ex:score ?v } }",
            true,
        ),
        (
            "SELECT (COUNT(*) AS ?n) (SUM(?v) AS ?m) WHERE { GRAPH ?g { ?s ex:score ?v } \
             FILTER(?v > 1000) }",
            true,
        ),
        // A type error in an aggregated expression (an IRI is not a number).
        (
            "SELECT (SUM(?o) AS ?bad) (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ex:rel1 ?o } }",
            true,
        ),
        // A subquery that groups, under an outer pattern.
        (
            "SELECT ?n (COUNT(*) AS ?hosts) WHERE { { SELECT ?host (COUNT(*) AS ?n) WHERE { \
             GRAPH ?h { ?np a ex:Nanopublication } \
             BIND(STRBEFORE(STRAFTER(STR(?np), \"//\"), \"/\") AS ?host) } GROUP BY ?host } } \
             GROUP BY ?n ORDER BY ?n",
            true,
        ),
        // A set of the values seen cannot be merged: the workers send rows
        // to the one fold.
        (
            "SELECT (COUNT(DISTINCT ?e) AS ?n) WHERE { GRAPH ?pv { ?x ex:evidence ?e } }",
            true,
        ),
        (
            "SELECT ?e (COUNT(DISTINCT ?x) AS ?n) (GROUP_CONCAT(DISTINCT ?l; SEPARATOR=\",\") AS ?ls)              (SUM(?one) AS ?rows) WHERE { GRAPH ?pv { ?x ex:evidence ?e }              OPTIONAL { GRAPH ?pv { ?e rdfs:label ?l } } BIND(1 AS ?one) } GROUP BY ?e ORDER BY ?e",
            true,
        ),
        // A hash join: the right side is built once and every slice probes it.
        (
            "SELECT ?e (COUNT(*) AS ?n) WHERE { GRAPH ?h { ?np ex:hasAssertion ?a }              GRAPH ?pv { ?a ex:evidence ?e } } GROUP BY ?e ORDER BY ?e",
            true,
        ),
        (
            "SELECT ?l (COUNT(*) AS ?n) (COUNT(?src) AS ?with) WHERE {              GRAPH ?h { ?np ex:hasAssertion ?a }              OPTIONAL { GRAPH ?pv { ?a ex:source ?src FILTER(STRENDS(STR(?src), \"#3\")) } }              OPTIONAL { GRAPH ?pv2 { ?a ex:evidence ?e . ?e rdfs:label ?l } } }              GROUP BY ?l ORDER BY ?l",
            true,
        ),
        // A join with no shared variable, then a filter across it.
        (
            "SELECT ?e (COUNT(*) AS ?n) WHERE { GRAPH ?pv { ?x ex:evidence ?e }              GRAPH ?a { ?s ex:score ?v } FILTER(?a = ?x) } GROUP BY ?e ORDER BY ?e",
            true,
        ),
        // A join against a sub-select that groups.
        (
            "SELECT ?n (COUNT(*) AS ?rows) WHERE { GRAPH ?pv { ?x ex:evidence ?e }              { SELECT ?e (COUNT(*) AS ?n) WHERE { GRAPH ?p2 { ?e rdfs:label ?l } } GROUP BY ?e } }              GROUP BY ?n ORDER BY ?n",
            true,
        ),
        // Several patterns under GRAPH ?g: the graphs are sliced.
        (
            "SELECT ?v (COUNT(*) AS ?n) (SAMPLE(?o) AS ?any) WHERE {              GRAPH ?a { ?s ex:score ?v ; ex:rel0 ?o } } GROUP BY ?v ORDER BY ?v",
            true,
        ),
        // A block under GRAPH ?g that is not a plain BGP: the graphs its
        // required pattern is found in are listed once and sliced.
        (
            "SELECT ?v (COUNT(*) AS ?n) (COUNT(?o) AS ?os) WHERE { \
             GRAPH ?a { ?s ex:score ?v OPTIONAL { ?s ex:rel0 ?o } FILTER(?v < 30) } } \
             GROUP BY ?v ORDER BY ?v",
            true,
        ),
        // No pattern with a constant under GRAPH ?g: every graph, sliced.
        (
            "SELECT ?p (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o FILTER(isIRI(?o)) } } \
             GROUP BY ?p ORDER BY ?p",
            true,
        ),
        // A graph named by a BIND: possibly bound, so a lookup per row.
        (
            "SELECT ?e (COUNT(*) AS ?n) WHERE { GRAPH ?h { ?np ex:hasAssertion ?a } \
             BIND(IRI(REPLACE(STR(?a), \"#assertion$\", \"#provenance\")) AS ?pv) \
             OPTIONAL { GRAPH ?pv { ?a ex:evidence ?e } } } GROUP BY ?e ORDER BY ?e",
            true,
        ),
        // UNION with a branch that cannot be split (a sub-select): slice 0 runs it.
        (
            "SELECT (COUNT(*) AS ?n) (COUNT(?k) AS ?ks) WHERE { { GRAPH ?g { ?s ex:score ?v } } UNION              { SELECT ?k WHERE { ?np ex:featured ?k } } }",
            true,
        ),
        // Not split: EXISTS reads the whole row and keeps its own table.
        (
            "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?pv { ?x ex:evidence ?e } \
             FILTER EXISTS { GRAPH ?pv { ?e rdfs:label ?l } } }",
            false,
        ),
        // Not split: the default graph's few triples are one tile.
        (
            "SELECT (COUNT(*) AS ?n) (SUM(?k) AS ?s) WHERE { ?np ex:featured ?k }",
            false,
        ),
    ]
}

/// A graph of typed things, each with a value and a link to another, in the
/// default graph, cut into tiles of a few dozen triples.
fn things() -> Rete {
    use rete_core::{write_file_v05, DictionaryBuilder, GraphIndexBuilder};
    let mut t: Vec<(String, String, String)> = Vec::new();
    for i in 0..N {
        let thing = iri(&format!("http://ex/thing/{i:05}"));
        t.push((
            thing.clone(),
            iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            iri(&format!("http://ex/Kind{}", i % 7)),
        ));
        t.push((
            thing.clone(),
            iri("http://ex/value"),
            format!("\"{}\"^^<{XSD_INT}>", i % 13),
        ));
        t.push((
            thing,
            iri("http://ex/link"),
            iri(&format!("http://ex/thing/{:05}", (i * 31) % N)),
        ));
    }
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &t {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
    for (s, p, o) in &t {
        ib.push(dict.encode(s, p, o).unwrap());
    }
    Rete::open(&write_file_v05(&dict, &ib.build(), false, &[], 0)).unwrap()
}

/// Basic graph patterns of several triple patterns, over [`things`].
fn pattern_queries() -> Vec<(&'static str, bool)> {
    vec![
        // A star: the prefix is built once, the last pattern's scan is sliced.
        (
            "SELECT ?k (COUNT(*) AS ?n) (SUM(?v) AS ?sum) WHERE { ?t a ?k ; ex:value ?v }              GROUP BY ?k ORDER BY ?k",
            true,
        ),
        // A star and a hop, a filter, order-sensitive aggregates.
        (
            "SELECT ?k ?k2 (COUNT(*) AS ?n) (SAMPLE(?u) AS ?any)              (GROUP_CONCAT(?v; SEPARATOR=\"\") AS ?vs)              WHERE { ?t a ?k ; ex:value ?v ; ex:link ?u . ?u a ?k2 FILTER(?v < 3) }              GROUP BY ?k ?k2 ORDER BY ?k ?k2",
            true,
        ),
        // A constant class, a hop, a distinct count.
        (
            "SELECT ?v (COUNT(DISTINCT ?u) AS ?n) WHERE {              ?t a ex:Kind3 ; ex:link ?u . ?u ex:value ?v } GROUP BY ?v ORDER BY ?v",
            true,
        ),
        // A pattern joined to an OPTIONAL pattern and a second block.
        (
            "SELECT ?k (COUNT(*) AS ?n) (COUNT(?w) AS ?ws) WHERE { ?t a ?k              OPTIONAL { ?t ex:link ?u . ?u ex:value ?w FILTER(?w > 6) } } GROUP BY ?k ORDER BY ?k",
            true,
        ),
    ]
}

#[test]
fn four_threads_answer_as_one_does() {
    let (quads, things) = (file(), things());
    let parallel = cfg!(feature = "parallel");
    let cases = queries()
        .into_iter()
        .map(|c| (&quads, c))
        .chain(pattern_queries().into_iter().map(|c| (&things, c)));
    for (rete, (q, split)) in cases {
        let q = format!("{P}{q}");
        set_query_threads(1);
        let before = sharded_queries();
        let one = rows(rete, &q);
        assert_eq!(sharded_queries(), before, "one thread never splits: {q}");
        set_query_threads(4);
        let before = sharded_queries();
        let four = rows(rete, &q);
        let took = sharded_queries() - before;
        assert_eq!(one, four, "{q}");
        assert!(one.len() > 1, "a result to compare: {q}");
        if parallel {
            assert_eq!(took > 0, split, "split or not: {q}");
        }
    }

    // The same file read through a reader whose reads are round trips: a scan
    // is still sliced (fewer, larger slices), a lookup per row is not, and the
    // rows are the local file's.
    let far = Rete::open_ranged_lazy(Far(image(GraphLayout::Quad))).unwrap();
    for (q, split) in [
        (
            "SELECT ?p (COUNT(*) AS ?n) (MIN(?o) AS ?lo) WHERE { GRAPH ?g { ?s ?p ?o } } \
             GROUP BY ?p ORDER BY ?p",
            true,
        ),
        (
            "SELECT ?e ?label (COUNT(*) AS ?n) WHERE { GRAPH ?pv { ?x ex:evidence ?e } \
             OPTIONAL { GRAPH ?pv { ?e rdfs:label ?label } } } \
             GROUP BY ?e ?label ORDER BY ?e ?label",
            false,
        ),
    ] {
        let q = format!("{P}{q}");
        set_query_threads(1);
        let local = rows(&quads, &q);
        set_query_threads(4);
        let before = sharded_queries();
        let remote = rows(&far, &q);
        let took = sharded_queries() - before;
        assert_eq!(local, remote, "{q}");
        if parallel {
            assert_eq!(took > 0, split, "split or not over a far reader: {q}");
        }
    }

    // A block under GRAPH ?g finds its graphs from its required pattern: the
    // same rows as a file with an index per graph, which walks them all.
    set_query_threads(1);
    let per_graph = Rete::open(&image(GraphLayout::PerGraph)).unwrap();
    for q in [
        "SELECT ?a ?s ?v ?o WHERE { GRAPH ?a { ?s ex:score ?v OPTIONAL { ?s ex:rel0 ?o } \
         FILTER(?v < 3) } }",
        "SELECT ?a ?s ?v WHERE { GRAPH ?a { ?s ex:score ?v MINUS { ?s ex:rel1 ?o } \
         FILTER(?v > 38) } }",
        "SELECT ?g ?s ?l ?x WHERE { GRAPH ?g { { ?s rdfs:label ?l } { ?x ex:evidence ?s } \
         BIND(STRLEN(?l) AS ?len) } FILTER(STRENDS(STR(?g), \"7#provenance\")) }",
    ] {
        let q = format!("{P}{q}");
        let sorted = |rete: &Rete| {
            let mut r = rows(rete, &q);
            r.sort();
            r
        };
        let got = sorted(&quads);
        assert!(got.len() > 1, "{q}");
        assert_eq!(got, sorted(&per_graph), "{q}");
    }
    set_query_threads(0);
}

/// A FILTER written before the BINDs of its group runs below them when it
/// reads none of their variables. The oracle wraps the BINDs in a sub-select,
/// which the filter cannot move into.
#[test]
fn a_filter_below_the_binds_keeps_the_same_rows() {
    let rete = file();
    let moved = format!(
        "{P}SELECT ?np ?tail ?host WHERE {{ GRAPH ?h {{ ?np a ex:Nanopublication }} \
         FILTER(STRENDS(STR(?np), \"7\") && STRSTARTS(STR(?np), \"http://gamma\")) \
         BIND(REPLACE(STR(?np), \"^.*/\", \"\") AS ?tail) \
         BIND(STRBEFORE(STRAFTER(STR(?np), \"//\"), \"/\") AS ?host) }} ORDER BY ?np"
    );
    let oracle = format!(
        "{P}SELECT ?np ?tail ?host WHERE {{ {{ SELECT ?np ?tail ?host WHERE {{ \
         GRAPH ?h {{ ?np a ex:Nanopublication }} \
         BIND(REPLACE(STR(?np), \"^.*/\", \"\") AS ?tail) \
         BIND(STRBEFORE(STRAFTER(STR(?np), \"//\"), \"/\") AS ?host) }} }} \
         FILTER(STRENDS(STR(?np), \"7\") && STRSTARTS(STR(?np), \"http://gamma\")) }} ORDER BY ?np"
    );
    let got = rows(&rete, &moved);
    assert_eq!(got, rows(&rete, &oracle));
    assert!(got.len() > 30, "{} rows", got.len());

    // The plan: the filter sits under both BINDs; a conjunct that reads one of
    // them stays on top.
    let sel = rete_core::parse_select(&format!(
        "{P}SELECT * WHERE {{ ?s ex:p ?o FILTER(?o > 3 && ?b != \"x\") \
         BIND(STR(?o) AS ?a) BIND(STR(?s) AS ?b) }}"
    ))
    .unwrap();
    use rete_core::Plan;
    let Plan::Filter(_, top) = &sel.plan else {
        panic!("the conjunct on ?b stays above: {:?}", sel.plan);
    };
    let Plan::Extend(b, _, under) = &**top else {
        panic!("{:?}", sel.plan);
    };
    let Plan::Extend(a, _, base) = &**under else {
        panic!("{:?}", sel.plan);
    };
    assert_eq!((a.as_str(), b.as_str()), ("a", "b"));
    assert!(
        matches!(&**base, Plan::Filter(_, inner) if matches!(&**inner, Plan::Bgp(_))),
        "the conjunct on ?o runs on the scan: {:?}",
        sel.plan
    );
}
