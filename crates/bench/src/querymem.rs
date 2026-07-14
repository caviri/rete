//! Query-path memory profiler: for one `.rete` + SPARQL query, measure the peak
//! heap and time of (a) `eval_query` and (b) building the playground JSON
//! envelope — the work the WASM `query()` does before the result string crosses
//! the worker boundary. It contrasts the **current** serializer (build a
//! `serde_json::Value` tree, then `to_string`) with a **direct-to-string** one,
//! to size the win before changing the shipped code.
//!
//! Run: `cargo run --release -p rete-bench -- --query-mem <file.rete> "<sparql>"`

use std::time::Instant;

use anyhow::{Context, Result};
use rete_core::{eval_query, QueryOutput, Rete};
use serde_json::{json, Map, Value};

use crate::mem;

/// Median ms + peak extra heap over `reps`, each window reset.
fn measure(reps: usize, mut f: impl FnMut()) -> (usize, f64) {
    let baseline = mem::live();
    let mut times = Vec::with_capacity(reps);
    let mut peak = 0usize;
    for _ in 0..reps {
        mem::reset_peak();
        let t = Instant::now();
        f();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        peak = peak.max(mem::peak().saturating_sub(baseline));
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (peak, times[times.len() / 2])
}

fn row(stage: &str, peak: usize, ms: f64) {
    println!("| {stage:<28} | {:>10} | {:>8.2} |", mem::mib(peak), ms);
}

/// SELECT/CONSTRUCT/ASK variable order, mirroring `query_value`.
fn select_vars(project: &[String], solutions: &[rete_core::Binding]) -> Vec<String> {
    if !project.is_empty() {
        return project.to_vec();
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut vars = Vec::new();
    for s in solutions {
        for k in s.keys() {
            if seen.insert(k.clone()) {
                vars.push(k.clone());
            }
        }
    }
    vars
}

/// CURRENT path: build a `serde_json::Value` envelope, then `to_string`.
fn serialize_via_value(out: &QueryOutput) -> String {
    let v = match out {
        QueryOutput::Ask(b) => json!({ "kind": "ask", "boolean": b }),
        QueryOutput::Select(project, solutions) => {
            let vars = select_vars(project, solutions);
            let rows: Vec<Value> = solutions
                .iter()
                .map(|s| {
                    let mut obj = Map::new();
                    for var in &vars {
                        if let Some(term) = s.get(var) {
                            obj.insert(var.clone(), Value::String(term.clone()));
                        }
                    }
                    Value::Object(obj)
                })
                .collect();
            json!({ "kind": "select", "vars": vars, "rows": rows })
        }
        QueryOutput::Construct(triples) => {
            let arr: Vec<Value> = triples.iter().map(|(s, p, o)| json!([s, p, o])).collect();
            json!({ "kind": "construct", "triples": arr })
        }
        _ => json!({ "error": "unsupported query result kind" }),
    };
    serde_json::to_string(&v).unwrap()
}

/// DIRECT path: the **shipped** writer (`rete_core::results_envelope_json`) — no
/// `Value` tree, no per-cell clones — so this profiles the real code.
fn serialize_direct(out: &QueryOutput) -> String {
    rete_core::results_envelope_json(out, "")
}

pub fn run(path: &str, query: &str) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {path}"))?;
    let rete = Rete::open(&bytes)?;
    let out = eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    let shape = match &out {
        QueryOutput::Select(_, rows) => format!("SELECT — {} rows", rows.len()),
        QueryOutput::Ask(b) => format!("ASK — {b}"),
        QueryOutput::Construct(t) => format!("CONSTRUCT — {} triples", t.len()),
        _ => "unsupported query result kind".to_owned(),
    };
    // Confirm the two serializers agree as JSON (key order may differ; compare the
    // parsed values so a refactor can't silently change the payload).
    let a: Value = serde_json::from_str(&serialize_via_value(&out))?;
    let b: Value = serde_json::from_str(&serialize_direct(&out))?;
    let agree = a == b;

    println!("# Query-memory profile: `{path}`\n");
    println!("`{query}`\n\n{shape} · serializers agree (as JSON values): {agree}\n");
    println!("| Stage | peak heap MiB | ms |");
    println!("|---|--:|--:|");

    let reps = 7;
    let (ep, em) = measure(reps, || {
        std::hint::black_box(eval_query(&rete, query).ok());
    });
    row("eval_query", ep, em);
    let (vp, vm) = measure(reps, || {
        std::hint::black_box(serialize_via_value(&out));
    });
    row("serialize: Value + to_string", vp, vm);
    let (dp, dm) = measure(reps, || {
        std::hint::black_box(serialize_direct(&out));
    });
    row("serialize: direct to String", dp, dm);

    let bytes_len = serialize_direct(&out).len();
    println!(
        "\nresult JSON: {bytes_len} bytes · serialization peak heap {:.1}× the payload (Value) vs {:.1}× (direct)",
        vp as f64 / bytes_len as f64,
        dp as f64 / bytes_len as f64,
    );
    Ok(())
}
