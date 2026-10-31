//! A query's constant terms resolve in ONE batched dictionary read on a lazy
//! (ranged) open, instead of one dependent read per term and section (G5a).
//! Remotely every read is a round trip, and nothing can be fetched from the
//! index before the constants have ids, so these reads are pure latency.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_opts, RawQuad};
use rete_core::{eval_query, Header, QueryOutput, RangeReader, Rete, SliceReader};

/// The ranges of each read call, in call order.
type CallLog = Arc<Mutex<Vec<Vec<(u64, u64)>>>>;

/// Every read call, in order: one entry per `read_at`, one per `read_many`
/// batch (with all its ranges) — what a remote reader turns into round trips.
#[derive(Clone)]
struct Calls {
    data: Arc<Vec<u8>>,
    calls: CallLog,
}

impl Calls {
    fn new(data: Vec<u8>) -> Self {
        Calls {
            data: Arc::new(data),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn take(&self) -> Vec<Vec<(u64, u64)>> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl RangeReader for Calls {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.calls.lock().unwrap().push(vec![(offset, len)]);
        SliceReader::new(&self.data).read_at(offset, len)
    }
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        self.calls.lock().unwrap().push(ranges.to_vec());
        let r = SliceReader::new(&self.data);
        ranges.iter().map(|&(o, l)| r.read_at(o, l)).collect()
    }
}

/// Enough terms for many dictionary chunks in every section: entities that are
/// subjects and objects (shared), long literals (object-only), and subjects
/// that are never objects (subject-only).
fn quads() -> Vec<RawQuad> {
    let iri = |s: String| format!("<http://example.org/{s}>");
    let mut q = Vec::new();
    for k in 0..4000 {
        let e = iri(format!("entity/{k:05}"));
        q.push((
            e.clone(),
            iri("title".into()),
            format!("\"Title number {k:05}, padded so that the literals fill several dictionary chunks\""),
            None,
        ));
        q.push((
            e.clone(),
            iri("cites".into()),
            iri(format!("entity/{:05}", (k * 7 + 3) % 4000)),
            None,
        ));
        q.push((
            iri(format!("record/{k:05}/about")),
            iri("about".into()),
            e,
            None,
        ));
    }
    q
}

fn select(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        other => panic!("unexpected output {other:?}"),
    }
}

fn touches(call: &[(u64, u64)], (off, len): (u64, u64)) -> bool {
    call.iter().any(|&(o, l)| o < off + len && off < o + l)
}

#[test]
fn constants_resolve_in_one_batched_read_before_any_tile() {
    let (bytes, _) = assemble_dataset_with_opts(quads(), false, false, None, |_, _| Vec::new());
    let h = Header::from_bytes(&bytes).unwrap();
    let dict = (h.dictionary_offset, h.dictionary_len);
    let index = (h.root_dir_offset, h.root_dir_len);
    let resident = Rete::open(&bytes).unwrap();
    let queries = [
        // Shared IRIs, an object-only literal, three predicates: their chunks
        // lie in three sections and far apart.
        "SELECT ?t ?c WHERE { <http://example.org/entity/00010> <http://example.org/title> ?t . \
         <http://example.org/entity/03990> <http://example.org/cites> ?c . \
         ?r <http://example.org/about> <http://example.org/entity/02000> }",
        "SELECT ?e WHERE { ?e <http://example.org/title> \"Title number 03333, padded so that the literals fill several dictionary chunks\" }",
        "SELECT ?e ?t WHERE { VALUES ?e { <http://example.org/entity/00001> <http://example.org/entity/01999> <http://example.org/entity/03998> } ?e <http://example.org/title> ?t }",
        "SELECT ?r WHERE { ?r <http://example.org/about>/<http://example.org/cites> <http://example.org/entity/00003> }",
    ];
    for q in queries {
        let want = select(&resident, q);
        assert!(!want.is_empty(), "{q}");
        let rec = Calls::new(bytes.clone());
        let lazy = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
        rec.take();
        assert_eq!(select(&lazy, q), want, "{q}");
        assert!(!lazy.index_incomplete());
        let calls = rec.take();
        let first_tile = calls
            .iter()
            .position(|c| touches(c, index))
            .expect("the query reads index tiles");
        let dict_before: Vec<&Vec<(u64, u64)>> = calls[..first_tile]
            .iter()
            .filter(|c| touches(c, dict))
            .collect();
        assert_eq!(
            dict_before.len(),
            1,
            "one batched dictionary read before the first tile, got {dict_before:?}: {q}"
        );
    }
}
