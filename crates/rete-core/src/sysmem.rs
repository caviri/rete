//! How much memory this process can still use, and the default query budget
//! that follows from it.
//!
//! A lazily opened file caps its dictionary, tile and block caches with a
//! memory budget ([`crate::Rete::set_memory_budget`]). A fixed default of
//! [`DEFAULT_QUERY_BUDGET_MB`] (4096 MiB) is more than a 4 GiB container holds
//! before the query's own working state is counted, so the default is sized
//! from the memory actually available instead: half of it, at least
//! [`MIN_DEFAULT_QUERY_BUDGET_MB`], at most the old fixed figure.
//! `RETE_QUERY_BUDGET_MB` overrides the default, and an explicit budget (the
//! CLI's `--memory-budget-mb`, a client's `set_memory_budget`) wins over both.
//!
//! The probe is the one the CLI's memory-aware local open uses: the smaller of
//! the kernel's `MemAvailable` and the headroom under every cgroup memory
//! limit above the process.

use crate::file::DEFAULT_QUERY_BUDGET_MB;

/// The lowest default query budget, whatever the memory probe says: below it
/// the caches thrash rather than save memory (one decoded dictionary chunk or
/// index tile is 64 KiB to a few MiB).
pub const MIN_DEFAULT_QUERY_BUDGET_MB: u64 = 256;

/// The default budget takes `1 / QUERY_BUDGET_SHARE` of the available memory,
/// leaving the rest to what the budget does not cover: hash-join build sides,
/// `GROUP BY` groups, `DISTINCT` sets, the result itself.
const QUERY_BUDGET_SHARE: u64 = 2;

/// The environment variable that overrides the default query budget, in MiB
/// (`0` = unlimited).
pub const QUERY_BUDGET_ENV: &str = "RETE_QUERY_BUDGET_MB";

/// The default query budget in MiB (`0` = unlimited) for this process:
/// `RETE_QUERY_BUDGET_MB` when set, else sized from [`available_memory`] (see
/// [`default_query_budget_mb_from`]).
pub fn default_query_budget_mb() -> u64 {
    let env = std::env::var(QUERY_BUDGET_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());
    // Only probe the memory when it is going to be used.
    let available = if env.is_some() {
        None
    } else {
        available_memory()
    };
    default_query_budget_mb_from(env, available)
}

/// [`default_query_budget_mb`] with its inputs explicit: `env_mb` is the
/// override (it wins outright, `0` = unlimited), `available` the bytes the
/// process can still allocate (`None` = unknown, which keeps the fixed
/// [`DEFAULT_QUERY_BUDGET_MB`]). Otherwise half the available memory, clamped
/// to `[MIN_DEFAULT_QUERY_BUDGET_MB, DEFAULT_QUERY_BUDGET_MB]`.
pub fn default_query_budget_mb_from(env_mb: Option<u64>, available: Option<u64>) -> u64 {
    if let Some(mb) = env_mb {
        return mb;
    }
    match available {
        Some(bytes) => ((bytes >> 20) / QUERY_BUDGET_SHARE)
            .clamp(MIN_DEFAULT_QUERY_BUDGET_MB, DEFAULT_QUERY_BUDGET_MB),
        None => DEFAULT_QUERY_BUDGET_MB,
    }
}

/// Bytes this process can still allocate before something kills it: the
/// smaller of the kernel's `MemAvailable` and the headroom under every cgroup
/// memory limit above the process (v2 `memory.max`, or v1
/// `memory.limit_in_bytes`). Inside a container `MemAvailable` describes the
/// whole VM, so the cgroup limit is the one that matters there. Reclaimable
/// page cache (`inactive_file`) does not count as used. `None` when neither
/// source is readable (not Linux).
pub fn available_memory() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| meminfo_available(&s));
    let cgroup = cgroup_headroom(std::path::Path::new("/sys/fs/cgroup"));
    match (meminfo, cgroup) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// `MemAvailable` from a `/proc/meminfo` text, in bytes.
fn meminfo_available(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemAvailable:")?;
        let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
        Some(kb.saturating_mul(1024))
    })
}

/// One `key value` field of a cgroup `memory.stat` file.
fn stat_field(stat: &str, key: &str) -> Option<u64> {
    stat.lines().find_map(|line| {
        let (k, v) = line.split_once(' ')?;
        (k == key).then(|| v.trim().parse().ok())?
    })
}

