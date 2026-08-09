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
use rete_core::ingest::RawQuad;
use rete_core::Rete;
use std::path::Path;

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

/// `rete merge a.rete b.rete … -o out.rete`
#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_cmd(
    inputs: &[String],
    output: &str,
    memory_budget_mb: u64,
    tmp_dir: Option<&str>,
    card_args: CardArgs,
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

    // The external builder is DEFAULT-GRAPH ONLY, and it discovers that when the
    // first named quad reaches it — which on a multi-gigabyte shard is hours in.
    // Every input's graph list is in its header, so check all of them up front
    // and fail in a second instead.
    // The merged file carries the UNION of its inputs' permutation sets: an
    // all-lean merge stays lean, and one full input is enough to keep the
    // merge-join orders that input's queries relied on. `merge` has no
    // `--permutations` of its own — it consolidates shards, it does not
    // re-decide how they were built.
    let mut perms_bits = rete_core::PermSet::CORE.bits();
    for path in inputs {
        let rete = crate::commands::range_source::open_local(path)?;
        perms_bits |= rete.header().perms.bits();
        let names = rete.graph_names();
        if !names.is_empty() {
            anyhow::bail!(
                "{path} carries {} named graph(s) (e.g. {}), and the memory-bounded \
                 builder merge uses handles the default graph only. Merge the \
                 default-graph shards, or export to .nq and use the standard build.",
                names.len(),
                names[0]
            );
        }
    }

    let curated = if card_args.requested() {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    // Build conditions (kind-7 section, outside the content hash). A merged
    // file names its shards in the card's curated `derived_from`; the build
    // info records when/by what/under which budget the fold happened.
    let build_info = if curated.is_some() {
        crate::commands::buildinfo::new_build_info(crate::commands::buildinfo::BuildParams {
            command: Some("merge".to_string()),
            no_pyramid: true,
            memory_budget_mb: Some(memory_budget_mb),
            ..Default::default()
        })
        .to_json_bytes()
    } else {
        Vec::new()
    };

    let perms = rete_core::PermSet::from_bits(perms_bits).map_err(|e| anyhow::anyhow!("{e}"))?;
    eprintln!("merge: {} input file(s) -> {output}", inputs.len());
    if perms != rete_core::PermSet::ALL {
        eprintln!(
            "merge: every input is {}-permutation; the merged file will be too",
            perms.len()
        );
    }
    let out_path = Path::new(output).to_path_buf();
    let stats = rete_core::extbuild::build_external(
        |visit| {
            for path in inputs {
                // `open_local` is the canonical local opener: it reads small files
                // whole and puts large ones behind a block-cached range reader, so
                // a multi-gigabyte shard never becomes resident.
                let rete = crate::commands::range_source::open_local(path).map_err(|e| {
                    rete_core::extbuild::ExtBuildError::Ingest(rete_core::ingest::IngestError::Io(
                        format!("{path}: {e}"),
                    ))
                })?;
                stream_file(&rete, path, visit)?;
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
            metadata: Box::new(move |stats| match curated {
                Some(curated) => {
                    let blob = card::curated_counts_card(
                        stats.statements as u64,
                        stats.terms as u64,
                        curated,
                    )
                    .to_json_bytes();
                    eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
                    blob
                }
                None => Vec::new(),
            }),
            build_info,
            perms,
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "merge: wrote {output}: {} statement(s), {} term(s), {:.2} MB",
        stats.statements,
        stats.terms,
        size as f64 / 1e6
    );
    Ok(())
}
