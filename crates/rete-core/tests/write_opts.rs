//! The writer knobs (`rete_core::write_opts`; `--tile-kib`, `--dict-chunk-kib`,
//! `--zstd-level`, `--frequent-objects`, the zstd frame shape) change a file's
//! bytes and never its answers, every writer path produces the same bytes under
//! each of them, and the files open eagerly and over a lazy range reader alike.
//!
//! ONE test on purpose: the settings are process-wide (the CLI sets them once
//! per build), so two tests in this binary changing them concurrently would race.
//! Every other test binary keeps the defaults.

use std::sync::Arc;

use rete_core::extbuild::{build_external, ExternalBuildOptions};
use rete_core::ingest::{self, RawQuad};
use rete_core::write_opts::{self, FrameShape, WriteOptions};
use rete_core::{eval_query, verify, QueryOutput, RangeReader, Rete};

/// Whether this build's writer compresses (the `compression` feature, i.e. a
/// C zstd); without it tiles and chunks are stored raw.
const COMPRESSED: bool = cfg!(feature = "compression");

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

/// A typed graph whose classes and status literals are frequent object-only
/// terms (the shape frequency promotion is for), plus shared entities, unique
/// literals, language tags, datatypes and blank nodes.
fn quads() -> Vec<RawQuad> {
    let mut q = Vec::new();
    for i in 0..6000usize {
        let s = format!("<http://ex/item/{i:05}>");
        let class = ["Book", "Article", "Thesis"][i % 3];
        q.push((
            s.clone(),
            RDF_TYPE.into(),
            format!("<http://vocab/{class}>"),
            None,
        ));
        q.push((
            s.clone(),
            "<http://ex/title>".into(),
            format!("\"Title {i}\"@en"),
            None,
        ));
        q.push((
            s.clone(),
            "<http://ex/year>".into(),
            format!(
                "\"{}\"^^<http://www.w3.org/2001/XMLSchema#integer>",
                1990 + i % 30
            ),
            None,
        ));
        q.push((
            s.clone(),
            "<http://ex/status>".into(),
            "\"published\"".into(),
            None,
        ));
        q.push((
            s.clone(),
            "<http://ex/cites>".into(),
            format!("<http://ex/item/{:05}>", (i * 7 + 3) % 6000),
            None,
        ));
        if i % 4 == 0 {
            q.push((s, "<http://ex/note>".into(), format!("_:n{i}"), None));
        }
    }
    q
}

/// With a full-text index: its token table is the one payload the external
/// build compresses from disk (`compress_to`), so every knob must keep that
/// path byte-identical to the in-memory one too.
fn build_mem(quads: &[RawQuad]) -> Vec<u8> {
    let (bytes, _) = ingest::assemble_dataset_streaming(
        |visit: &mut dyn FnMut(RawQuad)| {
            for q in quads.iter().cloned() {
                visit(q);
            }
            Ok(())
        },
        false,
        true,
        None,
        |_, _, _| Vec::new(),
    )
    .unwrap();
    bytes
}

fn build_ext(quads: &[RawQuad], tag: &str) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("rete-write-opts-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.rete");
    build_external(
        |visit| {
            for q in quads.iter().cloned() {
                visit(q)?;
            }
            Ok(())
        },
        &out,
        ExternalBuildOptions {
            memory_budget: 0,
            tmp_dir: Some(dir.clone()),
            text_index: Some(rete_core::TextIndexScope::default()),
            ..Default::default()
        },
    )
    .unwrap();
    let bytes = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    bytes
}

/// An owned in-memory range reader, for the lazy open.
struct VecReader(Vec<u8>);

impl RangeReader for VecReader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let (s, e) = (offset as usize, (offset + len) as usize);
        self.0
            .get(s..e)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::other("out of range"))
    }
}

const QUERIES: [&str; 4] = [
    "SELECT ?s WHERE { ?s a <http://vocab/Thesis> ; <http://ex/year> ?y FILTER(?y > 2015) }",
    "SELECT ?t WHERE { <http://ex/item/00042> <http://ex/cites> ?c . ?c <http://ex/title> ?t }",
    "SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s a ?c } GROUP BY ?c",
    // ORDER BY: without it LIMIT may keep any 50 rows, and which ones follows
    // the scan order, which the tiling legitimately changes.
    "SELECT ?s WHERE { ?s <http://ex/status> \"published\" ; <http://ex/note> ?n } ORDER BY ?s LIMIT 50",
];

