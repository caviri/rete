//! `rete estimate` — predict a build's size, time and spill **before** running it.
//!
//! A billion-triple build is a multi-hour, multi-hundred-GB commitment; finding out
//! it needed 60 GB of spill or produced a 90 GB file *after* two days is the
//! expensive way to learn. This streams the input (all of it, or a leading sample),
//! parses every statement exactly as `rete build` would, and reports what the build
//! would cost — without writing a byte of output or spilling anything to disk.
//!
//! Distinct terms are counted with **HyperLogLog** (2^14 registers, 16 KiB, ~0.8 %
//! relative error) rather than a HashSet: the dictionary of a 9.8 B-triple graph
//! holds 1.9 B distinct terms, and an exact set of those would need tens of GB —
//! i.e. the estimator would hit the very wall it exists to warn about.
//!
//! ## How the size is projected (G38)
//!
//! Up to 0.5.5 the size came from three constants fitted to scholarly graphs
//! (0.62 B per id-triple per permutation, a 14 % dictionary, an 18 % pyramid). It
//! had no term for the label index, the text index or the card, and the
//! constants are right only for graphs shaped like the ones they were fitted to:
//! a tutorial graph of 9.9 M triples came out at 47.5–71.3 MB against 105.9 MB.
//!
//! The size is now **measured**. [`DEFAULT_SAMPLE_STATEMENTS`] statements, cut
//! from windows spread over the input ([`Windows`]), are kept in memory and built
//! by the real in-memory writer, with the flags the real build would use
//! (`--label-index`, `--text-index`), and every section of that image is
//! measured. Each section is then scaled up by what drives it: the index, labels,
//! text and directories by the statement count, the dictionary by the bytes of
//! its distinct terms ([`DistinctSample`]). When the whole input fits in the
//! sample the sections are exact and the projection is the file. The in-memory and the
//! memory-bounded builds write byte-identical sections (`tests/bounded_v06.rs`),
//! so the measurement holds for `--memory-budget-mb` builds.
//!
//! The spill, the time and the memory are fitted to measured builds, the
//! evidence in the pull request of G38; the constants are named below with their
//! sources.

use std::time::Instant;

use rete_core::hll::Hll;
use rete_core::ingest::{self, AssembleOptions, GraphLayout, RawQuad};

/// Statements kept and built to measure the sections. Most of the estimator's
/// own time is the build of this sample (2 to 5 s per million statements);
/// `--sample-statements` changes it.
pub(crate) const DEFAULT_SAMPLE_STATEMENTS: usize = 1_000_000;

/// The pyramid, as a fraction of dictionary + index, for an in-RAM build that
/// writes one (a bounded build, `--memory-budget-mb`, never does). Measured on
/// the published graphs in 0.4; the sample is built without a pyramid because
/// Louvain on a million statements would take longer than the rest of the
/// estimate, so this term alone is not re-measured.
const PYRAMID_OVERHEAD: f64 = 0.18;

/// The card region the build writes for `--card`: the profile with its top-N
/// lists and the build record. 18.4 KB at 0.8 M statements, 24 KB at 30 M.
const CARD_BYTES: f64 = 24.0 * 1024.0;

/// Relative width of the size band when every statement was built (the sample is
/// the file; what is left is the card and the record).
const SIZE_BAND_EXACT: f64 = 0.03;
/// Relative width of the size band when the sections were scaled from a sample:
/// (below, above) the projection. The real file was 2% under to 13% over the
/// middle of it in the matrix, never below: a graph's ids get longer as it grows
/// and a sample cannot show that (the index of the scholarly graph is 14.1 bytes
/// per statement at 0.8 M and 16.6 at 7.8 M, the solar graph's 9.8 and 10.2).
const SIZE_BAND_SCALED: (f64, f64) = (0.12, 0.20);

