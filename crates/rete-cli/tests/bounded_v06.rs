//! Format 0x06, phase 2: the memory-bounded build and `merge` write `0x06`
//! themselves, in one pass over their spill.
//!
//! The same input built three ways must give the same bytes:
//!   * `build --memory-budget-mb` (the bounded build, straight to `0x06`),
//!   * `build --no-pyramid` (in RAM, `0x06`),
//!   * `build --no-pyramid` with `RETE_WRITE_FORMAT=5` (in RAM, the legacy
//!     `0x05`), then `rete convert`.
//!
//! A merge of parts must give the direct build of their union. Cards are left
//! out of the byte comparison on purpose: the bounded build derives the full
//! card from its sections, the in-RAM build a counts card, so they differ by
//! design; the card path is checked on its own below.

mod common;

use std::path::Path;

use common::{rete, stored_card};

const WORLDCUP: &str = include_str!("../../../tests/gate/fixtures/worldcup2026.nt");
const NAMED_ONLY: &str = include_str!("../../../tests/gate/fixtures/named-graphs-only.nq");

/// A small catalog-style file: labels in two languages, types, a family/given
/// name pair, literals worth a text index, links, and named graphs.
fn catalog(graphs: bool) -> String {
    let mut s = String::new();
    for i in 0..60 {
        s.push_str(&format!(
            "<http://example.test/w{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Work number {i}\"@en .\n\
             <http://example.test/w{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Werk Nummer {i}\"@de .\n\
             <http://example.test/w{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.test/Class{}> .\n\
             <http://example.test/w{i}> <http://purl.org/dc/terms/title> \"A study of glucose and café {i} in the lab\" .\n\
             <http://example.test/w{i}> <http://example.test/cites> <http://example.test/w{}> .\n\
             <http://example.test/w{i}> <http://example.test/creator> <http://example.test/p{}> .\n",
            i % 4,
            (i * 7 + 1) % 60,
            i % 9
        ));
    }
    for p in 0..9 {
        s.push_str(&format!(
            "<http://example.test/p{p}> <http://xmlns.com/foaf/0.1/familyName> \"Family{p}\" .\n\
             <http://example.test/p{p}> <http://xmlns.com/foaf/0.1/givenName> \"Given{}\" .\n\
             <http://example.test/p{p}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.test/Person> .\n",
            p % 3
        ));
    }
    // a duplicate, and a blank node
    s.push_str("<http://example.test/w0> <http://example.test/cites> <http://example.test/w1> .\n");
    s.push_str("<http://example.test/w0> <http://example.test/note> _:b0 .\n");
    if graphs {
        for g in 0..5 {
            for i in 0..8 {
                s.push_str(&format!(
                    "<http://example.test/w{i}> <http://example.test/seen> \"snapshot {g}\" <http://example.test/g{g}> .\n"
                ));
            }
        }
        // the same triple in two graphs and in the default graph
        s.push_str("<http://example.test/w1> <http://example.test/cites> <http://example.test/w2> <http://example.test/g1> .\n");
        s.push_str("<http://example.test/w1> <http://example.test/cites> <http://example.test/w2> <http://example.test/g2> .\n");
    }
    s
}

