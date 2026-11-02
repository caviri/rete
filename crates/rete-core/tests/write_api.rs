//! The public in-memory writers (`write_file`, `write_dataset`,
//! `write_dataset_with_metadata`) write format `0x06`, like every other
//! writer; the legacy `0x05` image is the `_v05` twin. Named graphs, given per
//! graph here, come out as the quad index.

use rete_core::{
    verify, write_dataset, write_dataset_v05, write_dataset_with_metadata,
    write_dataset_with_metadata_v05, write_file, write_file_v05, DictionaryBuilder,
    GraphIndexBuilder, Rete,
};

const P: &str = "<http://ex/p>";

fn node(i: u32) -> String {
    format!("<http://ex/n{i}>")
}

/// A default graph of 5 triples and two named graphs of 3 and 2.
fn parts() -> (
    rete_core::dictionary::Dictionary,
    rete_core::GraphIndex,
    Vec<(String, rete_core::GraphIndex)>,
) {
    let mut db = DictionaryBuilder::new();
    let mut triples = Vec::new();
    for i in 0..10u32 {
        let (s, o) = (node(i), node(i + 1));
        db.observe(&s, P, &o);
        triples.push((i, s, o));
    }
    let dict = db.build();
    let mut def = GraphIndexBuilder::new();
    let mut g0 = GraphIndexBuilder::new();
    let mut g1 = GraphIndexBuilder::new();
    for (i, s, o) in &triples {
        let t = dict.encode(s, P, o).unwrap();
        match *i {
            0..=4 => def.push(t),
            5..=7 => g0.push(t),
            _ => g1.push(t),
        }
    }
    let named = vec![
        ("<http://ex/g0>".to_string(), g0.build()),
        ("<http://ex/g1>".to_string(), g1.build()),
    ];
    (dict, def.build(), named)
}

#[test]
fn write_file_is_0x06_and_its_v05_twin_is_0x05() {
    let (dict, def, _) = parts();
    let new = write_file(&dict, &def, false, &[], 0);
    let old = write_file_v05(&dict, &def, false, &[], 0);
    assert_eq!((&new[..4], new[4]), (&b"RETE"[..], 6));
    assert_eq!((&old[..4], old[4]), (&b"RETE"[..], 5));
    assert!(verify(&new).unwrap() && verify(&old).unwrap());
    let (a, b) = (Rete::open(&new).unwrap(), Rete::open(&old).unwrap());
    assert_eq!(a.header().quad_count, b.header().quad_count);
    assert_eq!(a.dump(None), b.dump(None));
}

#[test]
fn per_graph_named_graphs_are_written_as_a_quad_index() {
    let (dict, def, named) = parts();
    let new = write_dataset(&dict, &def, &named, true, &[], 0);
    assert_eq!(new[4], 6);
    assert!(verify(&new).unwrap());
    let old = write_dataset_v05(&dict, &def, &named, true, &[], 0);
    assert_eq!(old[4], 5);
    let (a, b) = (Rete::open(&new).unwrap(), Rete::open(&old).unwrap());
    assert_eq!(a.named_graph_count(), 2);
    assert_eq!(a.header().quad_count, 10);
    assert!(a.header().has_quad_index(), "the quad index");
    let mut names_a: Vec<String> = a.graph_names().iter().map(|g| g.to_string()).collect();
    let mut names_b: Vec<String> = b.graph_names().iter().map(|g| g.to_string()).collect();
    names_a.sort();
    names_b.sort();
    assert_eq!(names_a, names_b);
    for g in &names_a {
        let mut x = a.dump(Some(g));
        let mut y = b.dump(Some(g));
        x.sort();
        y.sort();
        assert_eq!(x, y, "graph {g}");
    }
    assert_eq!(a.dump(None), b.dump(None));
}

#[test]
fn a_card_travels_through_the_0x06_writer() {
    let (dict, def, _) = parts();
    let card = br#"{"title":"T","format_version":5}"#;
    let new = write_dataset_with_metadata(&dict, &def, &[], false, &[], 0, card, &[]);
    let old = write_dataset_with_metadata_v05(&dict, &def, &[], false, &[], 0, card, &[]);
    assert_eq!((new[4], old[4]), (6, 5));
    assert!(verify(&new).unwrap());
    let stored = Rete::open(&new)
        .unwrap()
        .metadata()
        .map(<[u8]>::to_vec)
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&stored).unwrap();
    assert_eq!(v["title"], "T");
    // A card written into 0x06 says so.
    assert_eq!(v["format_version"], 6);
}
