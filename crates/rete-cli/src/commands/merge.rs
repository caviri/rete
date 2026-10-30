//! `rete merge` — fold several `.rete` files into one, without going back
//! through text.
//!
//! # Why this exists
//!
//! A sharded dataset is built by splitting the source and running one build per
//! shard. Consolidating it into a single file used to mean rebuilding from the
//! original input: re-running whatever produced the RDF (for OpenAIRE, ~110 GB of
//! Parquet through a converter), re-parsing every line, and re-deriving the
//! dictionary from raw strings. That is the whole build again, and the source is
//! not always still on disk.
//!
//! Merging reads the SHARDS instead — dictionary-encoded and compressed, so
//! roughly a quarter of the bytes for OpenAIRE — and skips conversion and RDF
//! parsing outright.
//!
//! # What it does NOT save
//!
//! Not the sorting. It would be nice to claim the shards are already sorted and
//! a k-way merge yields sorted output for free, but that is false for this
//! format: the dictionary is HDT-style, with `shared` (a term used as BOTH
//! subject and object), `subject-only` and `object-only` sections, and IDs are
//! assigned per section. A term that is subject-only in shard A and object-only
//! in shard B becomes SHARED in the merge — it changes section, so its ID moves
//! non-monotonically against its neighbours and their relative order can invert.
//! Only a merge that recomputed the shared set up front and then re-derived every
//! permutation could exploit ordering, and it would still have to sort five of
//! the six.
//!
//! So the honest description is: this skips the conversion and the parse, and
//! feeds the SAME memory-bounded external builder the raw path uses. The sorting
//! is unchanged, and it spills to disk exactly as it would have.
//!
//! # Memory
//!
//! Bounded on both ends. Each input is opened LAZILY and walked with
//! `Rete::dump_batch`, whose entire resume state is one subject id, so an input
//! is never resident; the builder chunks and spills under `--memory-budget-mb`.
//! Merging files far larger than RAM is the point.
//!
//! # The card
//!
//! With a card, the merged file gets the **full** one, exactly as
//! `rete build --memory-budget-mb` writes it: derived from the merged file's
//! own sorted sections once they are on disk, its starter queries run against
//! it, the card and the build record appended after the data
//! ([`crate::commands::build::append_full_card`]). The inputs' own cards are not
//! read into it: a shard's title, description or coherence verdict describes
//! the shard, so the curated half comes from `--card-file` and the curated
//! flags alone, and the build record names the shards it folded (file name,
//! size, BLAKE3).
use rete_core::ingest::RawQuad;
use rete_core::Rete;
use std::path::Path;

use crate::commands::buildinfo;
use crate::commands::card::{self, CardArgs};

/// Quads per `dump_batch` call. Same reasoning as the wasm cursor's batch: big
/// enough that the per-call overhead disappears, small enough that the transient
/// buffer stays trivial next to the builder's own budget.
const BATCH: usize = 10_000;

/// Push every quad of one opened `.rete` into `visit`, graph by graph.
///
/// `dump_batch` cuts batches on a subject boundary and reports `done`, so this
/// resumes with a `u32` rather than holding a scan open across calls.
fn stream_file<E>(
    rete: &Rete,
    label: &str,
    visit: &mut dyn FnMut(RawQuad) -> Result<(), E>,
) -> Result<u64, E> {
    // `None` = the default graph, then each named graph in turn — the same slot
    // walk the language clients do.
    let mut slots: Vec<Option<String>> = vec![None];
    slots.extend(rete.graph_names().iter().map(|g| Some((*g).to_string())));

    let mut total: u64 = 0;
    for slot in slots {
        let mut cursor = 0u32;
        loop {
            let (triples, next, done) = rete.dump_batch(slot.as_deref(), cursor, BATCH);
            for (s, p, o) in triples {
                visit((s, p, o, slot.clone()))?;
                total += 1;
            }
            cursor = next;
            if done {
                break;
            }
        }
    }
    eprintln!("merge: {label} contributed {total} quad(s)");
    Ok(total)
}