// Spill. Peak bytes of the `--tmp-dir` of a bounded build, from the matrix of
// builds in the G38 PR (synthetic graphs of 0.8 to 29.8 M statements in two
// shapes, budgets 512 and 1500, `--no-pyramid --label-index`, with and without
// `--text-index`), polled from the directory every 0.2 s. The build record's own
// `scratch peak` is up to 35 % lower on a text-index build, which is why the
// directory is the reference. The peak is in the last permutations, where the
// finished sections are most of it (docs/cli.md, "Its spill is compressed").
/// Spill per byte of the finished dictionary, index, label and directory
/// sections. Against the sections the builds really wrote the ratio is 1.06 (at
/// 1 M statements) to 1.18 (at 29.8 M) on the solar graph and 1.29 on the
/// scholarly one without text; 1.25 is fitted to the projected sections, which
/// carry their own errors.
const SPILL_PER_CORE_BYTE: f64 = 1.25;
/// Spill per byte of the finished text-index section. The word spill is not
/// front-coded and holds every (word, subject) pair: 5.5 to 7 times the section
/// on the scholarly graph, 6 as fitted.
const SPILL_PER_TEXT_BYTE: f64 = 6.0;
/// Relative width of the spill range. The fit is within 17 % at every measured
/// point.
const SPILL_BAND: f64 = 0.25;
/// Bytes a statement takes in a chunk, before the dictionary: fitted so that
/// the chunk count matches the builds'. 8 chunks for 9.9 M statements at 512
/// (3 at 1500) and 24 for 29.8 M on the solar graph, 8 for 7.8 M on the
/// scholarly one, whose terms are longer; a chunk is half the budget.
const CHUNK_BYTES_PER_STATEMENT: f64 = 190.0;
/// And per byte of distinct-term text per statement.
const CHUNK_BYTES_PER_TERM_BYTE: f64 = 1.7;

// Time. `build = K × (parse pass over the whole input + the sample's build,
// scaled to every statement)`. K = 1.6 to 2.4 over the matrix (the six
// permutation merges and the spill's compression, which the in-memory sample
// does not do), 2.0 as fitted. The sample's build and the parse are measured
// on the machine that runs the estimate, so the projection follows its speed.
const K_BUILD: f64 = 2.0;
/// Relative width of the time range.
const TIME_BAND: f64 = 0.30;
/// The 20 starter queries measured against the finished file (a card without
/// `--no-card-costs`): 13.2 s at 1 M statements, 7 s at 10 M, 7 to 14 s in the
/// tutorial's runs. Bounded queries, so it does not grow with the graph.
const STARTER_SECS: (f64, f64) = (6.0, 15.0);

/// Peak resident memory of a bounded build, relative to its budget: 0.64 to
/// 1.27 of a budget of 512 and 0.22 to 1.11 of 1500 over the matrix. A small
/// graph does not use a large budget; long text went past a small one, by a
/// fixed amount (the merge's buffers and the allocator: 650 MB at 512, 1,631 at
/// 1500, on the 30 M scholarly graph). So the range is a fraction of the budget
/// and a fixed slack above it.
const RAM_FRACTION: (f64, f64) = (0.2, 1.15);
/// Lowest peak projected (a build's own floor), and the slack above the budget.
const RAM_FLOOR: f64 = 128.0 * 1048576.0;
const RAM_SLACK: f64 = 128.0 * 1048576.0;

fn human(bytes: f64) -> String {
    let u = ["B", "KB", "MB", "GB", "TB"];
    let mut b = bytes;
    let mut i = 0;
    while b >= 1000.0 && i < u.len() - 1 {
        b /= 1000.0;
        i += 1;
    }
    format!("{b:.1} {}", u[i])
}

fn hms(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

/// The arguments of `rete estimate`.
pub(crate) struct EstimateArgs<'a> {
    pub inputs: &'a [String],
    pub format: Option<&'a str>,
    pub sample_mb: Option<u64>,
    pub memory_budget_mb: Option<u64>,
    pub no_pyramid: bool,
    pub label_index: bool,
    pub text_index: bool,
    pub card: bool,
    pub no_card_costs: bool,
    pub sample_statements: usize,
    pub json: bool,
}

