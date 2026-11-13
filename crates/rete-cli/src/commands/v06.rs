//! Formats `0x06` to `0x08` in the CLI: what the in-RAM writers finish with, and
//! `rete convert` (a `0x05`, `0x06` or `0x07` file to `0x08`: a re-encode through
//! the memory-bounded builder, because the dictionary order changed). The
//! memory-bounded writers (`build --memory-budget-mb`, `merge`) do not come
//! through the in-RAM finish: they write the file themselves, once, from their
//! spill.

use rete_core::v06::write::{WriteOpts, MIN_FILE_LEN};

/// `RETE_WRITE_FORMAT=5`: the in-RAM build writes the legacy `0x05` layout —
/// a test hook (the byte-identity test of `convert`), not a user option. One
/// switch for every front end: it lives in `rete_core::build_record`.
pub(crate) fn writes_legacy() -> bool {
    rete_core::build_record::writes_legacy()
}

/// Turn a finished in-memory `0x05` image into the `0x08` file the writer
/// emits (unless [`writes_legacy`]).
pub(crate) fn finish(image: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    finish_with(image, Some(rete_core::derived::DeriveOpts::default()))
}

/// [`finish`] with derived-IRI detection on (`Some`) or off.
pub(crate) fn finish_with(
    image: Vec<u8>,
    derive: Option<rete_core::derived::DeriveOpts>,
) -> anyhow::Result<Vec<u8>> {
    rete_core::build_record::finish_in_ram_with(
        image,
        None,
        WriteOpts::default().with_derive(derive),
    )
    .map_err(|e| anyhow::anyhow!("writing 0x08: {e}"))
}

/// Is the file at `path` a `0x06` file? (Reads its first bytes.)
pub(crate) fn file_is_v06(path: &std::path::Path) -> anyhow::Result<bool> {
    use std::io::Read;
    let mut head = [0u8; 8];
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut head)?;
    Ok(rete_core::v06::open::is_v06(&head[..n]))
}

/// Fill a `0x06` file's reserved build record in place (it is outside the
/// content hash); `false` when the record does not fit its reservation.
pub(crate) fn patch_record_in_place(path: &std::path::Path, info: &[u8]) -> anyhow::Result<bool> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut head = vec![0u8; MIN_FILE_LEN as usize];
    f.read_exact(&mut head)?;
    let h = rete_core::v06::Header06::from_bytes(&head)?;
    let r = h.build_range();
    if r.len == 0 {
        return Ok(false);
    }
    // What the writer's derived-IRI detection said stays in the record: it
    // belongs to the file, not to the measurements this patch replaces.
    let mut old = vec![0u8; r.len as usize];
    f.seek(SeekFrom::Start(r.offset))?;
    f.read_exact(&mut old)?;
    let carried = rete_core::build_record::derive_report_text(&old)
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|report| rete_core::build_record::with_derive_report(info, &report));
    let info = carried.as_deref().unwrap_or(info);
    if info.len() as u64 > r.len {
        return Ok(false);
    }
    let mut buf = info.to_vec();
    buf.resize(r.len as usize, b' ');
    f.seek(SeekFrom::Start(r.offset))?;
    f.write_all(&buf)?;
    f.sync_all()?;
    Ok(true)
}

/// What `rete convert` does about the card's starter-query costs.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CardCosts {
    /// `--no-card-costs`: leave the costs unmeasured.
    pub skip: bool,
    /// `--memory-budget-mb`: the guard the measurement runs under.
    pub memory_budget_mb: Option<u64>,
}