/// Every answer of the file: its full dump and the query battery, rendered
/// order-independently.
fn answers(rete: &Rete) -> Vec<String> {
    let mut out: Vec<String> = rete
        .dump(None)
        .into_iter()
        .map(|t| format!("{t:?}"))
        .collect();
    out.sort();
    for q in QUERIES {
        let mut rows: Vec<String> = match eval_query(rete, q).unwrap() {
            QueryOutput::Select(_, rows) => rows.iter().map(|b| format!("{b:?}")).collect(),
            other => vec![format!("{other:?}")],
        };
        rows.sort();
        out.push(format!("{q} -> {rows:?}"));
    }
    out
}

/// The lazy, range-read open must answer exactly like the eager one.
fn lazy_answers(bytes: &[u8]) -> Vec<String> {
    let rete = Rete::open_ranged_lazy(Arc::new(VecReader(bytes.to_vec()))).unwrap();
    let mut out = Vec::new();
    for q in QUERIES {
        let mut rows: Vec<String> = match eval_query(&rete, q).unwrap() {
            QueryOutput::Select(_, rows) => rows.iter().map(|b| format!("{b:?}")).collect(),
            other => vec![format!("{other:?}")],
        };
        rows.sort();
        out.push(format!("{q} -> {rows:?}"));
    }
    assert!(!rete.index_incomplete());
    out
}

/// Single-segment zstd frames carry their content size (Frame_Header_Descriptor
/// bit 5); the first frame after the dictionary container's section headers is
/// hard to locate generically, so scan the file for zstd magic numbers instead.
fn frame_flags(bytes: &[u8]) -> (usize, usize) {
    let magic = [0x28u8, 0xB5, 0x2F, 0xFD];
    let (mut single, mut stream) = (0, 0);
    for w in bytes.windows(5) {
        if w[..4] == magic {
            if w[4] & 0x20 != 0 {
                single += 1;
            } else {
                stream += 1;
            }
        }
    }
    (single, stream)
}

