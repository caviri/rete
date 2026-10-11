//! `RETE_MEM_TRACE=<seconds>`: a diagnostic memory trace for `rete export`.
//!
//! Off unless the variable is set. When set, the export prints one line to
//! stderr at most every `<seconds>` (checked every 64 Ki rows, so it costs
//! nothing measurable) with the process RSS and peak RSS, the dictionary and
//! tile caches' resident bytes, and the open's fixed directories
//! ([`rete_core::Rete::fixed_overhead`]) — so a long dump's RSS can be set
//! against what `--memory-budget-mb` accounts for at any point. `slot=open`
//! and `slot=end` lines bracket the dump. Linux only for the RSS figures (read
//! from `/proc/self/status`); zeros elsewhere.

use std::time::Instant;

/// `(VmRSS, VmHWM)` in bytes from `/proc/self/status`; zeros elsewhere.
fn rss() -> (u64, u64) {
    let Ok(s) = std::fs::read_to_string("/proc/self/status") else {
        return (0, 0);
    };
    let field = |name: &str| {
        s.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(0, |kb| kb * 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

pub(crate) struct MemTrace {
    every: f64,
    start: Instant,
    last: f64,
    rows: u64,
}

const MB: f64 = (1u64 << 20) as f64;

impl MemTrace {
    pub(crate) fn from_env() -> Option<Self> {
        let every = std::env::var("RETE_MEM_TRACE").ok()?.parse::<f64>().ok()?;
        Some(MemTrace {
            every,
            start: Instant::now(),
            last: f64::NEG_INFINITY,
            rows: 0,
        })
    }

    /// Count one emitted row; print a sample when the interval has elapsed.
    #[inline]
    pub(crate) fn row(&mut self, rete: &rete_core::Rete, slot: &str) {
        self.rows += 1;
        if self.rows & 0xFFFF == 0 {
            let t = self.start.elapsed().as_secs_f64();
            if t - self.last >= self.every {
                self.last = t;
                self.sample(rete, slot);
            }
        }
    }

    pub(crate) fn sample(&self, rete: &rete_core::Rete, slot: &str) {
        let t = self.start.elapsed().as_secs_f64();
        let (rss, hwm) = rss();
        let d = rete.dict_cache_stats();
        let x = rete.index_cache_stats();
        let f = rete.fixed_overhead();
        eprintln!(
            "[memtrace] t={t:.1} rows={} slot={slot} rss_mb={:.0} hwm_mb={:.0} dict_res_mb={:.0} dict_entries={} tile_res_mb={:.0} tile_entries={} fixed_mb={:.1} dict_dir_mb={:.1} index_dir_mb={:.1} named_mb={:.1} decoded_gb={:.1} evictions={}",
            self.rows,
            rss as f64 / MB,
            hwm as f64 / MB,
            d.resident_bytes as f64 / MB,
            d.entries,
            x.resident_bytes as f64 / MB,
            x.entries,
            f.total() as f64 / MB,
            f.dict_directory as f64 / MB,
            f.index_directory as f64 / MB,
            f.named_graphs as f64 / MB,
            d.decoded_bytes as f64 / (1u64 << 30) as f64,
            d.evictions,
        );
    }
}
