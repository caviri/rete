//! The build record (kind-7 build-info section): how a file was made, in the
//! file. `rete build`, `repyramid` and `recard` write it, in both build modes;
//! `rete card` shows it (text, `--json`, `--format jsonld`).
//!
//! What is pinned here, beyond "the fields exist":
//! * the record names inputs by file name only — never a directory, which can
//!   name a user or a machine — and its digest is the BLAKE3 of the bytes on
//!   disk, gzip or not, stdin included;
//! * the counts are the file's (read vs unique vs duplicates);
//! * `RETE_BUILD_RECORD=reproducible` makes two builds byte-identical, record
//!   included, and without it the files still differ ONLY in the record.

mod common;

use std::io::Write as _;
use std::path::Path;

use common::rete;
use serde_json::Value;

/// Two statements are the same, one is in a named graph.
const NQ: &str = concat!(
    "<http://example.test/alice> <http://example.test/knows> <http://example.test/bob> .\n",
    "<http://example.test/alice> <http://example.test/knows> <http://example.test/bob> .\n",
    "<http://example.test/bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.test/Person> .\n",
    "<http://example.test/bob> <http://example.test/name> \"Bob\"@en .\n",
    "<http://example.test/alice> <http://example.test/name> \"Alice\"@en <http://example.test/people> .\n",
);

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

fn record(file: &Path) -> Value {
    let out = rete().arg("card").arg("--json").arg(file).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["build"].clone()
}

fn build(src: &Path, out: &Path, extra: &[&str]) {
    let o = rete()
        .arg("build")
        .arg(src)
        .arg("-o")
        .arg(out)
        .args(extra)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

struct Setup {
    dir: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.nq.gz"), gzip(NQ.as_bytes())).unwrap();
        std::fs::write(
            dir.path().join("my card.json"),
            r#"{"title":"Record test"}"#,
        )
        .unwrap();
        Setup { dir }
    }
    fn p(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }
    fn card_flag(&self) -> String {
        self.p("my card.json").to_string_lossy().into_owned()
    }
}

