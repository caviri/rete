//! Property-based invariants (proptest): for arbitrary small graphs the format
//! must round-trip exactly, the build must be byte-deterministic, and — the
//! headline for the serverless/range path — the **lazy (ranged) reader must agree
//! with the in-memory reader on every query**. proptest shrinks any counterexample
//! to a minimal failing graph, complementing the fixed-seed `roundtrip` suite.

use std::collections::BTreeSet;
use std::sync::Arc;

use proptest::prelude::*;
use rete_core::{
    build_pyramid_meta, eval_sparql, write_file, Binding, DictionaryBuilder, GraphIndexBuilder,
    RangeReader, Rete, DEFAULT_TILE_BUDGET,
};

/// An owned in-memory `RangeReader`: `SliceReader` borrows, but `open_ranged_lazy`
/// needs `Send + Sync + 'static`, so the lazy side serves from owned bytes — the
/// same shape a real HTTP reader has, exercised entirely in-process.
struct VecReader(Vec<u8>);
impl RangeReader for VecReader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let s = offset as usize;
        let e = s
            .checked_add(len as usize)
            .filter(|&e| e <= self.0.len())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))?;
        Ok(self.0[s..e].to_vec())
    }
}

// Small pools that share the node space across subject/object roles and mix in
// literals (object-only) — the shapes that stress front-coding and role joins.
fn node(i: usize) -> String {
    format!("<http://ex/n/{i}>")
}
fn pred(i: usize) -> String {
    format!("<http://ex/p/{i}>")
}
fn lit(i: usize) -> String {
    format!("\"value {i}\"")
}

#[derive(Debug, Clone)]
enum Obj {
    Node(usize),
    Lit(usize),
}

const N_NODES: usize = 8;
const N_PREDS: usize = 4;
const N_LITS: usize = 5;

/// A graph: an arbitrary list of triple specs (deduplicated to a set on build).
fn graph() -> impl Strategy<Value = Vec<(usize, usize, Obj)>> {
    let obj = prop_oneof![
        (0..N_NODES).prop_map(Obj::Node),
        (0..N_LITS).prop_map(Obj::Lit),
    ];
    prop::collection::vec((0..N_NODES, 0..N_PREDS, obj), 0..40)
}

fn triples(specs: &[(usize, usize, Obj)]) -> BTreeSet<(String, String, String)> {
    specs
        .iter()
        .map(|(s, p, o)| {
            let obj = match o {
                Obj::Node(i) => node(*i),
                Obj::Lit(i) => lit(*i),
            };
            (node(*s), pred(*p), obj)
        })
        .collect()
}

