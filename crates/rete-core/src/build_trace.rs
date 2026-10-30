//! The **build trace**: where a build spent its wall clock, and what its
//! external sorts did — the debugging half of the build record that `rete build`
//! stores in the kind-7 build-info section.
//!
//! It is a lap timer, not a profiler. The builders call [`lap`] at the end of
//! each phase; the time since the previous lap is charged to that phase, so the
//! phases **partition** the build's wall clock and never overlap. Counters
//! ([`add`], [`max`]) record what the memory-bounded build's spill did.
//!
//! Recording happens only while a [`Collector`] is alive on the **calling
//! thread** — the CLI starts one around a build. Every other caller (the
//! bindings, the browser builder, tests that build in parallel threads) pays
//! one thread-local check per phase and records nothing. That check also keeps
//! the clock out of `wasm32-unknown-unknown`, where `std::time::Instant::now()`
//! panics: without a collector it is never called, and on wasm32 the functions
//! are no-ops.

#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;
use std::time::Duration;

/// One phase of a build. A closed set, so a reader of the record knows every
/// name it can meet, and a writer can size a record before the build has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    /// Reading the input `.rete` back (`repyramid`, `recard`).
    Read,
    /// Reading and parsing the inputs into memory (the in-RAM text path).
    Parse,
    /// `--materialize` / `--reason`: the reasoner over the default graph.
    Reason,
    /// The two-pass streaming build's first pass: read + parse + observe every
    /// term, then sort the dictionary.
    ParseDictionary,
    /// The two-pass streaming build's second pass: read + parse + encode.
    ParseEncode,
    /// The memory-bounded build's single pass: read + parse + spill chunks.
    ParseChunk,
    /// Building the dictionary from parsed statements (in-RAM), or merging the
    /// chunk dictionaries (memory-bounded).
    Dictionary,
    /// Encoding statements to id-triples (in-RAM), or remapping the chunks'
    /// local ids to global ones (memory-bounded).
    Encode,
    /// Deriving the Dataset Card.
    CardDerive,
    /// The community pyramid.
    Pyramid,
    /// The full-text index.
    TextIndex,
    /// The LABEL_INDEX and SCHEMA sections.
    LabelsSchema,
    /// Compressing and serializing the dictionary section.
    DictionarySection,
    /// Sorting and tiling the index permutations (and, in-RAM, the named
    /// graphs' indexes).
    IndexPermutations,
    /// The memory-bounded build's named-graph indexes or quad index.
    NamedGraphs,
    /// Assembling the file image in memory.
    Serialize,
    /// Running the card's starter queries against the finished file.
    StarterQueries,
    /// Writing the file to disk.
    Write,
    /// Folding the content hash over the written sections for a card placed
    /// after the data (the memory-bounded build, `rete recard --in-place`):
    /// the card is hashed first, so the rest of the hash can only be folded
    /// once it exists — one read of every hashed section.
    ContentHash,
}

impl Phase {
    /// Every phase, in declaration order.
    pub const ALL: [Phase; 19] = [
        Phase::Read,
        Phase::Parse,
        Phase::Reason,
        Phase::ParseDictionary,
        Phase::ParseEncode,
        Phase::ParseChunk,
        Phase::Dictionary,
        Phase::Encode,
        Phase::CardDerive,
        Phase::Pyramid,
        Phase::TextIndex,
        Phase::LabelsSchema,
        Phase::DictionarySection,
        Phase::IndexPermutations,
        Phase::NamedGraphs,
        Phase::Serialize,
        Phase::StarterQueries,
        Phase::Write,
        Phase::ContentHash,
    ];

    /// The name the build record stores.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Read => "read",
            Phase::Parse => "parse",
            Phase::Reason => "reason",
            Phase::ParseDictionary => "parse+dictionary",
            Phase::ParseEncode => "parse+encode",
            Phase::ParseChunk => "parse+chunk",
            Phase::Dictionary => "dictionary",
            Phase::Encode => "encode",
            Phase::CardDerive => "card_derive",
            Phase::Pyramid => "pyramid",
            Phase::TextIndex => "text_index",
            Phase::LabelsSchema => "labels+schema",
            Phase::DictionarySection => "dictionary_section",
            Phase::IndexPermutations => "index_permutations",
            Phase::NamedGraphs => "named_graphs",
            Phase::Serialize => "serialize",
            Phase::StarterQueries => "starter_queries",
            Phase::Write => "write",
            Phase::ContentHash => "content_hash",
        }
    }
}

/// What a [`Collector`] recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace {
    /// Wall-clock time per phase, in the order each phase first ended. A phase
    /// that ends more than once (a lap per permutation, say) is summed.
    pub phases: Vec<(Phase, Duration)>,
    /// Named counters, sorted by name. The memory-bounded build records
    /// `chunks`, `sort_runs`, `merge_passes` and `scratch_peak_bytes`.
    pub counters: Vec<(&'static str, u64)>,
}

