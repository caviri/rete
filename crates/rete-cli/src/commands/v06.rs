//! Format `0x06` in the CLI: what the in-RAM writers finish with, and
//! `rete convert` (a `0x05` file to `0x06`, reading sections only).

use rete_core::v06::write::{WriteOpts, MIN_FILE_LEN};

/// `RETE_WRITE_FORMAT=5`: the in-RAM build writes the legacy `0x05` layout —
/// a test hook (the byte-identity test of `convert`), not a user option.
pub(crate) fn writes_legacy() -> bool {
    std::env::var("RETE_WRITE_FORMAT").is_ok_and(|v| v.trim() == "5")
}

/// Turn a finished in-memory `0x05` image into the `0x06` file the writer
/// emits (unless [`writes_legacy`]). The same code `rete convert` runs, so a
/// build and a converted build of the same input are the same bytes.
pub(crate) fn finish(image: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    if writes_legacy() || rete_core::v06::open::is_v06(&image) {
        return Ok(image);
    }
    rete_core::v06::write::convert_image(&image, WriteOpts::default())
        .map_err(|e| anyhow::anyhow!("writing 0x06: {e}"))
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
    if r.len == 0 || info.len() as u64 > r.len {
        return Ok(false);
    }
    let mut buf = info.to_vec();
    buf.resize(r.len as usize, b' ');
    f.seek(SeekFrom::Start(r.offset))?;
    f.write_all(&buf)?;
    f.sync_all()?;
    Ok(true)
}

/// `rete convert IN -o OUT`: a `0x05` file to `0x06`, reading its sections
/// through ranges (never text), payload copied verbatim. A file whose named
/// graphs are stored per graph (kind 5) is re-encoded as a quad index from its
/// decoded statements, as `repyramid` does.
pub(crate) fn convert(input: &str, output: &str, quad: Option<&str>) -> anyhow::Result<()> {
    use rete_core::v06::write::QuadClustering;
    if std::path::Path::new(input) == std::path::Path::new(output) {
        anyhow::bail!("convert writes a new file: OUT must differ from IN");
    }
    let mut opts = WriteOpts::default();
    match quad {
        Some("graph") => opts.quad = QuadClustering::GraphMajor,
        Some("order") => opts.quad = QuadClustering::OrderMajor,
        Some("auto") | None => {}
        Some(x) => anyhow::bail!("--quad-clustering must be auto, graph or order (got {x})"),
    }
    let reader = super::range_source::RangedSourceReader::open(input)?;
    let head = rete_core::RangeReader::read_at(&reader, 0, 8.min(rete_core::RangeReader::len(&reader)))?;
    if rete_core::v06::open::is_v06(&head) {
        anyhow::bail!("{input} is already format 0x06");
    }
    let started = std::time::Instant::now();
    let tmp = std::path::PathBuf::from(format!("{output}.convert-tmp"));
    let written = {
        let mut out = std::io::BufWriter::with_capacity(
            8 << 20,
            std::fs::File::create(&tmp)?,
        );
        let w = match rete_core::v06::write::convert(&reader, &mut out, opts) {
            Ok(w) => w,
            Err(rete_core::format::FileError::Container(m)) if m.contains("kind 5") => {
                drop(out);
                let _ = std::fs::remove_file(&tmp);
                return convert_per_graph(input, output);
            }
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&tmp);
                return Err(anyhow::anyhow!("converting {input}: {e}"));
            }
        };
        std::io::Write::flush(&mut out)?;
        w
    };
    std::fs::rename(&tmp, output)?;
    let in_len = rete_core::RangeReader::len(&reader);
    eprintln!(
        "convert: wrote {output} — format 0x06, {} bytes ({:+.2}% vs {} bytes), content hash {}, {:.1}s",
        written.header.file_len,
        (written.header.file_len as f64 - in_len as f64) * 100.0 / in_len.max(1) as f64,
        in_len,
        hex16(&written.header.content_hash),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Kind-5 inputs: decode every statement and rebuild with the quad index —
/// the per-graph layout is not one `0x06` can carry.
fn convert_per_graph(input: &str, output: &str) -> anyhow::Result<()> {
    eprintln!(
        "convert: {input} stores its named graphs per graph (kind 5); re-encoding them as a quad index"
    );
    let bytes = std::fs::read(input)?;
    let rete = rete_core::Rete::open(&bytes)?;
    let card = rete.metadata().map(<[u8]>::to_vec);
    let mut quads: Vec<rete_core::format::RawQuad> = rete
        .dump(None)
        .into_iter()
        .map(|(s, p, o)| (s, p, o, None))
        .collect();
    let named: Vec<String> = rete.graph_names().iter().map(|g| g.to_string()).collect();
    for g in &named {
        for (s, p, o) in rete.dump(Some(g)) {
            quads.push((s, p, o, Some(g.clone())));
        }
    }
    let mut opts = rete_core::ingest::AssembleOptions::sectioned();
    opts.graph_layout = rete_core::GraphLayout::Quad;
    opts.with_pyramid = rete.header().pyramid_meta_len > 0;
    opts.with_text_index = rete.has_text_index();
    opts.label_index = rete.has_label_index();
    opts.perms = rete.header().perms;
    let (image, _) = rete_core::ingest::assemble_dataset_opts(quads, &opts, |_, _| {
        card.clone().unwrap_or_default()
    });
    let image = finish(image)?;
    std::fs::write(output, &image)?;
    eprintln!("convert: wrote {output} — format 0x06, {} bytes", image.len());
    Ok(())
}

fn hex16(b: &[u8; 16]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
