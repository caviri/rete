//! Derived-IRI families: the root decoder never panics, a decoded root
//! re-encodes to the same bytes, and what a family builds, it splits back into.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rete_core::derived::{decode_families, encode_families, Family, Input};

/// The holes of a family, written so that no literal piece contains them.
fn check(f: &Family) {
    if f.literals.iter().any(|l| l.contains('\u{10FFFF}')) {
        return;
    }
    let holes: Vec<String> = (0..f.inputs.len())
        .map(|i| format!("\u{10FFFF}{i}"))
        .collect();
    let refs: Vec<&str> = holes.iter().map(String::as_str).collect();
    let term = f.build(&refs);
    assert_eq!(f.split(&term), Some(refs));
}

fuzz_target!(|data: &[u8]| {
    if let Ok(fams) = decode_families(data, 1 << 20) {
        assert_eq!(encode_families(&fams), data);
        for f in &fams {
            check(f);
        }
    }
    // A family from pieces (the builder's own constructor): a literal, then a
    // predicate, a prefix and a literal per hole, separated by U+0001.
    if let Ok(text) = std::str::from_utf8(data) {
        let parts: Vec<&str> = text.split('\u{1}').collect();
        if parts.len() >= 4 && parts.len() % 3 == 1 {
            let literals: Vec<String> = std::iter::once(parts[0])
                .chain(parts[1..].chunks(3).map(|c| c[2]))
                .map(str::to_string)
                .collect();
            let inputs: Vec<Input> = parts[1..]
                .chunks(3)
                .map(|c| Input {
                    predicate: c[0].to_string(),
                    prefix: c[1].to_string(),
                })
                .collect();
            if let Ok(f) = Family::new(literals, inputs) {
                check(&f);
                let bytes = encode_families(std::slice::from_ref(&f));
                assert_eq!(decode_families(&bytes, 1 << 20).ok(), Some(vec![f]));
            }
        }
    }
});
