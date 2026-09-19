//! `rete export --compress` at the CLI surface.
//!
//! The unit tests in `commands::compress` cover the encoder chain — that `close`
//! writes a trailer, that an abandoned encoder does not, that a failing sink
//! surfaces its error. What they cannot cover is the command: that stdout really
//! is pure binary, that every diagnostic still goes to stderr where it cannot
//! corrupt the stream, that an early exit leaves nothing behind, and that the
//! compressed bytes decode back to exactly the dump you would have got without
//! the flag.
//!
//! That last one is the whole contract, and it is checked per format rather than
//! once: compression sits in a different place in each serializer's writer chain,
//! so "works for N-Quads" is not evidence for Turtle.

mod common;

use std::io::Read;

const QUADS: &str = concat!(
    "<http://ex/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/T> .\n",
    "<http://ex/a> <http://ex/p> <http://ex/1> .\n",
    "<http://ex/a> <http://ex/p> <http://ex/2> .\n",
    "<http://ex/a> <http://ex/q> \"he said \\\"no\\\"\" .\n",
    "<http://ex/b> <http://ex/p> <http://ex/3> <http://ex/g1> .\n",
    "<http://ex/c> <http://ex/p> <http://ex/4> <http://ex/g2> .\n",
);

/// Only named graphs — the shape that makes `--format ttl` refuse, which is the
/// early-exit path a compressed stream must handle without leaving a stub frame.
const NAMED_ONLY: &str = concat!(
    "<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/g1> .\n",
    "<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/g2> .\n",
);

fn build(nq: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    let out = dir.path().join("out.rete");
    std::fs::write(&src, nq).unwrap();
    common::build(&src, &out, &["--no-pyramid"]);
    (dir, out)
}

fn run(file: &std::path::Path, args: &[&str]) -> std::process::Output {
    common::rete()
        .arg("export")
        .arg(file)
        .args(args)
        .output()
        .unwrap()
}

