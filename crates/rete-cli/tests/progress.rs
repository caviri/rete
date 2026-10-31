//! `--progress` on `rete build` (in RAM and `--memory-budget-mb`), `rete merge`
//! and `rete recard`: the `json` events parse and keep their promises (one
//! `start` first, one `done` last, the step never goes back, no percentage
//! past 100), `off` and the non-terminal default print nothing, the variable
//! works, the bar keeps the build's own lines whole — and the file written is
//! the same bytes with and without progress.

mod common;

use std::path::{Path, PathBuf};

use common::rete;

/// A graph big enough to take a few progress samples in a debug build: 32 k
/// statements over typed items that cite each other, in a named graph too.
fn source(dir: &Path) -> PathBuf {
    let mut nq = String::new();
    for i in 0..8_000 {
        let class = ["Book", "Article", "Thesis"][i % 3];
        nq.push_str(&format!(
            "<http://example.test/item/{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://vocab.test/{class}> .\n\
             <http://example.test/item/{i}> <http://www.w3.org/2000/01/rdf-schema#label> \
             \"Item number {i}\"@en .\n\
             <http://example.test/item/{i}> <http://example.test/cites> \
             <http://example.test/item/{}> .\n\
             <http://example.test/item/{i}> <http://example.test/seenIn> \
             <http://example.test/source/{}> <http://example.test/g/{}> .\n",
            (i * 7 + 3) % 8_000,
            i % 50,
            i % 20
        ));
    }
    let path = dir.join("graph.nq");
    std::fs::write(&path, nq).unwrap();
    path
}

