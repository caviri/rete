#![no_main]
//! Fuzz the **eager** untrusted-byte open + query path: any input must yield an
//! `Err`, never a panic / UB / out-of-bounds. A `.rete` faulted from a CDN is
//! untrusted input. `cargo +nightly fuzz run open`.

use libfuzzer_sys::fuzz_target;
use rete_core::{eval_sparql, schema_classes, schema_summary, verify, Rete};

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

fuzz_target!(|data: &[u8]| {
    let data = &*fix_v06(data);
    let _ = verify(data);
    if let Ok(rete) = Rete::open(data) {
        // A surprising "valid" parse of fuzz bytes must still be safe to walk.
        let _ = rete.header();
        let _ = rete.file_layout();
        let _ = rete.dump(None);
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
        // Half the inputs probe SPOG for a class-scoped block, half walk the graphs.
        rete_core::bgp::set_class_route(Some(data.len() % 2 == 0));
        for q in [
            // Class-clustered SPO and SOP (`0x07` flag bit 4): a bound subject is
            // looked for in every class run, a type-scoped join reads one.
            "SELECT ?p ?o WHERE { <http://x/a> ?p ?o } LIMIT 5",
            "SELECT ?s ?o WHERE { ?s a <http://x/C> . ?s <http://x/p> ?o } LIMIT 5",
            // Class-clustered SPOG (`0x07` flag bit 4): a bound subject of the quad
            // index is looked for in every run, a class-scoped block probes one.
            "SELECT ?g ?s ?p ?o WHERE { GRAPH ?g { ?s a <http://x/C> . ?s ?p ?o } } LIMIT 5",
            "SELECT ?g ?s ?n WHERE { GRAPH ?g { ?s a <http://x/C> . ?s <http://x/p> ?n } } LIMIT 5",
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
