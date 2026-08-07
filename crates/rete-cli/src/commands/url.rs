//! The ranged-read group: read a `.rete` from an `http(s)://` URL or local path.
//! Summary and triple-pattern operations fetch only their required ranges.
//! SPARQL eagerly opens small remote objects and keeps lazy tile faulting for
//! larger remote objects and local paths. Each wraps the transport in a
//! `CountingReader` and reports bytes fetched + range-request count.

use std::ffi::OsStr;

use rete_core::{
    auto_block, eval_query, BlockCacheReader, CountingReader, Header, RangeReader, Rete,
    SummaryView, HEADER_LEN,
};

use crate::commands::card;
use crate::commands::range_source::RangedSourceReader;
use crate::commands::render::print_query_output;

const DEFAULT_EAGER_MAX_BYTES: u64 = 8 * 1024 * 1024;

fn parse_eager_max_bytes(raw: Option<&OsStr>) -> anyhow::Result<u64> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_EAGER_MAX_BYTES);
    };
    let text = raw
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("RETE_EAGER_MAX_MB must be valid UTF-8"))?;
    let mb = text.parse::<u64>().map_err(|_| {
        anyhow::anyhow!("RETE_EAGER_MAX_MB must be a non-negative integer, got {text}")
    })?;
    let bytes = mb.checked_mul(1024 * 1024).ok_or_else(|| {
        anyhow::anyhow!("RETE_EAGER_MAX_MB value {text} overflows its byte count")
    })?;
    usize::try_from(bytes)
        .map_err(|_| anyhow::anyhow!("RETE_EAGER_MAX_MB value {text} exceeds this platform"))?;
    Ok(bytes)
}

fn eager_max_bytes() -> anyhow::Result<u64> {
    parse_eager_max_bytes(std::env::var_os("RETE_EAGER_MAX_MB").as_deref())
}

fn should_eager_open(source: &str, len: u64, max: u64) -> bool {
    crate::commands::range_source::is_url(source) && len != 0 && max != 0 && len <= max
}

/// Fetch just the embedded Dataset Card over HTTP — the index-free CARD tier.
/// Reads only the 128-byte header and the metadata range (two small range
/// requests), never the dictionary, index, or pyramid: the cold-start
/// self-description a newcomer needs before they know what to query.
pub(crate) fn card_url(url: &str, json: bool) -> anyhow::Result<()> {
    let reader = CountingReader::new(RangedSourceReader::open(url)?);
    let total = reader.len();
    match card::load_card_ranged(&reader)? {
        None => println!("(no dataset card)"),
        Some(dataset_card) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&card::card_json(&dataset_card))?
                );
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

/// Run SPARQL against a `.rete` over HTTP(S). Small non-empty HTTP objects are
/// opened from one full-file range request; larger objects retain lazy tile
/// faulting, fetching index tiles only when scans and probes touch them.
pub(crate) fn sparql_url(
    url: &str,
    query: &str,
    json: bool,
    entail: bool,
    #[cfg(feature = "unsafe-decode-bench")] unsafe_decode: bool,
) -> anyhow::Result<()> {
    let eager_max = eager_max_bytes()?;
    // `reader` always counts the PHYSICAL HTTP fetches. A read-through block
    // cache (client-side; works over any single-range backend incl. S3) sits
    // above it, so a query's scattered range reads coalesce into a few aligned
    // block fetches. `RETE_BLOCK_KB=0` disables it (one fetch per logical read).
    let reader = std::sync::Arc::new(CountingReader::new(RangedSourceReader::open(url)?));
    let total = reader.len();
    // The block size is computed only in the lazy branch. An explicit
    // `RETE_BLOCK_KB` wins (0 disables the cache); otherwise auto-tune it.
    let mut rete = if should_eager_open(url, total, eager_max) {
        let image = reader.read_at(0, total)?;
        Rete::open(&image)?
    } else {
        let block = match std::env::var("RETE_BLOCK_KB")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
        {
            Some(kb) => kb
                .checked_mul(1024)
                .ok_or_else(|| anyhow::anyhow!("RETE_BLOCK_KB overflows u64"))?,
            None => auto_block(total),
        };
        if block == 0 {
            Rete::open_ranged_lazy(reader.clone())?
        } else {
            Rete::open_ranged_lazy(std::sync::Arc::new(BlockCacheReader::new(
                reader.clone(),
                block,
            )))?
        }
    };
    #[cfg(feature = "unsafe-decode-bench")]
    if unsafe_decode {
        eprintln!(
            "WARNING: --unsafe-decode assumes every fetched index block is a complete, immutable, rete-produced image; malformed or truncated input can cause undefined behavior"
        );
        // SAFETY: the hidden research flag is the operator's explicit assertion
        // that this controlled benchmark URL satisfies the invariant printed
        // above for its full lifetime. Normal builds cannot compile this call.
        unsafe { rete.assume_valid_index_blocks() };
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn eager_threshold_contract() {
        assert_eq!(parse_eager_max_bytes(None).unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_eager_max_bytes(Some(OsStr::new("0"))).unwrap(), 0);
        assert_eq!(
            parse_eager_max_bytes(Some(OsStr::new("16"))).unwrap(),
            16 << 20
        );
        for raw in ["-1", "8.5", "eight", "18446744073709551615"] {
            assert!(parse_eager_max_bytes(Some(OsStr::new(raw))).is_err());
        }
    }

    #[test]
    fn eager_policy_is_http_nonempty_bounded_and_inclusive() {
        let max = 8 << 20;
        assert!(should_eager_open("https://host/g.rete", max, max));
        assert!(!should_eager_open("https://host/g.rete", 0, max));
        assert!(!should_eager_open("https://host/g.rete", max + 1, max));
        assert!(!should_eager_open("graph.rete", 1024, max));
        assert!(!should_eager_open("https://host/g.rete", 1024, 0));
    }
}