/// The sections of the sample's image, in bytes.
#[derive(Debug, Clone, Default)]
struct Sample {
    statements: u64,
    terms: u64,
    /// Bytes of the distinct terms the sample holds, as text.
    term_bytes: f64,
    dict: f64,
    index: f64,
    labels: f64,
    text: f64,
    /// Directories, leaves, schema, root: everything that is not one of the above
    /// or the card.
    other: f64,
    /// Seconds the in-memory build of the sample took.
    build_secs: f64,
}

/// Build `quads` as the writer would (no pyramid, quad index for named graphs)
/// and measure the sections.
fn measure_sample(
    quads: Vec<RawQuad>,
    terms: u64,
    term_bytes: f64,
    label_index: bool,
    text_index: bool,
) -> anyhow::Result<Sample> {
    let statements = quads.len() as u64;
    let opts = AssembleOptions {
        with_pyramid: false,
        with_text_index: text_index,
        text_index_scope: None,
        graph_layout: GraphLayout::Quad,
        label_index,
        ..AssembleOptions::sectioned()
    };
    let started = Instant::now();
    let (image, _) = ingest::assemble_dataset_opts(quads, &opts, |_, _| Vec::<u8>::new());
    // The in-memory assembler's image is the legacy layout; the file `rete build`
    // writes is the one this finishes it into (the same code `rete convert` runs).
    let image = super::v06::finish(image)?;
    let build_secs = started.elapsed().as_secs_f64();
    let rete = rete_core::Rete::open(&image)?;
    let mut s = Sample {
        statements,
        terms,
        term_bytes,
        build_secs,
        ..Sample::default()
    };
    let mut fixed = 0.0;
    for seg in rete.file_layout() {
        let len = seg.len as f64;
        // The section's own name (`v06::kind::name`), which tells the data
        // sections from the directory leaves that route into them.
        match seg.label.as_str() {
            "dict" => s.dict += len,
            "index" | "quad" => s.index += len,
            "labels" => s.labels += len,
            "text" => s.text += len,
            _ if seg.kind == "metadata" || seg.kind == "build-info" => fixed += len,
            _ => s.other += len,
        }
    }
    // Framing the layout does not name (section table padding) is part of "other".
    let accounted = s.dict + s.index + s.labels + s.text + s.other + fixed;
    s.other += (image.len() as f64 - accounted).max(0.0);
    Ok(s)
}

/// A uniform sample of the distinct terms, chosen by hash (a term is in or out
/// wherever it occurs, so the sample does not depend on the input's order), and
/// thinned by one hash bit whenever it outgrows its cap.
struct DistinctSample {
    cap: usize,
    level: u32,
    terms: std::collections::HashMap<u64, u32>,
}

impl DistinctSample {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            level: 0,
            terms: std::collections::HashMap::new(),
        }
    }

    fn hash(t: &str) -> u64 {
        // FNV-1a, then a finalizer so the low bits are well mixed.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in t.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h
    }

    fn add(&mut self, t: &str) {
        let h = Self::hash(t);
        if h.trailing_zeros() < self.level {
            return;
        }
        self.terms.insert(h, t.len() as u32);
        while self.terms.len() > self.cap {
            self.level += 1;
            let level = self.level;
            self.terms.retain(|h, _| h.trailing_zeros() >= level);
        }
    }

    /// (distinct terms, their bytes), each scaled back up by the sampling rate.
    fn totals(&self) -> (f64, f64) {
        let f = (1u64 << self.level) as f64;
        let bytes: u64 = self.terms.values().map(|&l| l as u64).sum();
        (self.terms.len() as f64 * f, bytes as f64 * f)
    }
}

/// Statements per window of the sample.
const WINDOW_STATEMENTS: usize = 50_000;

/// The sample: windows of consecutive statements, `capacity / WINDOW_STATEMENTS`
/// of them, drawn evenly from the whole stream by reservoir sampling (window `w`
/// replaces a random kept one with probability `kept / (w + 1)`, from a fixed
/// seed, so an estimate is repeatable). Consecutive, because what the index
/// costs depends on how statements about one subject sit together; spread over
/// the input, because a dump is rarely uniform from its first line to its last
/// (a block of people before the papers, a header of schema). An input no
/// bigger than the capacity is kept whole and in order.
struct Windows {
    /// Statements per window: `WINDOW_STATEMENTS`, or the whole capacity when
    /// that is smaller.
    win: usize,
    keep: usize,
    seen: u64,
    cur: Vec<RawQuad>,
    kept: Vec<(u64, Vec<RawQuad>)>,
    rng: u64,
}

