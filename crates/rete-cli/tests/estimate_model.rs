//! `rete estimate` against a real memory-bounded build (G38).
//!
//! The estimate used to model the dictionary and the six permutations only: no
//! label index, no text index, no card, and a spill taken from the raw size of
//! the input (18 times too high on the tutorial's 9.9 M-triple graph). It now
//! measures the file's sections by building a sample with the real writer and
//! projects the spill and the time from them.
//!
//! These tests build one small graph both ways, with the flags a published file
//! carries (`--label-index --text-index --card`), and require every projection
//! to cover what the build did:
//!
//! * the file size, inside the projected range;
//! * the spill, the directory's peak size polled while the build runs, inside
//!   the projected range;
//! * the wall time, within a stated factor (CI machines vary far more in time
//!   than in bytes, and a graph this small is mostly process start).
//!
//! The wider evidence (0.8 to 29.8 M statements, two shapes, two budgets) is in
//! the pull request and in `docs/build-tutorial.md`.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::rete;
use serde_json::Value;

const NS: &str = "https://example.org/";
const LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// A scholarly-shaped graph of `papers` papers (about 9 statements each): a
/// label, an abstract of 30 to 90 words (the long text), a DOI literal, a year,
/// authors and citations as IRI links, and a labelled person per three papers.
/// Deterministic.
fn graph(papers: u64) -> String {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n.max(1)
    };
    let vocab: Vec<String> = (0..2000)
        .map(|_| {
            (0..3 + next(8))
                .map(|_| (b'a' + next(26) as u8) as char)
                .collect()
        })
        .collect();
    let words = |k: u64, next: &mut dyn FnMut(u64) -> u64| {
        (0..k)
            .map(|_| vocab[next(2000) as usize].as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut s = String::new();
    let people = (papers / 3).max(3);
    for a in 0..people {
        s += &format!("<{NS}person/{a}> <{TYPE}> <{NS}Person> .\n");
        s += &format!("<{NS}person/{a}> <{LABEL}> \"{}\" .\n", words(2, &mut next));
    }
    for i in 0..papers {
        s += &format!("<{NS}paper/{i}> <{TYPE}> <{NS}Paper> .\n");
        s += &format!(
            "<{NS}paper/{i}> <{LABEL}> \"{}\"@en .\n",
            words(6 + next(8), &mut next)
        );
        s += &format!(
            "<{NS}paper/{i}> <{NS}abstract> \"{}\"@en .\n",
            words(30 + next(60), &mut next)
        );
        s += &format!(
            "<{NS}paper/{i}> <{NS}doi> \"10.{}/{i:x}\" .\n",
            1000 + next(9000)
        );
        s += &format!(
            "<{NS}paper/{i}> <{NS}year> \"{}\"^^<http://www.w3.org/2001/XMLSchema#gYear> .\n",
            1980 + next(45)
        );
        for _ in 0..1 + next(3) {
            s += &format!(
                "<{NS}paper/{i}> <{NS}author> <{NS}person/{}> .\n",
                next(people)
            );
        }
        if i > 0 {
            for _ in 0..next(4) {
                s += &format!("<{NS}paper/{i}> <{NS}cites> <{NS}paper/{}> .\n", next(i));
            }
        }
    }
    s
}

/// Size of every file under `dir`.
fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// What the build did: output bytes, peak bytes of its `--tmp-dir`, wall seconds.
struct Real {
    size: f64,
    spill: f64,
    secs: f64,
}

/// Run the bounded build `rete build <src> -o <out> --memory-budget-mb 32 ...`
/// and poll its `--tmp-dir` while it runs.
fn real_build(dir: &Path, src: &Path, flags: &[&str]) -> Real {
    let out = dir.join("real.rete");
    let tmp = dir.join("spill");
    std::fs::create_dir_all(&tmp).unwrap();
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rete"))
        .arg("build")
        .arg(src)
        .arg("-o")
        .arg(&out)
        .args(["--memory-budget-mb", "32", "--tmp-dir"])
        .arg(&tmp)
        .args(flags)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let poller = {
        let (done, tmp) = (done.clone(), tmp.clone());
        std::thread::spawn(move || {
            let mut peak = 0;
            while !done.load(Ordering::Relaxed) {
                peak = peak.max(dir_bytes(&tmp));
                std::thread::sleep(Duration::from_millis(2));
            }
            peak.max(dir_bytes(&tmp))
        })
    };
    let status = child.wait().unwrap();
    let secs = started.elapsed().as_secs_f64();
    done.store(true, Ordering::Relaxed);
    let polled = poller.join().unwrap();
    assert!(status.success(), "the bounded build failed");

    // The build record's own scratch peak, the other source of the spill figure.
    // It sees less than the directory does on a text-index build, so the larger
    // of the two is the reference.
    let bytes = std::fs::read(&out).unwrap();
    let rec = rete_core::read_build_info(&bytes)
        .unwrap()
        .expect("a record");
    let rec: Value =
        serde_json::from_slice(String::from_utf8_lossy(&rec).trim_end().as_bytes()).unwrap();
    let recorded = rec["spill"]["scratch_peak_bytes"].as_u64().unwrap_or(0);
    Real {
        size: bytes.len() as f64,
        spill: polled.max(recorded) as f64,
        secs,
    }
}

/// `rete estimate --json` with the same flags.
fn estimate(src: &Path, extra: &[&str]) -> Value {
    let out = rete()
        .arg("estimate")
        .arg(src)
        .args([
            "--memory-budget-mb",
            "32",
            "--label-index",
            "--text-index",
            "--card",
            "--no-card-costs",
            "--json",
        ])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn range(v: &Value) -> (f64, f64) {
    (v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}

fn check(est: &Value, real: &Real, what: &str) {
    let p = &est["projected"];
    eprintln!(
        "{what}: size {:?} real {}; spill {:?} real {}; build {:?}s real {:.2}s",
        p["size"], real.size, p["spill"], real.spill, p["build_secs"], real.secs
    );
    let (lo, hi) = range(&p["size"]);
    assert!(
        (lo..=hi).contains(&real.size),
        "{what}: file is {} B, projected {lo:.0} – {hi:.0}",
        real.size
    );
    // The spill is a peak polled from the directory every 2 ms: a poll that
    // misses the peak reads low, never high, so the floor is 40 % under the range
    // (the full-scale builds in the PR were polled every 200 ms against peaks
    // that last seconds, and fell inside it).
    let (lo, hi) = range(&p["spill"]);
    assert!(
        (lo * 0.6..=hi).contains(&real.spill),
        "{what}: spill peaked at {} B, projected {lo:.0} – {hi:.0}",
        real.spill
    );
    // Time: within a factor of 4 either side, plus two seconds of slack for a
    // loaded CI machine. A build this small is mostly process start, so only the
    // order of magnitude is asserted; the measured error at scale is in the PR.
    let (lo, hi) = range(&p["build_secs"]);
    assert!(
        real.secs <= 4.0 * hi + 2.0 && real.secs + 2.0 >= lo / 4.0,
        "{what}: build took {:.2}s, projected {lo:.2} – {hi:.2}s",
        real.secs
    );
}

fn setup(papers: u64) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("g.nt");
    std::fs::write(&src, graph(papers)).unwrap();
    std::fs::write(
        dir.path().join("card.json"),
        r#"{"title":"estimate test","license":"CC0-1.0"}"#,
    )
    .unwrap();
    (dir, src)
}

const BUILD_FLAGS: &[&str] = &[
    "--no-pyramid",
    "--label-index",
    "--text-index",
    "--no-card-costs",
    "--card-file",
];

/// The whole input fits in the sample: the sections are measured exactly, so
/// the size range is the file's within its few percent.
#[test]
fn estimate_covers_the_whole_input_build() {
    let (dir, src) = setup(2500);
    let card = dir.path().join("card.json");
    let mut flags: Vec<&str> = BUILD_FLAGS.to_vec();
    flags.push(card.to_str().unwrap());
    let real = real_build(dir.path(), &src, &flags);
    let est = estimate(&src, &[]);
    check(&est, &real, "whole input");
    // Every term the issue named is in the projection, and is not nothing.
    let p = &est["projected"];
    for k in ["dictionary", "index", "labels", "text", "card"] {
        assert!(p[k].as_f64().unwrap() > 0.0, "{k} is missing from {p}");
    }
}

/// The input is bigger than the sample (here a fifth of it): the sections are
/// scaled, and the real file must still sit inside the wider range.
#[test]
fn estimate_covers_a_scaled_build() {
    let (dir, src) = setup(2500);
    let card = dir.path().join("card.json");
    let mut flags: Vec<&str> = BUILD_FLAGS.to_vec();
    flags.push(card.to_str().unwrap());
    let real = real_build(dir.path(), &src, &flags);
    let n = est_statements(&src);
    let est = estimate(&src, &["--sample-statements", &(n / 5).to_string()]);
    check(&est, &real, "scaled sample");
}

fn est_statements(src: &Path) -> u64 {
    std::fs::read_to_string(src).unwrap().lines().count() as u64
}

/// Without `--text-index` or `--label-index` the projection has no such term, and
/// the card's starter-query measurement adds time that `--no-card-costs` removes.
#[test]
fn estimate_terms_follow_the_flags() {
    let (_dir, src) = setup(300);
    let run = |args: &[&str]| -> Value {
        let out = rete()
            .arg("estimate")
            .arg(&src)
            .args(["--memory-budget-mb", "32", "--json"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let bare = run(&[]);
    assert_eq!(bare["projected"]["labels"].as_f64().unwrap(), 0.0);
    assert_eq!(bare["projected"]["text"].as_f64().unwrap(), 0.0);
    assert_eq!(bare["projected"]["card"].as_f64().unwrap(), 0.0);
    let full = run(&["--label-index", "--text-index", "--card"]);
    assert!(full["projected"]["labels"].as_f64().unwrap() > 0.0);
    assert!(full["projected"]["text"].as_f64().unwrap() > 0.0);
    assert!(full["projected"]["card"].as_f64().unwrap() > 0.0);
    let cheap = run(&["--card", "--no-card-costs"]);
    let costly = run(&["--card"]);
    assert_eq!(
        cheap["projected"]["starter_query_secs"][1].as_f64(),
        Some(0.0)
    );
    assert!(
        costly["projected"]["starter_query_secs"][1]
            .as_f64()
            .unwrap()
            > 0.0
    );
}
