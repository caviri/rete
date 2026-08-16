use rete_core::{AdaptiveReadController, ReadIntent, ReadObservation};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

fn observe(c: &AdaptiveReadController, bytes: u64, micros: u64, success: bool) {
    c.observe(ReadObservation {
        requested_bytes: bytes,
        returned_bytes: if success { bytes } else { 0 },
        physical_ranges: 1,
        elapsed_micros: Some(micros),
        success,
    });
}

fn trained_high_rtt_controller() -> AdaptiveReadController {
    let c = AdaptiveReadController::new();
    observe(&c, MIB, 120_000, true);
    observe(&c, MIB, 120_000, true);
    c
}

#[test]
fn cold_start_is_the_current_static_policy() {
    let c = AdaptiveReadController::new();
    let p = c.plan(ReadIntent::SelectiveProbe, 256 * KIB, 4096, 8);
    assert_eq!(p.coalesce_gap, 4096);
    assert_eq!(p.prefetch_start, 4);
    assert_eq!(p.prefetch_cap, 512);
    assert_eq!(p.max_in_flight, 8);
}

#[test]
fn high_rtt_fast_link_merges_more_after_two_samples() {
    let c = trained_high_rtt_controller();
    let p = c.plan(ReadIntent::SelectiveProbe, MIB, 4096, 8);
    assert!(p.coalesce_gap > 4096);
    assert!(p.coalesce_gap <= 256 * KIB);
}

#[test]
fn plan_never_exceeds_hard_limits() {
    let c = trained_high_rtt_controller();
    let p = c.plan(ReadIntent::FullScan, u64::MAX, u64::MAX, usize::MAX);
    assert!(p.max_span <= 2 * MIB);
    assert!(p.coalesce_gap <= 256 * KIB);
    assert!(p.max_in_flight <= 16);
}

#[test]
fn one_outlier_does_not_change_prefetch_tier() {
    let c = AdaptiveReadController::new();
    observe(&c, 64 * KIB, 20_000, true);
    observe(&c, 64 * KIB, 20_000, true);
    let before = c.plan(ReadIntent::BoundedScan, 512 * KIB, 4096, 8);
    observe(&c, MIB, 500_000, true);
    let after = c.plan(ReadIntent::BoundedScan, 512 * KIB, 4096, 8);
    assert_eq!(after.prefetch_start, before.prefetch_start);
    assert_eq!(after.prefetch_cap, before.prefetch_cap);
}

#[test]
fn failed_sample_does_not_train_throughput_and_shrinks_aggression() {
    let c = trained_high_rtt_controller();
    let before = c.plan(ReadIntent::SelectiveProbe, MIB, 4096, 8);
    let samples = c.successful_samples();
    observe(&c, MIB, 1_000_000, false);
    let after = c.plan(ReadIntent::SelectiveProbe, MIB, 4096, 8);
    assert_eq!(c.successful_samples(), samples);
    assert!(after.coalesce_gap <= before.coalesce_gap);
}

#[test]
fn low_bounded_scan_consumption_shrinks_next_window() {
    let c = trained_high_rtt_controller();
    let before = c.plan(ReadIntent::BoundedScan, 512 * KIB, 4096, 8);
    c.report_consumption(ReadIntent::BoundedScan, 1, before.prefetch_start);
    let after = c.plan(ReadIntent::BoundedScan, 512 * KIB, 4096, 8);
    assert!(after.prefetch_start <= before.prefetch_start);
    assert!(after.prefetch_cap <= before.prefetch_cap);
}
