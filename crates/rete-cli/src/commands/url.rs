//! The remote (HTTP range) group: read a `.rete` over `http(s)://` by fetching
//! only the byte ranges each operation needs — the pyramid summary
//! (`summary_url`), a triple pattern (`query_url`), or a full SPARQL query
//! (`sparql_url`) — never a full download. Each wraps the transport in a
//! `CountingReader` and reports bytes fetched + range-request count. The on-disk
//! variants live in `commands::query` / `commands::inspect`.

use rete_core::{eval_query, CountingReader, RangeReader, Rete, SummaryView};

use crate::commands::render::print_query_output;
use crate::http::HttpRangeReader;

/// Fetch just the pyramid summary (coarse graph) over HTTP — header, dictionary,
/// and summary only, skipping the (large) triple index.
pub(crate) fn summary_url(url: &str) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    match SummaryView::open_ranged(&reader)? {
        Some(view) => {
            println!(
                "pyramid round {} — {} communities, {} superedge(s):",
                view.round,
                view.community_count(),
                view.summary.len()
            );
            for e in &view.summary {
                let pred = view
                    .predicate_term(e.predicate)
                    .unwrap_or_else(|| format!("#{}", e.predicate));
                let arrow = if e.s_comm == e.o_comm {
                    "(internal)"
                } else {
                    "->"
                };
                println!(
                    "  C{} {arrow} C{}  via {pred}  x{}",
                    e.s_comm, e.o_comm, e.count
                );
            }
            eprintln!(
                "fetched {} of {} bytes in {} range request(s) — index NOT fetched",
                reader.bytes_read(),
                total,
                reader.requests()
            );
        }
        None => eprintln!("file has no pyramid"),
    }
    Ok(())
}

/// Query a triple pattern over HTTP, fetching only the byte ranges needed.
pub(crate) fn query_url(
    url: &str,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    let rete = Rete::open_ranged(&reader)?;
    let results = rete.query(s.as_deref(), p.as_deref(), o.as_deref());
    for (s, p, o) in &results {
        println!("{s} {p} {o} .");
    }
    eprintln!(
        "{} result(s) · fetched {} bytes in {} range request(s) (file is {} bytes)",
        results.len(),
        reader.bytes_read(),
        reader.requests(),
        total
    );
    Ok(())
}

/// Run SPARQL against a `.rete` over HTTP(S), fetching only the byte ranges the
/// open needs (header, dictionary, index, pyramid) — never a full download.
pub(crate) fn sparql_url(url: &str, query: &str, json: bool) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    let rete = Rete::open_ranged(&reader)?;
    let result = eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, json);
    eprintln!(
        "(fetched {} bytes in {} range request(s); file is {} bytes)",
        reader.bytes_read(),
        reader.requests(),
        total
    );
    Ok(())
}
