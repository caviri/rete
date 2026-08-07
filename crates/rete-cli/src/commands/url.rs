//! The ranged-read group: read a `.rete` by fetching only the byte ranges each
//! operation needs — the pyramid summary (`summary_url`), a triple pattern
//! (`query_url`), or a full SPARQL query (`sparql_url`) — never a full load.
//! The source is an `http(s)://` URL or a local path: both go through the same
//! lazy tile-faulting reads, so a file larger than RAM stays queryable in
//! bounded memory (the local case is how the external build's output is
//! validated). Each wraps the transport in a `CountingReader` and reports bytes
//! fetched + range-request count. The load-it-all on-disk variants live in
//! `commands::query` / `commands::inspect`.

use rete_core::{
    auto_block, eval_query, BlockCacheReader, CountingReader, RangeReader, Rete, SearchView,
    SummaryView,
};

use crate::commands::card;
use crate::commands::range_source::RangedSourceReader;
use crate::commands::render::print_query_output;

/// Fetch just the embedded Dataset Card (and, when present, the adjacent
/// build-info record) over HTTP — the index-free CARD tier. Reads only the 1 KiB
/// header and one coalesced metadata+build-info range (two small range
/// requests), never the dictionary, index, or pyramid: the cold-start
/// self-description a newcomer needs before they know what to query.
pub(crate) fn card_url(
    url: &str,
    json: bool,
    format: Option<&str>,
    sha256: Option<&str>,
) -> anyhow::Result<()> {
    let format = card::CardFormat::resolve(json, format)?;
    let reader = CountingReader::new(RangedSourceReader::open(url)?);
    let total = reader.len();
    match card::load_card_and_build_ranged(&reader)? {
        (_, None, _) => println!("(no dataset card)"),
        (header, Some(dataset_card), build) => {
            // The content hash is carried in the header we already fetched.
            let checksum = card::hex16(&header.content_hash);
            card::print_card(
                &dataset_card,
                build.as_ref(),
                &checksum,
                url,
                format,
                sha256,
            )?;
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

/// Search a `.rete` over HTTP — the remote `rete search`, in both its modes.
///
/// The open is deliberately narrower than [`sparql_url`]'s: a [`SearchView`]
/// fetches the subject halves of the dictionary and stops. No object-only
/// directory, no permutation tile directories, no pyramid, no index — on
/// `epfl-infoscience.rete` that is 29.5 MB against 270 MB for an open that
/// answers nothing at all. `--contains` then faults the TEXT_INDEX
/// token table once and one range per posting list; the bare prefix mode faults
/// the pyramid (where the label index lives) instead. The byte report on stderr
/// is the point of the command as much as the hits are — it shows how little of
/// a multi-gigabyte file a search actually has to read.
pub(crate) fn search_url(
    url: &str,
    prefix: &str,
    words: &[String],
    contains_prefix: Option<&str>,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let reader = std::sync::Arc::new(CountingReader::new(RangedSourceReader::open(url)?));
    let total = reader.len();
    // Same block-cache arrangement as `sparql_url`: a search's reads are few and
    // scattered (a posting here, a dictionary chunk there), which is exactly the
    // shape a read-through cache coalesces. `RETE_BLOCK_KB=0` disables it.
    let block: u64 = match std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(kb) => kb * 1024,
        None => auto_block(total),
    };
    let view = if block == 0 {
        SearchView::open_ranged(reader.clone())?
    } else {
        SearchView::open_ranged(std::sync::Arc::new(BlockCacheReader::new(
            reader.clone(),
            block,
        )))?
    };

    let full_text = !words.is_empty() || contains_prefix.is_some();
    if full_text && !view.has_text_index() {
        if json {
            println!(
                "{{\"schemaVersion\":{},\"matches\":[]}}",
                crate::JSON_SCHEMA_VERSION
            );
        } else {
            eprintln!("(this file has no text index — rebuild with `rete build --text-index`)");
        }
        return Ok(());
    }
    if !full_text && !view.has_pyramid() {
        if json {
            println!(
                "{{\"schemaVersion\":{},\"matches\":[]}}",
                crate::JSON_SCHEMA_VERSION
            );
        } else {
            eprintln!("(this file has no label index — rebuild with a recent `rete build`)");
        }
        return Ok(());
    }

    // `(label, subject)` in prefix mode; full-text yields subjects only, so the
    // label side stays empty and the printers below branch on it.
    let hits: Vec<(Option<String>, String)> = if full_text {
        let word_refs: Vec<&str> = words.iter().map(String::as_str).collect();
        view.text_search(&word_refs, contains_prefix, limit)
            .into_iter()
            .map(|iri| (None, iri))
            .collect()
    } else {
        view.prefix_search(prefix, limit)
            .into_iter()
            .map(|(label, iri)| (Some(label), iri))
            .collect()
    };

    if json {
        let items: Vec<String> = hits
            .iter()
            .map(|(label, iri)| match label {
                Some(l) => format!(
                    "{{\"label\":{},\"subject\":{}}}",
                    crate::commands::inspect::json_str(l),
                    crate::commands::inspect::json_str(iri)
                ),
                None => format!(
                    "{{\"subject\":{}}}",
                    crate::commands::inspect::json_str(iri)
                ),
            })
            .collect();
        println!(
            "{{\"schemaVersion\":{},\"matches\":[{}]}}",
            crate::JSON_SCHEMA_VERSION,
            items.join(",")
        );
    } else if hits.is_empty() {
        if full_text {
            let mut terms: Vec<String> = words.to_vec();
            if let Some(p) = contains_prefix {
                terms.push(format!("{p}*"));
            }
            println!("(no entities contain {})", terms.join(" + "));
        } else {
            println!("(no labels match \"{prefix}\")");
        }
    } else {
        for (label, iri) in &hits {
            match label {
                Some(l) => println!("{l}\t{iri}"),
                None => println!("{iri}"),
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
    let reader = CountingReader::new(RangedSourceReader::open(url)?);
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
    let reader = CountingReader::new(RangedSourceReader::open(url)?);
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
pub(crate) fn sparql_url(url: &str, query: &str, json: bool, entail: bool) -> anyhow::Result<()> {
    // `reader` always counts the PHYSICAL HTTP fetches. A read-through block
    // cache (client-side; works over any single-range backend incl. S3) sits
    // above it, so a query's scattered range reads coalesce into a few aligned
    // block fetches. `RETE_BLOCK_KB=0` disables it (one fetch per logical read).
    let reader = std::sync::Arc::new(CountingReader::new(RangedSourceReader::open(url)?));
    let total = reader.len();
    // Block size: an explicit `RETE_BLOCK_KB` wins (0 disables the cache); else
    // auto-tune from the file length, exactly like the wasm client — bigger files
    // get bigger blocks, so a remote query makes far fewer round trips.
    let block: u64 = match std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(kb) => kb * 1024,
        None => auto_block(total),
    };
    let mut rete = if block == 0 {
        Rete::open_ranged_lazy(reader.clone())?
    } else {
        Rete::open_ranged_lazy(std::sync::Arc::new(BlockCacheReader::new(
            reader.clone(),
            block,
        )))?
    };
    // SERVICE blocks federate to remote SPARQL endpoints over HTTP.
    rete.set_service_client(Box::new(super::service_http::HttpServiceClient));
    let eval = if entail {
        rete_core::eval_query_reasoned
    } else {
        eval_query
    };
    let result = eval(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
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

/// Explain a triple-pattern result over a **remote** `.rete` — which permutation,
/// section, and byte ranges answered it — fetching only the routed tiles. The CLI
/// counterpart of the browser's `why_url`, mirroring [`sparql_url`]'s lazy
/// block-cached open.
pub(crate) fn why_url(
    url: &str,
    subject: Option<String>,
    predicate: Option<String>,
    object: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let reader = std::sync::Arc::new(CountingReader::new(RangedSourceReader::open(url)?));
    let total = reader.len();
    // Block size: an explicit `RETE_BLOCK_KB` wins (0 disables the cache); else
    // auto-tune from the file length, exactly like the wasm client — bigger files
    // get bigger blocks, so a remote query makes far fewer round trips.
    let block: u64 = match std::env::var("RETE_BLOCK_KB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(kb) => kb * 1024,
        None => auto_block(total),
    };
    let rete = if block == 0 {
        Rete::open_ranged_lazy(reader.clone())?
    } else {
        Rete::open_ranged_lazy(std::sync::Arc::new(BlockCacheReader::new(
            reader.clone(),
            block,
        )))?
    };
    let results =
        rete.query_with_provenance(subject.as_deref(), predicate.as_deref(), object.as_deref());
    if rete.index_incomplete() {
        anyhow::bail!(
            "a range request failed while explaining the pattern over {url}; the \
             provenance would be incomplete — retry"
        );
    }
    crate::commands::query::print_provenance(
        subject.as_deref(),
        predicate.as_deref(),
        object.as_deref(),
        &results,
        json,
    )?;
    eprintln!(
        "(fetched {} bytes in {} range request(s); file is {} bytes)",
        reader.bytes_read(),
        reader.requests(),
        total
    );
    Ok(())
}
