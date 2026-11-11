//! Derived-IRI families: the root decoder and the spec parser never panic, a
//! decoded root re-encodes to the same bytes, and what a family builds, it
//! splits back into.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rete_core::derived::{decode_families, encode_families, Family};

fuzz_target!(|data: &[u8]| {
    if let Ok(fams) = decode_families(data, 1 << 20) {
        assert_eq!(encode_families(&fams), data);
        for f in &fams {
            // Holes that no literal piece contains, so the split is unambiguous.
            if f.literals.iter().any(|l| l.contains('\u{10FFFF}')) {
                continue;
            }
            let holes: Vec<String> = (0..f.inputs.len())
                .map(|i| format!("\u{10FFFF}{i}"))
                .collect();
            let refs: Vec<&str> = holes.iter().map(String::as_str).collect();
            let term = f.build(&refs);
            assert_eq!(f.split(&term), Some(refs));
        }
    }
    if let Ok(spec) = std::str::from_utf8(data) {
        if let Ok(f) = Family::parse_spec(spec) {
            assert_eq!(Family::parse_spec(&f.spec()).ok(), Some(f));
        }
    }
});