impl Trace {
    /// A counter's value, if it was recorded.
    pub fn counter(&self, name: &str) -> Option<u64> {
        self.counters
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, v)| v)
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct State {
    last: std::time::Instant,
    trace: Trace,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static ACTIVE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Records the build trace of the current thread for as long as it lives.
/// Starting a second collector on a thread replaces the first.
pub struct Collector {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Collector {
    /// Start recording on this thread; the first lap is timed from now.
    pub fn start() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        ACTIVE.with(|a| {
            *a.borrow_mut() = Some(State {
                last: std::time::Instant::now(),
                trace: Trace::default(),
            })
        });
        Collector {
            _not_send: std::marker::PhantomData,
        }
    }

    /// What has been recorded so far (the collector keeps recording).
    pub fn snapshot(&self) -> Trace {
        #[cfg(not(target_arch = "wasm32"))]
        {
            ACTIVE.with(|a| {
                a.borrow()
                    .as_ref()
                    .map(|s| s.trace.clone())
                    .unwrap_or_default()
            })
        }
        #[cfg(target_arch = "wasm32")]
        Trace::default()
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }
}

/// End `phase`: charge it the wall-clock time since the previous lap (or since
/// the collector started). A no-op without a collector on this thread.
pub fn lap(phase: Phase) {
    #[cfg(not(target_arch = "wasm32"))]
    ACTIVE.with(|a| {
        if let Some(s) = a.borrow_mut().as_mut() {
            let now = std::time::Instant::now();
            let d = now.duration_since(s.last);
            s.last = now;
            match s.trace.phases.iter_mut().find(|(p, _)| *p == phase) {
                Some((_, total)) => *total += d,
                None => s.trace.phases.push((phase, d)),
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    let _ = phase;
}

/// Restart the lap clock without charging anything — for time a caller does
/// not want attributed to the next phase.
pub fn skip() {
    #[cfg(not(target_arch = "wasm32"))]
    ACTIVE.with(|a| {
        if let Some(s) = a.borrow_mut().as_mut() {
            s.last = std::time::Instant::now();
        }
    });
}

fn update(name: &'static str, f: impl FnOnce(u64) -> u64) {
    #[cfg(not(target_arch = "wasm32"))]
    ACTIVE.with(|a| {
        if let Some(s) = a.borrow_mut().as_mut() {
            let c = &mut s.trace.counters;
            match c.binary_search_by(|(n, _)| (*n).cmp(name)) {
                Ok(i) => c[i].1 = f(c[i].1),
                Err(i) => c.insert(i, (name, f(0))),
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    let _ = (name, f);
}

/// Add `v` to the counter `name`. A no-op without a collector.
pub fn add(name: &'static str, v: u64) {
    update(name, |c| c.saturating_add(v));
}

/// Raise the counter `name` to `v` if it is lower. A no-op without a collector.
pub fn max(name: &'static str, v: u64) {
    update(name, |c| c.max(v));
}

/// One k-way merge over `inputs` spilled, sorted files (the memory-bounded
/// build): every such merge reads each of its inputs once, start to end.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn note_merge(inputs: usize) {
    add("merge_passes", 1);
    add("sort_runs", inputs as u64);
}

/// True while a collector records on this thread — for a measurement that
/// costs something (a directory walk) and is worth skipping otherwise.
pub fn active() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ACTIVE.with(|a| a.borrow().is_some())
    }
    #[cfg(target_arch = "wasm32")]
    false
}

/// The Cargo features this `rete-core` was compiled with that change what a
/// build writes or how fast it runs — recorded with the engine identity.
pub fn enabled_features() -> Vec<&'static str> {
    let mut f = Vec::new();
    if cfg!(feature = "compression") {
        f.push("compression");
    }
    if cfg!(feature = "parallel") {
        f.push("parallel");
    }
    if cfg!(feature = "rdf12-turtle") {
        f.push("rdf12-turtle");
    }
    if cfg!(feature = "unicode-regex") {
        f.push("unicode-regex");
    }
    f
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn laps_partition_and_accumulate_only_under_a_collector() {
        lap(Phase::Parse);
        add("chunks", 3);
        assert!(!active());
        let c = Collector::start();
        assert!(active());
        lap(Phase::Parse);
        lap(Phase::Encode);
        lap(Phase::Parse);
        add("sort_runs", 2);
        add("sort_runs", 5);
        max("scratch_peak_bytes", 10);
        max("scratch_peak_bytes", 4);
        let t = c.snapshot();
        let names: Vec<_> = t.phases.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            names,
            ["parse", "encode"],
            "first-end order, repeats summed"
        );
        assert_eq!(t.counter("sort_runs"), Some(7));
        assert_eq!(t.counter("scratch_peak_bytes"), Some(10));
        assert_eq!(t.counter("chunks"), None, "nothing before the collector");
        drop(c);
        assert!(!active());
        lap(Phase::Write);
    }

    #[test]
    fn phase_names_are_distinct() {
        let mut names: Vec<_> = Phase::ALL.iter().map(|p| p.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Phase::ALL.len());
    }
}
