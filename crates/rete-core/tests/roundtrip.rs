//! Property-style round-trip: for many randomly-shaped graphs, building a
//! `.rete` image and reading it back must reproduce exactly the input triples.
//! This complements the malformed-input `robustness` suite (which checks we
//! never panic) by checking we are *correct* across graph shapes the fixed
//! integration tests don't enumerate — shared subject/object terms, prefix-heavy
//! terms that stress front-coding, literals, and delta-encoded permutations.

use std::collections::{BTreeMap, BTreeSet};

use rete_core::{
    build_pyramid_meta, write_dataset, DictionaryBuilder, GraphIndexBuilder, Header, Rete,
    CURRENT_FORMAT_VERSION, DEFAULT_TILE_BUDGET,
};

#[test]
fn paired_generation_round_trips_every_pattern_and_named_graph() {
    let triples = [
        ("<http://ex/alice>", "<http://ex/knows>", "<http://ex/bob>"),
        ("<http://ex/alice>", "<http://ex/likes>", "<http://ex/cake>"),
        ("<http://ex/bob>", "<http://ex/knows>", "<http://ex/carol>"),
        ("<http://ex/carol>", "<http://ex/likes>", "<http://ex/cake>"),
    ];
    let named_triple = ("<http://ex/dan>", "<http://ex/knows>", "<http://ex/alice>");
    let graph = "<http://ex/people>";

    let mut db = DictionaryBuilder::new();
    for &(s, p, o) in &triples {
        db.observe(s, p, o);
    }
    db.observe(named_triple.0, named_triple.1, named_triple.2);
    let dict = db.build();

    let mut default = GraphIndexBuilder::new().with_tile_budget(64);
    for &(s, p, o) in &triples {
        default.push(dict.encode(s, p, o).unwrap());
    }
    let mut named = GraphIndexBuilder::new().with_tile_budget(64);
    named.push(
        dict.encode(named_triple.0, named_triple.1, named_triple.2)
            .unwrap(),
    );
    let image = write_dataset(
        &dict,
        &default.build(),
        &[(graph.to_string(), named.build())],
        true,
        &[],
        0,
    );

    let header = Header::from_bytes(&image).unwrap();
    assert_eq!(CURRENT_FORMAT_VERSION, 0x06);
    assert_eq!(header.version, 0x06);
    let rete = Rete::open(&image).expect("the current paired generation opens eagerly");

    let sample = triples[0];
    for (s, p, o) in [
        (None, None, None),
        (Some(sample.0), None, None),
        (None, Some(sample.1), None),
        (None, None, Some(sample.2)),
        (Some(sample.0), Some(sample.1), None),
        (Some(sample.0), None, Some(sample.2)),
        (None, Some(sample.1), Some(sample.2)),
        (Some(sample.0), Some(sample.1), Some(sample.2)),
    ] {
        let got = rete.query(s, p, o);
        assert!(!got.is_empty(), "pattern {:?} lost its match", (s, p, o));
    }
    assert_eq!(
        rete.query_in_graph(
            Some(graph),
            Some(named_triple.0),
            Some(named_triple.1),
            Some(named_triple.2),
        ),
        vec![(
            named_triple.0.to_string(),
            named_triple.1.to_string(),
            named_triple.2.to_string(),
        )]
    );
}

/// A tiny deterministic LCG — no `rand` dependency, fully reproducible per seed.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        // Numerical Recipes constants.
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 16
    }
    fn upto(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
}