fn ok(file: &std::path::Path, args: &[&str]) -> Vec<u8> {
    let out = run(file, args);
    assert!(
        out.status.success(),
        "export {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut v)
        .unwrap();
    v
}

// --- the contract -----------------------------------------------------------

#[test]
fn compressed_output_decodes_to_exactly_the_uncompressed_dump() {
    let (_dir, file) = build(QUADS);
    for fmt in ["nq", "trig", "jsonld"] {
        let plain = ok(&file, &["--format", fmt]);
        assert!(!plain.is_empty(), "{fmt} produced nothing");

        let z = ok(&file, &["--format", fmt, "--compress", "zstd"]);
        assert_eq!(
            zstd::decode_all(&z[..]).unwrap(),
            plain,
            "{fmt}: zstd output must decode to the plain dump"
        );

        let g = ok(&file, &["--format", fmt, "--compress", "gzip"]);
        assert_eq!(
            gunzip(&g),
            plain,
            "{fmt}: gzip output must decode to the plain dump"
        );
    }
}

#[test]
fn turtle_compresses_too() {
    // Turtle takes a different route through the writer (the TurtleWriter owns
    // the sink), so it needs its own check rather than riding on N-Quads'.
    let (_dir, file) = build(QUADS);
    let plain = ok(&file, &["--format", "ttl"]);
    let z = ok(&file, &["--format", "ttl", "--compress", "zstd"]);
    assert_eq!(zstd::decode_all(&z[..]).unwrap(), plain);
}

#[test]
fn the_default_is_still_uncompressed_and_unchanged() {
    let (_dir, file) = build(QUADS);
    for fmt in ["nq", "ttl", "trig"] {
        let implicit = ok(&file, &["--format", fmt]);
        let explicit = ok(&file, &["--format", fmt, "--compress", "none"]);
        assert_eq!(
            implicit, explicit,
            "{fmt}: --compress none must be the default"
        );
        assert!(
            implicit.starts_with(b"<") || implicit.starts_with(b"@") || implicit.starts_with(b"G"),
            "{fmt}: the default must still be text, got {:?}",
            &implicit[..implicit.len().min(8)]
        );
    }
}

// --- stdout is a binary stream ---------------------------------------------

#[test]
fn stdout_carries_the_frame_and_nothing_else() {
    let (_dir, file) = build(QUADS);

    let z = ok(&file, &["--format", "nq", "--compress", "zstd"]);
    // zstd frame magic, RFC 8878 §3.1.1 — at byte 0, so nothing was printed first.
    assert_eq!(
        &z[..4],
        &[0x28, 0xB5, 0x2F, 0xFD],
        "zstd magic must start the stream"
    );

    let g = ok(&file, &["--format", "nq", "--compress", "gzip"]);
    assert_eq!(&g[..2], &[0x1F, 0x8B], "gzip magic must start the stream");
}

#[test]
fn every_diagnostic_goes_to_stderr_when_the_stream_is_binary() {
    // `--sanitize-iris` prints a multi-line report, and the graph ladder prints
    // notes. On a compressed run any of that on stdout would corrupt the frame.
    let (_dir, file) = build(concat!(
        "<http://ex/a[b]> <http://ex/p> <http://ex/o> .\n",
        "<http://ex/c> <http://ex/p> <http://ex/o> <http://ex/g1> .\n",
    ));
    let out = run(
        &file,
        &["--format", "ttl", "--compress", "zstd", "--sanitize-iris"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("percent-encoded"),
        "the report belongs on stderr: {err}"
    );
    assert!(
        err.contains("zstd-compressed"),
        "…as does the compression note: {err}"
    );

    // And stdout is a frame that decodes, i.e. nothing leaked into it.
    let text = zstd::decode_all(&out.stdout[..]).expect("stdout must be a clean zstd frame");
    assert!(String::from_utf8(text).unwrap().contains("%5Bb%5D"));
}

// --- early exits ------------------------------------------------------------

#[test]
fn an_error_before_any_data_leaves_stdout_empty() {
    // `--format ttl` on a file with an empty default graph and several named
    // graphs refuses. It must refuse without having opened a frame: a zero-byte
    // stdout, not a valid-but-empty archive that hides the failure from anything
    // downstream checking only for decodability.
    let (_dir, file) = build(NAMED_ONLY);
    let out = run(&file, &["--format", "ttl", "--compress", "zstd"]);
    assert!(!out.status.success(), "must not exit 0");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty, got {} bytes",
        out.stdout.len()
    );
}

#[test]
fn a_bad_level_fails_before_the_export_starts() {
    let (_dir, file) = build(QUADS);
    for args in [
        vec!["--compress", "zstd", "--compress-level", "23"],
        vec!["--compress", "gzip", "--compress-level", "10"],
        // A level with no codec is a mistake worth naming rather than ignoring.
        vec!["--compress-level", "3"],
    ] {
        let out = run(&file, &args);
        assert!(!out.status.success(), "{args:?} should have failed");
        assert!(out.stdout.is_empty(), "{args:?} wrote to stdout");
    }
}

#[test]
fn an_unknown_codec_is_rejected_by_the_parser() {
    let (_dir, file) = build(QUADS);
    let out = run(&file, &["--compress", "brotli"]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}

// --- levels -----------------------------------------------------------------

#[test]
fn every_accepted_level_produces_a_decodable_frame() {
    let (_dir, file) = build(QUADS);
    let plain = ok(&file, &["--format", "nq"]);
    for level in ["-7", "-1", "1", "3", "6", "12", "19", "22"] {
        let z = ok(
            &file,
            &[
                "--format",
                "nq",
                "--compress",
                "zstd",
                "--compress-level",
                level,
            ],
        );
        assert_eq!(
            zstd::decode_all(&z[..]).unwrap(),
            plain,
            "zstd level {level}"
        );
    }
    for level in ["1", "6", "9"] {
        let g = ok(
            &file,
            &[
                "--format",
                "nq",
                "--compress",
                "gzip",
                "--compress-level",
                level,
            ],
        );
        assert_eq!(gunzip(&g), plain, "gzip level {level}");
    }
}

#[test]
fn a_higher_level_is_not_larger() {
    // Not a ratio claim — just that the level reaches the codec at all. If the
    // flag were ignored, every level would produce identical bytes.
    let (_dir, file) = build(&QUADS.repeat(400));
    let low = ok(
        &file,
        &[
            "--format",
            "nq",
            "--compress",
            "zstd",
            "--compress-level",
            "1",
        ],
    );
    let high = ok(
        &file,
        &[
            "--format",
            "nq",
            "--compress",
            "zstd",
            "--compress-level",
            "19",
        ],
    );
    assert!(
        high.len() < low.len(),
        "level 19 ({}) should beat level 1 ({})",
        high.len(),
        low.len()
    );
}

// --- the streaming guarantee -----------------------------------------------

#[test]
fn compression_does_not_break_budget_invariance() {
    // The bounded-export contract (#245-#248) has to survive the codec: the
    // budget still changes only residency, so the compressed bytes are identical
    // at every budget too.
    let (_dir, file) = build(QUADS);
    for fmt in ["nq", "ttl", "trig"] {
        let base = ok(
            &file,
            &[
                "--format",
                fmt,
                "--compress",
                "zstd",
                "--memory-budget-mb",
                "4096",
            ],
        );
        for mb in ["1", "16", "0"] {
            assert_eq!(
                base,
                ok(
                    &file,
                    &[
                        "--format",
                        fmt,
                        "--compress",
                        "zstd",
                        "--memory-budget-mb",
                        mb
                    ]
                ),
                "{fmt} differs at --memory-budget-mb {mb}"
            );
        }
    }
}