/// Hash one input for the build record: its size and the BLAKE3 of its bytes
/// as stored, in one sequential read (the walk reads it by range, in index
/// order, so it cannot be hashed on the way).
fn hash_input(path: &str, tap: &buildinfo::InputTap) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let file = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, file);
    let mut hasher = tap.hasher.borrow_mut();
    let n = std::io::copy(&mut reader, &mut *hasher).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    tap.bytes.set(n);
    eprintln!(
        "merge: hashed {path} ({:.1} MB) in {:.1} s",
        n as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// The names of the inputs whose own card carries curated fields (title,
/// licence, description, …) — the fields a merge does not carry over.
fn inputs_with_curated_cards(inputs: &[String]) -> Vec<String> {
    inputs
        .iter()
        .filter(|path| {
            crate::commands::range_source::LocalRangeReader::open(path)
                .ok()
                .and_then(|r| card::load_card_and_build_ranged(&r).ok())
                .and_then(|read| read.card)
                .is_some_and(|c| !card::CardInput::from_card(&c).is_empty())
        })
        .map(|p| buildinfo::file_name(p))
        .collect()
}

/// `rete merge a.rete b.rete … -o out.rete`
pub(crate) fn merge_cmd(
    inputs: &[String],
    output: &str,
    memory_budget_mb: u64,
    tmp_dir: Option<&str>,
    card_args: CardArgs,
    measure_costs: bool,
) -> anyhow::Result<()> {
    if inputs.is_empty() {
        anyhow::bail!("merge needs at least one input .rete file");
    }
    for i in inputs {
        if i == "-" {
            anyhow::bail!(
                "merge reads .rete files, not stdin — a .rete is read by range, not streamed"
            );
        }
        if !Path::new(i).exists() {
            anyhow::bail!("no such file: {i}");
        }
    }
    if inputs.iter().any(|i| i == output) {
        anyhow::bail!("refusing to overwrite an input ({output} is also an input)");
    }
    // Build conditions (kind-7 section, outside the content hash), with a card
    // only: the flags in force, the inputs by name, size and BLAKE3, then —
    // once the merged file is on disk — the counts, the spill, the card's
    // costs and every phase's time. Timed from here.
    let tapped: Vec<(&str, &'static str)> = inputs.iter().map(|i| (i.as_str(), "rete")).collect();
    let rec = buildinfo::Recorder::start(if card_args.requested() {
        &tapped[..]
    } else {
        &[]
    });

    // The merged file carries the UNION of its inputs' permutation sets: an
    // all-lean merge stays lean, and one full input is enough to keep the
    // merge-join orders that input's queries relied on. `merge` has no
    // `--permutations` of its own — it consolidates shards, it does not
    // re-decide how they were built.
    // The named-graph layout follows the same rule: one quad-index input makes
    // the merge a quad-index file (its readers are new enough by construction).
    let mut perms_bits = rete_core::PermSet::CORE.bits();
    // Likewise the label index: kept when any input carries one. The merged
    // file always carries a SCHEMA section, as `rete build` writes. And the
    // named-graph layout: a quad-index input makes a quad-index merge.
    let mut label_index = false;
    let mut graphs = rete_core::GraphLayout::PerGraph;
    for path in inputs {
        let rete = crate::commands::range_source::open_local(path)?;
        perms_bits |= rete.header().perms.bits();
        label_index |= rete.has_label_index();
        if rete.header().has_quad_index() {
            graphs = rete_core::GraphLayout::Quad;
        }
    }

    // The inputs' own cards are not carried (see the module docs): say so when
    // one of them has curated fields that the merged file will not.
    let curated_inputs = inputs_with_curated_cards(inputs);
    if !curated_inputs.is_empty() {
        let what = if card_args.requested() {
            "the merged card's curated fields come from --card-file and the curated flags only"
        } else {
            "the merged file has no card unless --card-file (or --card) gives it one"
        };
        eprintln!(
            "merge: the cards of {} carry curated fields; a merge does not carry an input's \
             card: {what}",
            curated_inputs.join(", ")
        );
    }

    let perms = rete_core::PermSet::from_bits(perms_bits).map_err(|e| anyhow::anyhow!("{e}"))?;
    let curated = if card_args.requested() {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    let flags = crate::commands::build::RecordFlags {
        command: "merge",
        format: None,
        no_pyramid: true,
        pyramid_algo: rete_core::PyramidAlgo::Louvain,
        text_index: None,
        materialize: false,
        reason: false,
        sections: crate::commands::build::BuildSections {
            label_index,
            schema: true,
        },
        perms,
        graphs,
        collapse_graphs: false,
        strict: false,
        surface: Default::default(),
        type_predicate: None,
        measure_costs,
        memory_budget_mb: Some(memory_budget_mb),
        card_args: &card_args,
    };
    let mut info = curated.is_some().then(|| {
        let mut info = buildinfo::new_build_info(flags.params(None));
        rec.begin(&mut info);
        info
    });
    for (path, tap) in inputs
        .iter()
        .zip(&rec.taps)
        .take(buildinfo::MAX_INPUTS_LISTED)
    {
        hash_input(path, tap)?;
    }
    if !rec.taps.is_empty() {
        rec.lap(rete_core::build_trace::Phase::Read);
    }

    eprintln!("merge: {} input file(s) -> {output}", inputs.len());
    if perms != rete_core::PermSet::ALL {
        eprintln!(
            "merge: every input is {}-permutation; the merged file will be too",
            perms.len()
        );
    }
    let out_path = Path::new(output).to_path_buf();
    let read = rec.statements_read.clone();
    let stats = rete_core::extbuild::build_external(
        |visit| {
            let mut counted = |q: RawQuad| {
                read.set(read.get() + 1);
                visit(q)
            };
            for path in inputs {
                // `open_local` is the canonical local opener: it reads small files
                // whole and puts large ones behind a block-cached range reader, so
                // a multi-gigabyte shard never becomes resident.
                let rete = crate::commands::range_source::open_local(path).map_err(|e| {
                    rete_core::extbuild::ExtBuildError::Ingest(rete_core::ingest::IngestError::Io(
                        format!("{path}: {e}"),
                    ))
                })?;
                stream_file(&rete, path, &mut counted)?;
                // A shard that lost bytes to a failed read would silently shrink
                // the merged graph, which is worse than failing.
                if rete.index_incomplete() {
                    return Err(rete_core::extbuild::ExtBuildError::Ingest(
                        rete_core::ingest::IngestError::Io(format!(
                            "{path}: reads failed during the walk — the merge would be short"
                        )),
                    ));
                }
            }
            Ok(())
        },
        &out_path,
        rete_core::extbuild::ExternalBuildOptions {
            memory_budget: memory_budget_mb.saturating_mul(1 << 20),
            tmp_dir: tmp_dir.map(std::path::PathBuf::from),
            // The card is derived from the merged file and appended after it
            // (below), so the stream writes neither a card nor a record.
            metadata: Box::new(|_| Vec::new()),
            build_info: Vec::new(),
            perms,
            label_index,
            schema: true,
            type_override: None,
            text_index: None,
            graphs,
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let (Some(info), Some(curated)) = (info.as_mut(), curated) {
        crate::commands::build::append_full_card(
            output,
            info,
            curated,
            measure_costs,
            memory_budget_mb,
            &stats,
            &rete_core::IriReport::default(),
            &rec,
            "merge",
        )?;
    }

    let size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "merge: wrote {output}: {} statement(s), {} term(s), {:.2} MB",
        stats.statements,
        stats.terms,
        size as f64 / 1e6
    );
    Ok(())
}
