//! Malformed-input robustness: a `.rete` may be fetched truncated or corrupt
//! from an arbitrary URL, so `Rete::open` (and the ranged readers) must return
//! `Err` — never panic — on any byte sequence. These tests build a valid image
//! and then assert that every truncation and a swath of byte corruptions are
//! handled gracefully.

use std::collections::BTreeMap;

use rete_core::{
    build_pyramid_meta, eval_sparql, write_dataset, write_file, DictionaryBuilder,
    GraphIndexBuilder, Rete, SliceReader, DEFAULT_TILE_BUDGET,
};

/// Exercise the read paths *past* `open`: a file may open successfully yet carry
/// corrupt block/dictionary internals, so iterating and querying it must also be
/// panic-free. Resolution returns `Option`/`Result`; we only require no panic.
fn exercise(rete: &Rete) {
    let _ = rete.dump(None);
    let _ = rete.graph_names();
    for g in rete.graph_names().to_vec() {
        let _ = rete.dump(Some(g));
    }
    let _ = rete.query(None, None, None);
    let _ = eval_sparql(rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10");
    let _ = eval_sparql(
        rete,
        "PREFIX ex: <http://ex/> SELECT ?x WHERE { ?x ex:knows+ ?y }",
    );
}

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

/// A small dataset *with named graphs*, so the corrupt-input paths exercise
/// `decode_named_graphs` too (where the OOB slices lived).
fn valid_image() -> Vec<u8> {
    let triples = [
        ("Alice", "type", "Person", None),
        ("Bob", "type", "Person", None),
        ("Alice", "knows", "Bob", Some("g")),
        ("Bob", "knows", "Carol", Some("g")),
    ];
    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in triples {
        db.observe(&iri(s), &iri(p), &iri(o));
    }
    let dict = db.build();
    let mut def = GraphIndexBuilder::new();
    let mut named: BTreeMap<String, GraphIndexBuilder> = BTreeMap::new();
    for (s, p, o, g) in triples {
        let t = dict.encode(&iri(s), &iri(p), &iri(o)).unwrap();
        match g {
            None => def.push(t),
            Some(name) => named.entry(iri(name)).or_default().push(t),
        }
    }
    let named_idx: Vec<(String, _)> = named.into_iter().map(|(g, b)| (g, b.build())).collect();
    write_dataset(&dict, &def.build(), &named_idx, true, &[], 0)
}

/// A clustered graph large enough to produce a real multi-tile pyramid, so the
/// `PyramidMeta::decode` + summary path is exercised by the fuzz below (the
/// dataset image above is built with no pyramid).
fn valid_image_with_pyramid() -> Vec<u8> {
    // Two communities of 8 nodes, densely connected within, one bridge between.
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for c in 0..2u32 {
        let base = c * 8;
        for i in 0..8u32 {
            for j in 0..8u32 {
                if i != j {
                    edges.push((base + i, base + j));
                }
            }
        }
    }
    edges.push((0, 8)); // the bridge

    let node = |n: u32| iri(&format!("n{n}"));
    let knows = iri("knows");
    let mut db = DictionaryBuilder::new();
    for &(s, o) in &edges {
        db.observe(&node(s), &knows, &node(o));
    }
    let dict = db.build();
    let ids: Vec<_> = edges
        .iter()
        .map(|&(s, o)| dict.encode(&node(s), &knows, &node(o)).unwrap())
        .collect();
    let mut ib = GraphIndexBuilder::new();
    for &t in &ids {
        ib.push(t);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
    write_file(&dict, &ib.build(), false, &meta, levels)
}

/// Same truncation + corruption fuzz, but over a pyramid-bearing image so the
/// pyramid-meta decode path is covered.
#[test]
fn pyramid_image_corruption_never_panics() {
    let image = valid_image_with_pyramid();
    assert!(Rete::open(&image).unwrap().pyramid().is_some());
    for len in 0..image.len() {
        if let Ok(r) = Rete::open(&image[..len]) {
            exercise(&r);
        }
    }
    let probes: [u8; 4] = [0x00, 0xff, 0x7f, 0x80];
    for i in 0..image.len() {
        for &v in &probes {
            let mut bad = image.clone();
            if bad[i] == v {
                continue;
            }
            bad[i] = v;
            if let Ok(r) = Rete::open(&bad) {
                exercise(&r);
            }
        }
    }
}

/// `open` must not panic on any prefix of a valid file (a truncated download).
#[test]
fn truncation_never_panics() {
    let image = valid_image();
    // The intact file still opens.
    assert!(Rete::open(&image).is_ok());
    for len in 0..image.len() {
        let prefix = &image[..len];
        // A panic here fails the test; we only require "doesn't panic".
        if let Ok(r) = Rete::open(prefix) {
            exercise(&r);
        }
        let _ = Rete::open_ranged(&SliceReader::new(prefix));
    }
}

/// `open` must not panic when header/section bytes are corrupted. We walk the
/// header (the highest-leverage bytes — offsets and lengths drive every slice)
/// and a sample of the body, flipping each byte to a few adversarial values.
#[test]
fn corruption_never_panics() {
    let image = valid_image();
    let probes: [u8; 5] = [0x00, 0x01, 0xff, 0x7f, 0x80];
    // Header is 1024 bytes (HEADER_LEN); step through the whole file but
    // densely cover it.
    for i in 0..image.len() {
        for &v in &probes {
            let mut bad = image.clone();
            if bad[i] == v {
                continue;
            }
            bad[i] = v;
            if let Ok(r) = Rete::open(&bad) {
                exercise(&r);
            }
            let _ = Rete::open_ranged(&SliceReader::new(&bad));
        }
    }
}

/// Pure garbage of every small length must be rejected without panicking.
#[test]
fn arbitrary_bytes_never_panic() {
    for len in 0..600usize {
        // A cheap deterministic byte pattern — no RNG needed.
        let bytes: Vec<u8> = (0..len).map(|k| ((k * 31 + 7) & 0xff) as u8).collect();
        if let Ok(r) = Rete::open(&bytes) {
            exercise(&r);
        }
        let _ = Rete::open_ranged(&SliceReader::new(&bytes));
    }
}
