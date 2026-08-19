#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildPhase {
    ParseIngest,
    ChunkSeal,
    Canonicalize,
    Remap,
    Pyramid,
    TextIndex,
    SubjectFamily,
    PredicateFamily,
    ObjectFamily,
    TileEncodeCompress,
    FinalWrite,
    Install,
    Total,
}

impl BuildPhase {
    fn label(self) -> &'static str {
        match self {
            Self::ParseIngest => "parse+ingest",
            Self::ChunkSeal => "chunk seal",
            Self::Canonicalize => "canonicalize",
            Self::Remap => "remap",
            Self::Pyramid => "pyramid",
            Self::TextIndex => "text index",
            Self::SubjectFamily => "index families",
            Self::PredicateFamily => "predicate family",
            Self::ObjectFamily => "object family",
            Self::TileEncodeCompress => "tile encode+compress",
            Self::FinalWrite => "final write",
            Self::Install => "install",
            Self::Total => "total",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BuildCounters {
    pub statements: u64,
    pub input_bytes: Option<u64>,
    pub spill_bytes: u64,
    pub output_bytes: u64,
    pub family_runs: [u64; 3],
}

pub(crate) struct BuildTiming {
    enabled: bool,
    samples: Vec<(BuildPhase, u128)>,
    counters: BuildCounters,
    #[cfg(not(target_arch = "wasm32"))]
    lap_started: Option<std::time::Instant>,
}

impl BuildTiming {
    pub(crate) fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let enabled = std::env::var_os("RETE_BUILD_TIMING").is_some();
            Self {
                enabled,
                samples: Vec::new(),
                counters: BuildCounters::default(),
                lap_started: enabled.then(std::time::Instant::now),
            }
        }

        #[cfg(target_arch = "wasm32")]
        Self {
            enabled: false,
            samples: Vec::new(),
            counters: BuildCounters::default(),
        }
    }

    pub(crate) fn lap(&mut self, phase: BuildPhase) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(started) = self.lap_started.as_mut() {
            self.samples.push((phase, started.elapsed().as_millis()));
            *started = std::time::Instant::now();
        }
    }

    pub(crate) fn set_counters(&mut self, counters: BuildCounters) {
        self.counters = counters;
    }

    pub(crate) fn finish(&mut self) {
        if !self.enabled {
            return;
        }
        self.lap(BuildPhase::Total);
        for line in self.render_lines() {
            eprintln!("{line}");
        }
    }

    pub(crate) fn render_lines(&self) -> Vec<String> {
        let mut lines = self
            .samples
            .iter()
            .map(|(phase, elapsed)| format!("  [build] {}: {elapsed} ms", phase.label()))
            .collect::<Vec<_>>();
        let input = self
            .counters
            .input_bytes
            .map_or_else(|| "unknown".to_owned(), |bytes| format!("{bytes} B"));
        lines.push(format!(
            "  [build] statements: {}, input: {input}, spill: {} B, output: {} B",
            self.counters.statements, self.counters.spill_bytes, self.counters.output_bytes
        ));
        lines.push(format!(
            "  [build] family runs (S/P/O): {}/{}/{}",
            self.counters.family_runs[0],
            self.counters.family_runs[1],
            self.counters.family_runs[2],
        ));
        lines
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self {
            enabled: false,
            samples: Vec::new(),
            counters: BuildCounters::default(),
            #[cfg(not(target_arch = "wasm32"))]
            lap_started: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn record_for_test(&mut self, phase: BuildPhase, elapsed: u128) {
        self.samples.push((phase, elapsed));
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildCounters, BuildPhase, BuildTiming};

    #[test]
    fn timing_render_is_ordered_and_machine_independent() {
        let mut timing = BuildTiming::new_for_test();
        timing.record_for_test(BuildPhase::ParseIngest, 12);
        timing.record_for_test(BuildPhase::Canonicalize, 7);
        timing.set_counters(BuildCounters {
            statements: 3,
            input_bytes: Some(99),
            spill_bytes: 0,
            output_bytes: 42,
            family_runs: [1, 1, 1],
        });
        assert_eq!(
            timing.render_lines(),
            vec![
                "  [build] parse+ingest: 12 ms",
                "  [build] canonicalize: 7 ms",
                "  [build] statements: 3, input: 99 B, spill: 0 B, output: 42 B",
                "  [build] family runs (S/P/O): 1/1/1",
            ]
        );
    }
}
