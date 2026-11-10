//! The decode-ahead of a variable-predicate scan is capped across the whole
//! scan (G52): a query's demand is counted once, not once per run of 64
//! triples, and a scan whose rows a FILTER drops does not fetch the terms of
//! rows nothing will decode. Counted at the reader, on a lazy open.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_opts, RawQuad};
use rete_core::{eval_query, QueryOutput, RangeReader, Rete, SliceReader};

/// Every read call (one per `read_at`, one per `read_many` batch) and the
/// bytes they asked for.
#[derive(Clone)]
struct Calls {
    data: Arc<Vec<u8>>,
    calls: Arc<Mutex<(u64, u64)>>,
}

impl Calls {
    fn new(data: Vec<u8>) -> Self {
        Calls {
            data: Arc::new(data),
            calls: Arc::new(Mutex::new((0, 0))),
        }
    }
    fn take(&self) -> (u64, u64) {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl RangeReader for Calls {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let mut c = self.calls.lock().unwrap();
        c.0 += 1;
        c.1 += len;
        SliceReader::new(&self.data).read_at(offset, len)
    }
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let mut c = self.calls.lock().unwrap();
        c.0 += 1;
        c.1 += ranges.iter().map(|r| r.1).sum::<u64>();
        drop(c);
        let r = SliceReader::new(&self.data);
        ranges.iter().map(|&(o, l)| r.read_at(o, l)).collect()
    }
}

/// Many distinct predicates (so every run of 64 scanned triples meets new
/// ones and canonicalizes them), over long literals that fill many chunks.
fn quads() -> Vec<RawQuad> {
    let iri = |s: String| format!("<http://example.org/{s}>");
    // Literals that do not compress: the dictionary is a few MB.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..60000)
        .map(|k| {
            let noise: String = (0..40)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    char::from(b'a' + (state % 26) as u8)
                })
                .collect();
            (
                iri(format!("s/{:05}", k / 3)),
                iri(format!("p/{:04}", k / 10)),
                format!("\"Value number {k:05}, {noise}\""),
                None,
            )
        })
        .collect()
}

/// The fixture as a `0x06` file.
fn image() -> Vec<u8> {
    let (v5, _) = assemble_dataset_with_opts(quads(), false, false, None, |_, _| Vec::new());
    rete_core::v06::write::convert_image(&v5, Default::default()).unwrap()
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        other => panic!("unexpected output {other:?}"),
    }
}

/// `(rows, read calls, bytes asked for)` of `q` on a cold lazy open.
fn cold(bytes: &[u8], q: &str) -> (Vec<String>, u64, u64) {
    let rec = Calls::new(bytes.to_vec());
    let lazy = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
    rec.take();
    let got = rows(&lazy, q);
    let (calls, asked) = rec.take();
    (got, calls, asked)
}

#[test]
fn a_filter_over_the_scan_decodes_nothing_ahead() {
    let bytes = image();
    let resident = Rete::open(&bytes).unwrap();
    // All but the first thousand rows are dropped by the FILTER, which reads only the
    // predicate: no subject or object of a dropped row is ever decoded.
    let q =
        "SELECT ?s ?o WHERE { ?s ?p ?o FILTER(STRSTARTS(STR(?p), \"http://example.org/p/00\")) }";
    let want = rows(&resident, q);
    assert!(!want.is_empty() && want.len() < 2_000);
    let (got, calls, asked) = cold(&bytes, q);
    assert_eq!(got, want);
    // With a decode-ahead of 128 terms on every run of the scan this took 23
    // read calls and 1.59 MB of the 1.98 MB file (the terms of 59,000 dropped
    // rows); the thousand surviving rows need a few.
    eprintln!(
        "filtered scan: {calls} read calls, {asked} bytes of {}",
        bytes.len()
    );
    assert!(calls <= 6, "{calls} read calls, {asked} bytes");
    assert!(
        asked * 8 < bytes.len() as u64,
        "{asked} of {} bytes",
        bytes.len()
    );
}

#[test]
fn a_bare_limit_scan_still_decodes_ahead_in_few_reads() {
    let bytes = image();
    let resident = Rete::open(&bytes).unwrap();
    // The decode-ahead still serves a bare scan: LIMIT 100 is two runs of 64, and
    // the terms of those 100 rows (not 128: the cap counts across runs) come
    // with the predicate lookups.
    let q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 100";
    let want = rows(&resident, q);
    let (got, calls, asked) = cold(&bytes, q);
    assert_eq!(got, want);
    eprintln!("LIMIT 100: {calls} read calls, {asked} bytes");
    // (3 reads alone, 5 with the workspace's features unified.)
    assert!(calls <= 6, "{calls} read calls, {asked} bytes");
    assert!(
        asked * 3 < bytes.len() as u64,
        "{asked} of {} bytes",
        bytes.len()
    );
}