impl Windows {
    fn new(capacity: usize) -> Self {
        Self::with_window(capacity, WINDOW_STATEMENTS)
    }

    fn with_window(capacity: usize, window: usize) -> Self {
        let win = window.min(capacity.max(1));
        Windows {
            win,
            keep: (capacity / win).max(1),
            seen: 0,
            cur: Vec::new(),
            kept: Vec::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn push(&mut self, s: &str, p: &str, o: &str, g: &Option<String>) {
        self.cur
            .push((s.to_string(), p.to_string(), o.to_string(), g.clone()));
        if self.cur.len() >= self.win {
            self.close();
        }
    }

    fn close(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let w = self.seen;
        self.seen += 1;
        let win = std::mem::take(&mut self.cur);
        if self.kept.len() < self.keep {
            self.kept.push((w, win));
        } else {
            // xorshift64
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let j = (self.rng % (w + 1)) as usize;
            if j < self.keep {
                self.kept[j] = (w, win);
            }
        }
    }

    /// The sample, windows in the order they appeared in the input.
    fn finish(mut self) -> Vec<RawQuad> {
        self.close();
        self.kept.sort_by_key(|(w, _)| *w);
        self.kept.into_iter().flat_map(|(_, v)| v).collect()
    }
}

/// What the stream pass and the sample measured; the projection's input.
struct Measured {
    statements: f64,
    /// Bytes of every distinct term of the input, as text.
    est_term_bytes: f64,
    /// `scale` of a sampled input (`--sample-mb`), 1.0 when all of it was read.
    input_scale: f64,
    parse_all_secs: f64,
    sample: Sample,
}

/// The projection, in bytes and seconds, each a (low, high) range.
#[derive(Debug, Clone)]
struct Projection {
    dict: f64,
    index: f64,
    labels: f64,
    text: f64,
    other: f64,
    card: f64,
    pyramid: f64,
    size: (f64, f64),
    build_secs: (f64, f64),
    starter_secs: (f64, f64),
    spill: (f64, f64),
    chunks: f64,
    ram: (f64, f64),
    parse_all_secs: f64,
}

struct Plan {
    budget_bytes: f64,
    pyramid: bool,
    card: bool,
    card_costs: bool,
}

fn project(m: &Measured, plan: &Plan) -> Projection {
    let s = &m.sample;
    let n_s = (s.statements as f64).max(1.0);
    let per_statement = m.statements / n_s;
    // The sample is the whole input exactly when it holds every statement.
    let exact = m.input_scale <= 1.0 && s.statements as f64 >= m.statements - 0.5;
    // The dictionary follows the bytes of its distinct terms, at the ratio the
    // sample's dictionary reached on its own. Not the term count: a sample of
    // windows holds references to terms whose own statements it does not hold,
    // so it has relatively more short IRIs than the input has.
    let dict_ratio = if s.term_bytes > 0.0 {
        s.dict / s.term_bytes
    } else {
        0.0
    };
    let (dict, index, labels, text, other) = if exact {
        (s.dict, s.index, s.labels, s.text, s.other)
    } else {
        (
            m.est_term_bytes * dict_ratio,
            s.index * per_statement,
            s.labels * per_statement,
            s.text * per_statement,
            s.other * per_statement,
        )
    };
    let core = dict + index + other;
    let pyramid = if plan.pyramid {
        (dict + index) * PYRAMID_OVERHEAD
    } else {
        0.0
    };
    let card = if plan.card { CARD_BYTES } else { 0.0 };
    let total = core + labels + text + pyramid + card;
    let (below, above) = if exact {
        (SIZE_BAND_EXACT, SIZE_BAND_EXACT)
    } else {
        SIZE_BAND_SCALED
    };

    let build = K_BUILD * (m.parse_all_secs + s.build_secs * per_statement);
    let starter = if plan.card && plan.card_costs {
        STARTER_SECS
    } else {
        (0.0, 0.0)
    };

    let spill = SPILL_PER_CORE_BYTE * (dict + index + labels + other) + SPILL_PER_TEXT_BYTE * text;
    let chunk_budget = plan.budget_bytes * 0.5;
    let term_bytes_per_statement = m.est_term_bytes / m.statements.max(1.0);
    let chunk_bytes = m.statements
        * (CHUNK_BYTES_PER_STATEMENT + CHUNK_BYTES_PER_TERM_BYTE * term_bytes_per_statement);
    let chunks = (chunk_bytes / chunk_budget).ceil().max(1.0);

    Projection {
        dict,
        index,
        labels,
        text,
        other,
        card,
        pyramid,
        size: (total * (1.0 - below), total * (1.0 + above)),
        build_secs: (build * (1.0 - TIME_BAND), build * (1.0 + TIME_BAND)),
        starter_secs: starter,
        spill: (spill * (1.0 - SPILL_BAND), spill * (1.0 + SPILL_BAND)),
        chunks,
        ram: (
            (plan.budget_bytes * RAM_FRACTION.0).max(RAM_FLOOR),
            plan.budget_bytes * RAM_FRACTION.1 + RAM_SLACK,
        ),
        parse_all_secs: m.parse_all_secs,
    }
}

/// Stream `inputs`, measure, and print the projected build cost.
pub(crate) fn estimate(a: &EstimateArgs<'_>) -> anyhow::Result<()> {
    let sample_bytes = a.sample_mb.map(|m| m * (1 << 20));
    let mut total_input: u64 = 0;
    for i in a.inputs {
        if i != "-" {
            total_input += std::fs::metadata(i).map(|m| m.len()).unwrap_or(0);
        }
    }

    let mut hll = Hll::new();
    let mut statements: u64 = 0;
    // A hash-chosen, position-independent sample of the distinct terms: their
    // number and bytes over the whole input, which HyperLogLog's count alone does
    // not give.
    let mut distinct = DistinctSample::new(1_000_000);
    let mut literals: u64 = 0;
    let mut graphs = std::collections::HashSet::new();
    let mut read_bytes: u64 = 0;
    // The statements the real writer measures: `sample_statements` of them, taken
    // as contiguous windows spread over the whole input (see [`Windows`]).
    let mut windows = Windows::new(a.sample_statements);
    let started = Instant::now();

    for path in a.inputs {
        let fmt = super::build::input_format(path, a.format);
        if !matches!(fmt, "nt" | "nq" | "ttl" | "trig" | "jelly") {
            anyhow::bail!(
                "estimate reads N-Triples/N-Quads/Turtle/TriG/Jelly (got {fmt} for {path}); \
                 pipe a converter's output with --format nt"
            );
        }
        if sample_bytes.is_some_and(|limit| read_bytes >= limit) {
            break;
        }
        // The whole input streams through one parser pass. `--sample-mb` is enforced
        // UNDER the parser by a counting reader that reports EOF at the limit — which
        // works for Turtle and TriG, where a statement spans lines and the earlier
        // line-chunking trick could not. Cutting the stream mid-statement makes the
        // parser (rightly) complain about the truncated tail, so a parse error is
        // tolerated only when the cut is what caused it.
        let remaining = sample_bytes.map(|limit| limit.saturating_sub(read_bytes));
        let (reader, counter) = super::build::open_reader_counted(path, remaining)?;
        let res = rete_core::ingest::stream_reader(reader, fmt, &mut |(s, p, o, g)| {
            statements += 1;
            if o.starts_with('"') {
                literals += 1;
            }
            windows.push(&s, &p, &o, &g);
            for t in [&s, &p, &o] {
                hll.add(t);
                distinct.add(t);
            }
            if let Some(g) = g {
                hll.add(&g);
                if graphs.len() < 100_000 {
                    graphs.insert(g);
                }
            }
        });
        let truncated = remaining.is_some_and(|limit| counter.get() >= limit);
        read_bytes += counter.get();
        if let Err(e) = res {
            if !truncated {
                anyhow::bail!("{path}: {e}");
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64().max(1e-6);
    let sampled = sample_bytes.is_some() && read_bytes < total_input;
    let scale = if sampled && read_bytes > 0 {
        total_input as f64 / read_bytes as f64
    } else {
        1.0
    };

    let est_statements = statements as f64 * scale;
    // Distinct terms grow sub-linearly with the input: past the first appearance of
    // each subject/predicate/object, new statements mostly REUSE terms. Extrapolate
    // the sampled cardinality by sqrt(scale) — the empirical middle ground between
    // "no new terms" (scale^0) and "all terms new" (scale^1) on the published
    // scholarly graphs. Exact when the whole input was read (scale = 1).
    let est_terms = hll.count() as f64 * scale.sqrt();
    let (distinct_n, distinct_bytes) = distinct.totals();
    // The same sub-linear extrapolation as the count, for a sampled input.
    let est_term_bytes = distinct_bytes * scale.sqrt();
    let avg_uniq_term = if distinct_n > 0.0 {
        distinct_bytes / distinct_n
    } else {
        0.0
    };
    drop(distinct);

    let rate = read_bytes as f64 / elapsed; // input bytes/s, parse-only
    let parse_all = total_input as f64 / rate;

    let kept = windows.finish();
    let (terms_in_sample, term_bytes_in_sample) = {
        let mut t = std::collections::HashSet::new();
        for (s, p, o, g) in &kept {
            t.extend([s, p, o]);
            t.extend(g.iter());
        }
        (
            t.len() as u64,
            t.iter().map(|x| x.len() as f64).sum::<f64>(),
        )
    };
    // With nothing parsed there is nothing to build; the projection is zero.
    let sample = if kept.is_empty() {
        Sample::default()
    } else {
        measure_sample(
            kept,
            terms_in_sample,
            term_bytes_in_sample,
            a.label_index,
            a.text_index,
        )?
    };

    let budget = a.memory_budget_mb.unwrap_or(4096) as f64 * (1u64 << 20) as f64;
    // A bounded build writes no pyramid; an in-RAM build writes one unless told not to.
    let plan = Plan {
        budget_bytes: budget,
        pyramid: !a.no_pyramid && a.memory_budget_mb.is_none(),
        card: a.card,
        card_costs: !a.no_card_costs,
    };
    let measured = Measured {
        statements: est_statements,
        est_term_bytes,
        input_scale: scale,
        parse_all_secs: parse_all,
        sample,
    };
    let p = project(&measured, &plan);

    if a.json {
        let j = serde_json::json!({
            "input_bytes": total_input,
            "read_bytes": read_bytes,
            "sampled": sampled,
            "statements": est_statements,
            "distinct_terms": est_terms,
            "distinct_term_bytes": est_term_bytes,
            "sample": {
                "statements": measured.sample.statements,
                "terms": measured.sample.terms,
                "build_secs": measured.sample.build_secs,
                "dictionary": measured.sample.dict,
                "index": measured.sample.index,
                "labels": measured.sample.labels,
                "text": measured.sample.text,
                "other": measured.sample.other,
            },
            "projected": {
                "dictionary": p.dict, "index": p.index, "labels": p.labels,
                "text": p.text, "other": p.other, "card": p.card, "pyramid": p.pyramid,
                "size": [p.size.0, p.size.1],
                "build_secs": [p.build_secs.0, p.build_secs.1],
                "starter_query_secs": [p.starter_secs.0, p.starter_secs.1],
                "spill": [p.spill.0, p.spill.1],
                "spill_chunks": p.chunks,
                "ram": [p.ram.0, p.ram.1],
                "parse_secs": p.parse_all_secs,
            },
        });
        println!("{}", serde_json::to_string_pretty(&j)?);
        return Ok(());
    }

    println!("rete estimate — projected build cost (nothing was written)\n");
    println!("input");
    println!("  files                {}", a.inputs.len());
    println!("  total size           {}", human(total_input as f64));
    if sampled {
        println!(
            "  READ (sample)        {}  ({:.2}% — figures below are extrapolated)",
            human(read_bytes as f64),
            100.0 * read_bytes as f64 / total_input as f64
        );
    } else {
        println!(
            "  read                 {} (complete — counts are exact)",
            human(read_bytes as f64)
        );
    }
    println!("\nwhat it contains");
    println!(
        "  statements           {:.0}{}",
        est_statements,
        if sampled { " (est.)" } else { "" }
    );
    println!(
        "  distinct terms       {:.0}{}",
        est_terms,
        if sampled {
            " (est., HLL)"
        } else {
            " (HLL ±0.8%)"
        }
    );
    println!(
        "  literal objects      {:.1}%",
        100.0 * literals as f64 / statements.max(1) as f64
    );
    println!("  avg term length      {avg_uniq_term:.1} B");
    if !graphs.is_empty() {
        println!(
            "  named graphs         {}{}",
            graphs.len(),
            if sampled { "+ (in sample)" } else { "" }
        );
    }
    let s = &measured.sample;
    println!(
        "\nmeasured by building a sample of {} statements ({} terms) in memory, {:.1}s",
        s.statements, s.terms, s.build_secs
    );
    println!("projected output");
    println!("  dictionary           ~{}", human(p.dict));
    println!("  index (6 perms)      ~{}", human(p.index));
    if a.label_index {
        println!("  label index          ~{}", human(p.labels));
    }
    if a.text_index {
        println!("  text index           ~{}", human(p.text));
    }
    println!("  directories, schema  ~{}", human(p.other));
    if p.card > 0.0 {
        println!("  card and record      ~{}", human(p.card));
    }
    if p.pyramid > 0.0 {
        println!(
            "  pyramid              ~{}  (--no-pyramid drops it)",
            human(p.pyramid)
        );
    }
    println!(
        "  TOTAL .rete          {} – {}   (~{:.1} bytes/triple)",
        human(p.size.0),
        human(p.size.1),
        (p.size.0 + p.size.1) / 2.0 / est_statements.max(1.0)
    );
    println!("\nprojected build");
    println!("  parse-only pass      {}", hms(p.parse_all_secs));
    println!(
        "  full build           {} – {}   (parse + sort/spill/merge + index + compress{})",
        hms(p.build_secs.0 + p.starter_secs.0),
        hms(p.build_secs.1 + p.starter_secs.1),
        if p.starter_secs.1 > 0.0 {
            format!(
                "; {} – {} of it the starter-query costs",
                hms(p.starter_secs.0),
                hms(p.starter_secs.1)
            )
        } else {
            String::new()
        }
    );
    println!("  at --memory-budget-mb {}", budget as u64 >> 20);
    println!("    spill chunks       ~{:.0}", p.chunks);
    println!(
        "    peak spill on disk {} – {}   (put --tmp-dir on a fast NATIVE disk)",
        human(p.spill.0),
        human(p.spill.1)
    );
    println!(
        "    peak RAM           {} – {} (the budget bounds it; the spill is what needs room)",
        human(p.ram.0),
        human(p.ram.1)
    );
    if p.size.1 > 0.0 {
        println!(
            "\nplan: {} of free space for the output plus {} for the spill.",
            human(p.size.1 * 1.1),
            human(p.spill.1 * 1.1)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hll_estimates_within_a_few_percent() {
        let mut h = Hll::new();
        for i in 0..200_000u32 {
            h.add(&format!("<http://example.org/resource/{i}>"));
        }
        let est = h.count() as f64;
        let err = (est - 200_000.0).abs() / 200_000.0;
        assert!(err < 0.05, "HLL off by {:.1}% (est {est})", err * 100.0);
    }

    fn quad(i: usize) -> RawQuad {
        (format!("<s{i}>"), "<p>".into(), format!("\"o{i}\""), None)
    }

    fn index_of(q: &RawQuad) -> usize {
        q.0[2..q.0.len() - 1].parse().unwrap()
    }

    #[test]
    fn a_small_input_is_kept_whole_and_in_order() {
        let mut w = Windows::with_window(1000, 100);
        for i in 0..250 {
            let q = quad(i);
            w.push(&q.0, &q.1, &q.2, &q.3);
        }
        let kept = w.finish();
        assert_eq!(kept.len(), 250);
        assert!(kept.iter().enumerate().all(|(i, q)| index_of(q) == i));
    }

    #[test]
    fn a_big_input_is_sampled_from_all_of_it() {
        // 10,000 windows of 10 statements, 100 kept.
        let mut w = Windows::with_window(1000, 10);
        for i in 0..100_000 {
            let q = quad(i);
            w.push(&q.0, &q.1, &q.2, &q.3);
        }
        let idx: Vec<usize> = w.finish().iter().map(index_of).collect();
        assert_eq!(idx.len(), 1000);
        assert!(idx.windows(2).all(|p| p[0] < p[1]), "in input order");
        for fifth in 0..5 {
            assert!(
                idx.iter().any(|&i| i / 20_000 == fifth),
                "nothing kept from fifth {fifth}: {idx:?}"
            );
        }
        // Contiguous: every window is 10 statements in a row.
        assert!(idx.chunks(10).all(|c| c[9] - c[0] == 9));
    }

    #[test]
    fn distinct_sample_counts_terms_and_bytes() {
        let mut d = DistinctSample::new(5_000);
        for round in 0..3u32 {
            for i in 0..200_000u32 {
                // Each term three times, in a different order each round.
                d.add(&format!(
                    "<http://example.org/r/{:06}>",
                    (i * 7 + round) % 200_000
                ));
            }
        }
        let (n, bytes) = d.totals();
        assert!((n - 200_000.0).abs() / 200_000.0 < 0.06, "terms {n}");
        let exact = 200_000.0 * "<http://example.org/r/000000>".len() as f64;
        assert!(
            (bytes - exact).abs() / exact < 0.06,
            "bytes {bytes} vs {exact}"
        );
    }

    #[test]
    fn projection_scales_each_section_by_what_drives_it() {
        let sample = Sample {
            statements: 1000,
            terms: 400,
            term_bytes: 10_000.0,
            dict: 2_000.0,
            index: 8_000.0,
            labels: 500.0,
            text: 1_000.0,
            other: 100.0,
            build_secs: 0.5,
        };
        let m = Measured {
            statements: 10_000.0,
            est_term_bytes: 50_000.0,
            input_scale: 1.0,
            parse_all_secs: 2.0,
            sample,
        };
        let plan = Plan {
            budget_bytes: 512.0 * 1048576.0,
            pyramid: false,
            card: true,
            card_costs: true,
        };
        let p = project(&m, &plan);
        // The dictionary by the bytes of its terms (5x), the rest by statements (10x).
        assert_eq!(p.dict, 10_000.0);
        assert_eq!(p.index, 80_000.0);
        assert_eq!(p.labels, 5_000.0);
        assert_eq!(p.text, 10_000.0);
        assert!(p.size.0 < p.size.1 && p.starter_secs.1 > 0.0);
        // The spill carries the text term, and the time is K x (the parse plus the
        // sample's build, scaled to every statement).
        let spill = SPILL_PER_CORE_BYTE * (10_000.0 + 80_000.0 + 5_000.0 + 1_000.0)
            + SPILL_PER_TEXT_BYTE * 10_000.0;
        assert!((p.spill.0 / (1.0 - SPILL_BAND) - spill).abs() < 1e-6);
        assert!((p.build_secs.0 / (1.0 - TIME_BAND) - K_BUILD * (2.0 + 5.0)).abs() < 1e-9);
    }

    #[test]
    fn hll_small_cardinality_uses_linear_counting() {
        let mut h = Hll::new();
        for i in 0..50u32 {
            h.add(&format!("t{i}"));
        }
        assert!(
            (h.count() as i64 - 50).abs() <= 2,
            "small-range est {}",
            h.count()
        );
    }
}
