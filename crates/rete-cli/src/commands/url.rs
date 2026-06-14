//! The remote (HTTP range) group: read a `.rete` over `http(s)://` by fetching
//! only the byte ranges each operation needs — the pyramid summary
//! (`summary_url`), a triple pattern (`query_url`), or a full SPARQL query
//! (`sparql_url`) — never a full download. Each wraps the transport in a
//! `CountingReader` and reports bytes fetched + range-request count. The on-disk
//! variants live in `commands::query` / `commands::inspect`.

use rete_core::{eval_query, CountingReader, Header, RangeReader, Rete, SummaryView, HEADER_LEN};

use crate::commands::card;
use crate::commands::render::print_query_output;
use crate::http::HttpRangeReader;

/// Fetch just the embedded Dataset Card over HTTP — the index-free CARD tier.
/// Reads only the 128-byte header and the metadata range (two small range
/// requests), never the dictionary, index, or pyramid: the cold-start
/// self-description a newcomer needs before they know what to query.
pub(crate) fn card_url(url: &str, json: bool) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    match card::load_card_ranged(&reader)? {
        None => println!("(no dataset card)"),
        Some(dataset_card) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&dataset_card)?);
            } else {
                // The content hash is carried in the header we already fetched.
                let head = reader.read_at(0, HEADER_LEN as u64)?;
                let checksum = Header::from_bytes(&head)
                    .map(|h| card::hex16(&h.content_hash))
                    .unwrap_or_default();
                println!("{}", card::format_card(&dataset_card, &checksum));
            }
        }
    }
    eprintln!(
        "fetched {} of {} bytes in {} range request(s) — index NOT fetched",
        reader.bytes_read(),
        total,
        reader.requests()
    );
    Ok(())
}

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
            // The schema pyramid (leveled type histogram) rides the same
            // index-free fetch — show it after the super-edge graph.
            crate::commands::inspect::render_schema_overview(&view);
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

/// Query a triple pattern over HTTP: header + dictionary + the one selected
/// SPO/POS/OSP permutation payload. Unknown bound terms skip the index.
pub(crate) fn query_url(
    url: &str,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
) -> anyhow::Result<()> {
    let reader = CountingReader::new(HttpRangeReader::open(url)?);
    let total = reader.len();
    let results = Rete::query_ranged(&reader, s.as_deref(), p.as_deref(), o.as_deref())?;
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

/// Run SPARQL against a `.rete` over HTTP(S) with **lazy tile faulting**: the
/// open fetches the header, dictionary, pyramid, and the index's small tile
/// directories; index tiles are then fetched one range request at a time, only
/// when the query's scans and probes actually touch them. A selective query
/// reads O(touched tiles), not the whole index. (Pre-tiling v0.1 files fall
/// back to fetching the index whole.)
pub(crate) fn sparql_url(url: &str, query: &str, json: bool) -> anyhow::Result<()> {
    let reader = std::sync::Arc::new(CountingReader::new(HttpRangeReader::open(url)?));
    let total = reader.len();
    let rete = Rete::open_ranged_lazy(reader.clone())?;
    let result = eval_query(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    // Lazy tile fetches surface failures out-of-band: a partial answer must
    // become an error, never quietly fewer rows.
    if rete.index_incomplete() {
        anyhow::bail!(
            "a range request failed while streaming index tiles from {url}; \
             results would be incomplete — retry"
        );
    }
    print_query_output(&result, json);
    eprintln!(
        "(fetched {} bytes in {} range request(s); file is {} bytes)",
        reader.bytes_read(),
        reader.requests(),
        total
    );
    Ok(())
}
