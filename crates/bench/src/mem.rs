//! Heap and RSS instrumentation for the benchmark binary.
//!
//! A counting [`GlobalAlloc`] wrapper tracks the process's live heap and its
//! high-water mark, so each query can report its **peak extra heap** (peak
//! during the query minus live bytes at its start) — a deterministic,
//! engine-agnostic memory metric that works for rete and Oxigraph alike.
//! `vm_hwm_kb` additionally reads the OS-level peak RSS (Linux `VmHWM`).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

pub struct CountingAlloc;

fn add(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

fn sub(size: usize) {
    LIVE.fetch_sub(size, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            add(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            add(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        sub(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            sub(layout.size());
            add(new_size);
        }
        p
    }
}

/// Live heap bytes right now.
pub fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// Reset the high-water mark to the current live size (call before a
/// measurement window).
pub fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// High-water mark since the last [`reset_peak`].
pub fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// OS-reported peak resident set size in KiB (Linux `VmHWM`); `None` where
/// `/proc` is unavailable.
pub fn vm_hwm_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Render a byte count as a compact MiB string.
pub fn mib(bytes: usize) -> String {
    format!("{:.2}", bytes as f64 / (1024.0 * 1024.0))
}