/// `rete convert IN -o OUT`: a `0x05`, `0x06` or `0x07` file to `0x08`.
///
/// `0x08` keeps its dictionary in the SPARQL order (SPEC.md section 5), so a
/// term gets a new id and every structure that holds ids is written again:
/// this is a re-encode, not a copy of the payload. The statements are walked
/// through the source's own readers and fed to the memory-bounded builder
/// (`rete merge` of one file, `rete_core::extbuild`), which sorts the terms,
/// remaps and sorts every index, and writes the file; the dictionary, the
/// four quad orders, the label and the text index, the schema and the class
/// clusters are all rebuilt for the new ids, and the card (with its annex)
/// and the build record are kept. The peak memory is the builder's:
/// `--memory-budget-mb` (default 2048), the spill goes to `--tmp-dir`
/// (default: beside the output).
///
/// A source's community pyramid is carried over, not computed again: its
/// summary and descriptors are written as they are, the superedges under the
/// predicate ids of the new dictionary. Per-graph named graphs (kind 5) are
/// walked graph by graph and written in the quad index. A `0x08` input is
/// written again under the current directory-leaf policy, payload copied.
pub(crate) fn convert(
    input: &str,
    output: &str,
    quad: Option<&str>,
    tmp_dir: Option<&str>,
    label_index: bool,
    costs: CardCosts,
    derive: Option<rete_core::derived::DeriveOpts>,
) -> anyhow::Result<()> {
    use rete_core::v06::write::QuadClustering;
    // The sorts that derive the quad orders spill beside the output unless
    // told otherwise (a big file's spill is bytes per statement and order).
    let spill_dir = match tmp_dir {
        Some(d) => std::path::PathBuf::from(d),
        None => std::path::Path::new(output)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(
                || std::path::PathBuf::from("."),
                std::path::Path::to_path_buf,
            ),
    };
    if std::path::Path::new(input) == std::path::Path::new(output) {
        anyhow::bail!("convert writes a new file: OUT must differ from IN");
    }
    let mut opts = WriteOpts::default()
        .with_spill_dir(Some(spill_dir.clone()))
        .with_derive(derive);
    match quad {
        Some("graph") => opts.quad = QuadClustering::GraphMajor,
        Some("order") => opts.quad = QuadClustering::OrderMajor,
        Some("auto") | None => {}
        Some(x) => anyhow::bail!("--quad-clustering must be auto, graph or order (got {x})"),
    }
    let reader = super::range_source::RangedSourceReader::open(input)?;
    let head =
        rete_core::RangeReader::read_at(&reader, 0, 8.min(rete_core::RangeReader::len(&reader)))?;
    if rete_core::v06::open::is_v06(&head) && head.get(4) == Some(&rete_core::v06::VERSION) {
        return rewrite_v06(input, output, &reader, opts);
    }
    let started = std::time::Instant::now();
    // The record says what happened: the source's own record, as it was, with
    // this conversion appended (or just the conversion, for a file without one).
    let in_len = rete_core::RangeReader::len(&reader);
    let mut conversion = conversion_entry(input, &reader, quad, label_index, derive)?;
    let source_record = rete_core::file::read_card_and_build_info_ranged(&reader)
        .map(|(_, record)| record)
        .map_err(|e| anyhow::anyhow!("reading {input}: {e}"))?;
    // The card's queries are copied verbatim, so the source's card says which
    // ones to measure. Their costs depend on the new layout, so they are measured
    // on the file once written and sealed into a record reserved at its widest.
    let starters = if costs.skip {
        None
    } else {
        super::card::load_card_ranged(&reader)?.filter(|c| !c.queries.is_empty())
    };
    if let Some(card) = &starters {
        conversion.query_costs = Some(super::buildinfo::widest_conversion_costs(&card.queries));
    }
    let measured = starters.is_some();
    let record = super::buildinfo::with_conversion(source_record.as_deref(), &conversion)?;
    let budget_mb = costs.memory_budget_mb.unwrap_or(2048);
    let share = super::merge::reader_share(budget_mb.saturating_mul(1 << 20));
    let probe = if super::range_source::is_url(input) {
        rete_core::Rete::open_ranged_lazy(super::range_source::RangedSourceReader::open(input)?)?
    } else {
        super::merge::open_input(input, share)?
    };
    let tmp = std::path::PathBuf::from(format!("{output}.convert-tmp"));
    let stats = match convert_bounded(
        input,
        &tmp,
        probe,
        &reader,
        &spill_dir,
        budget_mb,
        &record,
        derive,
        label_index,
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if let Err(e) = seal_costs(
        &tmp.to_string_lossy(),
        measured,
        source_record.as_deref(),
        conversion,
        costs,
    ) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, output)?;
    let out_len = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "convert: wrote {output} — format 0x08, {out_len} bytes ({:+.2}% vs {in_len} bytes), \
         {} statement(s), {} term(s), {:.1}s",
        (out_len as f64 - in_len as f64) * 100.0 / in_len.max(1) as f64,
        stats.statements,
        stats.terms,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// The bounded re-encode: walk every statement of `probe` into the
/// memory-bounded builder, with the features the source has.
#[allow(clippy::too_many_arguments)]
fn convert_bounded(
    input: &str,
    out: &std::path::Path,
    probe: rete_core::Rete,
    reader: &super::range_source::RangedSourceReader,
    spill_dir: &std::path::Path,
    budget_mb: u64,
    record: &[u8],
    derive: Option<rete_core::derived::DeriveOpts>,
    legacy_labels_to_index: bool,
) -> anyhow::Result<rete_core::ingest::BuildStats> {
    use rete_core::extbuild::{build_external, ExtBuildError, ExternalBuildOptions};
    let budget = budget_mb.saturating_mul(1 << 20);
    let share = super::merge::reader_share(budget);
    let header = probe.header().clone();
    let pyramid = carried_pyramid(input, &probe)?;
    // A file that only has the pyramid's legacy label block gets the label
    // index (unless `--no-label-index`): the block is not carried.
    let legacy_labels = probe.pyramid().is_some_and(|p| !p.label_index.is_empty());
    let label_index = probe.has_label_index() || (legacy_labels_to_index && legacy_labels);
    let text_index = probe.text_index_scope().filter(|_| probe.has_text_index());
    let card = rete_core::file::read_card_full_ranged(reader)
        .map_err(|e| anyhow::anyhow!("reading the card of {input}: {e}"))?
        .unwrap_or_default();
    drop(probe);
    eprintln!(
        "convert: {input} (format 0x{:02x}) -> format 0x08: re-encoding {} statement(s) through \
         the memory-bounded builder (budget {budget_mb} MiB){}{}{}{}",
        header.version,
        header.quad_count,
        match &pyramid {
            Some(p) => format!(
                ", pyramid carried over ({} superedge(s), round {})",
                p.summary.len(),
                p.round
            ),
            None => String::new(),
        },
        if header.named_graphs_len > 0 {
            ", per-graph named graphs (kind 5) into the quad index"
        } else {
            ""
        },
        if label_index { ", label index" } else { "" },
        if text_index.is_some() {
            ", text index"
        } else {
            ""
        },
    );
    build_external(
        |visit| {
            let mut rete = super::merge::open_input(input, share).map_err(|e| {
                ExtBuildError::Ingest(rete_core::ingest::IngestError::Io(format!("{input}: {e}")))
            })?;
            super::merge::stream_file(&mut rete, input, share, visit)?;
            if rete.index_incomplete() {
                return Err(ExtBuildError::Ingest(rete_core::ingest::IngestError::Io(
                    format!("{input}: reads failed during the walk: the conversion would be short"),
                )));
            }
            Ok(())
        },
        out,
        ExternalBuildOptions {
            memory_budget: budget,
            tmp_dir: Some(spill_dir.to_path_buf()),
            metadata: Box::new(move |_| card),
            build_info: record.to_vec(),
            perms: header.perms,
            label_index,
            schema: true,
            type_override: None,
            text_index,
            graphs: rete_core::GraphLayout::Quad,
            input_memory: share,
            card_from_image: None,
            derive,
            pyramid,
        },
    )
    .map_err(|e| anyhow::anyhow!("converting {input}: {e}"))
}

/// The source's community pyramid, to be written as it is: the summary with
/// the term of each predicate it uses, and the descriptors.
fn carried_pyramid(
    input: &str,
    probe: &rete_core::Rete,
) -> anyhow::Result<Option<rete_core::extbuild::CarriedPyramid>> {
    if probe.header().pyramid_meta_len == 0 {
        return Ok(None);
    }
    let Some(meta) = probe.pyramid() else {
        anyhow::bail!("{input}: the pyramid section cannot be read");
    };
    let mut ids: Vec<u32> = meta.summary.iter().map(|e| e.predicate).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut predicates = Vec::with_capacity(ids.len());
    for id in ids {
        let term = probe.dictionary().predicate_term(id).ok_or_else(|| {
            anyhow::anyhow!("{input}: the pyramid uses predicate id {id}, which is not a term")
        })?;
        predicates.push((id, term));
    }
    Ok(Some(rete_core::extbuild::CarriedPyramid {
        levels: probe.header().pyramid_levels,
        round: meta.round,
        summary: meta.summary.clone(),
        predicates,
        descriptors: meta.descriptors.clone(),
    }))
}

/// Measure the starter queries on the converted file at `path` and patch the
/// record, now with the measured costs in the conversion entry, into the
/// reservation the widest figures made. Nothing else in the file changes.
fn seal_costs(
    path: &str,
    measured: bool,
    source_record: Option<&[u8]>,
    mut conversion: super::buildinfo::Conversion,
    costs: CardCosts,
) -> anyhow::Result<()> {
    if !measured {
        return Ok(());
    }
    conversion.query_costs =
        super::buildinfo::measure_conversion_costs(path, costs.memory_budget_mb)?;
    let record = super::buildinfo::with_conversion(source_record, &conversion)?;
    super::buildinfo::patch_build_info(std::path::Path::new(path), &record)
}

/// This conversion, for the build record: the engine, the time, the command
/// with the flags that shaped the result, and the file it started from.
fn conversion_entry(
    input: &str,
    reader: &super::range_source::RangedSourceReader,
    quad: Option<&str>,
    label_index: bool,
    derive: Option<rete_core::derived::DeriveOpts>,
) -> anyhow::Result<super::buildinfo::Conversion> {
    use super::buildinfo as bi;
    let len = rete_core::RangeReader::len(reader);
    let head = rete_core::RangeReader::read_at(
        reader,
        0,
        (rete_core::format::HEADER_LEN as u64).min(len),
    )?;
    let h = rete_core::format::Header::from_bytes(&head)?;
    let mut command = String::from("rete convert");
    if let Some(q) = quad {
        command.push_str(&format!(" --quad-clustering {q}"));
    }
    if !label_index {
        command.push_str(" --no-label-index");
    }
    if rete_core::write_opts::cluster().is_some() {
        command.push_str(" --cluster-subjects-by-class");
    }
    if derive.is_none() {
        command.push_str(" --no-derive-iris");
    }
    Ok(bi::Conversion {
        at: bi::timestamp(),
        engine: Some(bi::engine()),
        command,
        source_format: format!("0x{:02x}", h.version),
        source_content_hash: hex16(&h.content_hash),
        source_bytes: len,
        source_name: Some(bi::file_name(input)),
        query_costs: None,
    })
}

fn hex16(b: &[u8; 16]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Give the `0x06` file at `path` a new card (or keep its own) and build
/// record. In place when the pair fits a free gap of the card region: the pair
/// is written beside the old one, the file synced, then the 128-byte header —
/// the one commit point — written and synced; nothing else moves. Otherwise
/// the file is copied beside itself with a fresh card region (the data region
/// byte for byte) and renamed over it. Returns the new header and whether the
/// file was copied.
pub(crate) fn recard_file(
    path: &str,
    card: rete_core::v06::write::NewCard<'_>,
    record: Option<&[u8]>,
) -> anyhow::Result<(rete_core::v06::Header06, bool)> {
    use rete_core::v06::write::{commit_header, recard_in_place};
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("opening {path} for writing: {e}"))?;
    if let Some(h) =
        recard_in_place(&mut f, card, record).map_err(|e| anyhow::anyhow!("{path}: {e}"))?
    {
        f.sync_data()?;
        commit_header(&mut f, &h).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
        f.sync_all()?;
        return Ok((h, false));
    }
    drop(f);
    let tmp = std::path::PathBuf::from(format!("{path}.recard-tmp"));
    let h = {
        let reader = super::range_source::LocalRangeReader::open(path)?;
        let mut out = std::io::BufWriter::with_capacity(8 << 20, std::fs::File::create(&tmp)?);
        let written =
            rete_core::v06::write::rewrite_card_stream(&reader, &mut out, card, record, &mut |n| {
                rete_core::build_progress::set_done(n)
            });
        let h = match written {
            Ok(h) => h,
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&tmp);
                anyhow::bail!("rewriting {path}: {e}");
            }
        };
        out.flush()?;
        out.into_inner()
            .map_err(|e| anyhow::anyhow!("flushing {}: {e}", tmp.display()))?
            .sync_all()?;
        h
    };
    std::fs::rename(&tmp, path)?;
    Ok((h, true))
}

