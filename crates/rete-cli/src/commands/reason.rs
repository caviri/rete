//! The `reason` command: run the prototype OWL RL / RDFS reasoner over a local or
//! remote (`--url`) `.rete`, report inferred triples + inconsistencies, and (with
//! `--materialize`) print the base + inferred graph. `--check` is a terse CI gate;
//! `--verify-card` re-checks a build-time coherence stamp. Exits non-zero on any
//! inconsistency.

use rete_core::{CountingReader, RangeReader, Rete};

use crate::commands::card::{self, Coherence};
use crate::commands::export::export_turtle;
use crate::http::HttpRangeReader;
use crate::commands::range_source::open_local;

type Triples = Vec<(String, String, String)>;

pub(crate) fn reason_cmd(
    file: Option<&str>,
    url: Option<&str>,
    materialize: bool,
    format: &str,
    check: bool,
    verify_card: bool,
) -> anyhow::Result<()> {
    if verify_card {
        return verify_card_cmd(file, url);
    }

    let base = load_base(file, url)?;
    let result = rete_core::reason(&base);

    // `--check`: a single verdict line + non-zero exit on incoherence (CI gate).
    if check {
        if result.inconsistencies.is_empty() {
            println!("coherent ({} inferred)", result.inferred.len());
            return Ok(());
        }
        println!(
            "incoherent: {} inconsistency(ies)",
            result.inconsistencies.len()
        );
        for inc in &result.inconsistencies {
            println!("  [{}] {}", inc.kind, inc.detail);
        }
        return Err(crate::NonConformance::new(format!(
            "{} inconsistency(ies) — graph is incoherent",
            result.inconsistencies.len()
        ))
        .into());
    }

    println!("inferred {} new triple(s)", result.inferred.len());
    if result.inconsistencies.is_empty() {
        println!("coherent: no inconsistencies found");
    } else {
        println!("{} inconsistency(ies) found:", result.inconsistencies.len());
        for inc in &result.inconsistencies {
            println!("  [{}] {}", inc.kind, inc.detail);
        }
    }

    if materialize {
        let mut all = base.clone();
        all.extend(result.inferred.iter().cloned());
        match format {
            "ttl" => print!("{}", export_turtle(&all)),
            _ => {
                for (s, p, o) in &all {
                    println!("{s} {p} {o} .");
                }
            }
        }
    }

    if !result.inconsistencies.is_empty() {
        return Err(crate::NonConformance::new(format!(
            "{} inconsistency(ies) — graph is incoherent",
            result.inconsistencies.len()
        ))
        .into());
    }
    Ok(())
}

/// Resolve the default-graph triples from a local file or a remote URL. The remote
/// arm fetches lazily over HTTP ranges and refuses an incomplete result (a failed
/// mid-read becomes an error, never a silently-partial — and so possibly false —
/// verdict), mirroring `sparql_url`.
fn load_base(file: Option<&str>, url: Option<&str>) -> anyhow::Result<Triples> {
    match (file, url) {
        (Some(path), None) => {
            Ok(open_local(path)?.dump(None))
        }
        (None, Some(u)) => {
            let reader = std::sync::Arc::new(CountingReader::new(HttpRangeReader::open(u)?));
            let total = reader.len();
            let rete = Rete::open_ranged_lazy(reader.clone())?;
            let base = rete.dump(None);
            if rete.index_incomplete() {
                anyhow::bail!(
                    "a range request failed while reading {u}; results would be incomplete — retry"
                );
            }
            eprintln!(
                "fetched {} of {} bytes in {} range request(s)",
                reader.bytes_read(),
                total,
                reader.requests()
            );
            Ok(base)
        }
        (Some(_), Some(_)) => anyhow::bail!("provide either a file path or --url, not both"),
        (None, None) => anyhow::bail!("provide a .rete file path or --url <url>"),
    }
}

/// `--verify-card`: load the file's baked coherence stamp (index-free), recompute
/// the verdict from a fresh reasoning run, and assert they match and the ruleset
/// tag is current. Fails (non-zero) on drift or a stale ruleset — so a stamp can
/// never be trusted as a current guarantee when the rules or the data have moved.
fn verify_card_cmd(file: Option<&str>, url: Option<&str>) -> anyhow::Result<()> {
    let (loaded, base) = match (file, url) {
        (Some(path), None) => {
            let bytes = std::fs::read(path)?;
            let loaded = card::load_card(&bytes)?;
            let base = Rete::open(&bytes)?.dump(None);
            (loaded, base)
        }
        (None, Some(u)) => {
            let reader = std::sync::Arc::new(CountingReader::new(HttpRangeReader::open(u)?));
            let loaded = card::load_card_ranged(reader.as_ref())?;
            let rete = Rete::open_ranged_lazy(reader.clone())?;
            let base = rete.dump(None);
            if rete.index_incomplete() {
                anyhow::bail!("a range request failed while reading {u}; cannot verify the card");
            }
            (loaded, base)
        }
        (Some(_), Some(_)) => anyhow::bail!("provide either a file path or --url, not both"),
        (None, None) => anyhow::bail!("provide a .rete file path or --url <url>"),
    };

    let stamped = match loaded.map(|c| c.coherence) {
        Some(c) if !c.is_empty() => c,
        _ => anyhow::bail!(
            "no coherence stamp in the card — rebuild with `rete build --reason` to stamp one"
        ),
    };

    // A stamp from a different ruleset is meaningless as a current guarantee.
    if stamped.rules != rete_core::REASON_RULESET {
        anyhow::bail!(
            "coherence stamp is from ruleset {} but this build is {} — re-stamp with `rete build --reason`",
            stamped.rules,
            rete_core::REASON_RULESET
        );
    }

    let fresh = Coherence::from_reasoning(&rete_core::reason(&base), stamped.materialized);
    if stamped != fresh {
        anyhow::bail!(
            "coherence stamp does NOT match a fresh reasoning run — it is stale (re-stamp with `rete build --reason`)"
        );
    }

    println!(
        "coherence card verified: {} ({} inconsistency(ies), rules {})",
        if stamped.coherent {
            "coherent"
        } else {
            "incoherent"
        },
        stamped.inconsistency_count,
        stamped.rules
    );
    Ok(())
}