fn run(args: &[&str], legacy: bool) {
    let mut c = rete();
    c.env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000000");
    if legacy {
        c.env("RETE_WRITE_FORMAT", "5");
    } else {
        c.env_remove("RETE_WRITE_FORMAT");
    }
    let o = c.args(args).output().unwrap();
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn content_hash(bytes: &[u8]) -> [u8; 16] {
    rete_core::v06::Header06::from_bytes(bytes)
        .unwrap()
        .content_hash
}

/// The three builds of `src` with `extra`, compared pairwise.
fn three_ways(name: &str, src_name: &str, src: &str, extra: &[&str]) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join(src_name);
    std::fs::write(&input, src).unwrap();
    let (bounded, ram, old, conv) = (
        dir.path().join("bounded.rete"),
        dir.path().join("ram.rete"),
        dir.path().join("old.rete"),
        dir.path().join("conv.rete"),
    );
    let tmp = dir.path().join("spill");
    std::fs::create_dir(&tmp).unwrap();
    let with = |first: &[&str], out: &Path| {
        let mut a: Vec<String> = ["build", s(&input), "-o", s(out)]
            .iter()
            .map(|x| x.to_string())
            .collect();
        a.extend(first.iter().map(|x| x.to_string()));
        a.extend(extra.iter().map(|x| x.to_string()));
        a
    };
    let go = |a: Vec<String>, legacy: bool| {
        let r: Vec<&str> = a.iter().map(String::as_str).collect();
        run(&r, legacy);
    };
    go(
        with(
            &["--memory-budget-mb", "64", "--tmp-dir", s(&tmp)],
            &bounded,
        ),
        false,
    );
    go(with(&["--no-pyramid"], &ram), false);
    go(with(&["--no-pyramid"], &old), true);
    run(&["convert", s(&old), "-o", s(&conv)], false);

    let (b, r, c) = (
        std::fs::read(&bounded).unwrap(),
        std::fs::read(&ram).unwrap(),
        std::fs::read(&conv).unwrap(),
    );
    assert_eq!(b[4], 6, "{name}: the bounded build writes 0x06");
    assert_eq!(std::fs::read(&old).unwrap()[4], 5, "{name}: legacy hook");
    assert_eq!(content_hash(&b), content_hash(&r), "{name}: content hash");
    assert!(
        b == r,
        "{name}: bounded build differs from the in-RAM build"
    );
    assert!(
        b == c,
        "{name}: bounded build differs from the in-RAM 0x05 build plus convert"
    );
    rete().arg("verify").arg(&bounded).assert().success();
    if src_name.ends_with(".nq") {
        // A per-graph 0x05 file (the legacy layout) is re-encoded as a quad
        // index by `convert`: the same bytes again.
        let (pg, pgc) = (dir.path().join("pg.rete"), dir.path().join("pgc.rete"));
        go(
            with(&["--no-pyramid", "--named-graphs", "per-graph"], &pg),
            true,
        );
        run(&["convert", s(&pg), "-o", s(&pgc)], false);
        assert!(
            b == std::fs::read(&pgc).unwrap(),
            "{name}: bounded build differs from the in-RAM per-graph 0x05 build plus convert"
        );
    }
    // Nothing of the build is left beside the output or in the spill dir.
    let left: Vec<_> = std::fs::read_dir(&tmp)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{name}: spill left behind: {left:?}");
    let beside: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".partial"))
        .collect();
    assert!(beside.is_empty(), "{name}: partial file left: {beside:?}");
    b
}

const FLAG_SETS: &[&[&str]] = &[
    &[],
    &["--label-index"],
    &["--text-index"],
    &["--label-index", "--text-index"],
];

#[test]
fn bounded_equals_in_ram_and_convert_on_the_gate_fixtures() {
    for (i, flags) in FLAG_SETS.iter().enumerate() {
        let _ = three_ways(&format!("worldcup {i}"), "worldcup2026.nt", WORLDCUP, flags);
        three_ways(
            &format!("named-only {i}"),
            "named-graphs-only.nq",
            NAMED_ONLY,
            flags,
        );
    }
}

#[test]
fn bounded_equals_in_ram_and_convert_on_catalog_style_files() {
    for (i, flags) in FLAG_SETS.iter().enumerate() {
        three_ways(
            &format!("catalog {i}"),
            "catalog.nt",
            &catalog(false),
            flags,
        );
        three_ways(
            &format!("catalog+graphs {i}"),
            "catalog.nq",
            &catalog(true),
            flags,
        );
    }
}