/// [`finish`] for the in-RAM writers that are not `rete build`: the file as
/// `0x06` with a lean build record (`front_end: "cli"`, the entry point named
/// `command`, the counts) beside it, so no CLI-written file lacks one.
pub(crate) fn finish_recorded(
    image: Vec<u8>,
    command: &str,
    stats: &rete_core::ingest::BuildStats,
) -> anyhow::Result<Vec<u8>> {
    finish_recorded_with(
        image,
        command,
        stats,
        Some(rete_core::derived::DeriveOpts::default()),
    )
}

/// [`finish_recorded`] with derived-IRI detection on (`Some`) or off.
pub(crate) fn finish_recorded_with(
    image: Vec<u8>,
    command: &str,
    stats: &rete_core::ingest::BuildStats,
    derive: Option<rete_core::derived::DeriveOpts>,
) -> anyhow::Result<Vec<u8>> {
    use rete_core::build_record as br;
    let record = br::to_json(
        &br::Record {
            front_end: br::front_end::CLI.into(),
            front_end_version: env!("CARGO_PKG_VERSION").into(),
            built_at: br::timestamp(),
            options: br::Options {
                command: command.into(),
                ..Default::default()
            },
            inputs: Vec::new(),
            statements_read: None,
        },
        Some(stats),
    );
    rete_core::build_record::finish_in_ram_with(
        image,
        Some(&record),
        WriteOpts::default().with_derive(derive),
    )
    .map_err(|e| anyhow::anyhow!("writing 0x06: {e}"))
}

/// `rete convert IN -o OUT` on a `0x06` input: the same file written again
/// under the writer's current directory-leaf policy. The payload is copied
/// verbatim; the card and the build record are kept as they are.
fn rewrite_v06(
    input: &str,
    output: &str,
    reader: &super::range_source::RangedSourceReader,
    opts: WriteOpts,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let tmp = std::path::PathBuf::from(format!("{output}.convert-tmp"));
    let written = {
        let mut out = std::io::BufWriter::with_capacity(8 << 20, std::fs::File::create(&tmp)?);
        let w = rete_core::v06::write::rewrite(reader, &mut out, opts).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            anyhow::anyhow!("rewriting {input}: {e}")
        })?;
        std::io::Write::flush(&mut out)?;
        w
    };
    std::fs::rename(&tmp, output)?;
    let in_len = rete_core::RangeReader::len(reader);
    eprintln!(
        "convert: wrote {output} — format 0x{:02x}, {} bytes ({:+.2}% vs {} bytes), root {} bytes, content hash {}, {:.1}s",
        written.header.version,
        written.header.file_len,
        (written.header.file_len as f64 - in_len as f64) * 100.0 / in_len.max(1) as f64,
        in_len,
        written.header.root_len,
        hex16(&written.header.content_hash),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
