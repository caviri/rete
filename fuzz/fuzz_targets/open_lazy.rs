#![no_main]
//! Fuzz the **lazy (ranged)** open + query path — the serverless / HTTP-range
//! code, which faults sections on demand and parses untrusted tile/dict bytes as
//! it goes. An owned in-memory `RangeReader` stands in for a remote one, so the
//! fuzzer drives exactly the byte-range parsing the browser/CLI hit over the wire.
//! `cargo +nightly fuzz run open_lazy`.

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use rete_core::{eval_sparql, schema_classes, schema_summary, RangeReader, Rete};


/// A `0x06` or `0x07` input (byte 4 is 6 or 7) gets the two fields a mutation would almost
/// always break, re-derived: the file length (bytes 24..32) and the root digest
/// (bytes 112..128, over the root that follows the 128-byte header). Without
/// this the open refuses at the header and the root, section-table and
/// unit-root parsing are never reached.
fn fix_v06(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if data.len() < 128 || (data[4] != 6 && data[4] != 7) {
        return std::borrow::Cow::Borrowed(data);
    }
    let mut v = data.to_vec();
    let len = v.len() as u64;
    v[24..32].copy_from_slice(&len.to_le_bytes());
    let root_len = u32::from_le_bytes([v[32], v[33], v[34], v[35]]) as usize;
    let end = 128usize.saturating_add(root_len).min(v.len());
    let d = rete_core::v06::digest16(&v[128..end]);
    v[112..128].copy_from_slice(&d);
    std::borrow::Cow::Owned(v)
}

/// Owned bytes as a `RangeReader` (the lazy open needs `Send + Sync + 'static`).
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

fuzz_target!(|data: &[u8]| {
    let data = &*fix_v06(data);
    if let Ok(rete) = Rete::open_ranged_lazy(Arc::new(VecReader(data.to_vec()))) {
        let _ = rete.header();
        let _ = rete.file_layout();
        let _ = rete.query(None, None, None);
        let _ = schema_classes(&rete);
        let _ = schema_summary(&rete);
        let _ = eval_sparql(&rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5");
        // The cold sections of 0x07: the label index (its root node is a
        // section of its own) and the card annex.
        let _ = rete.prefix_search("a", 5);
        let _ = rete_core::read_card_full_ranged(&rete_core::SliceReader::new(data));
        // The quad orders of 0x07 (SPOG, POSG, OSPG): a graph variable with a
        // bound subject, predicate or object seeks them, and the deep check
        // decodes every tile.
        for q in [
            // Class-clustered SPO and SOP (`0x07` flag bit 4): a bound subject is
            // looked for in every class run, a type-scoped join reads one.
            "SELECT ?p ?o WHERE { <http://x/a> ?p ?o } LIMIT 5",
            "SELECT ?s ?o WHERE { ?s a <http://x/C> . ?s <http://x/p> ?o } LIMIT 5",
            "SELECT ?g ?p ?o WHERE { GRAPH ?g { <http://x/a> ?p ?o } } LIMIT 5",
            "SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s <http://x/p> ?o } } LIMIT 5",
            "SELECT ?g ?s ?p WHERE { GRAPH ?g { ?s ?p <http://x/b> } } LIMIT 5",
            "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://x/p> <http://x/b> } } LIMIT 5",
            "SELECT ?s ?p ?o WHERE { GRAPH <http://x/g> { ?s ?p ?o } } LIMIT 5",
        ] {
            let _ = eval_sparql(&rete, q);
        }
        let _ = rete.named_graph_count();
        let _ = rete_core::v06::check_orders(&rete_core::SliceReader::new(data));
        let _ = rete_core::v06::check_clusters(&rete_core::SliceReader::new(data));
    }
});
