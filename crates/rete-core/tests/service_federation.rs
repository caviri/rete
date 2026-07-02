//! SPARQL 1.1 `SERVICE` federation: engine behavior with a mock
//! [`ServiceClient`] — join semantics, SILENT degradation, error surfacing —
//! no network involved (the CLI/wasm hosts own transport).

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset, parse_statements};
use rete_core::{eval_sparql, query_predicates, Binding, Rete, ServiceClient, SparqlError};

/// A scripted endpoint: records every (endpoint, query) it receives and
/// returns canned solutions (or fails).
#[derive(Default)]
struct MockState {
    sent: Mutex<Vec<(String, String)>>,
    solutions: Vec<Binding>,
    fail: bool,
}

struct Mock(Arc<MockState>);

impl ServiceClient for Mock {
    fn query(&self, endpoint: &str, query: &str) -> Result<Vec<Binding>, String> {
        self.0
            .sent
            .lock()
            .unwrap()
            .push((endpoint.to_string(), query.to_string()));
        if self.0.fail {
            Err("endpoint unreachable".to_string())
        } else {
            Ok(self.0.solutions.clone())
        }
    }
}

fn binding(pairs: &[(&str, &str)]) -> Binding {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A small local graph: two entities with codes, one with a label.
fn local_file() -> Vec<u8> {
    let nt = "<http://ex/a> <http://ex/code> \"42\" .\n\
              <http://ex/b> <http://ex/code> \"99\" .\n\
              <http://ex/a> <http://ex/name> \"A\" .\n";
    let quads = parse_statements(nt, "nt").unwrap();
    assemble_dataset(quads, &[]).0
}

fn open_with(state: Arc<MockState>) -> Rete {
    let bytes = local_file();
    let mut rete = Rete::open(&bytes).unwrap();
    rete.set_service_client(Box::new(Mock(state)));
    rete
}

#[test]
fn pure_service_block_returns_remote_solutions() {
    let state = Arc::new(MockState {
        solutions: vec![
            binding(&[("x", "<http://w/e1>"), ("y", "\"one\"@en")]),
            binding(&[("x", "<http://w/e2>"), ("y", "\"two\"@en")]),
        ],
        ..Default::default()
    });
    let rete = open_with(state.clone());
    let q = "SELECT ?x ?y WHERE { SERVICE <http://remote/sparql> { ?x <http://w/p> ?y } }";
    let (_, rows) = eval_sparql(&rete, q).unwrap();
    assert_eq!(rows.len(), 2);
    let mut ys: Vec<&str> = rows.iter().map(|b| b["y"].as_str()).collect();
    ys.sort();
    assert_eq!(ys, ["\"one\"@en", "\"two\"@en"]);

    // Exactly one call, to the right endpoint, and the shipped sub-query is
    // itself valid SPARQL naming the block's predicate.
    let sent = state.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "http://remote/sparql");
    let shipped_preds = query_predicates(&sent[0].1).unwrap();
    assert!(
        shipped_preds.contains("<http://w/p>"),
        "shipped query {:?} must contain the block's pattern",
        sent[0].1
    );
}

#[test]
fn service_solutions_join_local_bindings_on_shared_vars() {
    // Remote knows labels by code; only code "42" exists locally on <http://ex/a>.
    let state = Arc::new(MockState {
        solutions: vec![
            binding(&[("code", "\"42\""), ("label", "\"Answer\"@en")]),
            binding(&[("code", "\"77\""), ("label", "\"Nobody\"@en")]),
        ],
        ..Default::default()
    });
    let rete = open_with(state);
    let q = "SELECT ?s ?label WHERE { \
                ?s <http://ex/code> ?code . \
                SERVICE <http://remote/sparql> { ?e <http://w/code> ?code ; <http://w/label> ?label } \
             }";
    let (_, rows) = eval_sparql(&rete, q).unwrap();
    assert_eq!(rows.len(), 1, "only code 42 co-occurs");
    assert_eq!(rows[0]["s"], "<http://ex/a>");
    assert_eq!(rows[0]["label"], "\"Answer\"@en");
}

#[test]
fn silent_failure_degrades_to_one_empty_solution() {
    let state = Arc::new(MockState {
        fail: true,
        ..Default::default()
    });
    let rete = open_with(state);
    // Per the spec, a failed SERVICE SILENT contributes a single empty
    // solution — the local rows survive with ?label unbound.
    let q = "SELECT ?s ?label WHERE { \
                ?s <http://ex/code> ?code . \
                SERVICE SILENT <http://remote/sparql> { ?e <http://w/label> ?label } \
             }";
    let (_, rows) = eval_sparql(&rete, q).unwrap();
    assert_eq!(rows.len(), 2, "both local rows survive");
    assert!(rows.iter().all(|b| !b.contains_key("label")));
}

#[test]
fn failure_without_silent_errors_and_does_not_leak() {
    let state = Arc::new(MockState {
        fail: true,
        ..Default::default()
    });
    let rete = open_with(state);
    let q = "SELECT ?x WHERE { SERVICE <http://remote/sparql> { ?x ?p ?o } }";
    let err = eval_sparql(&rete, q).unwrap_err();
    assert!(
        matches!(&err, SparqlError::Service(m) if m.contains("unreachable")),
        "got {err}"
    );
    // The recorded failure must not poison a later, SERVICE-free query.
    let (_, rows) = eval_sparql(&rete, "SELECT ?s WHERE { ?s <http://ex/code> ?c }").unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn no_attached_client_is_a_clear_error() {
    let bytes = local_file();
    let rete = Rete::open(&bytes).unwrap();
    let q = "SELECT ?x WHERE { SERVICE <http://remote/sparql> { ?x ?p ?o } }";
    let err = eval_sparql(&rete, q).unwrap_err();
    assert!(matches!(&err, SparqlError::Service(m) if m.contains("no SERVICE client")));
}

#[test]
fn variable_endpoint_is_unsupported() {
    let bytes = local_file();
    let rete = Rete::open(&bytes).unwrap();
    let q = "SELECT ?x WHERE { SERVICE ?ep { ?x ?p ?o } }";
    assert!(matches!(
        eval_sparql(&rete, q),
        Err(SparqlError::Unsupported(_))
    ));
}
