//! `rete estimate` — predict a build's size, time and memory **before** running it.
//!
//! A billion-triple build is a multi-hour, multi-hundred-GB commitment; finding out
//! it needed 60 GB of spill or produced a 90 GB file *after* two days is the
//! expensive way to learn. This streams the input (all of it, or a leading sample),
//! parses every statement exactly as `rete build` would, and reports what the build
//! would cost — without writing a byte of output or spilling anything.
//!
//! Distinct terms are counted with **HyperLogLog** (2^14 registers, 16 KiB, ~0.8 %
//! relative error) rather than a HashSet: the dictionary of a 9.8 B-triple graph
//! holds 1.9 B distinct terms, and an exact set of those would need tens of GB —
//! i.e. the estimator would hit the very wall it exists to warn about.

use std::time::Instant;

/// HyperLogLog cardinality estimator (Flajolet et al.), 2^P registers.
struct Hll {
    reg: Vec<u8>,
}

const P: u32 = 14;
const M: usize = 1 << P;

impl Hll {
    fn new() -> Self {
        Hll { reg: vec![0u8; M] }
    }

    fn add(&mut self, s: &str) {
        let h = fxhash64(s.as_bytes());
        let idx = (h >> (64 - P)) as usize;
        // rank = position of the first 1 bit in the remaining bits (1-based)
        let w = (h << P) | (1 << (P - 1));
        let rank = (w.leading_zeros() + 1) as u8;
        if rank > self.reg[idx] {
            self.reg[idx] = rank;
        }
    }

    fn count(&self) -> u64 {
        let m = M as f64;
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let sum: f64 = self.reg.iter().map(|&r| 2f64.powi(-(r as i32))).sum();
        let raw = alpha * m * m / sum;
        let zeros = self.reg.iter().filter(|&&r| r == 0).count();
        // small-range correction: linear counting while registers are sparse
        if raw <= 2.5 * m && zeros > 0 {
            (m * (m / zeros as f64).ln()).round() as u64
        } else {
            raw.round() as u64
        }
    }
}

/// A fast, allocation-free 64-bit string hash (FxHash-style, good enough for HLL).
fn fxhash64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    // final avalanche so the leading bits are well mixed (HLL reads the top P)
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h
}

/// Compressed bytes per id-triple per permutation, fitted to published builds
/// (datacite 9.83 B triples → 52.2 GB; crossref 3.78 B → 60.2 GB; opencitations
/// 5.18 B → 33.4 GB). Delta-coded then zstd'd, so it is remarkably stable.
const BYTES_PER_ID_TRIPLE_PER_PERM: f64 = 0.62;
const NUM_PERMS: f64 = 6.0;
/// Dictionary compression: terms are sorted, front-coded (shared prefixes — and a
/// scholarly graph's IRIs share nearly all of theirs) then zstd'd.
const DICT_COMPRESSION: f64 = 0.14;
/// The community + schema pyramid, as a fraction of dictionary+index.
const PYRAMID_OVERHEAD: f64 = 0.18;
/// How far the projection has landed from the real output on the builds it was
/// fitted against — reported as a band rather than a single misleading number.
const SIZE_BAND: f64 = 0.20;

fn human(bytes: f64) -> String {
    let u = ["B", "KB", "MB", "GB", "TB"];
    let mut b = bytes;
    let mut i = 0;
    while b >= 1000.0 && i < u.len() - 1 {
        b /= 1000.0;
        i += 1;
    }
    format!("{b:.1} {}", u[i])
}