/// Everything a reader needs to know how the file was made, and nothing about
/// where: the inputs by file name, the flags by effective value, the counts.
#[test]
fn in_ram_build_records_flags_inputs_counts_and_run() {
    let s = Setup::new();
    let out = s.p("ram.rete");
    let card = s.card_flag();
    build(
        &s.p("data.nq.gz"),
        &out,
        &[
            "--card-file",
            &card,
            "--label-index",
            "--text-index",
            "--permutations",
            "3",
            "--zstd-level",
            "12",
        ],
    );
    let b = record(&out);

    // Engine and host.
    assert_eq!(b["engine"]["name"], "rete-cli");
    assert!(b["engine"]["commit"]
        .as_str()
        .is_some_and(|c| !c.is_empty()));
    assert!(b["engine"]["target"].as_str().unwrap().contains('-'));
    let features: Vec<&str> = b["engine"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert!(features.contains(&"compression") && features.contains(&"parallel"));
    assert_eq!(b["host"]["os"], std::env::consts::OS);
    assert_eq!(b["host"]["arch"], std::env::consts::ARCH);
    assert_eq!(b["host"].as_object().unwrap().len(), 2, "OS and arch only");

    // Flags, as effective values.
    let p = &b["params"];
    assert_eq!(p["command"], "build");
    assert_eq!(p["permutations"], 3);
    assert_eq!(p["named_graphs"], "per-graph");
    assert_eq!(p["strict"], false);
    assert_eq!(p["collapse_graphs"], false);
    assert_eq!(p["quoted_triple_syntax"], "rdf-star");
    assert_eq!(p["label_index"], true);
    assert_eq!(p["text_index"], true);
    assert_eq!(p["card_costs"], true);
    assert_eq!(p["write"]["zstd_level"], 12);
    assert_eq!(p["card_file"]["name"], "my card.json");
    assert_eq!(
        p["card_file"]["blake3"],
        blake3::hash(br#"{"title":"Record test"}"#)
            .to_hex()
            .as_str()
    );

    // The input: name only, the bytes on disk, their BLAKE3.
    let gz = std::fs::read(s.p("data.nq.gz")).unwrap();
    let input = &b["inputs"][0];
    assert_eq!(input["name"], "data.nq.gz");
    assert_eq!(input["format"], "nq");
    assert_eq!(input["compression"], "gzip");
    assert_eq!(input["bytes"], gz.len() as u64);
    assert_eq!(input["blake3"], blake3::hash(&gz).to_hex().as_str());

    // Counts: 5 read, 1 duplicate, 4 unique, 1 of them in one named graph.
    let c = &b["counts"];
    assert_eq!(c["statements_read"], 5);
    assert_eq!(c["statements_unique"], 4);
    assert_eq!(c["duplicates_dropped"], 1);
    assert_eq!(c["default_graph_triples"], 3);
    assert_eq!(c["named_graphs"], 1);
    assert_eq!(c["named_graph_quads"], 1);

    // The run: timed phases, peak RSS where there is procfs.
    let run = &b["run"];
    assert!(run["wall_ms"].is_u64());
    let phases: Vec<&str> = run["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["phase"].as_str().unwrap())
        .collect();
    for want in [
        "parse+dictionary",
        "parse+encode",
        "index_permutations",
        "text_index",
        "labels+schema",
        "starter_queries",
        "write",
    ] {
        assert!(phases.contains(&want), "no {want} phase in {phases:?}");
    }
    if cfg!(target_os = "linux") {
        assert!(run["peak_rss_bytes"].as_u64().unwrap() > 0);
    } else {
        assert!(run["peak_rss_bytes"].is_null());
    }
    assert!(run["cpus"].as_u64().unwrap() >= 1);
    assert!(b["started_at"].is_string() && b["built_at"].is_string());
    assert!(
        b.get("spill").is_none(),
        "spill is the memory-bounded build's"
    );

    // No directory anywhere in the record: paths name users and machines.
    let dir = s.dir.path().to_string_lossy().into_owned();
    let text = serde_json::to_string(&b).unwrap();
    assert!(!text.contains(&dir), "the record leaks a path: {text}");
}

/// The memory-bounded build writes the same record, plus what its spill did,
/// from the same one pass over the input — and the same digest.
#[test]
fn memory_bounded_build_records_spill_and_the_same_digest() {
    let s = Setup::new();
    let ram = s.p("ram.rete");
    let ext = s.p("ext.rete");
    let card = s.card_flag();
    build(&s.p("data.nq.gz"), &ram, &["--card-file", &card]);
    build(
        &s.p("data.nq.gz"),
        &ext,
        &[
            "--card-file",
            &card,
            "--memory-budget-mb",
            "64",
            "--named-graphs",
            "quad",
        ],
    );
    let (r, e) = (record(&ram), record(&ext));
    assert_eq!(e["params"]["command"], "build --memory-budget-mb");
    assert_eq!(e["params"]["memory_budget_mb"], 64);
    assert_eq!(e["params"]["named_graphs"], "quad");
    assert_eq!(e["params"]["codec"], r["params"]["codec"]);
    assert_eq!(
        e["inputs"], r["inputs"],
        "one digest, whichever build read it"
    );
    assert_eq!(e["counts"]["statements_read"], 5);
    assert_eq!(e["counts"]["statements_unique"], 4);
    assert_eq!(e["counts"]["duplicates_dropped"], 1);
    let spill = &e["spill"];
    assert_eq!(spill["chunks"], 1);
    assert!(spill["merge_passes"].as_u64().unwrap() >= 6, "{spill}");
    assert!(spill["scratch_peak_bytes"].as_u64().unwrap() > 0);
    let phases: Vec<&str> = e["run"]["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases.first(), Some(&"parse+chunk"), "{phases:?}");
    assert!(phases.contains(&"write"));
    // The full card is derived from the written file, after it: its phases
    // come last, and the record carries what its starter queries cost.
    assert_eq!(
        &phases[phases.len() - 3..],
        ["card_derive", "starter_queries", "content_hash"],
        "{phases:?}"
    );
    assert!(e["query_costs"]["queries"].is_array(), "{e}");
    assert_eq!(e["params"]["card_top_n"], r["params"]["card_top_n"]);

    // stdin: hashed as it streams, named `stdin`.
    let stdin_out = s.p("stdin.rete");
    let o = rete()
        .args(["build", "-", "--format", "nq", "--memory-budget-mb", "64"])
        .args(["--card-file", &card, "-o"])
        .arg(&stdin_out)
        .write_stdin(NQ)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let i = &record(&stdin_out)["inputs"][0];
    assert_eq!(i["name"], "stdin");
    assert_eq!(i["bytes"], NQ.len() as u64);
    assert_eq!(i["blake3"], blake3::hash(NQ.as_bytes()).to_hex().as_str());
    assert!(i.get("compression").is_none());
}

/// Without the switch, two builds differ in the record and nowhere else; with
/// `RETE_BUILD_RECORD=reproducible` (and `SOURCE_DATE_EPOCH`), not even there.
#[test]
fn reproducible_record_makes_two_builds_byte_identical() {
    let s = Setup::new();
    let card = s.card_flag();
    for extra in [&[][..], &["--memory-budget-mb", "64"][..]] {
        let mut files = Vec::new();
        for n in 0..2 {
            let out = s.p(&format!("repro{n}.rete"));
            let o = rete()
                .env("RETE_BUILD_RECORD", "reproducible")
                .env("SOURCE_DATE_EPOCH", "1785801600")
                .arg("build")
                .arg(s.p("data.nq.gz"))
                .arg("-o")
                .arg(&out)
                .args(["--card-file", &card])
                .args(extra)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            files.push(std::fs::read(&out).unwrap());
        }
        assert!(files[0] == files[1], "{extra:?}: not byte-identical");
        let b = record(&s.p("repro0.rete"));
        assert!(b.get("run").is_none(), "no volatile run block");
        assert_eq!(b["built_at"], "2026-08-04T00:00:00Z");
        assert_eq!(b["started_at"], "2026-08-04T00:00:00Z");
        if let Some(costs) = b["query_costs"]["queries"].as_array() {
            assert!(costs.iter().all(|q| q["debug_ms"] == 0));
        }
    }
}

/// `rete card` shows the record as a Build block; `--format jsonld` maps the
/// parts PROV-O has terms for and keeps the rest under `rete:`.
#[test]
fn card_shows_the_build_block_and_jsonld_maps_it_to_prov() {
    let s = Setup::new();
    let out = s.p("shown.rete");
    let card = s.card_flag();
    build(
        &s.p("data.nq.gz"),
        &out,
        &["--card-file", &card, "--strict"],
    );

    let text = rete().arg("card").arg(&out).output().unwrap();
    let text = String::from_utf8(text.stdout).unwrap();
    for want in [
        "  build:",
        "engine      : rete-cli",
        "host        : ",
        "command     : rete build --permutations 6 --named-graphs per-graph",
        "--strict",
        "--card-file my card.json",
        "inputs      : data.nq.gz (nq, gzip)",
        "statements  : 5 read, 4 unique (1 duplicate(s) dropped)",
        "wall time   : ",
        "peak RAM    : ",
        "phases      : ",
    ] {
        assert!(text.contains(want), "no {want:?} in:\n{text}");
    }

    let ld = rete()
        .args(["card", "--format", "jsonld"])
        .arg(&out)
        .output()
        .unwrap();
    let ld: Value = serde_json::from_slice(&ld.stdout).unwrap();
    let act = &ld["prov:wasGeneratedBy"];
    assert_eq!(act["@type"], "prov:Activity");
    assert!(act["prov:startedAtTime"].is_string());
    assert!(act["prov:endedAtTime"].is_string());
    assert_eq!(
        act["prov:wasAssociatedWith"]["@type"],
        "SoftwareApplication"
    );
    assert!(act["prov:wasAssociatedWith"]["rete:gitCommit"].is_string());
    let used = act["prov:used"].as_array().unwrap();
    assert_eq!(used[0]["name"], "data.nq.gz");
    assert_eq!(used[0]["encodingFormat"], "application/n-quads");
    assert_eq!(used[0]["rete:blake3"].as_str().unwrap().len(), 64);
    assert!(used.iter().any(|u| u["rete:role"] == "card-file"));
    assert_eq!(act["rete:params"]["strict"], true);
    assert!(act["rete:run"]["wall_ms"].is_u64());
    assert_eq!(ld["@context"]["rete:run"]["@type"], "@json");
}

/// `repyramid` and `recard` write it too. A re-card keeps the build's record
/// and adds its own: engine, the file it read (by name), its flags, its run.
#[test]
fn repyramid_and_recard_record_themselves() {
    let s = Setup::new();
    let base = s.p("base.rete");
    build(&s.p("data.nq.gz"), &base, &["--no-pyramid"]);
    let rep = s.p("rep.rete");
    let card = s.card_flag();
    let o = rete()
        .arg("repyramid")
        .arg(&base)
        .arg("-o")
        .arg(&rep)
        .args(["--card-file", &card])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let b = record(&rep);
    assert_eq!(b["params"]["command"], "repyramid");
    assert!(
        b["params"].get("strict").is_none(),
        "no such flag on repyramid"
    );
    let base_bytes = std::fs::read(&base).unwrap();
    assert_eq!(b["inputs"][0]["name"], "base.rete");
    assert_eq!(b["inputs"][0]["format"], "rete");
    assert_eq!(
        b["inputs"][0]["blake3"],
        blake3::hash(&base_bytes).to_hex().as_str()
    );
    assert_eq!(b["counts"]["statements_unique"], 4);
    assert_eq!(b["run"]["phases"][0]["phase"], "read");

    let rc = s.p("recarded.rete");
    let o = rete()
        .arg("recard")
        .arg(&rep)
        .arg("-o")
        .arg(&rc)
        .arg("--keep-curated")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let r = record(&rc);
    assert_eq!(
        r["params"]["command"], "repyramid",
        "the build's record is kept"
    );
    assert_eq!(r["inputs"], b["inputs"]);
    let re = &r["recarded"];
    assert_eq!(re["engine"]["name"], "rete-cli");
    assert_eq!(re["input"]["name"], "rep.rete");
    assert_eq!(re["params"]["keep_curated"], true);
    assert!(re["run"]["wall_ms"].is_u64());
    let transport = r["query_costs"]["context"]["transport"].as_str().unwrap();
    assert!(
        !transport.contains(&*s.dir.path().to_string_lossy()),
        "the cost context names the file, not its path: {transport}"
    );
    rete().arg("verify").arg(&rc).assert().success();
}