fn build_image(want: &BTreeSet<(String, String, String)>) -> Vec<u8> {
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in want {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let ids: Vec<_> = want
        .iter()
        .map(|(s, p, o)| dict.encode(s, p, o).expect("just-observed term"))
        .collect();
    let mut ib = GraphIndexBuilder::new();
    for &t in &ids {
        ib.push(t);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
    write_file(&dict, &ib.build(), false, &meta, levels)
}

/// Run a SELECT and return its solutions as a canonical (sorted) multiset, so the
/// comparison is order-independent. `Binding` is a `BTreeMap`, hence `Ord`.
fn rows(rete: &Rete, q: &str) -> Vec<Binding> {
    let (_, mut sols) = eval_sparql(rete, q).expect("query evaluates");
    sols.sort();
    sols
}

/// A spread of query shapes instantiated from the graph's own terms: the all-rows
/// scan, each single-bound position, and a two-pattern join (the BGP path).
fn sample_queries(want: &BTreeSet<(String, String, String)>) -> Vec<String> {
    let mut qs = vec!["SELECT ?s ?p ?o WHERE { ?s ?p ?o }".to_string()];
    if let Some((s, p, o)) = want.iter().next() {
        qs.push(format!("SELECT ?p ?o WHERE {{ {s} ?p ?o }}"));
        qs.push(format!("SELECT ?s ?o WHERE {{ ?s {p} ?o }}"));
        if !o.starts_with('"') {
            qs.push(format!("SELECT ?s ?p WHERE {{ ?s ?p {o} }}"));
        }
        // Two-hop join: a shared variable bridges two patterns.
        qs.push(format!(
            "SELECT ?s ?o WHERE {{ ?s {p} ?mid . ?mid ?p2 ?o }}"
        ));
    }
    qs
}

proptest! {
    /// Build → dump reproduces the input triple set exactly, across random shapes.
    #[test]
    fn prop_roundtrip(specs in graph()) {
        let want = triples(&specs);
        let rete = Rete::open(&build_image(&want)).unwrap();
        let got: BTreeSet<_> = rete.dump(None).into_iter().collect();
        prop_assert_eq!(got, want);
    }

    /// Building the same graph twice is byte-identical (reproducible content hash).
    #[test]
    fn prop_deterministic(specs in graph()) {
        let want = triples(&specs);
        prop_assert_eq!(build_image(&want), build_image(&want));
    }

    /// The lazy (ranged) reader agrees with the in-memory reader on every query —
    /// the invariant that guards the serverless / HTTP-range path. Any divergence
    /// between the two read paths shrinks to a minimal failing graph + query.
    #[test]
    fn prop_lazy_equals_eager(specs in graph()) {
        let want = triples(&specs);
        let image = build_image(&want);
        let eager = Rete::open(&image).unwrap();
        let lazy = Rete::open_ranged_lazy(Arc::new(VecReader(image.clone()))).unwrap();
        for q in sample_queries(&want) {
            prop_assert_eq!(rows(&eager, &q), rows(&lazy, &q), "lazy != eager for: {}", q);
        }
        prop_assert!(!lazy.index_incomplete(), "lazy open faulted incompletely");
    }

    /// A **filtered dump returns exactly the quads an unfiltered dump returns,
    /// filtered** — for arbitrary graphs, every bound/unbound shape, and on both
    /// read paths. The sibling of `prop_lazy_equals_eager` for issue #117's
    /// filtered dumps.
    ///
    /// Worth stating as a property rather than a fixture: the saving comes from
    /// *not fetching* index tiles a synopsis proves cannot match, and the way
    /// that goes wrong is a missing row, not a crash. The oracle is deliberately
    /// the dumbest possible one — dump everything, then `retain` in Rust — so
    /// nothing about the routing is assumed by the thing checking the routing.
    /// proptest shrinks any disagreement to a minimal graph and shape.
    #[test]
    fn prop_filtered_dump_equals_the_full_dump_filtered(specs in graph()) {
        let want = triples(&specs);
        let image = build_image(&want);
        let eager = Rete::open(&image).unwrap();
        let lazy = Rete::open_ranged_lazy(Arc::new(VecReader(image.clone()))).unwrap();

        let mut full: Vec<(String, String, String)> = Vec::new();
        eager.dump_each(None, |s, p, o| full.push((s.into(), p.into(), o.into())));
        full.sort();

        // Every shape instantiated from a term the graph HAS, plus one it does
        // not (an unresolvable bound term must yield nothing, not everything).
        let sample = full.first().cloned();
        let mut shapes: Vec<(Option<&str>, Option<&str>, Option<&str>)> = vec![
            (None, None, None),
            (Some("<http://ex/n/absent>"), None, None),
            (None, Some("<http://ex/p/absent>"), None),
        ];
        if let Some((s, p, o)) = sample.as_ref() {
            shapes.extend([
                (Some(s.as_str()), None, None),
                (None, Some(p.as_str()), None),
                (None, None, Some(o.as_str())),
                (Some(s.as_str()), Some(p.as_str()), None),
                (Some(s.as_str()), None, Some(o.as_str())),
                (None, Some(p.as_str()), Some(o.as_str())),
                (Some(s.as_str()), Some(p.as_str()), Some(o.as_str())),
            ]);
        }

        for (s, p, o) in shapes {
            let mut expect = full.clone();
            expect.retain(|(ts, tp, to)| {
                s.is_none_or(|x| x == ts) && p.is_none_or(|x| x == tp) && o.is_none_or(|x| x == to)
            });
            for (tag, rete) in [("eager", &eager), ("lazy", &lazy)] {
                let mut got: Vec<(String, String, String)> = Vec::new();
                rete.dump_filtered_each(None, s, p, o, |s, p, o| {
                    got.push((s.into(), p.into(), o.into()))
                });
                got.sort();
                prop_assert_eq!(
                    &got, &expect,
                    "{} filtered dump != full dump filtered for {:?}", tag, (s, p, o)
                );
            }
        }
        prop_assert!(!lazy.index_incomplete(), "lazy open faulted incompletely");
    }
}

proptest! {
    // Fuzz the untrusted-input surface. proptest's harness catches any panic and
    // shrinks to the minimal input, so these assert the contract "ANY bytes →
    // Err, never a panic / UB / wrong-answer" the way SQLite fuzzes malformed DB
    // files — but always-on in CI, not a separate campaign. Complements the
    // structured corruption in `robustness.rs`.
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Opening and querying ARBITRARY bytes must never panic — a `.rete` faulted
    /// from a CDN could be anything. A successful open is fine; an `Err` is fine.
    #[test]
    fn fuzz_arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        if let Ok(rete) = Rete::open(&bytes) {
            let _ = rete.dump(None);
            let _ = eval_sparql(&rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5");
        }
    }

    /// Bit-flipping a VALID image at random offsets must never panic on open or
    /// query, via either the eager or the lazy (ranged) path — a corrupt or
    /// truncated download.
    #[test]
    fn fuzz_mutated_image_never_panic(
        specs in graph(),
        muts in prop::collection::vec((any::<u8>(), 0.0f64..1.0), 1..24),
    ) {
        let mut image = build_image(&triples(&specs));
        for (byte, frac) in muts {
            let i = ((frac * image.len() as f64) as usize).min(image.len() - 1);
            image[i] = byte;
        }
        if let Ok(rete) = Rete::open(&image) {
            let _ = rete.dump(None);
            let _ = eval_sparql(&rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5");
        }
        if let Ok(rete) = Rete::open_ranged_lazy(Arc::new(VecReader(image.clone()))) {
            let _ = eval_sparql(&rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5");
        }
    }
}
