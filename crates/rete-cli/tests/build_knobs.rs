//! `rete build --tile-kib / --dict-chunk-kib / --zstd-level / --frequent-objects`
//! and the zstd frame shape: the knobs change the bytes and never the rows, the
//! flag beats its environment variable, the external build writes the same
//! bytes as the in-memory one under every knob, `--frequent-objects` included,
//! and a card build records the settings in its build info.

mod common;

use std::path::{Path, PathBuf};

use common::{fixture, rete};

/// A typed graph: three classes and a status literal are frequent object-only
/// terms, the items are shared (they cite each other), titles are unique.
fn graph() -> String {
    let mut nt = String::new();
    for i in 0..3000 {
        let class = ["Book", "Article", "Thesis"][i % 3];
        nt.push_str(&format!(
            "<http://example.test/item/{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://vocab.test/{class}> .\n\
             <http://example.test/item/{i}> <http://example.test/title> \"Title {i}\"@en .\n\
             <http://example.test/item/{i}> <http://example.test/status> \"published\" .\n\
             <http://example.test/item/{i}> <http://example.test/cites> \
             <http://example.test/item/{}> .\n",
            (i * 7 + 3) % 3000
        ));
    }
    nt
}

fn build(src: &Path, out: &Path, extra: &[&str], env: &[(&str, &str)]) {
    let mut cmd = rete();
    cmd.arg("build").arg(src).arg("-o").arg(out).args(extra);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.assert().success();
}

