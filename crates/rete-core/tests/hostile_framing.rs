//! Fuzz-style robustness for the length, count and id varints that frame a
//! `.rete` file. `robustness.rs` flips single bytes, which almost never makes a
//! huge varint; here every position of a valid image is overwritten with the
//! varints that broke the old arithmetic (u64::MAX, 2^63, isize::MAX + 1, 2^32,
//! an overlong 10th byte), a seeded random pass mutates several bytes at once,
//! and a reader shortens each read of an open in turn. Every open path — eager,
//! ranged, lazy — and the queries after it must answer or refuse, never panic.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rete_core::varint::write_uvarint;
use rete_core::{
    attach_build_info, eval_sparql, read_card_and_build_info_ranged, replace_metadata,
    write_dataset, DictionaryBuilder, GraphIndexBuilder, RangeReader, Rete, SliceReader,
    HEADER_LEN,
};

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

/// A small dataset whose every framing layer has more than one entry: a
/// multi-tile default graph (a 64-byte tile budget), two multi-tile named
/// graphs, a card and a build record.
fn image() -> Vec<u8> {
    let mut quads = Vec::new();
    for i in 0..120u32 {
        let g = match i % 3 {
            0 => None,
            1 => Some(iri("g1")),
            _ => Some(iri("g2")),
        };
        quads.push((
            iri(&format!("s{}", i % 13)),
            iri(&format!("p{}", i % 4)),
            iri(&format!("o{i}")),
            g,
        ));
    }
    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in &quads {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let builder = || GraphIndexBuilder::new().with_tile_budget(64);
    let (mut def, mut g1, mut g2) = (builder(), builder(), builder());
    for (s, p, o, g) in &quads {
        let t = dict.encode(s, p, o).unwrap();
        match g.as_deref() {
            None => def.push(t),
            Some(name) if name.ends_with("g1>") => g1.push(t),
            Some(_) => g2.push(t),
        }
    }
    let named = vec![(iri("g1"), g1.build()), (iri("g2"), g2.build())];
    let image = write_dataset(&dict, &def.build(), &named, true, &[], 0);
    let image = replace_metadata(&image, br#"{"title":"hostile framing"}"#).unwrap();
    attach_build_info(&image, br#"{"engine":"test"}"#).unwrap()
}

/// A truncated response: read number `.0` returns at most `.1` bytes, and at
/// least one byte fewer than asked — still as a success.
type Short = Option<(usize, usize)>;

/// Owned bytes as a reader (the lazy open needs `'static`).
struct VecReader {
    data: Vec<u8>,
    calls: AtomicUsize,
    short_at: Short,
}

impl VecReader {
    fn new(data: Vec<u8>, short_at: Short) -> Arc<Self> {
        Arc::new(Self {
            data,
            calls: AtomicUsize::new(0),
            short_at,
        })
    }
}

impl RangeReader for VecReader {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        let range = usize::try_from(offset).ok().and_then(|s| {
            let e = s.checked_add(usize::try_from(len).ok()?)?;
            (e <= self.data.len()).then_some(s..e)
        });
        let mut range =
            range.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))?;
        if let Some((k, cap)) = self.short_at {
            if k == call && !range.is_empty() {
                range.end = (range.end - 1).min(range.start.saturating_add(cap));
            }
        }
        Ok(self.data[range].to_vec())
    }
}

/// Every read path past `open`: dumps, pattern scans, SPARQL over the default
/// and the named graphs, a property path, the card. `light` keeps the dumps
/// (every tile, every term) and drops the rest, for the exhaustive pass.
fn exercise(rete: &Rete, light: bool) {
    let _ = rete.dump(None);
    for g in rete
        .graph_names()
        .iter()
        .map(|g| g.to_string())
        .collect::<Vec<_>>()
    {
        let _ = rete.dump(Some(&g));
    }
    if light {
        return;
    }
    let _ = rete.query(Some(&iri("s1")), None, None);
    let _ = rete.query(None, None, None);
    let _ = eval_sparql(rete, "SELECT ?s ?o WHERE { ?s <http://ex/p1> ?o } LIMIT 20");
    let _ = eval_sparql(
        rete,
        "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }",
    );
    let _ = eval_sparql(
        rete,
        "SELECT ?x WHERE { <http://ex/s1> <http://ex/p0>+ ?x }",
    );
    let _ = rete.metadata();
}