/// Run `rete` with `args` and `env`, the build record made reproducible so
/// two runs can be compared byte for byte. Returns (stdout, stderr).
fn run(args: &[&str], env: &[(&str, &str)]) -> (String, String) {
    let mut cmd = rete();
    cmd.args(args)
        .env_remove("RETE_PROGRESS")
        .env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1793457125");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(out.status.success(), "rete {args:?} failed:\n{stderr}");
    (stdout, stderr)
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The `json` events on stderr: the lines that start with `{`.
fn events(stderr: &str) -> Vec<serde_json::Value> {
    stderr
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}

/// What every `--progress=json` run must hold to.
fn check_events(evs: &[serde_json::Value], command: &str) {
    assert!(evs.len() >= 3, "start, a phase, done at least: {evs:#?}");
    assert_eq!(evs[0]["event"], "start", "{evs:#?}");
    assert_eq!(evs.last().unwrap()["event"], "done", "{evs:#?}");
    let plan: Vec<String> = evs[0]["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    assert!(!plan.is_empty());
    let mut step = 0u64;
    let mut seen_phases = Vec::new();
    for e in evs {
        assert_eq!(e["command"], command);
        assert!(e["t"].as_f64().unwrap() >= 0.0);
        if matches!(e["event"].as_str(), Some("phase" | "progress")) {
            let k = e["step"].as_u64().unwrap();
            let n = e["steps"].as_u64().unwrap();
            assert!(k >= 1 && k <= n, "step {k} of {n}: {e}");
            assert!(k >= step, "the step went back: {e}");
            step = k;
            if let Some(p) = e["percent"].as_f64() {
                assert!((0.0..=100.0).contains(&p), "percent {p}: {e}");
            }
            let phase = e["phase"].as_str().unwrap().to_string();
            if seen_phases.last() != Some(&phase) {
                assert!(!seen_phases.contains(&phase), "{phase} came back: {e}");
                seen_phases.push(phase);
            }
        }
    }
    // The phases seen run in the planned order (some may pass between two
    // samples, and planned ones may not run at all).
    let mut at = 0;
    for p in &seen_phases {
        match plan[at..].iter().position(|q| q == p) {
            Some(i) => at += i + 1,
            None => panic!("{p} out of the planned order {plan:?}"),
        }
    }
    let done = evs.last().unwrap();
    assert!(done["statements"].as_u64().unwrap() > 0);
    assert!(done["output_bytes"].as_u64().unwrap() > 0);
}

#[test]
fn json_progress_parses_and_keeps_its_promises() {
    let dir = tempfile::tempdir().unwrap();
    let src = source(dir.path());
    let card = dir.path().join("card.json");
    std::fs::write(&card, r#"{"title": "Progress test"}"#).unwrap();
    let json = ["--progress", "json", "--progress-interval", "0.05"];

    // In RAM.
    let a = dir.path().join("a.rete");
    let (_, err) = run(
        &[
            &["build", s(&src), "-o", s(&a), "--card-file", s(&card)][..],
            &json,
        ]
        .concat(),
        &[],
    );
    let evs = events(&err);
    check_events(&evs, "build");
    assert_eq!(evs[0]["phases"][0], "parse+dictionary");
    assert!(
        err.contains("embedded build info"),
        "the build's own lines are still there:\n{err}"
    );

    // Memory-bounded.
    let b = dir.path().join("b.rete");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&tmp).unwrap();
    let (_, err) = run(
        &[
            &[
                "build",
                s(&src),
                "-o",
                s(&b),
                "--memory-budget-mb",
                "64",
                "--tmp-dir",
                s(&tmp),
                "--card-file",
                s(&card),
            ][..],
            &json,
        ]
        .concat(),
        &[],
    );
    let evs = events(&err);
    check_events(&evs, "build");
    assert_eq!(evs[0]["phases"][0], "parse+chunk");
    assert!(
        evs[0]["phases"]
            .as_array()
            .unwrap()
            .contains(&"content_hash".into()),
        "the full card's steps are planned: {}",
        evs[0]
    );

    // Merge, of the two.
    let m = dir.path().join("m.rete");
    let (_, err) = run(
        &[
            &[
                "merge",
                s(&a),
                s(&b),
                "-o",
                s(&m),
                "--memory-budget-mb",
                "64",
                "--tmp-dir",
                s(&tmp),
                "--card-file",
                s(&card),
            ][..],
            &json,
        ]
        .concat(),
        &[],
    );
    check_events(&events(&err), "merge");

    // Recard.
    let r = dir.path().join("r.rete");
    let (_, err) = run(
        &[
            &["recard", s(&m), "-o", s(&r), "--card-file", s(&card)][..],
            &json,
        ]
        .concat(),
        &[],
    );
    let evs = events(&err);
    check_events(&evs, "recard");
    assert_eq!(evs[0]["phases"][0], "read");
}

#[test]
fn off_and_the_default_off_a_terminal_print_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let src = source(dir.path());
    let quiet = |name: &str, extra: &[&str], env: &[(&str, &str)]| {
        let out = dir.path().join(name);
        let (stdout, stderr) = run(
            &[
                &["build", s(&src), "-o", s(&out), "--no-pyramid"][..],
                extra,
            ]
            .concat(),
            env,
        );
        (stdout.replace(name, "X"), stderr)
    };
    let (out_default, err_default) = quiet("d.rete", &[], &[]);
    let (out_off, err_off) = quiet("o.rete", &["--progress", "off"], &[]);
    let (_, err_env) = quiet("e.rete", &[], &[("RETE_PROGRESS", "off")]);
    for err in [&err_default, &err_off, &err_env] {
        assert!(
            !err.lines()
                .any(|l| l.starts_with("progress:") || l.starts_with('{') || l.contains('\r')),
            "progress output with progress off:\n{err}"
        );
    }
    assert_eq!(err_default, err_off);
    assert_eq!(err_default, err_env);
    assert_eq!(out_default, out_off);

    // The variable turns it on, and the flag beats the variable.
    let (out_plain, err_plain) = quiet("p.rete", &[], &[("RETE_PROGRESS", "plain")]);
    assert_eq!(
        out_plain.replace("p.rete", "X"),
        out_default,
        "stdout unchanged"
    );
    assert!(
        err_plain.lines().any(|l| l.starts_with("progress: build ")),
        "{err_plain}"
    );
    assert!(err_plain
        .lines()
        .any(|l| l.starts_with("progress: build done in ")));
    let (_, err_flag) = quiet(
        "f.rete",
        &["--progress", "off"],
        &[("RETE_PROGRESS", "json")],
    );
    assert_eq!(err_flag, err_default);

    // A bad value is refused, naming where it came from.
    let bad = rete()
        .args(["build", s(&src), "-o", s(&dir.path().join("x.rete"))])
        .env("RETE_PROGRESS", "loud")
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("RETE_PROGRESS=loud"));
}

