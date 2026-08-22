//! Feature-gated paired-family construction preflight.

use std::hint::black_box;
use std::time::{Duration, Instant};

use crate::index::GraphIndexBuilder;
use crate::triples::TripleBlock;
use crate::Triple;

const PEOPLE: u32 = 400_000;
const EXPECTED_INPUT: usize = 2_782_169;
const EXPECTED_UNIQUE: usize = 2_746_900;
const EXPECTED_HASH: u64 = 0x8c9f_be95_effd_c90d;
const TILE_BUDGET: usize = 16 * 1024;
const WARMUPS: usize = 2;
const ACCEPTED: usize = 15;

fn workload() -> Vec<Triple> {
    let mut triples = Vec::with_capacity(2_800_000);
    let mut state = 42u64;
    for person in 0..PEOPLE {
        triples.push((person, 0, 18 + person % 60));
        triples.push((person, 1, person));
        for _ in 0..5 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let random = (state >> 32) as u32;
            let target = if !random.is_multiple_of(10) {
                (person / 100) * 100 + random % 100
            } else {
                random % PEOPLE
            };
            if target != person {
                triples.push((person, 2, target));
            }
        }
    }
    triples
}

fn workload_hash(triples: &[Triple]) -> u64 {
    triples.iter().fold(0u64, |hash, &(s, p, o)| {
        hash.rotate_left(7) ^ u64::from(s).wrapping_mul(31) ^ u64::from(p) ^ u64::from(o)
    })
}

fn payload_hash(index: &crate::index::GraphIndex) -> u64 {
    index.sections.iter().flatten().fold(0u64, |hash, tile| {
        tile.bytes()
            .iter()
            .fold(hash.rotate_left(5), |hash, &byte| hash ^ u64::from(byte))
    })
}

fn decoded(index: &crate::index::GraphIndex) -> Vec<Vec<Triple>> {
    index
        .sections
        .iter()
        .map(|section| {
            section
                .iter()
                .flat_map(|tile| {
                    TripleBlock::parse(tile.bytes())
                        .expect("builder tile parses")
                        .triples()
                })
                .collect()
        })
        .collect()
}

/// Capture elapsed construction time before any payload identity work. Keeping
/// the index alive in the returned pair gives both benchmark arms identical
/// post-timing hashing and deallocation lifetimes.
fn timed_construction(
    build: impl FnOnce() -> crate::index::GraphIndex,
) -> (Duration, crate::index::GraphIndex) {
    let started = Instant::now();
    let index = build();
    black_box(&index);
    (started.elapsed(), index)
}

/// Run the reproducible paired-family preflight and panic if it misses 1.5x.
pub fn run() {
    let triples = workload();
    assert_eq!(triples.len(), EXPECTED_INPUT);
    assert_eq!(workload_hash(&triples), EXPECTED_HASH);
    let mut unique = triples.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), EXPECTED_UNIQUE);

    let reference = GraphIndexBuilder::from_triples(triples.clone())
        .with_tile_budget(TILE_BUDGET)
        .build();
    let paired = GraphIndexBuilder::from_triples(triples.clone())
        .with_tile_budget(TILE_BUDGET)
        .build_families()
        .expect("paired family build");
    assert_eq!(decoded(&paired), decoded(&reference));
    black_box((payload_hash(&paired), payload_hash(&reference)));

    let mut paired_times = Vec::with_capacity(ACCEPTED);
    let mut reference_times = Vec::with_capacity(ACCEPTED);
    for sample in 0..WARMUPS + ACCEPTED {
        for paired_turn in [sample % 2 == 0, sample % 2 != 0] {
            let (elapsed, index) = timed_construction(|| {
                if paired_turn {
                    GraphIndexBuilder::from_triples(triples.clone())
                        .with_tile_budget(TILE_BUDGET)
                        .build_families()
                        .expect("paired family build")
                } else {
                    GraphIndexBuilder::from_triples(triples.clone())
                        .with_tile_budget(TILE_BUDGET)
                        .build()
                }
            });
            // This identity pass is deliberately after `timed_construction`:
            // timing covers construction plus `black_box(&index)` only.
            black_box(payload_hash(&index));
            if sample >= WARMUPS {
                (if paired_turn {
                    &mut paired_times
                } else {
                    &mut reference_times
                })
                .push(elapsed);
            }
        }
    }
    paired_times.sort_unstable();
    reference_times.sort_unstable();
    let median = |samples: &[Duration]| samples[samples.len() / 2];
    let ratio = median(&reference_times).as_secs_f64() / median(&paired_times).as_secs_f64();
    println!(
        "task5-preflight candidate={} workload={} unique={} hash={EXPECTED_HASH:016x} paired={paired_times:?} reference={reference_times:?} ratio={ratio:.3}",
        env!("CARGO_PKG_VERSION"),
        triples.len(),
        unique.len(),
    );
    assert!(ratio >= 1.5, "paired/reference median ratio {ratio:.3}");
}
