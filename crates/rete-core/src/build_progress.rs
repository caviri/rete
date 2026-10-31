//! **Build progress**: where a running build is, for a renderer on another
//! thread — what `rete build|merge|recard --progress` draws.
//!
//! [`crate::build_trace`] records what a build did, on the build's own thread,
//! for the build record. This is its live sibling: the builders publish the
//! phase they enter, how far through it they are, and a few running totals into
//! process-wide atomics, which a renderer samples whenever it likes.
//!
//! Nothing is published until someone [`listen`]s. Until then every call is one
//! relaxed atomic load and a branch, and a [`Ticker`] — the per-record counter
//! the hot loops use — adds to a local integer and publishes once per
//! [`Ticker::EVERY`] records. On wasm32 every function is a no-op.
//!
//! The figures are progress, not accounting: a phase's `done` may stop short
//! of its `total` (a deduplicating merge reads fewer records than were
//! spilled), and a renderer clamps it to the total.

use crate::build_trace::Phase;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};

/// What a phase's `done` and `total` count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unit {
    /// Nothing countable: a renderer shows a spinner.
    #[default]
    None,
    /// Input bytes as stored (under any gzip decoder), or bytes written.
    Bytes,
    /// Statements read or remapped.
    Statements,
    /// Chunk-local dictionary terms merged.
    Terms,
    /// Sorted records merged, over every permutation of the phase.
    Records,
    /// Starter queries run.
    Queries,
}

impl Unit {
    #[cfg(not(target_arch = "wasm32"))]
    const ALL: [Unit; 6] = [
        Unit::None,
        Unit::Bytes,
        Unit::Statements,
        Unit::Terms,
        Unit::Records,
        Unit::Queries,
    ];

    /// The name `--progress=json` prints.
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::None => "none",
            Unit::Bytes => "bytes",
            Unit::Statements => "statements",
            Unit::Terms => "terms",
            Unit::Records => "records",
            Unit::Queries => "queries",
        }
    }
}

/// One sample of the published state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The phase begun last (`None` before the first).
    pub phase: Option<Phase>,
    /// How many phases have begun: changes whenever `phase` does.
    pub seq: u64,
    /// Every phase begun since [`listen`], with when, in order — so a
    /// renderer sampling now and then still learns of a phase that began and
    /// ended between two samples, and how long each one took.
    pub begun: Vec<(Phase, std::time::Instant)>,
    /// How far through the phase, in `unit`s.
    pub done: u64,
    /// The phase's total, when known.
    pub total: Option<u64>,
    /// What `done` and `total` count.
    pub unit: Unit,
    /// Statements read so far (the parse), then as counted by the builder.
    pub statements: u64,
    /// Distinct dictionary terms, once the dictionary is built.
    pub terms: u64,
    /// Chunks the memory-bounded build has sealed.
    pub chunks: u64,
    /// The memory-bounded build's spill directory while it exists.
    pub spill_dir: Option<std::path::PathBuf>,
}

impl Snapshot {
    /// Whether `phase` has begun since [`listen`].
    pub fn has_begun(&self, phase: Phase) -> bool {
        self.begun.iter().any(|(p, _)| *p == phase)
    }
}

/// `phase`'s position in [`Phase::ALL`].
#[cfg(not(target_arch = "wasm32"))]
fn phase_index(phase: Phase) -> usize {
    Phase::ALL.iter().position(|p| *p == phase).unwrap_or(0)
}

#[cfg(not(target_arch = "wasm32"))]
mod state {
    use super::*;
    pub(super) static LISTENERS: AtomicUsize = AtomicUsize::new(0);
    /// `Phase::ALL` index + 1; 0 = none yet.
    pub(super) static PHASE: AtomicU8 = AtomicU8::new(0);
    pub(super) static SEQ: AtomicU64 = AtomicU64::new(0);
    pub(super) static BEGUN: std::sync::Mutex<Vec<(Phase, std::time::Instant)>> =
        std::sync::Mutex::new(Vec::new());
    pub(super) static DONE: AtomicU64 = AtomicU64::new(0);
    /// `u64::MAX` = unknown.
    pub(super) static TOTAL: AtomicU64 = AtomicU64::new(u64::MAX);
    pub(super) static UNIT: AtomicU8 = AtomicU8::new(0);
    pub(super) static STATEMENTS: AtomicU64 = AtomicU64::new(0);
    pub(super) static TERMS: AtomicU64 = AtomicU64::new(0);
    pub(super) static CHUNKS: AtomicU64 = AtomicU64::new(0);
    pub(super) static SPILL_DIR: std::sync::Mutex<Option<std::path::PathBuf>> =
        std::sync::Mutex::new(None);
}