#[test]
fn the_bar_prints_the_builds_own_lines_whole() {
    let dir = tempfile::tempdir().unwrap();
    let src = source(dir.path());
    let out = dir.path().join("bar.rete");
    let (stdout, stderr) = run(
        &[
            "build",
            s(&src),
            "-o",
            s(&out),
            "--memory-budget-mb",
            "64",
            "--tmp-dir",
            s(dir.path()),
            "--progress",
            "bar",
        ],
        &[],
    );
    assert!(
        stdout.starts_with("wrote "),
        "stdout stays clean: {stdout:?}"
    );
    assert!(!stdout.contains('\r'));
    // Each of the build's own lines starts a line of its own (after the
    // bar's clear-line sequence), never in the middle of a bar.
    let lines: Vec<&str> = stderr.split(['\n', '\r']).collect();
    let own: Vec<&&str> = lines.iter().filter(|l| l.contains("extbuild:")).collect();
    assert!(own.len() >= 3, "{stderr:?}");
    for l in own {
        let l = l.trim_start_matches("\x1b[2K");
        assert!(l.starts_with("extbuild:"), "a line through the bar: {l:?}");
    }
    assert!(stderr.contains("build done in "), "{stderr:?}");
}

#[test]
fn the_file_is_the_same_bytes_with_and_without_progress() {
    let dir = tempfile::tempdir().unwrap();
    let src = source(dir.path());
    let card = dir.path().join("card.json");
    std::fs::write(&card, r#"{"title": "Identical"}"#).unwrap();
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&tmp).unwrap();
    let modes: [&[&str]; 3] = [
        &["--progress", "off"],
        &["--progress", "json", "--progress-interval", "0.01"],
        &["--progress", "bar"],
    ];
    let bytes = |args: &[&str], out: &Path| {
        run(args, &[]);
        std::fs::read(out).unwrap()
    };
    let mut builds: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
    for (name, base) in [
        ("in RAM", vec!["build", s(&src), "--card-file", s(&card)]),
        (
            "bounded",
            vec![
                "build",
                s(&src),
                "--card-file",
                s(&card),
                "--memory-budget-mb",
                "64",
                "--tmp-dir",
                s(&tmp),
            ],
        ),
    ] {
        let mut outs = Vec::new();
        for (i, m) in modes.iter().enumerate() {
            let out = dir
                .path()
                .join(format!("{}-{i}.rete", name.replace(' ', "")));
            outs.push(bytes(&[&base[..], &["-o", s(&out)], m].concat(), &out));
        }
        builds.push((name.to_string(), outs));
    }
    // Merge and recard, from the in-RAM build.
    let input = dir.path().join("inRAM-0.rete");
    let half = dir.path().join("half.rete");
    std::fs::copy(dir.path().join("bounded-0.rete"), &half).unwrap();
    for (name, base) in [
        (
            "merge",
            vec![
                "merge",
                s(&input),
                s(&half),
                "--card-file",
                s(&card),
                "--memory-budget-mb",
                "64",
                "--tmp-dir",
                s(&tmp),
            ],
        ),
        ("recard", vec!["recard", s(&input), "--card-file", s(&card)]),
    ] {
        let mut outs = Vec::new();
        for (i, m) in modes.iter().enumerate() {
            let out = dir.path().join(format!("{name}-{i}.rete"));
            outs.push(bytes(&[&base[..], &["-o", s(&out)], m].concat(), &out));
        }
        builds.push((name.to_string(), outs));
    }
    for (name, outs) in &builds {
        for (i, o) in outs.iter().enumerate().skip(1) {
            assert!(
                o == &outs[0],
                "{name}: the file differs with {:?} ({} vs {} bytes)",
                modes[i],
                o.len(),
                outs[0].len()
            );
        }
    }
    // In place too (the record names the file, so the two have one name).
    for sub in ["a", "b"] {
        std::fs::create_dir(dir.path().join(sub)).unwrap();
    }
    let a = dir.path().join("a").join("inplace.rete");
    let b = dir.path().join("b").join("inplace.rete");
    std::fs::copy(&input, &a).unwrap();
    std::fs::copy(&input, &b).unwrap();
    run(
        &[
            "recard",
            s(&a),
            "--in-place",
            "--card-file",
            s(&card),
            "--progress",
            "off",
        ],
        &[],
    );
    run(
        &[
            "recard",
            s(&b),
            "--in-place",
            "--card-file",
            s(&card),
            "--progress",
            "json",
        ],
        &[],
    );
    assert!(std::fs::read(&a).unwrap() == std::fs::read(&b).unwrap());
}