fn read_u64(path: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Headroom under one cgroup v2 directory's `memory.max` (`None` when it has
/// no limit: `max`, or no such file).
fn cgroup_v2_headroom(dir: &std::path::Path) -> Option<u64> {
    let max = read_u64(&dir.join("memory.max"))?;
    let current = read_u64(&dir.join("memory.current")).unwrap_or(0);
    let inactive = std::fs::read_to_string(dir.join("memory.stat"))
        .ok()
        .and_then(|s| stat_field(&s, "inactive_file"))
        .unwrap_or(0);
    Some(max.saturating_sub(current.saturating_sub(inactive)))
}

/// Headroom under a cgroup v1 memory controller directory. v1 reports "no
/// limit" as a page-rounded `i64::MAX`, so anything past 2^60 is no limit.
fn cgroup_v1_headroom(dir: &std::path::Path) -> Option<u64> {
    let limit = read_u64(&dir.join("memory.limit_in_bytes")).filter(|&l| l < 1 << 60)?;
    let usage = read_u64(&dir.join("memory.usage_in_bytes")).unwrap_or(0);
    let inactive = std::fs::read_to_string(dir.join("memory.stat"))
        .ok()
        .and_then(|s| stat_field(&s, "total_inactive_file"))
        .unwrap_or(0);
    Some(limit.saturating_sub(usage.saturating_sub(inactive)))
}

/// The tightest headroom of the cgroups this process sits in, under the
/// cgroup filesystem mounted at `root`. For v2 that walks from the process's
/// own group (`0::<path>` in `/proc/self/cgroup`) up to `root`, since a limit
/// set on any ancestor binds; inside a container with its own cgroup
/// namespace the path is `/` and `root` itself carries the container's limit.
fn cgroup_headroom(root: &std::path::Path) -> Option<u64> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok();
    let v2 = own
        .as_deref()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("0::")))
        .map(|p| p.trim().trim_start_matches('/').to_string());
    let mut best: Option<u64> = None;
    if let Some(rel) = v2 {
        let mut dir = root.join(rel);
        loop {
            if let Some(h) = cgroup_v2_headroom(&dir) {
                best = Some(best.map_or(h, |b| b.min(h)));
            }
            if dir.as_path() == root || !dir.pop() || !dir.starts_with(root) {
                break;
            }
        }
    }
    best.or_else(|| cgroup_v1_headroom(&root.join("memory")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;

    /// A fresh, empty directory under the system temp dir (no `tempfile` in
    /// this crate's dev-dependencies), removed when dropped.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let p =
                std::env::temp_dir().join(format!("rete-sysmem-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_default_budget_follows_available_memory() {
        // A 4 GiB container with ~3.8 GiB free: half of it, under the old
        // fixed 4096 MiB that the container could not hold.
        assert_eq!(default_query_budget_mb_from(None, Some(3800 * MIB)), 1900);
        // A roomy machine keeps the old default.
        assert_eq!(
            default_query_budget_mb_from(None, Some(48 * GIB)),
            DEFAULT_QUERY_BUDGET_MB
        );
        // A tiny one gets the floor.
        assert_eq!(
            default_query_budget_mb_from(None, Some(300 * MIB)),
            MIN_DEFAULT_QUERY_BUDGET_MB
        );
        // Unknown memory (macOS, Windows): the old fixed figure.
        assert_eq!(
            default_query_budget_mb_from(None, None),
            DEFAULT_QUERY_BUDGET_MB
        );
    }

    #[test]
    fn the_env_override_wins_unclamped() {
        assert_eq!(default_query_budget_mb_from(Some(0), Some(GIB)), 0);
        assert_eq!(default_query_budget_mb_from(Some(64), Some(48 * GIB)), 64);
        assert_eq!(default_query_budget_mb_from(Some(9000), Some(GIB)), 9000);
    }

    #[test]
    fn parses_meminfo() {
        let s =
            "MemTotal:       49283460 kB\nMemFree:         1234 kB\nMemAvailable:   40000000 kB\n";
        assert_eq!(meminfo_available(s), Some(40_000_000 * 1024));
        assert_eq!(meminfo_available("MemTotal: 1 kB\n"), None);
    }

    /// A fake cgroup v2 tree: a limit on the group, a tighter one on its
    /// parent; reclaimable page cache does not count as used.
    #[test]
    fn cgroup_v2_headroom_takes_the_tightest_limit() {
        let root = TempDir::new("v2");
        let leaf = root.0.join("a/b");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(leaf.join("memory.max"), "max\n").unwrap();
        std::fs::write(root.0.join("a/memory.max"), format!("{}\n", 4 * GIB)).unwrap();
        std::fs::write(
            root.0.join("a/memory.current"),
            format!("{}\n", GIB + 256 * MIB),
        )
        .unwrap();
        std::fs::write(
            root.0.join("a/memory.stat"),
            format!("anon 1\ninactive_file {}\nactive_file 9\n", 256 * MIB),
        )
        .unwrap();
        assert_eq!(cgroup_v2_headroom(&leaf), None, "`max` is no limit");
        assert_eq!(
            cgroup_v2_headroom(&root.0.join("a")),
            Some(3 * GIB),
            "4 GiB limit, 1.25 GiB charged of which 256 MiB is page cache"
        );
    }

    #[test]
    fn cgroup_v1_unlimited_is_no_limit() {
        let dir = TempDir::new("v1");
        std::fs::write(dir.0.join("memory.limit_in_bytes"), "9223372036854771712\n").unwrap();
        assert_eq!(cgroup_v1_headroom(&dir.0), None);
        std::fs::write(dir.0.join("memory.limit_in_bytes"), format!("{}", 2 * GIB)).unwrap();
        std::fs::write(dir.0.join("memory.usage_in_bytes"), format!("{GIB}")).unwrap();
        assert_eq!(cgroup_v1_headroom(&dir.0), Some(GIB));
    }

    /// On the Linux this suite runs on (the dev container), the probe finds
    /// something; elsewhere `None` is the documented answer.
    #[test]
    fn available_memory_is_readable_on_linux() {
        if cfg!(target_os = "linux") {
            let a = available_memory().expect("Linux exposes MemAvailable");
            assert!(a > 0);
        }
    }
}
