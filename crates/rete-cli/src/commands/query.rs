//! The local-query group: evaluate queries against a `.rete` file on disk — a
//! single triple pattern (`query`), a Basic Graph Pattern (`bgp`), a SPARQL
//! query (`sparql`), and the read-only Cypher subset (`cypher`). The HTTP/range
//! variants live in `commands::url`; the result rendering lives in `main.rs`.

use rete_core::{eval_bgp, eval_query, PatternTerm, Rete, TriplePattern};

use crate::cypher;
use crate::print_query_output;

/// Query a triple pattern: unspecified positions are variables, terms match as
/// canonical N-Triples tokens.
pub(crate) fn query(
    file: &str,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let results = rete.query(s.as_deref(), p.as_deref(), o.as_deref());
    for (s, p, o) in &results {
        println!("{s} {p} {o} .");
    }
    eprintln!("{} result(s)", results.len());
    Ok(())
}

/// Evaluate a Basic Graph Pattern: patterns separated by ` . `, terms by spaces,
/// `?name` is a variable.
pub(crate) fn bgp(file: &str, query: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;

    let mut patterns = Vec::new();
    for clause in query.split(" . ") {
        let toks: Vec<&str> = clause.split_whitespace().collect();
        if toks.len() != 3 {
            anyhow::bail!("each pattern needs 3 terms, got: {clause:?}");
        }
        patterns.push(TriplePattern {
            s: PatternTerm::parse(toks[0]),
            p: PatternTerm::parse(toks[1]),
            o: PatternTerm::parse(toks[2]),
        });
    }

    let solutions = eval_bgp(&rete, &patterns);
    for sol in &solutions {
        let row: Vec<String> = sol.iter().map(|(k, v)| format!("?{k}={v}")).collect();
        println!("{}", row.join("  "));
    }
    eprintln!("{} solution(s)", solutions.len());
    Ok(())
}

/// Run a SPARQL query (SELECT / ASK / CONSTRUCT) against a local file.
pub(crate) fn sparql(file: &str, query: &str, json: bool) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let result = eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    Ok(())
}

/// Run a read-only Cypher-subset query: translate it to SPARQL (see
/// `cypher.rs`), evaluate with the existing engine, and render like `sparql`.
pub(crate) fn cypher_cmd(file: &str, query: &str, base: &str, json: bool) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    let result = cypher::eval_cypher(&rete, query, base).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    Ok(())
}
