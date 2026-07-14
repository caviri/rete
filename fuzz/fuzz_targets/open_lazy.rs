#![no_main]
//! Fuzz the **lazy (ranged)** open + query path — the serverless / HTTP-range
//! code, which faults sections on demand and parses untrusted tile/dict bytes as
//! it goes. An owned in-memory `RangeReader` stands in for a remote one, so the
//! fuzzer drives exactly the byte-range parsing the browser/CLI hit over the wire.
//! `cargo +nightly fuzz run open_lazy`.

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use rete_core::{eval_sparql, schema_classes, schema_summary, RangeReader, Rete};

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
    if let Ok(rete) = Rete::open_ranged_lazy(Arc::new(VecReader(data.to_vec()))) {
        let _ = rete.header();
        let _ = rete.file_layout();
        let _ = rete.query(None, None, None);
        let _ = schema_classes(&rete);
        let _ = schema_summary(&rete);
        let _ = eval_sparql(&rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5");
    }
});