/// Publishes for as long as it lives; see [`listen`].
pub struct Listener {
    _private: (),
}

/// Start publishing, from a clean slate. Builds that run while the returned
/// guard lives publish their progress; drop it to stop.
pub fn listen() -> Listener {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use state::*;
        PHASE.store(0, Ordering::Relaxed);
        DONE.store(0, Ordering::Relaxed);
        TOTAL.store(u64::MAX, Ordering::Relaxed);
        UNIT.store(0, Ordering::Relaxed);
        if let Ok(mut b) = BEGUN.lock() {
            b.clear();
        }
        for c in [&STATEMENTS, &TERMS, &CHUNKS] {
            c.store(0, Ordering::Relaxed);
        }
        if let Ok(mut d) = SPILL_DIR.lock() {
            *d = None;
        }
        SEQ.store(0, Ordering::Release);
        LISTENERS.fetch_add(1, Ordering::AcqRel);
    }
    Listener { _private: () }
}

impl Drop for Listener {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        state::LISTENERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// True while a [`Listener`] lives.
#[inline]
pub fn listening() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        state::LISTENERS.load(Ordering::Relaxed) > 0
    }
    #[cfg(target_arch = "wasm32")]
    false
}

/// Enter `phase`: `done` restarts at 0 against `total` (`None` when nothing
/// bounds it).
pub fn begin(phase: Phase, total: Option<u64>, unit: Unit) {
    #[cfg(not(target_arch = "wasm32"))]
    if listening() {
        use state::*;
        let idx = phase_index(phase);
        DONE.store(0, Ordering::Relaxed);
        TOTAL.store(total.unwrap_or(u64::MAX), Ordering::Relaxed);
        UNIT.store(unit as u8, Ordering::Relaxed);
        PHASE.store(idx as u8 + 1, Ordering::Relaxed);
        if let Ok(mut b) = BEGUN.lock() {
            b.push((phase, std::time::Instant::now()));
        }
        SEQ.fetch_add(1, Ordering::Release);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (phase, total, unit);
}

/// Set the current phase's total once it is known.
pub fn set_total(total: Option<u64>, unit: Unit) {
    #[cfg(not(target_arch = "wasm32"))]
    if listening() {
        state::TOTAL.store(total.unwrap_or(u64::MAX), Ordering::Relaxed);
        state::UNIT.store(unit as u8, Ordering::Relaxed);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (total, unit);
}

/// Add `n` to the current phase's `done`.
#[inline]
pub fn advance(n: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    if listening() {
        state::DONE.fetch_add(n, Ordering::Relaxed);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = n;
}

/// Set the current phase's `done`.
pub fn set_done(n: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    if listening() {
        state::DONE.store(n, Ordering::Relaxed);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = n;
}

/// Running totals a renderer shows beside the phase.
#[derive(Debug, Clone, Copy)]
pub enum Stat {
    /// Statements read so far.
    Statements,
    /// Distinct dictionary terms.
    Terms,
    /// Chunks sealed by the memory-bounded build.
    Chunks,
}

/// Set a running total.
#[inline]
pub fn set_stat(stat: Stat, v: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    if listening() {
        match stat {
            Stat::Statements => &state::STATEMENTS,
            Stat::Terms => &state::TERMS,
            Stat::Chunks => &state::CHUNKS,
        }
        .store(v, Ordering::Relaxed);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (stat, v);
}

/// Name the directory the build spills into (`None` once it is gone), so a
/// renderer can show how much it holds.
pub fn set_spill_dir(dir: Option<&std::path::Path>) {
    #[cfg(not(target_arch = "wasm32"))]
    if listening() {
        if let Ok(mut d) = state::SPILL_DIR.lock() {
            *d = dir.map(std::path::Path::to_path_buf);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = dir;
}

/// Read the published state. The phase, its `done`, `total` and `unit` are
/// read as one: a sample never pairs a new phase with the last one's figures.
pub fn snapshot() -> Snapshot {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use state::*;
        loop {
            let seq = SEQ.load(Ordering::Acquire);
            let phase = PHASE.load(Ordering::Relaxed);
            let done = DONE.load(Ordering::Relaxed);
            let total = TOTAL.load(Ordering::Relaxed);
            let unit = UNIT.load(Ordering::Relaxed);
            let begun = BEGUN.lock().map(|b| b.clone()).unwrap_or_default();
            if SEQ.load(Ordering::Acquire) != seq {
                continue;
            }
            return Snapshot {
                phase: (phase as usize)
                    .checked_sub(1)
                    .and_then(|i| Phase::ALL.get(i).copied()),
                seq,
                begun,
                done,
                total: (total != u64::MAX).then_some(total),
                unit: Unit::ALL.get(unit as usize).copied().unwrap_or_default(),
                statements: STATEMENTS.load(Ordering::Relaxed),
                terms: TERMS.load(Ordering::Relaxed),
                chunks: CHUNKS.load(Ordering::Relaxed),
                spill_dir: SPILL_DIR.lock().ok().and_then(|d| d.clone()),
            };
        }
    }
    #[cfg(target_arch = "wasm32")]
    Snapshot::default()
}

/// A per-record counter for a hot loop: [`tick`](Self::tick) adds to a local
/// integer and publishes ([`advance`]) once per [`Self::EVERY`] records, and
/// what is left when it is dropped.
#[derive(Debug, Default)]
pub struct Ticker {
    pending: u64,
}

impl Ticker {
    /// Records between two publications.
    pub const EVERY: u64 = 1 << 16;

    /// A counter with nothing counted.
    pub fn new() -> Self {
        Ticker { pending: 0 }
    }

    /// Count `n` records.
    #[inline]
    pub fn tick(&mut self, n: u64) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.pending += n;
            if self.pending >= Self::EVERY {
                self.flush();
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = n;
    }

    /// Publish what has been counted.
    pub fn flush(&mut self) {
        if self.pending > 0 {
            advance(self.pending);
            self.pending = 0;
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// The one test in this crate that listens: the state is process-wide, so
    /// a second listening test running in parallel would see this one's
    /// figures.
    #[test]
    fn publishes_only_while_listened_to() {
        begin(Phase::Parse, Some(10), Unit::Bytes);
        advance(5);
        assert_eq!(snapshot().phase, None, "nothing before a listener");
        {
            let mut t = Ticker::new();
            t.tick(3);
        }
        let l = listen();
        assert!(listening());
        begin(Phase::ParseChunk, None, Unit::Bytes);
        set_total(Some(100), Unit::Bytes);
        advance(40);
        set_stat(Stat::Statements, 7);
        let s = snapshot();
        assert_eq!(s.phase, Some(Phase::ParseChunk));
        assert_eq!((s.done, s.total, s.unit), (40, Some(100), Unit::Bytes));
        assert_eq!(s.statements, 7);
        assert!(s.has_begun(Phase::ParseChunk) && !s.has_begun(Phase::Parse));
        let seq = s.seq;
        begin(Phase::Dictionary, Some(1), Unit::Terms);
        {
            let mut t = Ticker::new();
            for _ in 0..(Ticker::EVERY + 5) {
                t.tick(1);
            }
            assert_eq!(snapshot().done, Ticker::EVERY, "published once per EVERY");
        }
        let s = snapshot();
        assert_eq!(s.done, Ticker::EVERY + 5, "the rest on drop");
        assert_eq!(s.seq, seq + 1);
        assert_eq!(s.unit.as_str(), "terms");
        drop(l);
        assert!(!listening());
        begin(Phase::Write, None, Unit::None);
        assert_eq!(
            snapshot().phase,
            Some(Phase::Dictionary),
            "frozen once dropped"
        );
    }
}