/// `rete merge` of parts is the direct build of their union, byte for byte.
#[test]
fn merge_of_parts_equals_the_direct_build_of_the_union() {
    for (name, graphs, flags) in [
        ("plain", false, &[][..]),
        ("graphs", true, &[][..]),
        ("labels", true, &["--label-index"][..]),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let all = catalog(graphs);
        let lines: Vec<&str> = all.lines().collect();
        // Three parts, overlapping by a few lines so a merge has duplicates to fold.
        let cut = [0, lines.len() / 3 + 4, 2 * lines.len() / 3 + 2, lines.len()];
        let mut parts = Vec::new();
        for k in 0..3 {
            let lo = cut[k].saturating_sub(if k > 0 { 4 } else { 0 });
            let src = dir.path().join(format!("part{k}.nq"));
            std::fs::write(&src, lines[lo..cut[k + 1]].join("\n") + "\n").unwrap();
            let out = dir.path().join(format!("part{k}.rete"));
            let mut a = vec!["build", s(&src), "-o", s(&out), "--no-pyramid"];
            a.extend_from_slice(flags);
            run(&a, false);
            parts.push(out);
        }
        let union_src = dir.path().join("union.nq");
        std::fs::write(&union_src, all.as_bytes()).unwrap();
        let direct = dir.path().join("direct.rete");
        let mut a = vec!["build", s(&union_src), "-o", s(&direct), "--no-pyramid"];
        a.extend_from_slice(flags);
        run(&a, false);

        let merged = dir.path().join("merged.rete");
        let tmp = dir.path().join("spill");
        std::fs::create_dir(&tmp).unwrap();
        let mut m = vec!["merge"];
        for p in &parts {
            m.push(s(p));
        }
        m.extend_from_slice(&[
            "-o",
            s(&merged),
            "--memory-budget-mb",
            "64",
            "--tmp-dir",
            s(&tmp),
        ]);
        run(&m, false);

        let (d, m) = (
            std::fs::read(&direct).unwrap(),
            std::fs::read(&merged).unwrap(),
        );
        assert_eq!(m[4], 6, "{name}: merge writes 0x06");
        assert_eq!(content_hash(&d), content_hash(&m), "{name}: content hash");
        assert!(m == d, "{name}: merge differs from the direct build");
        rete().arg("verify").arg(&merged).assert().success();
        assert!(std::fs::read_dir(&tmp).unwrap().next().is_none());
    }
}

/// The bounded build with a card writes it into the card region of the one
/// file: every data section is the cardless build's, the file verifies, and a
/// re-card or a cost write fits the region's slack, so nothing moves.
#[test]
fn a_bounded_build_with_a_card_has_its_room_and_recards_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("catalog.nq");
    std::fs::write(&src, catalog(true)).unwrap();
    let card_file = dir.path().join("card.json");
    std::fs::write(&card_file, r#"{"title": "Catalog fixture"}"#).unwrap();
    let plain = dir.path().join("plain.rete");
    let carded = dir.path().join("carded.rete");
    run(
        &[
            "build",
            s(&src),
            "-o",
            s(&plain),
            "--memory-budget-mb",
            "64",
        ],
        false,
    );
    run(
        &[
            "build",
            s(&src),
            "-o",
            s(&carded),
            "--memory-budget-mb",
            "64",
            "--card-file",
            s(&card_file),
        ],
        false,
    );
    let (p, c) = (
        std::fs::read(&plain).unwrap(),
        std::fs::read(&carded).unwrap(),
    );
    let front =
        |b: &[u8]| rete_core::v06::open::read_front(&rete_core::SliceReader::new(b)).unwrap();
    let ((hp, rp), (hc, rc)) = (front(&p), front(&c));
    // Same data sections, byte for byte (the digests are of the stored bytes).
    use rete_core::v06::kind;
    let mut compared = 0;
    for k in [
        kind::DICT_DATA,
        kind::INDEX_DATA,
        kind::QUAD_DATA,
        kind::TEXT_DATA,
        kind::LABEL_DATA,
    ] {
        let (Some(a), Some(b)) = (rp.section(k), rc.section(k)) else {
            continue;
        };
        assert_eq!((a.digest, a.len), (b.digest, b.len), "section {k}");
        compared += 1;
    }
    assert!(compared >= 3, "{compared}");
    assert_eq!(hp.term_count, hc.term_count);
    assert_ne!(hp.content_hash, hc.content_hash, "the card is in the hash");
    assert!(stored_card(&c).is_some() && stored_card(&p).is_none());
    rete().arg("verify").arg(&carded).assert().success();
    let len = c.len();
    let hash = hc.content_hash;

    // Costs written back: the hash and the length stay.
    rete()
        .args([
            "card-audit",
            s(&carded),
            "--measure",
            "--write-costs",
            "--allow-empty",
        ])
        .assert()
        .success();
    let after = std::fs::read(&carded).unwrap();
    assert_eq!(after.len(), len, "the record fits the region's slack");
    assert_eq!(content_hash(&after), hash);
    rete().arg("verify").arg(&carded).assert().success();

    // A re-card with the same curated half: in place, still verifying.
    rete()
        .args([
            "recard",
            s(&carded),
            "--in-place",
            "--keep-curated",
            "--verify",
        ])
        .assert()
        .success();
    let again = std::fs::read(&carded).unwrap();
    assert_eq!(again.len(), len, "the new pair fits beside the old one");
    rete().arg("verify").arg(&carded).assert().success();
}

