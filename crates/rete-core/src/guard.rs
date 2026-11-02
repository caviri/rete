//! A memory ceiling for one query run on the calling thread.
//!
//! A lazily opened file caps its tile and dictionary caches
//! ([`crate::Rete::set_memory_budget`]), but a query's *own* state is not a
//! cache: a hash join builds its right side, a `GROUP BY` keeps its groups, a
//! collected left side is held whole. A starter query that reads 8 MB of a
//! file can still need 1.9 GB of working rows, and nothing in the evaluator
//! says so before it happens. Callers that must stay inside a budget (the
//! card's cost measurement under `--memory-budget-mb`) cannot cap that state,
//! so they **stop the query** when the process crosses a ceiling.
//!
//! The mechanism is cooperative and cheap. [`QueryGuard::start`] arms the
//! calling thread and starts a watchdog that reads the process's resident set
//! size every few milliseconds; past the ceiling it sets a flag. Every index
//! tile access and every row a scan produces ([`check`]) looks at the flag, and a tripped one unwinds the
//! evaluation with an [`OverBudget`] payload ([`std::panic::resume_unwind`], so
//! no panic message is printed). The caller catches it with [`run_guarded`]
//! and reports the query as over budget.
//!
//! Limits, stated plainly: the check lives where tiles are read, so a stage
//! that only reshuffles rows already in memory is not interrupted until the
//! next tile access; the overshoot is what one watchdog interval and one tile
//! can allocate. A query on another thread than the one that armed the guard
//! is not stopped. Where there is no `/proc` (not Linux) the watchdog has
//! nothing to read and the guard never trips.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// The unwind payload of a query stopped by a [`QueryGuard`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverBudget;

/// How many guards are armed, on any thread: while it is zero (every query
/// that is not a budgeted cost measurement) [`check`] is one relaxed load.
static ARMED_GUARDS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static ARMED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Called wherever the evaluator takes a tile or builds a scanned row: unwinds with [`OverBudget`]
/// when this thread's guard has tripped. One thread-local read, and one atomic
/// load when armed.
#[inline]
pub fn check() {
    if ARMED_GUARDS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let tripped = ARMED.with(|a| {
        a.try_borrow()
            .ok()
            .and_then(|g| g.as_ref().map(|f| f.load(Ordering::Relaxed)))
            .unwrap_or(false)
    });
    if tripped {
        std::panic::resume_unwind(Box::new(OverBudget));
    }
}

/// Resident set size of this process now, in bytes (`VmRSS`); `None` off Linux.
pub fn current_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// How often the watchdog reads the resident set size.
#[cfg(not(target_arch = "wasm32"))]
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// A ceiling on the process's resident set size for the queries run on this
/// thread while it lives. See the [module notes](self).
#[cfg(not(target_arch = "wasm32"))]
pub struct QueryGuard {
    tripped: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl QueryGuard {
    /// Arm this thread: the process may not pass `ceiling_bytes` of resident
    /// memory while the guard lives. `read` is the memory probe (the CLI passes
    /// [`current_rss_bytes`]; tests pass a stand-in).
    pub fn start(ceiling_bytes: u64, read: fn() -> Option<u64>) -> Self {
        let tripped = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = read().is_some().then(|| {
            let (tripped, stop) = (tripped.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if read().is_some_and(|rss| rss > ceiling_bytes) {
                        tripped.store(true, Ordering::Relaxed);
                        return;
                    }
                    std::thread::sleep(WATCH_INTERVAL);
                }
            })
        });
        ARMED.with(|a| *a.borrow_mut() = Some(tripped.clone()));
        ARMED_GUARDS.fetch_add(1, Ordering::Relaxed);
        QueryGuard {
            tripped,
            stop,
            thread,
        }
    }

    /// Whether the ceiling was crossed while the guard lived.
    pub fn tripped(&self) -> bool {
        self.tripped.load(Ordering::Relaxed)
    }

    /// Trip the guard by hand — what the watchdog does on a crossing.
    pub fn trip(&self) {
        self.tripped.store(true, Ordering::Relaxed);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for QueryGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        ARMED_GUARDS.fetch_sub(1, Ordering::Relaxed);
        ARMED.with(|a| {
            if let Ok(mut g) = a.try_borrow_mut() {
                *g = None;
            }
        });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Run `f`: `Err(OverBudget)` when a guard stopped it, any other panic
/// carried on.
pub fn run_guarded<T>(f: impl FnOnce() -> T) -> Result<T, OverBudget> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(payload) => match payload.downcast::<OverBudget>() {
            Ok(_) => Err(OverBudget),
            Err(other) => std::panic::resume_unwind(other),
        },
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn always_big() -> Option<u64> {
        Some(u64::MAX)
    }
    fn always_small() -> Option<u64> {
        Some(1)
    }

    #[test]
    fn an_unarmed_thread_never_unwinds() {
        check();
    }

    #[test]
    fn a_crossed_ceiling_stops_the_next_check() {
        let g = QueryGuard::start(1 << 20, always_big);
        let r = run_guarded(|| loop {
            check();
            std::thread::sleep(std::time::Duration::from_millis(1));
        });
        assert_eq!(r, Err(OverBudget));
        assert!(g.tripped());
    }

    #[test]
    fn a_ceiling_never_crossed_lets_the_run_finish() {
        let g = QueryGuard::start(1 << 20, always_small);
        let r = run_guarded(|| {
            for _ in 0..100 {
                check();
            }
            7
        });
        assert_eq!(r, Ok(7));
        assert!(!g.tripped());
    }

    #[test]
    fn dropping_the_guard_disarms_the_thread() {
        {
            let g = QueryGuard::start(1, always_big);
            g.trip();
        }
        check();
    }

    #[test]
    fn other_panics_pass_through() {
        let r = std::panic::catch_unwind(|| {
            let _ = run_guarded(|| std::panic::resume_unwind(Box::new("boom")));
        });
        assert!(r.is_err());
    }
}