fn hms(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

/// Stream `inputs`, measure, and print the projected build cost.
pub(crate) fn estimate(
    inputs: &[String],
    format: Option<&str>,
    sample_mb: Option<u64>,
    memory_budget_mb: Option<u64>,
    no_pyramid: bool,
) -> anyhow::Result<()> {
    let sample_bytes = sample_mb.map(|m| m * (1 << 20));
    let mut total_input: u64 = 0;
    for i in inputs {
        if i != "-" {
            total_input += std::fs::metadata(i).map(|m| m.len()).unwrap_or(0);
        }
    }

    let mut hll = Hll::new();
    let mut statements: u64 = 0;
    let mut term_bytes: u64 = 0; // every term occurrence (for the per-quad average)
    let mut uniq_sample_bytes: u64 = 0; // term bytes seen the first ~1M times
    let mut uniq_sample_n: u64 = 0;
    let mut seen_small = std::collections::HashSet::new();
    let mut literals: u64 = 0;
    let mut graphs = std::collections::HashSet::new();
    let mut read_bytes: u64 = 0;
    let started = Instant::now();

    for path in inputs {
        let fmt = super::build::input_format(path, format);
        if !matches!(fmt, "nt" | "nq" | "ttl" | "trig") {
            anyhow::bail!(
                "estimate reads N-Triples/N-Quads/Turtle/TriG (got {fmt} for {path}); \
                 pipe a converter's output with --format nt"
            );
        }
        if sample_bytes.is_some_and(|limit| read_bytes >= limit) {
            break;
        }
        // The whole input streams through one parser pass. `--sample-mb` is enforced
        // UNDER the parser by a counting reader that reports EOF at the limit — which
        // works for Turtle and TriG, where a statement spans lines and the earlier
        // line-chunking trick could not. Cutting the stream mid-statement makes the
        // parser (rightly) complain about the truncated tail, so a parse error is
        // tolerated only when the cut is what caused it.
        let remaining = sample_bytes.map(|limit| limit.saturating_sub(read_bytes));
        let (reader, counter) = super::build::open_reader_counted(path, remaining)?;
        let res = rete_core::ingest::stream_reader(reader, fmt, &mut |(s, p, o, g)| {
            statements += 1;
            if o.starts_with('"') {
                literals += 1;
            }
            for t in [&s, &p, &o] {
                term_bytes += t.len() as u64;
                hll.add(t);
                if seen_small.len() < 1_000_000 && seen_small.insert(t.clone()) {
                    uniq_sample_bytes += t.len() as u64;
                    uniq_sample_n += 1;
                }
            }
            if let Some(g) = g {
                hll.add(&g);
                if graphs.len() < 100_000 {
                    graphs.insert(g);
                }
            }
        });
        let truncated = remaining.is_some_and(|limit| counter.get() >= limit);
        read_bytes += counter.get();
        if let Err(e) = res {
            if !truncated {
                anyhow::bail!("{path}: {e}");
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64().max(1e-6);
    let sampled = sample_bytes.is_some() && read_bytes < total_input;
    let scale = if sampled && read_bytes > 0 {
        total_input as f64 / read_bytes as f64
    } else {
        1.0
    };

    let est_statements = statements as f64 * scale;
    // Distinct terms grow sub-linearly with the input: past the first appearance of
    // each subject/predicate/object, new statements mostly REUSE terms. Extrapolate
    // the sampled cardinality by sqrt(scale) — the empirical middle ground between
    // "no new terms" (scale^0) and "all terms new" (scale^1) on the published
    // scholarly graphs. Exact when the whole input was read (scale = 1).
    let est_terms = hll.count() as f64 * scale.sqrt();
    let avg_uniq_term = if uniq_sample_n > 0 {
        uniq_sample_bytes as f64 / uniq_sample_n as f64
    } else {
        0.0
    };
    let dict_raw = est_terms * avg_uniq_term;
    let dict_out = dict_raw * DICT_COMPRESSION;
    let index_out = est_statements * NUM_PERMS * BYTES_PER_ID_TRIPLE_PER_PERM;
    // A default build also writes the community + schema pyramid; measured at
    // ~18% of dictionary+index on the published graphs (pass --no-pyramid, as the
    // billion-triple builds do, and this term disappears).
    let pyramid_out = if no_pyramid {
        0.0
    } else {
        (dict_out + index_out) * PYRAMID_OVERHEAD
    };
    let est_out = dict_out + index_out + pyramid_out;

    let rate = read_bytes as f64 / elapsed; // input bytes/s, parse-only
                                            // A build does more per byte than a parse: chunk + sort + spill + merge + index
                                            // + compress. Measured across the published big builds, wall time lands around
                                            // 3-5x the parse-only pass; report the range rather than a false precision.
    let parse_all = total_input as f64 / rate;

    let budget = memory_budget_mb.unwrap_or(4096) as f64 * (1u64 << 20) as f64;
    let chunk_budget = budget * 0.5;
    let raw_spill =
        est_statements * (term_bytes as f64 / (statements.max(1) * 3) as f64 * 3.0 + 96.0);
    let chunks = (raw_spill / chunk_budget).ceil().max(1.0);

    println!("rete estimate — projected build cost (nothing was written)\n");
    println!("input");
    println!("  files                {}", inputs.len());
    println!("  total size           {}", human(total_input as f64));
    if sampled {
        println!(
            "  READ (sample)        {}  ({:.2}% — figures below are extrapolated)",
            human(read_bytes as f64),
            100.0 * read_bytes as f64 / total_input as f64
        );
    } else {
        println!(
            "  read                 {} (complete — counts are exact)",
            human(read_bytes as f64)
        );
    }
    println!("\nwhat it contains");
    println!(
        "  statements           {:.0}{}",
        est_statements,
        if sampled { " (est.)" } else { "" }
    );
    println!(
        "  distinct terms       {:.0}{}",
        est_terms,
        if sampled {
            " (est., HLL)"
        } else {
            " (HLL ±0.8%)"
        }
    );
    println!(
        "  literal objects      {:.1}%",
        100.0 * literals as f64 / statements.max(1) as f64
    );
    println!("  avg term length      {avg_uniq_term:.1} B");
    if !graphs.is_empty() {
        println!(
            "  named graphs         {}{}",
            graphs.len(),
            if sampled { "+ (in sample)" } else { "" }
        );
    }
    println!("\nprojected output");
    println!("  dictionary           ~{}", human(dict_out));
    println!("  index (6 perms)      ~{}", human(index_out));
    if pyramid_out > 0.0 {
        println!(
            "  pyramid              ~{}  (--no-pyramid drops it)",
            human(pyramid_out)
        );
    }
    println!(
        "  TOTAL .rete          {} – {}   (~{:.1} bytes/triple)",
        human(est_out * (1.0 - SIZE_BAND)),
        human(est_out * (1.0 + SIZE_BAND)),
        est_out / est_statements.max(1.0)
    );
    println!("\nprojected build");
    println!("  parse-only pass      {}", hms(parse_all));
    println!(
        "  full build           {} – {}   (parse + sort/spill/merge + index + compress)",
        hms(parse_all * 3.0),
        hms(parse_all * 5.0)
    );
    println!("  at --memory-budget-mb {}", budget as u64 >> 20);
    println!("    spill chunks       ~{chunks:.0}");
    println!(
        "    peak spill on disk ~{}   (put --tmp-dir on a fast NATIVE disk)",
        human(raw_spill)
    );
    println!(
        "    peak RAM           ~{} (the budget bounds it; the spill is what needs room)",
        human(budget)
    );
    if est_out > 0.0 {
        println!(
            "\nplan: {} of free space for the output plus {} for the spill.",
            human(est_out * 1.1),
            human(raw_spill * 1.1)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hll_estimates_within_a_few_percent() {
        let mut h = Hll::new();
        for i in 0..200_000u32 {
            h.add(&format!("<http://example.org/resource/{i}>"));
        }
        let est = h.count() as f64;
        let err = (est - 200_000.0).abs() / 200_000.0;
        assert!(err < 0.05, "HLL off by {:.1}% (est {est})", err * 100.0);
    }

    #[test]
    fn hll_small_cardinality_uses_linear_counting() {
        let mut h = Hll::new();
        for i in 0..50u32 {
            h.add(&format!("t{i}"));
        }
        assert!(
            (h.count() as i64 - 50).abs() <= 2,
            "small-range est {}",
            h.count()
        );
    }
}
