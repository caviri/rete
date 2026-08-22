//! Benchmark-only counters for the safe triple-read and property-path hot loop.

#[cfg(feature = "read-path-metrics")]
use std::cell::RefCell;
#[cfg(feature = "read-path-metrics")]
use std::collections::HashSet;

/// One thread's read-path counters since the last reset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadPathStats {
    /// Successfully decoded triple-block u32 varints.
    pub decoded_varints: u64,
    /// C values decoded only to skip a non-matching group.
    pub skipped_c_values: u64,
    /// Uncached property-path adjacency probes.
    pub path_probes: u64,
    /// Distinct lexical predicate dictionary lookups.
    pub predicate_resolutions: u64,
    /// Lazily built per-tile group directories.
    pub directory_builds: u64,
    /// Sum of retained prefix-2 directory bytes.
    pub directory_bytes_total: u64,
    /// Largest retained prefix-2 directory in one tile.
    pub directory_bytes_max: u64,
    /// Unique permutation tiles admitted to a scan.
    pub touched_tiles: u64,
}

#[cfg(feature = "read-path-metrics")]
#[derive(Default)]
struct State {
    stats: ReadPathStats,
    tiles: HashSet<(usize, usize)>,
}

#[cfg(feature = "read-path-metrics")]
std::thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Reset the current thread's benchmark counters and unique-tile set.
pub fn reset_read_path_stats() {
    #[cfg(feature = "read-path-metrics")]
    STATE.with(|state| *state.borrow_mut() = State::default());
}

/// Snapshot the current thread's benchmark counters.
pub fn read_path_stats() -> ReadPathStats {
    #[cfg(feature = "read-path-metrics")]
    return STATE.with(|state| state.borrow().stats);

    #[cfg(not(feature = "read-path-metrics"))]
    ReadPathStats::default()
}

#[inline(always)]
pub(crate) fn record_decoded_varint() {
    #[cfg(feature = "read-path-metrics")]
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.stats.decoded_varints = state.stats.decoded_varints.saturating_add(1);
    });
}

#[inline(always)]
pub(crate) fn record_skipped_c_values(count: u32) {
    #[cfg(feature = "read-path-metrics")]
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.stats.skipped_c_values = state
            .stats
            .skipped_c_values
            .saturating_add(u64::from(count));
    });
    #[cfg(not(feature = "read-path-metrics"))]
    let _ = count;
}

#[inline(always)]
pub(crate) fn record_path_probe() {
    #[cfg(feature = "read-path-metrics")]
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.stats.path_probes = state.stats.path_probes.saturating_add(1);
    });
}

#[inline(always)]
pub(crate) fn record_predicate_resolution() {
    #[cfg(feature = "read-path-metrics")]
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.stats.predicate_resolutions = state.stats.predicate_resolutions.saturating_add(1);
    });
}

#[inline(always)]
pub(crate) fn record_directory(bytes: usize) {
    #[cfg(feature = "read-path-metrics")]
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let bytes = bytes as u64;
        state.stats.directory_builds = state.stats.directory_builds.saturating_add(1);
        state.stats.directory_bytes_total = state.stats.directory_bytes_total.saturating_add(bytes);
        state.stats.directory_bytes_max = state.stats.directory_bytes_max.max(bytes);
    });
    #[cfg(not(feature = "read-path-metrics"))]
    let _ = bytes;
}

#[inline(always)]
pub(crate) fn record_tile(section: usize, tile: usize) {
    #[cfg(feature = "read-path-metrics")]
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.tiles.insert((section, tile)) {
            state.stats.touched_tiles = state.stats.touched_tiles.saturating_add(1);
        }
    });
    #[cfg(not(feature = "read-path-metrics"))]
    let _ = (section, tile);
}

#[cfg(all(test, feature = "read-path-metrics"))]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_all_read_path_counters() {
        reset_read_path_stats();
        record_decoded_varint();
        record_path_probe();
        record_directory(123);
        record_tile(2, 7);
        record_tile(2, 7);
        let stats = read_path_stats();
        assert_eq!(stats.decoded_varints, 1);
        assert_eq!(stats.path_probes, 1);
        assert_eq!(stats.directory_bytes_total, 123);
        assert_eq!(stats.touched_tiles, 1);
        reset_read_path_stats();
        assert_eq!(read_path_stats(), ReadPathStats::default());
    }
}