/// Open `bytes` every way there is and exercise each result. Returns a
/// description of the first panic, if any.
fn every_open(bytes: &[u8], short_at: Short, light: bool) -> Option<String> {
    let run = |what: &str, f: &dyn Fn()| -> Option<String> {
        catch_unwind(AssertUnwindSafe(f))
            .err()
            .map(|e| match e.downcast_ref::<&str>() {
                Some(s) => format!("{what}: {s}"),
                None => match e.downcast_ref::<String>() {
                    Some(s) => format!("{what}: {s}"),
                    None => format!("{what}: panic"),
                },
            })
    };
    run("open", &|| {
        if let Ok(r) = Rete::open(bytes) {
            exercise(&r, light);
        }
    })
    .or_else(|| {
        run("open_ranged", &|| {
            let reader = VecReader::new(bytes.to_vec(), short_at);
            if let Ok(r) = Rete::open_ranged(reader.as_ref()) {
                exercise(&r, light);
            }
            let reader = VecReader::new(bytes.to_vec(), short_at);
            let _ = Rete::query_ranged(reader.as_ref(), Some(&iri("s1")), None, None);
            let reader = VecReader::new(bytes.to_vec(), short_at);
            let _ = Rete::query_ranged(reader.as_ref(), None, Some(&iri("p2")), None);
            let reader = VecReader::new(bytes.to_vec(), short_at);
            let _ = read_card_and_build_info_ranged(reader.as_ref());
        })
    })
    .or_else(|| {
        run("open_ranged_lazy", &|| {
            if let Ok(r) = Rete::open_ranged_lazy(VecReader::new(bytes.to_vec(), short_at)) {
                exercise(&r, light);
            }
        })
    })
    .or_else(|| {
        run("slice", &|| {
            let _ = Rete::open_ranged(&SliceReader::new(bytes));
        })
    })
}

/// The varints that broke the old decoders, longest first.
fn hostile_varints() -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = [
        u64::MAX,
        isize::MAX as u64 + 1,
        (1 << 32) + 1,
        u64::from(u32::MAX),
    ]
    .iter()
    .map(|&v| {
        let mut b = Vec::new();
        write_uvarint(&mut b, v);
        b
    })
    .collect();
    // A 10th byte with more than bit 63 in it: decoded as a smaller number
    // before the varint reader refused it.
    let mut overlong = vec![0x80u8; 9];
    overlong.push(0x02);
    out.push(overlong);
    out
}

#[test]
fn the_fixture_is_valid() {
    let img = image();
    let r = Rete::open(&img).unwrap();
    assert_eq!(r.graph_names().len(), 2);
    assert!(r
        .default_index()
        .tile_sections()
        .iter()
        .any(|s| s.len() > 1));
    assert!(r.metadata().is_some());
    assert!(every_open(&img, None, false).is_none());
}

#[test]
fn hostile_varints_at_every_position_never_panic() {
    let img = image();
    let mut panics = Vec::new();
    for v in hostile_varints() {
        // The header is fixed-width; the framing starts after it. Overwrite,
        // so every later offset still points where it did.
        for pos in HEADER_LEN..img.len() {
            let mut bad = img.clone();
            let end = (pos + v.len()).min(bad.len());
            bad[pos..end].copy_from_slice(&v[..end - pos]);
            if let Some(p) = every_open(&bad, None, true) {
                panics.push(format!("{v:02x?} at {pos}: {p}"));
            }
        }
    }
    assert!(
        panics.is_empty(),
        "{} panics, first: {:#?}",
        panics.len(),
        &panics[..panics.len().min(10)]
    );
}

#[test]
fn random_multi_byte_mutations_never_panic() {
    let img = image();
    let hostile = hostile_varints();
    // xorshift64*: deterministic, no dependency, and the seed is in the
    // failure message.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };
    let mut panics = Vec::new();
    for round in 0..1500 {
        let mut bad = img.clone();
        for _ in 0..1 + next() % 4 {
            let pos = HEADER_LEN + (next() as usize) % (bad.len() - HEADER_LEN);
            if next() % 2 == 0 {
                bad[pos] = next() as u8;
            } else {
                let v = &hostile[(next() as usize) % hostile.len()];
                let end = (pos + v.len()).min(bad.len());
                bad[pos..end].copy_from_slice(&v[..end - pos]);
            }
        }
        if let Some(p) = every_open(&bad, None, false) {
            panics.push(format!("round {round}: {p}"));
        }
    }
    assert!(
        panics.is_empty(),
        "{} panics, first: {:#?}",
        panics.len(),
        &panics[..panics.len().min(10)]
    );
}

#[test]
fn a_short_read_anywhere_never_panics() {
    let img = image();
    // Count the reads a clean pass makes, then shorten each one in turn: by
    // one byte, to two bytes, to nothing.
    let probe = VecReader::new(img.clone(), None);
    if let Ok(r) = Rete::open_ranged_lazy(probe.clone()) {
        exercise(&r, false);
    }
    let reads = probe.calls.load(Ordering::Relaxed);
    assert!(reads > 10, "the fixture should take many reads: {reads}");
    let mut panics = Vec::new();
    for k in 0..reads + 5 {
        for cap in [usize::MAX, 2, 0] {
            if let Some(p) = every_open(&img, Some((k, cap)), false) {
                panics.push(format!("read #{k} cut to {cap}: {p}"));
            }
        }
    }
    assert!(
        panics.is_empty(),
        "{} panics, first: {:#?}",
        panics.len(),
        &panics[..panics.len().min(10)]
    );
}
