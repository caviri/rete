//! The builder ↔ reader contract for **polyglot** `.rete` files, round-tripped
//! through the real builder rather than through a fixture this test wrote itself.
//!
//! A polyglot is one object that is simultaneously an HTML page and a `.rete`
//! graph: byte 0 is `<`, and the graph is appended after `</html>`. A reader
//! finds it by scanning the resource's first [`HEADER_LEN`] bytes for
//! `RETE-BASE:` + [`POLYGLOT_DIGITS`] zero-padded digits.
//!
//! That agreement had two halves and no test. `experiments/polyglot/build_polyglot.py`
//! emitted a bare 12-digit number inside a JS `parseInt(…)` roughly 4 MB into the
//! file, while [`detect_polyglot_base`] looked for `RETE-BASE:` + 16 digits in the
//! first 1024 bytes — so it returned `None` for every polyglot this repository
//! could produce, and the "lazy" reader had nothing to open. A unit test that
//! builds its own shell cannot catch that: it agrees with itself by construction.
//! This one runs the actual builder and hands its output to the actual reader.

use std::path::{Path, PathBuf};
use std::process::Command;

use rete_core::range::{
    detect_polyglot_base, CountingReader, OffsetReader, RangeReader, SliceReader, POLYGLOT_DIGITS,
    POLYGLOT_MARKER,
};
use rete_core::{write_file, DictionaryBuilder, GraphIndexBuilder, Rete, HEADER_LEN};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repository root")
}

