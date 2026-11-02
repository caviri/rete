//! The card's cost measurement stays inside `--memory-budget-mb`.
//!
//! A bounded build runs the card's starter queries against the file it has
//! just written, and a query's own working state (a hash join's rows) is not
//! bounded by any cache. Under a budget each query therefore runs under a
//! memory ceiling and is stopped when it passes it; the build goes on, and the
//! record says which queries did not finish (`status: over_budget`) instead of
//! losing them or failing.
//!
//! The tests squeeze the ceiling (`RETE_COST_MEMORY_HEADROOM_MB=0` and a budget
//! below what the process holds) so that a small graph, which builds in
//! seconds, exercises what a billion-statement build meets.

mod common;

use common::{rete, Fixture};
use serde_json::Value;
use std::path::Path;

/// 5 000 subjects, each typed, labelled and linked to the next.
fn graph(fixture: &Fixture) -> std::path::PathBuf {
    let mut nt = String::new();
    for i in 0..5_000 {
        nt.push_str(&format!(
            "<http://example.test/s{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://example.test/Thing> .\n\
             <http://example.test/s{i}> <http://www.w3.org/2000/01/rdf-schema#label> \
             \"Thing number {i}\"@en .\n\
             <http://example.test/s{i}> <http://example.test/next> <http://example.test/s{}> .\n",
            (i + 1) % 5_000
        ));
    }
    fixture.write("graph.nt", nt)
}

fn build(source: &Path, out: &Path, budget_mb: &str, headroom_mb: &str, extra: &[&str]) {
    rete()
        .env("RETE_COST_MEMORY_HEADROOM_MB", headroom_mb)
        .arg("build")
        .arg(source)
        .arg("-o")
        .arg(out)
        .args(["--card", "--memory-budget-mb", budget_mb])
        .args(extra)
        .assert()
        .success();
}

fn card(file: &Path) -> Value {
    common::json(rete().arg("card").arg("--json").arg(file))
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|q| q["id"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// With room to spare every starter query is measured and none is marked; with
/// a budget the measurement cannot meet the build still finishes, no starter
/// query goes missing without a record, and the ones that did not finish say
/// so. The record of the roomy run also covers the cost step: the
/// `starter_queries` phase is there and no higher than the run's peak RSS.
#[test]
fn a_small_budget_records_the_queries_it_stopped_and_loses_none() {
    let fixture = common::fixture();
    let source = graph(&fixture);

    let roomy = fixture.path("roomy.rete");
    build(&source, &roomy, "4096", "32", &[]);
    let doc = card(&roomy);
    let generated = ids(&doc["build"]["query_costs"]["queries"]);
    assert!(generated.len() > 8, "{generated:?}");
    for c in doc["build"]["query_costs"]["queries"].as_array().unwrap() {
        assert!(c.get("status").is_none(), "{c}");
        assert!(c["rows"].as_u64().unwrap() > 0, "{c}");
    }
    let run = &doc["build"]["run"];
    let peak = run["peak_rss_bytes"].as_u64().unwrap();
    let phase = run["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["phase"] == "starter_queries")
        .expect("the cost step is a phase of the record");
    assert!(phase["peak_rss_bytes"].as_u64().unwrap() <= peak, "{run}");

    let tight = fixture.path("tight.rete");
    build(&source, &tight, "8", "0", &[]);
    let doc = card(&tight);
    let costs = &doc["build"]["query_costs"]["queries"];
    let stopped = costs
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["status"] == "over_budget")
        .count();
    let dropped = ids(&doc["build"]["dropped_queries"]);
    assert!(
        stopped > 0 || !dropped.is_empty(),
        "a budget of 8 MiB stopped nothing: {costs}"
    );

    // Every query the roomy build measured is accounted for: costed (finished
    // or marked over budget) or dropped with its reason. None vanished.
    let mut seen = ids(costs);
    seen.extend(dropped.iter().cloned());
    seen.sort();
    let mut want = generated.clone();
    want.sort();
    assert_eq!(
        seen, want,
        "costs + dropped_queries must cover the starters"
    );

    // A query that stayed in the card but did not finish says what it spent
    // and has no rows; the ones that finished carry no status at all.
    for c in costs.as_array().unwrap() {
        match c["status"].as_str() {
            None => assert!(c["rows"].as_u64().unwrap() > 0, "{c}"),
            Some(s) => {
                assert_eq!(s, "over_budget", "{c}");
                assert_eq!(c["rows"], 0, "{c}");
            }
        }
    }
    // What was dropped for a budget says so.
    for d in doc["build"]["dropped_queries"]
        .as_array()
        .into_iter()
        .flatten()
    {
        assert!(d["why"].as_str().unwrap().contains("did not finish"), "{d}");
    }

    // The file is whole.
    rete().arg("verify").arg(&tight).assert().success();
}
