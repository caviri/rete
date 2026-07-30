//! The local-query group: evaluate queries against a `.rete` file on disk — a
//! single triple pattern (`query`), a Basic Graph Pattern (`bgp`), a SPARQL
//! query (`sparql`), and the read-only Cypher subset (`cypher`). The HTTP/range
//! variants live in `commands::url`; the result rendering lives in `main.rs`.

use rete_core::{eval_bgp, eval_query, ByteRange, PatternTerm, TriplePattern, TripleProvenance};
use serde_json::json;

use crate::commands::range_source::open_local;
use crate::commands::render::print_query_output;
use crate::cypher;

/// Query a triple pattern: unspecified positions are variables, terms match as
/// canonical N-Triples tokens.
pub(crate) fn query(
    file: &str,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let results = rete.query(s.as_deref(), p.as_deref(), o.as_deref());
    for (s, p, o) in &results {
        println!("{s} {p} {o} .");
    }
    eprintln!("{} result(s)", results.len());
    Ok(())
}

fn range_json(range: ByteRange) -> serde_json::Value {
    json!({
        "offset": range.offset,
        "len": range.len,
        "end": range.end(),
    })
}

fn provenance_json(m: &TripleProvenance) -> serde_json::Value {
    let tile = match (&m.tile, m.tile_range) {
        (Some(id), Some(range)) => json!({
            "available": true,
            "id": id,
            "range": range_json(range),
        }),
        (Some(id), None) => json!({
            "available": true,
            "id": id,
        }),
        _ => json!({
            "available": false,
            "reason": "not_materialized",
        }),
    };

    json!({
        "terms": {
            "subject": m.terms.0,
            "predicate": m.terms.1,
            "object": m.terms.2,
        },
        "ids": {
            "subject": m.ids.0,
            "predicate": m.ids.1,
            "object": m.ids.2,
        },
        "provenance": {
            "graph": m.graph.as_deref().unwrap_or("default"),
            "matched_pattern": {
                "subject": m.matched_pattern.0,
                "predicate": m.matched_pattern.1,
                "object": m.matched_pattern.2,
            },
            "index_permutation": m.index_permutation.name(),
            "index_section": m.index_permutation.section_index(),
            "dictionary_range": range_json(m.dictionary_range),
            "index_range": range_json(m.index_range),
            "index_section_range": range_json(m.index_section_range),
            "pyramid_range": m.pyramid_range.map(range_json),
            "tile": tile,
        },
    })
}

/// Explain the provenance of each triple-pattern match.
pub(crate) fn why(
    file: &str,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
    as_json: bool,
) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let results = rete.query_with_provenance(s.as_deref(), p.as_deref(), o.as_deref());
    print_provenance(s.as_deref(), p.as_deref(), o.as_deref(), &results, as_json)
}

/// Render triple-pattern provenance (text or JSON). Shared by the on-disk `why`
/// and the remote [`crate::commands::url::why_url`].
pub(crate) fn print_provenance(
    s: Option<&str>,
    p: Option<&str>,
    o: Option<&str>,
    results: &[TripleProvenance],
    as_json: bool,
) -> anyhow::Result<()> {
    if as_json {
        let out = json!({
            "schemaVersion": crate::JSON_SCHEMA_VERSION,
            "pattern": { "subject": s, "predicate": p, "object": o },
            "result_count": results.len(),
            "results": results.iter().map(provenance_json).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for m in results {
            println!("{} {} {} .", m.terms.0, m.terms.1, m.terms.2);
            println!("  ids: s={} p={} o={}", m.ids.0, m.ids.1, m.ids.2);
            println!("  graph: {}", m.graph.as_deref().unwrap_or("default"));
            println!(
                "  index: {} section {} bytes [{}..{})",
                m.index_permutation.name(),
                m.index_permutation.section_index(),
                m.index_range.offset,
                m.index_range.end()
            );
            println!(
                "  index section payload: bytes [{}..{})",
                m.index_section_range.offset,
                m.index_section_range.end()
            );
            println!(
                "  dictionary: bytes [{}..{})",
                m.dictionary_range.offset,
                m.dictionary_range.end()
            );
            if let Some(range) = m.pyramid_range {
                println!("  pyramid: bytes [{}..{})", range.offset, range.end());
            } else {
                println!("  pyramid: absent");
            }
            match (&m.tile, m.tile_range) {
                (Some(id), Some(range)) => {
                    println!("  tile: {id} bytes [{}..{})", range.offset, range.end())
                }
                (Some(id), None) => println!("  tile: {id}"),
                _ => println!("  tile: not materialized (pre-tiling file)"),
            }
        }
        eprintln!("{} result(s)", results.len());
    }

    Ok(())
}

/// Evaluate a Basic Graph Pattern: patterns separated by ` . `, terms by spaces,
/// `?name` is a variable.
pub(crate) fn bgp(file: &str, query: &str) -> anyhow::Result<()> {
    let rete = open_local(file)?;

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
pub(crate) fn sparql(file: &str, query: &str, json: bool, entail: bool) -> anyhow::Result<()> {
    let mut rete = open_local(file)?;
    // SERVICE blocks federate to remote SPARQL endpoints over HTTP.
    rete.set_service_client(Box::new(super::service_http::HttpServiceClient));
    let eval = if entail {
        rete_core::eval_query_reasoned
    } else {
        eval_query
    };
    let result = eval(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    Ok(())
}

/// Run a read-only Cypher-subset query: translate it to SPARQL (see
/// `cypher.rs`), evaluate with the existing engine, and render like `sparql`.
pub(crate) fn cypher_cmd(file: &str, query: &str, base: &str, json: bool) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let result = cypher::eval_cypher(&rete, query, base).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    Ok(())
}