/// A small but real `.rete` image (the builder refuses anything else).
fn sample_rete() -> Vec<u8> {
    let pred = "<http://ex/knows>".to_string();
    let node = |n: u32| format!("<http://ex/n{n}>");
    let edges: Vec<(u32, u32)> = (0..64u32).map(|i| (i, (i + 1) % 64)).collect();

    let mut db = DictionaryBuilder::new();
    for &(s, o) in &edges {
        db.observe(&node(s), &pred, &node(o));
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new();
    for &(s, o) in &edges {
        ib.push(dict.encode(&node(s), &pred, &node(o)).unwrap());
    }
    write_file(&dict, &ib.build(), false, &[], 0)
}

/// Run the shipped builder. Returns `None` when there is no usable `python3`,
/// so a bare `cargo test` on a machine without it still passes — CI and the
/// devcontainer both have it, and the test is loud about skipping.
fn build_polyglot(rete: &Path, out: &Path) -> Option<std::process::Output> {
    let root = repo_root();
    let script = root.join("experiments/polyglot/build_polyglot.py");
    // The real page template — the marker's POSITION in the actual HTML is half
    // of what is under test. Only the 3 MB engine is stubbed out.
    let stub = root.join("crates/rete-core/tests/fixtures/v1/minimal.rete");
    for python in ["python3", "python"] {
        let out = Command::new(python)
            .arg(&script)
            .args(["--mode", "polyglot"])
            .arg("--rete")
            .arg(rete)
            .arg("--out")
            .arg(out)
            .arg("--template")
            .arg(root.join("experiments/polyglot/explorer.template.html"))
            // Any bytes will do for the engine: stub the glue with a comment and
            // the wasm with an existing small file.
            .arg("--glue")
            .arg(root.join("crates/rete-core/tests/fixtures/v1/glue-stub.js"))
            .arg("--wasm")
            .arg(&stub)
            .output();
        if let Ok(out) = out {
            return Some(out);
        }
    }
    eprintln!("SKIPPED: no python3 on PATH — cannot round-trip the polyglot builder");
    None
}

#[test]
fn the_builder_writes_a_marker_the_reader_finds_and_can_open_lazily() {
    let dir = std::env::temp_dir().join(format!("rete-polyglot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plain_path = dir.join("graph.rete");
    let poly_path = dir.join("page.polyglot.rete");

    let image = sample_rete();
    std::fs::write(&plain_path, &image).unwrap();

    let Some(out) = build_polyglot(&plain_path, &poly_path) else {
        return;
    };
    assert!(
        out.status.success(),
        "build_polyglot.py failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let poly = std::fs::read(&poly_path).unwrap();
    assert!(
        poly.len() > image.len(),
        "the page should precede the graph"
    );

    // 1. It is a web page: byte 0 is not the magic.
    assert_ne!(
        &poly[..4],
        b"RETE",
        "a polyglot must not start with the magic"
    );
    assert!(
        poly.starts_with(b"<!DOCTYPE html>"),
        "a polyglot must still be an HTML document"
    );

    // 2. THE contract: the reader finds the base in the window it actually
    //    fetches — the first HEADER_LEN bytes, nothing more.
    let head = &poly[..HEADER_LEN];
    let base = detect_polyglot_base(head).unwrap_or_else(|| {
        panic!(
            "detect_polyglot_base found no {} marker in the first {HEADER_LEN} bytes \
             written by build_polyglot.py — the builder and the reader disagree",
            String::from_utf8_lossy(POLYGLOT_MARKER),
        )
    });

    // 3. The marker is fixed-width, which is what lets the builder patch the
    //    offset in after the HTML length is known without moving a byte.
    let marker_at = head
        .windows(POLYGLOT_MARKER.len())
        .position(|w| w == POLYGLOT_MARKER)
        .unwrap();
    let digits = &head[marker_at + POLYGLOT_MARKER.len()..][..POLYGLOT_DIGITS];
    assert!(
        digits.iter().all(u8::is_ascii_digit),
        "the offset must be {POLYGLOT_DIGITS} plain ASCII digits, got {:?}",
        String::from_utf8_lossy(digits),
    );

    // 4. The offset it names is where the graph really is.
    assert_eq!(
        &poly[base as usize..base as usize + 4],
        b"RETE",
        "the marker points at {base}, which is not the graph"
    );
    assert_eq!(
        &poly[base as usize..],
        &image[..],
        "the appended tail must be the input .rete, byte for byte"
    );

    // 5. And it opens LAZILY through the offset shim: the same answers as the
    //    plain file, having touched fewer bytes than the HTML shell alone.
    let leaked: &'static [u8] = Box::leak(poly.into_boxed_slice());
    let counting = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
    let lazy = Rete::open_ranged_lazy(OffsetReader::new(counting.clone(), base)).unwrap();
    let view = OffsetReader::new(counting.clone(), base);
    assert_eq!(
        view.len(),
        image.len() as u64,
        "the offset view must report the graph's length, not the file's"
    );

    let mut want = Rete::open(&image).unwrap().query(None, None, None);
    want.sort();
    let mut got = lazy.query(None, None, None);
    got.sort();
    assert!(!got.is_empty(), "expected some triples");
    assert_eq!(
        got, want,
        "the polyglot answered differently to the plain file"
    );
    assert!(!lazy.index_incomplete(), "a lazy read failed");
    assert!(
        counting.bytes_read() < base,
        "read {} bytes to answer a query the HTML shell ({base} B) alone is bigger than — \
         the reader walked the page instead of the graph",
        counting.bytes_read(),
    );
}

/// `embed` mode inlines the graph as base64 and appends nothing, so there is no
/// base offset to name. A marker there would point at bytes that do not exist.
#[test]
fn embed_mode_carries_no_polyglot_marker() {
    let root = repo_root();
    let dir = std::env::temp_dir().join(format!("rete-polyglot-embed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plain_path = dir.join("graph.rete");
    let html_path = dir.join("portable.html");
    std::fs::write(&plain_path, sample_rete()).unwrap();

    let script = root.join("experiments/polyglot/build_polyglot.py");
    let Ok(out) = Command::new("python3")
        .arg(&script)
        .args(["--mode", "embed"])
        .arg("--rete")
        .arg(&plain_path)
        .arg("--out")
        .arg(&html_path)
        .arg("--template")
        .arg(root.join("experiments/polyglot/explorer.template.html"))
        .arg("--glue")
        .arg(root.join("crates/rete-core/tests/fixtures/v1/glue-stub.js"))
        .arg("--wasm")
        .arg(root.join("crates/rete-core/tests/fixtures/v1/minimal.rete"))
        .output()
    else {
        eprintln!("SKIPPED: no python3 on PATH");
        return;
    };
    assert!(
        out.status.success(),
        "build_polyglot.py --mode embed failed:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );

    let html = std::fs::read(&html_path).unwrap();
    assert!(
        detect_polyglot_base(&html[..HEADER_LEN.min(html.len())]).is_none(),
        "embed mode must not claim a base offset — it appends no tail"
    );
    assert!(
        html.windows(POLYGLOT_MARKER.len())
            .all(|w| w != POLYGLOT_MARKER),
        "embed mode must not carry the polyglot marker anywhere"
    );
}