// ---- the record's reservation, failure paths, a build with real structure ----

/// Run the CLI; `reproducible` pins the record's clock. Returns stderr.
fn run_stderr(args: &[&str], reproducible: bool) -> String {
    let mut c = rete();
    c.env_remove("RETE_WRITE_FORMAT");
    if reproducible {
        c.env("RETE_BUILD_RECORD", "reproducible")
            .env("SOURCE_DATE_EPOCH", "1700000000");
    } else {
        c.env_remove("RETE_BUILD_RECORD");
    }
    let o = c.args(args).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(o.status.success(), "{args:?}: {err}");
    err
}

/// The three record shapes that sized the reservation wrongly: a sealed
/// record longer than its room costs a copy of the whole file.
const RECORD_SHAPES: [(&str, bool, &[&str]); 3] = [
    ("default", false, &[]),
    ("no card costs", false, &["--no-card-costs"]),
    ("reproducible", true, &[]),
];

fn assert_in_place(name: &str, err: &str, file: &Path) {
    assert!(
        !err.contains("outgrew")
            && !err.contains("moving it")
            && !err.contains("rewriting the file"),
        "{name}: the build record did not fit its reservation:\n{err}"
    );
    rete().arg("verify").arg(file).assert().success();
}

/// More inputs than the record lists by name (it keeps eight), several
/// shapes: the sealed record fits the room reserved for it.
#[test]
fn a_bounded_build_with_a_card_never_outgrows_its_record_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card.json");
    std::fs::write(&card, r#"{"title": "Many inputs"}"#).unwrap();
    let mut inputs = Vec::new();
    for k in 0..10 {
        let p = dir.path().join(format!("in{k}.nt"));
        let body: String = (0..30)
            .map(|i| {
                format!(
                    "<http://example.test/s{k}_{i}> <http://example.test/p{}> \"value {k} {i}\" .\n",
                    i % 3
                )
            })
            .collect();
        std::fs::write(&p, body).unwrap();
        inputs.push(p);
    }
    for (name, repro, extra) in RECORD_SHAPES {
        let out = dir
            .path()
            .join(format!("out-{}.rete", name.replace(' ', "-")));
        let mut a = vec!["build"];
        a.extend(inputs.iter().map(|p| s(p)));
        a.extend_from_slice(&[
            "-o",
            s(&out),
            "--memory-budget-mb",
            "64",
            "--card-file",
            s(&card),
        ]);
        a.extend_from_slice(extra);
        let err = run_stderr(&a, repro);
        assert_in_place(name, &err, &out);
    }
}

/// Merge with a card, with more than eight inputs: the same.
#[test]
fn merge_with_a_card_and_many_inputs_fits_its_record_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card.json");
    std::fs::write(&card, r#"{"title": "Merged"}"#).unwrap();
    let mut parts = Vec::new();
    for k in 0..10 {
        let src = dir.path().join(format!("p{k}.nt"));
        let body: String = (0..20)
            .map(|i| {
                format!(
                    "<http://example.test/s{k}_{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Label {k} {i}\" .\n"
                )
            })
            .collect();
        std::fs::write(&src, body).unwrap();
        let out = dir.path().join(format!("p{k}.rete"));
        run(&["build", s(&src), "-o", s(&out), "--no-pyramid"], false);
        parts.push(out);
    }
    for (name, repro, extra) in RECORD_SHAPES {
        let out = dir
            .path()
            .join(format!("m-{}.rete", name.replace(' ', "-")));
        let mut a = vec!["merge"];
        a.extend(parts.iter().map(|p| s(p)));
        a.extend_from_slice(&[
            "-o",
            s(&out),
            "--memory-budget-mb",
            "64",
            "--card-file",
            s(&card),
        ]);
        a.extend_from_slice(extra);
        let err = run_stderr(&a, repro);
        assert_in_place(name, &err, &out);
        let bytes = std::fs::read(&out).unwrap();
        assert!(stored_card(&bytes).is_some(), "{name}: a card");
    }
}