/// Every statement of the file, sorted: what "the same answers" means here.
fn statements(file: &Path) -> Vec<String> {
    let out = rete()
        .args(["export", "--format", "nq"])
        .arg(file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

fn verify(file: &Path) {
    rete().arg("verify").arg(file).assert().success();
}

struct Case {
    _f: common::Fixture,
    src: PathBuf,
    dir: PathBuf,
}

fn case() -> Case {
    let f = fixture();
    let src = f.write("g.nt", graph());
    let dir = f.path("");
    Case { _f: f, src, dir }
}

#[test]
fn knobs_change_bytes_not_statements() {
    let c = case();
    let base = c.dir.join("base.rete");
    build(&c.src, &base, &["--no-pyramid"], &[]);
    verify(&base);
    let expected = statements(&base);
    assert_eq!(expected.len(), 12_000);
    let base_bytes = std::fs::read(&base).unwrap();

    let variants: [&[&str]; 5] = [
        &["--tile-kib", "4"],
        &["--dict-chunk-kib", "4"],
        &["--zstd-level", "3"],
        &["--zstd-level", "19"],
        &["--frequent-objects", "4"],
    ];
    for (i, flags) in variants.iter().enumerate() {
        let out = c.dir.join(format!("v{i}.rete"));
        let mut args = vec!["--no-pyramid"];
        args.extend_from_slice(flags);
        build(&c.src, &out, &args, &[]);
        verify(&out);
        assert_ne!(
            std::fs::read(&out).unwrap(),
            base_bytes,
            "{flags:?} changed nothing"
        );
        assert_eq!(statements(&out), expected, "{flags:?}");
    }
}

/// A flag wins over its environment variable; the variable alone still works.
#[test]
fn flag_beats_environment() {
    let c = case();
    let flag = c.dir.join("flag.rete");
    let env = c.dir.join("env.rete");
    let both = c.dir.join("both.rete");
    build(&c.src, &flag, &["--no-pyramid", "--zstd-level", "3"], &[]);
    build(&c.src, &env, &["--no-pyramid"], &[("RETE_ZSTD_LEVEL", "3")]);
    build(
        &c.src,
        &both,
        &["--no-pyramid", "--zstd-level", "3"],
        &[("RETE_ZSTD_LEVEL", "19")],
    );
    let f = std::fs::read(&flag).unwrap();
    assert_eq!(std::fs::read(&env).unwrap(), f);
    assert_eq!(std::fs::read(&both).unwrap(), f);
}

/// Count zstd frames by their Frame_Header_Descriptor single-segment bit.
fn frame_shapes(bytes: &[u8]) -> (usize, usize) {
    let (mut sized, mut stream) = (0, 0);
    for w in bytes.windows(5) {
        if w[..4] == [0x28, 0xB5, 0x2F, 0xFD] {
            if w[4] & 0x20 != 0 {
                sized += 1;
            } else {
                stream += 1;
            }
        }
    }
    (sized, stream)
}

/// Sized single-segment frames are the default; `RETE_ZSTD_FRAME=stream`
/// brings back the 0.3 streaming frames, with the same statements.
#[test]
fn sized_frames_are_the_default() {
    let c = case();
    let sized = c.dir.join("sized.rete");
    let stream = c.dir.join("stream.rete");
    build(&c.src, &sized, &["--no-pyramid"], &[]);
    build(
        &c.src,
        &stream,
        &["--no-pyramid"],
        &[("RETE_ZSTD_FRAME", "stream")],
    );
    let (s, t) = frame_shapes(&std::fs::read(&sized).unwrap());
    assert!(
        s > 0 && t == 0,
        "default build: {s} sized, {t} stream frames"
    );
    let (s, t) = frame_shapes(&std::fs::read(&stream).unwrap());
    assert!(
        s == 0 && t > 0,
        "RETE_ZSTD_FRAME=stream: {s} sized, {t} stream frames"
    );
    assert_eq!(statements(&sized), statements(&stream));
}

/// The external build writes the in-memory build's bytes under the knobs it
/// takes, and refuses frequent-object promotion instead of ignoring it.
#[test]
fn external_build_matches_under_every_knob() {
    let c = case();
    let knobs = [
        "--tile-kib",
        "4",
        "--dict-chunk-kib",
        "8",
        "--zstd-level",
        "5",
    ];
    let mem = c.dir.join("mem.rete");
    let ext = c.dir.join("ext.rete");
    let mut a = vec!["--no-pyramid"];
    a.extend_from_slice(&knobs);
    build(&c.src, &mem, &a, &[]);
    let mut b = vec!["--memory-budget-mb", "64"];
    b.extend_from_slice(&knobs);
    build(&c.src, &ext, &b, &[]);
    assert_eq!(std::fs::read(&ext).unwrap(), std::fs::read(&mem).unwrap());

    // `--frequent-objects` too (G3, refused up to 0.4.1): the memory-bounded
    // build promotes the same terms, and says how many it did.
    for n in ["8", "64"] {
        let mem = c.dir.join(format!("mem-fo{n}.rete"));
        let ext = c.dir.join(format!("ext-fo{n}.rete"));
        build(
            &c.src,
            &mem,
            &["--no-pyramid", "--frequent-objects", n],
            &[],
        );
        let run = rete()
            .arg("build")
            .arg(&c.src)
            .arg("-o")
            .arg(&ext)
            .args(["--memory-budget-mb", "64", "--frequent-objects", n])
            .assert()
            .success();
        let log = String::from_utf8_lossy(&run.get_output().stderr).into_owned();
        assert!(log.contains("extbuild: frequent objects"), "{log}");
        assert_eq!(
            std::fs::read(&ext).unwrap(),
            std::fs::read(&mem).unwrap(),
            "--frequent-objects {n}: memory-bounded and in-RAM builds differ"
        );
        assert_ne!(
            std::fs::read(&ext).unwrap(),
            std::fs::read(&c.dir.join("mem.rete")).unwrap()
        );
    }
}

#[test]
fn out_of_range_values_are_usage_errors() {
    let c = case();
    for flags in [
        ["--tile-kib", "2"],
        ["--dict-chunk-kib", "8192"],
        ["--zstd-level", "0"],
        ["--zstd-level", "23"],
    ] {
        rete()
            .arg("build")
            .arg(&c.src)
            .arg("-o")
            .arg(c.dir.join("x.rete"))
            .args(flags)
            .assert()
            .code(2);
    }
}

/// A card build records the writer settings in its build info.
#[test]
fn card_build_records_writer_settings() {
    let c = case();
    let out = c.dir.join("carded.rete");
    build(
        &c.src,
        &out,
        &[
            "--no-pyramid",
            "--card",
            "--no-card-costs",
            "--tile-kib",
            "32",
            "--zstd-level",
            "12",
            "--frequent-objects",
            "4",
        ],
        &[],
    );
    let bytes = std::fs::read(&out).unwrap();
    let info = rete_core::read_build_info(&bytes)
        .unwrap()
        .expect("build info");
    let v: serde_json::Value = serde_json::from_slice(&info).unwrap();
    let w = &v["params"]["write"];
    assert_eq!(w["tile_kib"], 32, "{v}");
    assert_eq!(w["dict_chunk_kib"], 64);
    assert_eq!(w["zstd_level"], 12);
    assert_eq!(w["zstd_frame"], "sized");
    assert_eq!(w["frequent_objects"], 4);
}