#[test]
fn writer_knobs_change_bytes_not_answers() {
    let quads = quads();

    // Defaults: what every file was written with before the knobs existed.
    write_opts::set_write_options(WriteOptions::default());
    assert_eq!(write_opts::write_options(), WriteOptions::default());
    let base = build_mem(&quads);
    assert!(verify(&base).unwrap());
    assert_eq!(
        base,
        build_ext(&quads, "base"),
        "default: external == in-memory"
    );
    let base_rete = Rete::open(&base).unwrap();
    let expected = answers(&base_rete);
    let expected_lazy = lazy_answers(&base);
    let base_dict = base_rete.dictionary();
    let (base_shared, base_obj) = (base_dict.shared_count(), base_dict.object_only_count());

    let d = WriteOptions::default();
    let variants: Vec<(&str, WriteOptions)> = vec![
        (
            "tiles4k",
            WriteOptions {
                tile_budget: 4096,
                ..d
            },
        ),
        (
            "chunks4k",
            WriteOptions {
                dict_chunk_budget: 4096,
                ..d
            },
        ),
        ("level3", WriteOptions { zstd_level: 3, ..d }),
        (
            "level19",
            WriteOptions {
                zstd_level: 19,
                ..d
            },
        ),
        (
            "stream",
            WriteOptions {
                frame: FrameShape::Stream,
                ..d
            },
        ),
        (
            "sized",
            WriteOptions {
                frame: FrameShape::Sized,
                ..d
            },
        ),
        (
            "all",
            WriteOptions {
                tile_budget: 8192,
                dict_chunk_budget: 4096,
                zstd_level: 15,
                frame: FrameShape::Sized,
                frequent_objects: 0,
            },
        ),
    ];
    for (tag, o) in &variants {
        write_opts::set_write_options(*o);
        let bytes = build_mem(&quads);
        assert!(verify(&bytes).unwrap(), "{tag}: content hash");
        // Without the `compression` feature the writer stores tiles and chunks
        // uncompressed, so the zstd level and frame shape cannot show.
        let zstd_differs = o.zstd_level != d.zstd_level || o.frame != d.frame;
        let layout_differs = o.tile_budget != d.tile_budget
            || o.dict_chunk_budget != d.dict_chunk_budget
            || o.frequent_objects != d.frequent_objects;
        if layout_differs || (COMPRESSED && zstd_differs) {
            assert_ne!(bytes, base, "{tag}: the knob must change the bytes");
        }
        assert_eq!(
            bytes,
            build_ext(&quads, tag),
            "{tag}: external == in-memory"
        );
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(answers(&rete), expected, "{tag}: answers");
        assert_eq!(lazy_answers(&bytes), expected_lazy, "{tag}: lazy answers");
        let (single, stream) = frame_flags(&bytes);
        match o.frame {
            _ if !COMPRESSED => assert_eq!((single, stream), (0, 0), "{tag}: uncompressed"),
            FrameShape::Sized => assert!(single > 0 && stream == 0, "{tag}: sized frames"),
            FrameShape::Stream => assert!(stream > 0 && single == 0, "{tag}: stream frames"),
        }
    }

    // Smaller tiles and chunks: more of them, and a larger file.
    write_opts::set_write_options(WriteOptions {
        tile_budget: 4096,
        dict_chunk_budget: 4096,
        ..d
    });
    let small = build_mem(&quads);
    assert!(small.len() > base.len());

    // Frequency promotion: the three classes and the status literal are the
    // most frequent object-only terms; they move into the shared section and
    // nothing else changes about what the file says.
    write_opts::set_write_options(WriteOptions {
        frequent_objects: 4,
        ..d
    });
    let promoted = build_mem(&quads);
    assert!(verify(&promoted).unwrap());
    let rete = Rete::open(&promoted).unwrap();
    assert_eq!(answers(&rete), expected, "promoted: answers");
    assert_eq!(
        lazy_answers(&promoted),
        expected_lazy,
        "promoted: lazy answers"
    );
    let dict = rete.dictionary();
    assert_eq!(dict.shared_count(), base_shared + 4);
    assert_eq!(dict.object_only_count(), base_obj - 4);
    for term in [
        "<http://vocab/Book>",
        "<http://vocab/Article>",
        "<http://vocab/Thesis>",
        "\"published\"",
    ] {
        let oid = dict
            .object_id(term)
            .expect("promoted term is still an object");
        assert!(
            oid <= dict.shared_count(),
            "{term} is in the shared section"
        );
    }
    // The shared section stays in term order, so where a promoted term lands
    // is where it sorts: a literal (`"`) ahead of every IRI, a
    // `<http://vocab/…>` class after the `<http://ex/item/…>` entities.
    assert_eq!(dict.object_id("\"published\""), Some(1));
    assert!(dict.object_id("<http://vocab/Book>").unwrap() > 6000);
    // The card a re-card derives by streaming the promoted file (`rete
    // recard`) is the card derived from the quads: a promoted literal in the
    // shared section is still a literal, a promoted class still an
    // object-only IRI (no duplicates, no multi-typed subject here).
    let (streamed, _) = rete_core::card::derive_card_streaming(
        &rete,
        rete_core::card::CardInput::default(),
        &mut |_| {},
    )
    .unwrap();
    let in_memory = rete_core::card::derive_card(
        &quads,
        rete.header().term_count,
        rete.named_graph_count() as u64,
        rete_core::card::CardInput::default(),
    );
    assert_eq!(
        String::from_utf8(streamed.to_json_bytes()).unwrap(),
        String::from_utf8(in_memory.to_json_bytes()).unwrap(),
        "promoted: the streamed card"
    );
    // Promotion is an in-memory-build feature: chunk-local dictionaries of the
    // external build never promote, whatever the setting says.
    let ext = build_ext(&quads, "promoted");
    assert_eq!(
        ext, base,
        "the external build ignores frequent-object promotion"
    );

    write_opts::set_write_options(WriteOptions::default());
}