/// `--named-graphs per-graph` with the bounded build is refused by name, even
/// where the in-RAM hook still writes that layout.
#[test]
fn per_graph_is_refused_by_the_bounded_build() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("g.nq");
    std::fs::write(&src, catalog(true)).unwrap();
    for legacy in [false, true] {
        let mut c = rete();
        if legacy {
            c.env("RETE_WRITE_FORMAT", "5");
        } else {
            c.env_remove("RETE_WRITE_FORMAT");
        }
        let o = c
            .args(["build", s(&src), "-o", s(&dir.path().join("o.rete"))])
            .args(["--memory-budget-mb", "64", "--named-graphs", "per-graph"])
            .output()
            .unwrap();
        assert!(!o.status.success());
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("quad index only"), "{err}");
        assert!(!dir.path().join("o.rete").exists());
    }
}

/// A predicate-restricted text index (section kind 11) from the bounded
/// build is the in-RAM build's, byte for byte, and is flagged partial.
#[test]
fn a_predicate_restricted_text_index_is_identical() {
    let flags = [
        "--text-index",
        "--text-index-include",
        "http://purl.org/dc/terms/title",
    ];
    let bytes = three_ways("kind 11", "catalog.nq", &catalog(true), &flags);
    let h = rete_core::v06::Header06::from_bytes(&bytes).unwrap();
    assert!(h.has(rete_core::v06::P_TEXT), "a text index");
    assert!(
        h.has(rete_core::v06::P_TEXT_PARTIAL),
        "predicate-restricted: kind 11"
    );
    // And the scope that excludes instead, with a label index alongside.
    let flags = [
        "--text-index",
        "--text-index-exclude",
        "http://purl.org/dc/terms/title",
        "--label-index",
    ];
    let _ = three_ways("kind 11 exclude", "catalog.nt", &catalog(false), &flags);
}

/// A bounded build whose input spans several chunks, with tiles and
/// dictionary chunks cut small so every directory has several leaves: the
/// in-RAM build's bytes again.
#[test]
fn a_build_of_several_chunks_tiles_and_leaves_equals_the_in_ram_build() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.nt");
    {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(&src).unwrap());
        let n = 90_000u64;
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for i in 0..n {
            // ~400 bytes of words per literal, so a chunk fills at 64 MiB.
            let mut lit = String::with_capacity(420);
            while lit.len() < 400 {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                lit.push_str(&format!("w{:x} ", x >> 40));
            }
            writeln!(
                w,
                "<http://example.test/s{i}> <http://example.test/p{}> \"{lit}\" .",
                i % 5
            )
            .unwrap();
            if i % 3 == 0 {
                writeln!(
                    w,
                    "<http://example.test/s{i}> <http://example.test/link> <http://example.test/s{}> .",
                    (i * 7) % n
                )
                .unwrap();
            }
        }
    }
    let knobs = ["--tile-kib", "4", "--dict-chunk-kib", "4"];
    let (bounded, ram) = (dir.path().join("bounded.rete"), dir.path().join("ram.rete"));
    let mut a = vec![
        "build",
        s(&src),
        "-o",
        s(&bounded),
        "--memory-budget-mb",
        "64",
    ];
    a.extend_from_slice(&knobs);
    let err = run_stderr(&a, true);
    let chunks: u64 = err
        .lines()
        .find_map(|l| {
            let l = l.strip_prefix("extbuild: ")?;
            let (n, rest) = l.split_once(" chunk(s)")?;
            rest.contains("spilled").then(|| n.parse().ok()).flatten()
        })
        .expect("the chunk count is logged");
    assert!(chunks >= 2, "the input must span several chunks: {chunks}");
    let mut b = vec!["build", s(&src), "-o", s(&ram), "--no-pyramid"];
    b.extend_from_slice(&knobs);
    run_stderr(&b, true);
    let (x, y) = (
        std::fs::read(&bounded).unwrap(),
        std::fs::read(&ram).unwrap(),
    );
    assert!(
        x == y,
        "bounded build of {chunks} chunks differs from the in-RAM build"
    );
    // Several leaves in the dictionary directory.
    let (_, root) = rete_core::v06::open::read_front(&rete_core::SliceReader::new(&x)).unwrap();
    let dict =
        rete_core::v06::decode_dict_root(root.unit(rete_core::v06::tag::DICT).unwrap()).unwrap();
    assert!(
        dict.iter().any(|d| d.leaves.len() > 1),
        "several dictionary leaves"
    );
    rete().arg("verify").arg(&bounded).assert().success();
}
