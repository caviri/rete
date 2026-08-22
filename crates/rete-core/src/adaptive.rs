//! Session-local policy for batching lazy byte-range reads.

use std::sync::Mutex;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const MAX_GAP: u64 = 256 * KIB;
const MAX_SPAN: u64 = 2 * MIB;
const MAX_CONCURRENCY: usize = 16;
const STATIC_PREFETCH_START: usize = 4;
const STATIC_PREFETCH_CAP: usize = 512;

/// Why a group of already-routed ranges is being fetched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadIntent {
    /// Bound index lookups or the routed tiles for a join-probe batch.
    SelectiveProbe,
    /// A scan whose consumer may stop before exhausting its input.
    BoundedScan,
    /// A scan or aggregate expected to consume the complete routed span.
    FullScan,
    /// Dictionary chunks needed by the current solution/output batch.
    DictionaryResolve,
}

/// Result of one physical backing-reader operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadObservation {
    /// Bytes requested from the physical reader.
    pub requested_bytes: u64,
    /// Bytes returned after exact-length validation.
    pub returned_bytes: u64,
    /// Number of physical byte ranges in the operation.
    pub physical_ranges: usize,
    /// Elapsed monotonic time when the host can supply it.
    pub elapsed_micros: Option<u64>,
    /// Whether the complete operation succeeded and passed validation.
    pub success: bool,
}

/// Bounded scheduling decision for one set of already-eligible ranges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadPlan {
    /// Largest byte gap that may be fetched to join two needed ranges.
    pub coalesce_gap: u64,
    /// Largest physical span emitted by the adaptive path.
    pub max_span: u64,
    /// First tile-window size for a scan.
    pub prefetch_start: usize,
    /// Largest tile-window size for a scan.
    pub prefetch_cap: usize,
    /// Largest physical range fan-out for one backing-reader call.
    pub max_in_flight: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tier {
    Conservative,
    Balanced,
    Aggressive,
}

#[derive(Debug)]
struct ControllerState {
    successful_samples: u32,
    latency_micros: u64,
    throughput_bytes_per_second: u64,
    tier: Tier,
    pending_tier: Tier,
    pending_votes: u8,
    failure_streak: u8,
    bounded_consumption_per_mille: Option<u16>,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            successful_samples: 0,
            latency_micros: 0,
            throughput_bytes_per_second: 0,
            tier: Tier::Balanced,
            pending_tier: Tier::Balanced,
            pending_votes: 0,
            failure_streak: 0,
            bounded_consumption_per_mille: None,
        }
    }
}

/// Thread-safe adaptive policy shared by one remotely opened physical source.
#[derive(Debug, Default)]
pub struct AdaptiveReadController {
    state: Mutex<ControllerState>,
}

impl AdaptiveReadController {
    /// Creates a cold controller that initially reproduces the static policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a bounded plan for ranges already admitted by query/index logic.
    pub fn plan(
        &self,
        intent: ReadIntent,
        known_bytes: u64,
        static_gap: u64,
        concurrency: usize,
    ) -> ReadPlan {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let max_in_flight = concurrency.clamp(1, MAX_CONCURRENCY);
        if state.successful_samples < 2 {
            return ReadPlan {
                coalesce_gap: static_gap,
                max_span: MAX_SPAN,
                prefetch_start: STATIC_PREFETCH_START,
                prefetch_cap: STATIC_PREFETCH_CAP,
                max_in_flight,
            };
        }

        let break_even = ((state.throughput_bytes_per_second as u128)
            .saturating_mul(state.latency_micros as u128)
            / 1_000_000)
            .saturating_mul(3)
            / 4;
        let gap_budget = (known_bytes / 4).min(MAX_GAP);
        let mut coalesce_gap = u64::try_from(break_even)
            .unwrap_or(u64::MAX)
            .min(gap_budget);
        coalesce_gap >>= state.failure_streak.min(8);

        let (prefetch_start, prefetch_cap) = match intent {
            ReadIntent::SelectiveProbe | ReadIntent::DictionaryResolve => (1, 1),
            ReadIntent::FullScan => match state.tier {
                Tier::Conservative => (4, 128),
                Tier::Balanced => (4, STATIC_PREFETCH_CAP),
                Tier::Aggressive => (8, STATIC_PREFETCH_CAP),
            },
            ReadIntent::BoundedScan => match state.bounded_consumption_per_mille {
                Some(used) if used < 500 => (2, 64),
                Some(used) if used > 875 => match state.tier {
                    Tier::Conservative => (2, 64),
                    Tier::Balanced => (4, 256),
                    Tier::Aggressive => (8, STATIC_PREFETCH_CAP),
                },
                Some(_) => (4, 128),
                None => match state.tier {
                    Tier::Conservative => (2, 128),
                    Tier::Balanced => (4, STATIC_PREFETCH_CAP),
                    Tier::Aggressive => (8, STATIC_PREFETCH_CAP),
                },
            },
        };

        ReadPlan {
            coalesce_gap,
            max_span: MAX_SPAN,
            prefetch_start,
            prefetch_cap,
            max_in_flight,
        }
    }

    /// Incorporates one validated physical-read result into the session model.
    pub fn observe(&self, observation: ReadObservation) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !observation.success {
            state.failure_streak = state.failure_streak.saturating_add(1);
            return;
        }
        let Some(elapsed) = observation.elapsed_micros.filter(|&v| v > 0) else {
            return;
        };
        if observation.physical_ranges == 0
            || observation.returned_bytes == 0
            || observation.returned_bytes > observation.requested_bytes
        {
            return;
        }
        let throughput =
            (observation.returned_bytes as u128).saturating_mul(1_000_000) / elapsed as u128;
        let Ok(throughput) = u64::try_from(throughput) else {
            return;
        };
        if throughput == 0 {
            return;
        }

        if state.successful_samples == 0 {
            state.latency_micros = elapsed;
            state.throughput_bytes_per_second = throughput;
        } else {
            state.latency_micros = state.latency_micros / 2 + elapsed / 2;
            state.throughput_bytes_per_second =
                state.throughput_bytes_per_second / 2 + throughput / 2;
        }
        state.successful_samples = state.successful_samples.saturating_add(1);
        state.failure_streak = 0;

        let bytes_per_observed_latency = (state.throughput_bytes_per_second as u128)
            .saturating_mul(state.latency_micros as u128)
            / 1_000_000;
        let target = if bytes_per_observed_latency <= 32 * KIB as u128 {
            Tier::Conservative
        } else if bytes_per_observed_latency >= 256 * KIB as u128 {
            Tier::Aggressive
        } else {
            Tier::Balanced
        };
        if target == state.tier {
            state.pending_tier = target;
            state.pending_votes = 0;
        } else {
            if target == state.pending_tier {
                state.pending_votes = state.pending_votes.saturating_add(1);
            } else {
                state.pending_tier = target;
                state.pending_votes = 1;
            }
            if state.pending_votes >= 2 {
                state.tier = target;
                state.pending_votes = 0;
            }
        }
    }

    /// Reports how much of an offered scan window the consumer actually used.
    pub fn report_consumption(&self, intent: ReadIntent, consumed: usize, offered: usize) {
        if intent != ReadIntent::BoundedScan || offered == 0 {
            return;
        }
        let ratio = (consumed.min(offered) as u128).saturating_mul(1000) / offered as u128;
        let ratio = u16::try_from(ratio).unwrap_or(1000);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.bounded_consumption_per_mille = Some(ratio);
    }

    /// Number of successful timed observations incorporated into this session.
    pub fn successful_samples(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .successful_samples
    }
}
