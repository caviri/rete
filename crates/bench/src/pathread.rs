//! Warm, in-process benchmark for the safe property-path read loop.

use std::time::Instant;

use anyhow::{ensure, Context, Result};
use rete_core::{
    eval_query, read_path_stats, reset_read_path_stats, results_envelope_json, ReadPathStats, Rete,
};
use sha2::{Digest, Sha256};

use crate::mem;

const QUERY: &str = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
SELECT ?name WHERE { \
  ?sub rdfs:subClassOf+ <http://purl.obolibrary.org/obo/CHMO_0000228> ; \
       rdfs:label ?name \
} ORDER BY ?name LIMIT 200";

fn percentile(sorted: &[f64], numerator: usize, denominator: usize) -> f64 {
    let rank = sorted
        .len()
        .saturating_mul(numerator)
        .div_ceil(denominator)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[rank]
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn render_stats(label: &str, stats: ReadPathStats) {
    println!(
        "{label}: decoded_varints={} skipped_c_values={} path_probes={} \
         predicate_resolutions={} directory_builds={} directory_bytes_total={} \
         directory_bytes_max={} touched_tiles={}",
        stats.decoded_varints,
        stats.skipped_c_values,
        stats.path_probes,
        stats.predicate_resolutions,
        stats.directory_builds,
        stats.directory_bytes_total,
        stats.directory_bytes_max,
        stats.touched_tiles,
    );
}

pub fn run(path: &str, samples: usize) -> Result<()> {
    ensure!(samples > 0, "--path-read samples must be positive");
    let bytes = std::fs::read(path).with_context(|| format!("read {path}"))?;
    let rete = Rete::open(&bytes)?;

    reset_read_path_stats();
    let warm = eval_query(&rete, QUERY).map_err(|error| anyhow::anyhow!("{error}"))?;
    let expected = results_envelope_json(&warm, "");
    let expected_hash = hash(&expected);
    let warm_stats = read_path_stats();
    drop(warm);

    let mut times = Vec::with_capacity(samples);
    let mut peak_heap = 0usize;
    let mut steady_stats = None;
    for sample in 0..samples {
        reset_read_path_stats();
        let baseline = mem::live();
        mem::reset_peak();
        let start = Instant::now();
        let output = eval_query(&rete, QUERY).map_err(|error| anyhow::anyhow!("{error}"))?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let stats = read_path_stats();
        peak_heap = peak_heap.max(mem::peak().saturating_sub(baseline));
        ensure!(
            results_envelope_json(&output, "") == expected,
            "path output changed at sample {}",
            sample + 1
        );
        if let Some(first) = steady_stats {
            ensure!(
                stats == first,
                "path counters changed at sample {}",
                sample + 1
            );
        } else {
            steady_stats = Some(stats);
        }
        times.push(elapsed);
    }
    times.sort_by(|a, b| a.total_cmp(b));

    println!("# Safe property-path read profile: `{path}`");
    println!("query: `{QUERY}`");
    println!("samples_ms: {times:?}");
    println!(
        "median_ms: {:.2} p90_ms: {:.2} peak_heap_mib: {} result_sha256: {}",
        percentile(&times, 1, 2),
        percentile(&times, 9, 10),
        mem::mib(peak_heap),
        expected_hash,
    );
    render_stats("warm", warm_stats);
    render_stats("steady", steady_stats.unwrap_or_default());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_are_stable() {
        let values: Vec<f64> = (1..=15).map(f64::from).collect();
        assert_eq!(percentile(&values, 1, 2), 8.0);
        assert_eq!(percentile(&values, 9, 10), 14.0);
    }
}