/// Build a term pool deliberately rich in shared prefixes and dual-role terms:
/// `<http://ex/node/N>` IRIs (used as both subjects and objects → shared dict
/// section), `<http://ex/p/N>` predicates, and a few literals (object-only).
fn term_pool(n: usize) -> (Vec<String>, Vec<String>, Vec<String>) {
    let nodes: Vec<String> = (0..n).map(|i| format!("<http://ex/node/{i}>")).collect();
    let preds: Vec<String> = (0..(n / 4).max(1))
        .map(|i| format!("<http://ex/p/{i}>"))
        .collect();
    let mut lits: Vec<String> = (0..(n / 3).max(1))
        .map(|i| format!("\"value number {i}\""))
        .collect();
    // Include literals carrying N-Triples escapes (quote, newline, backslash) so
    // the round-trip proves the dictionary's byte-level front-coding preserves
    // them exactly — these tokens are stored and dumped verbatim.
    lits.push(r#""he said \"hi\" then left""#.to_string());
    lits.push(r#""line1\nline2\tindented""#.to_string());
    lits.push(r#""back\\slash and a \"quote\"""#.to_string());
    (nodes, preds, lits)
}

/// Generate a random triple set for `seed`, build the image, read it back, and
/// assert the dumped triples equal the (deduplicated) input set.
fn check_seed(seed: u64, n_terms: usize, n_triples: usize, with_pyramid: bool) {
    let mut rng = Lcg(seed);
    let (nodes, preds, lits) = term_pool(n_terms);

    // Collect a deduplicated set of (s, p, o) triples.
    let mut want: BTreeSet<(String, String, String)> = BTreeSet::new();
    for _ in 0..n_triples {
        let s = nodes[rng.upto(nodes.len())].clone();
        let p = preds[rng.upto(preds.len())].clone();
        // Objects are sometimes nodes (shared role) and sometimes literals.
        let o = if rng.upto(2) == 0 {
            nodes[rng.upto(nodes.len())].clone()
        } else {
            lits[rng.upto(lits.len())].clone()
        };
        want.insert((s, p, o));
    }

    // Build the dictionary + index from the chosen triples.
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &want {
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
    let image = if with_pyramid {
        let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
        rete_core::write_file(&dict, &ib.build(), false, &meta, levels)
    } else {
        rete_core::write_file(&dict, &ib.build(), false, &[], 0)
    };

    // Read it back and compare as sets.
    let rete = Rete::open(&image).expect("our own image opens");
    let got: BTreeSet<(String, String, String)> = rete.dump(None).into_iter().collect();

    assert_eq!(
        got, want,
        "round-trip mismatch for seed {seed} ({n_triples} triples, pyramid={with_pyramid})"
    );
}

#[test]
fn roundtrip_many_shapes() {
    // A spread of seeds and sizes, including tiny graphs (edge cases for
    // front-coding / single-restart sections) and larger ones.
    for seed in 0..40u64 {
        check_seed(seed, 6, 3, false); // tiny
        check_seed(seed, 30, 60, false); // medium
        check_seed(seed, 80, 300, true); // larger, with a pyramid
    }
}

#[test]
fn roundtrip_single_triple() {
    check_seed(12345, 2, 1, false);
}

#[test]
fn roundtrip_at_scale() {
    // Thousands of terms cross ~150+ dictionary restart runs (interval 16), so
    // this exercises the binary search across runs, the within-run delta scan,
    // and multiple zone-map regions of the permutation blocks — and asserts the
    // full triple set is reproduced *exactly*. The benchmark builds at this scale
    // but only times it; this proves correctness at scale, not just no-crash.
    check_seed(2024, 2500, 9000, true);
}

/// Round-trip a *dataset* (named graphs): each random triple is assigned to one
/// of several graphs, and `dump(Some(g))` for each graph must reproduce exactly
/// that graph's triples — exercising the `write_dataset`/`decode_named_graphs`
/// quad path and per-graph index isolation.
#[test]
fn roundtrip_named_graphs() {
    for seed in 0..20u64 {
        let mut rng = Lcg(seed);
        let (nodes, preds, _lits) = term_pool(30);
        let graphs: Vec<String> = (0..4).map(|i| format!("<http://ex/g/{i}>")).collect();

        // Per-graph deduplicated triple sets.
        let mut want: BTreeMap<String, BTreeSet<(String, String, String)>> = BTreeMap::new();
        for _ in 0..120 {
            let g = graphs[rng.upto(graphs.len())].clone();
            let s = nodes[rng.upto(nodes.len())].clone();
            let p = preds[rng.upto(preds.len())].clone();
            let o = nodes[rng.upto(nodes.len())].clone();
            want.entry(g).or_default().insert((s, p, o));
        }

        // One shared dictionary over every term in every graph.
        let mut db = DictionaryBuilder::new();
        for set in want.values() {
            for (s, p, o) in set {
                db.observe(s, p, o);
            }
        }
        let dict = db.build();

        // A separate index per graph (default graph left empty here).
        let mut named: Vec<(String, _)> = Vec::new();
        for (g, set) in &want {
            let mut ib = GraphIndexBuilder::new();
            for (s, p, o) in set {
                ib.push(dict.encode(s, p, o).expect("observed term"));
            }
            named.push((g.clone(), ib.build()));
        }
        let default = GraphIndexBuilder::new().build();
        let image = write_dataset(&dict, &default, &named, true, &[], 0);

        let rete = Rete::open(&image).expect("dataset image opens");
        for (g, set) in &want {
            let got: BTreeSet<(String, String, String)> = rete.dump(Some(g)).into_iter().collect();
            assert_eq!(
                &got, set,
                "named-graph {g} round-trip mismatch (seed {seed})"
            );
        }
    }
}

#[test]
fn roundtrip_dense_small_alphabet() {
    // Few terms but many draws → heavy duplication and dense sharing.
    check_seed(999, 4, 500, true);
}
