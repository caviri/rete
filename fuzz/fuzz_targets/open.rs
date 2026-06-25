#![no_main]
//! Fuzz the **eager** untrusted-byte open + query path: any input must yield an
//! `Err`, never a panic / UB / out-of-bounds. A `.rete` faulted from a CDN is
//! untrusted input. `cargo +nightly fuzz run open`.

use libfuzzer_sys::fuzz_target;
use rete_core::{eval_sparql, Rete};

fuzz_target!(|data: &[u8]| {
    if let Ok(rete) = Rete::open(data) {
        // A surprising "valid" parse of fuzz bytes must still be safe to walk.
        let _ = rete.dump(None);
        let _ = eval_sparql(&rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5");
    }
});
