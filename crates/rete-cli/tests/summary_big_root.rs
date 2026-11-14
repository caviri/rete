//! `rete summary` on a file whose root is longer than the first read (128 KiB).
//! The root has the first key of every dictionary leaf, as far as it differs
//! from the key before it, so a file with long literals that differ late (WKT
//! geometries of some kilobytes, one per version of a feature) has a long root. The summary
//! reads the pyramid, the schema and the dictionary by range, and has to read
//! the rest of the root as well.

mod common;

use common::rete;

/// A literal of `len` hex digits that does not compress, from a counter.
fn long_literal(i: u64, len: usize) -> String {
    let mut x = i.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut s = String::with_capacity(len + 16);
    while s.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.push_str(&format!("{x:016x}"));
    }
    s
}

#[test]
fn summary_reads_a_root_longer_than_the_first_read() {
    let dir = tempfile::tempdir().unwrap();
    let nt = dir.path().join("g.nt");
    let mut s = String::new();
    // Literals in groups of five that differ in their last character only: the
    // key that tells one from its neighbour is the whole literal.
    for i in 0..750u64 {
        s.push_str(&format!(
            "<http://example.test/e{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.test/T{}> .
             <http://example.test/e{i}> <http://example.test/next> <http://example.test/e{}> .
             <http://example.test/e{i}> <http://example.test/shape> \"{}{}\" .
",
            i % 3,
            (i + 1) % 750,
            long_literal(i / 5, 60_000),
            i % 5
        ));
    }
    std::fs::write(&nt, s).unwrap();
    let out = dir.path().join("g.rete");
    let o = rete()
        .env_remove("RETE_WRITE_FORMAT")
        .args(["build", nt.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .args(["--pyramid-algo", "types", "--dict-chunk-kib", "4"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    // The fixture has to have the property the test is about.
    let bytes = std::fs::read(&out).unwrap();
    let root_len = rete_core::v06::Header06::from_bytes(&bytes)
        .unwrap()
        .root_len as u64;
    assert!(
        rete_core::v06::HEADER_LEN as u64 + root_len > rete_core::v06::FIRST_READ,
        "root of {root_len} B fits in the first read: the fixture no longer tests anything"
    );

    let o = rete()
        .args(["summary", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.starts_with("pyramid round"), "{text}");
    assert!(text.contains("<http://example.test/next>"), "{text}");
}
