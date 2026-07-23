//! `rete manifest` — a **writable logical graph** as an ordered log of
//! immutable `.rete` segments (see `rete_core::manifest` for the model).
//!
//! The workflow the subcommands cover:
//!
//! ```text
//! rete manifest init  g.rete-manifest.json base.rete     # start the log
//! rete manifest add   g.rete-manifest.json --adds d.rete # append a session's segment
//! rete serve          g.rete-manifest.json               # live SPARQL 1.1 + Update (journal)
//! rete manifest seal  g.rete-manifest.json               # journal → adds + tombstone segments
//! rete manifest query g.rete-manifest.json "SELECT …"    # the fold, queried as ONE graph
//! rete manifest compact g.rete-manifest.json             # fold the log → one fresh .rete
//! ```
//!
//! Queries here are **pattern-level correct** across segments (unlike
//! `rete federate`'s query-level UNION): the fold is re-assembled into one
//! in-memory image with a single dictionary, so joins spanning segments — a
//! delta adding a triple about a base entity — resolve. That re-assembly is
//! O(total quads) per open: right for the living-dataset scale `rete serve`
//! already targets, not for multi-GB catalogs (their lazy multi-segment view
//! is future work; `compact` is the bridge).
//!
//! Segments are verified against the manifest's `{size, blake3_16}` pins
//! before use (the same contract `datasets.lock.json` uses), so a swapped or
//! truncated segment fails loudly, never silently returns fewer rows.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rete_core::ingest::{assemble_dataset_with_opts, RawQuad};
use rete_core::manifest::{fold_entry, LogEntry, Manifest, SegmentRef};
use rete_core::{eval_query, Header, RangeReader, Rete, HEADER_LEN};
use serde_json::json;

use crate::commands::card::hex16;
use crate::commands::range_source::{is_url, RangedSourceReader};
use crate::commands::render::print_query_output;

/// Does this source name a manifest rather than a `.rete`? (Extension-based:
/// manifests are JSON documents, segments never are.)
pub(crate) fn is_manifest_path(source: &str) -> bool {
    source.ends_with(".json")
}

/// The directory segment-relative URLs resolve against: the manifest's own.
fn manifest_dir(manifest_path: &str) -> PathBuf {
    Path::new(manifest_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve a segment URL: absolute `http(s)://` stays; anything else joins the
/// manifest's directory (absolute paths pass through `join` unchanged).
fn resolve(dir: &Path, url: &str) -> String {
    if is_url(url) {
        url.to_string()
    } else {
        dir.join(url).to_string_lossy().into_owned()
    }
}

fn load_manifest(path: &str) -> anyhow::Result<Manifest> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read manifest {path}: {e}"))?;
    Manifest::parse(&text).map_err(|e| anyhow::anyhow!("{path}: {e}"))
}

/// Write the manifest via a temp file + rename, so a crash mid-write never
/// leaves a half-document. (Windows cannot rename over an existing file, so
/// the destination is removed first — an unavoidable, tiny non-atomic window.)
fn store_manifest(path: &str, m: &Manifest) -> anyhow::Result<()> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, m.to_json_pretty())?;
    if Path::new(path).exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Describe a segment for the manifest: its size and the 16-byte blake3
/// content hash every `.rete` carries at header bytes 8..24. `as_written` is
/// stored (relative URLs stay relative), `resolved` is what's opened.
fn describe_segment(resolved: &str, as_written: &str) -> anyhow::Result<SegmentRef> {
    let reader = RangedSourceReader::open(resolved)?;
    let size = reader.len();
    let head = reader.read_at(0, HEADER_LEN as u64)?;
    let header =
        Header::from_bytes(&head).map_err(|e| anyhow::anyhow!("{resolved}: bad header: {e}"))?;
    Ok(SegmentRef {
        url: as_written.to_string(),
        size,
        blake3_16: hex16(&header.content_hash),
    })
}

/// Check a segment still is the exact artifact the manifest pinned. A mismatch
/// means the file was replaced or truncated — results over it would be wrong.
fn verify_segment(resolved: &str, seg: &SegmentRef) -> anyhow::Result<()> {
    let actual = describe_segment(resolved, &seg.url)?;
    if actual.size != seg.size || actual.blake3_16 != seg.blake3_16 {
        anyhow::bail!(
            "segment {resolved} does not match its manifest pin: \
             size {} vs pinned {}, blake3_16 {} vs pinned {}",
            actual.size,
            seg.size,
            actual.blake3_16,
            seg.blake3_16
        );
    }
    Ok(())
}

/// Load every quad (default + named graphs) of one segment, term-level.
fn load_segment_quads(resolved: &str) -> anyhow::Result<Vec<RawQuad>> {
    let mut out: Vec<RawQuad> = Vec::new();
    let collect = |rete: &Rete, out: &mut Vec<RawQuad>| {
        rete.dump_each(None, |s, p, o| {
            out.push((s.to_string(), p.to_string(), o.to_string(), None));
        });
        let graphs: Vec<String> = rete.graph_names().iter().map(|g| g.to_string()).collect();
        for g in graphs {
            rete.dump_each(Some(&g), |s, p, o| {
                out.push((s.to_string(), p.to_string(), o.to_string(), Some(g.clone())));
            });
        }
    };
    if is_url(resolved) {
        let reader = RangedSourceReader::open(resolved)?;
        let rete = Rete::open_ranged(&reader)?;
        collect(&rete, &mut out);
        if rete.index_incomplete() {
            anyhow::bail!(
                "a range request failed while reading segment {resolved}; \
                 the fold would be incomplete — retry"
            );
        }
    } else {
        let bytes = std::fs::read(resolved)
            .map_err(|e| anyhow::anyhow!("cannot read segment {resolved}: {e}"))?;
        let rete = Rete::open(&bytes)?;
        collect(&rete, &mut out);
    }
    Ok(out)
}

/// The manifest's visible graph: verify every pinned segment, then apply the
/// ordered fold `(visible ∖ dels) ∪ adds` entry by entry.
pub(crate) fn visible_quads(manifest_path: &str) -> anyhow::Result<BTreeSet<RawQuad>> {
    let m = load_manifest(manifest_path)?;
    let dir = manifest_dir(manifest_path);
    let mut visible: BTreeSet<RawQuad> = BTreeSet::new();
    for entry in &m.log {
        let dels = match &entry.dels {
            Some(seg) => {
                let r = resolve(&dir, &seg.url);
                verify_segment(&r, seg)?;
                load_segment_quads(&r)?
            }
            None => Vec::new(),
        };
        let adds = match &entry.adds {
            Some(seg) => {
                let r = resolve(&dir, &seg.url);
                verify_segment(&r, seg)?;
                load_segment_quads(&r)?
            }
            None => Vec::new(),
        };
        fold_entry(&mut visible, dels, adds);
    }
    Ok(visible)
}

/// Assemble the fold into one queryable in-memory image. Pyramid and text
/// index off: this image is per-invocation and only needs to answer queries
/// fast, exactly like `rete serve`'s rebuild.
fn assemble(quads: Vec<RawQuad>) -> anyhow::Result<Rete> {
    let (image, _stats) = assemble_dataset_with_opts(quads, false, false, None, |_, _| Vec::new());
    Ok(Rete::open(&image)?)
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

/// `rete manifest init <manifest> <base.rete>` — start a log with one entry.
pub(crate) fn init(manifest_path: &str, base: &str, name: Option<&str>) -> anyhow::Result<()> {
    if Path::new(manifest_path).exists() {
        anyhow::bail!("{manifest_path} already exists — `add` appends to an existing manifest");
    }
    let dir = manifest_dir(manifest_path);
    let seg = describe_segment(&resolve(&dir, base), base)?;
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| default_name(manifest_path));
    let m = Manifest {
        name,
        generation: 1,
        log: vec![LogEntry {
            adds: Some(seg.clone()),
            dels: None,
        }],
    };
    store_manifest(manifest_path, &m)?;
    println!(
        "initialized {manifest_path} (generation 1)\n  adds {} ({} bytes, blake3 {})",
        seg.url, seg.size, seg.blake3_16
    );
    Ok(())
}

/// The default logical-graph name: the manifest's file stem, minus the
/// conventional suffixes.
fn default_name(manifest_path: &str) -> String {
    Path::new(manifest_path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| manifest_path.to_string())
        .trim_end_matches(".json")
        .trim_end_matches(".rete-manifest")
        .to_string()
}

/// `rete manifest add` — append one log entry (a session's built segment,
/// and/or a tombstone file) and bump the generation.
pub(crate) fn add(
    manifest_path: &str,
    adds: Option<&str>,
    dels: Option<&str>,
) -> anyhow::Result<()> {
    if adds.is_none() && dels.is_none() {
        anyhow::bail!("nothing to add: pass --adds and/or --dels");
    }
    let mut m = load_manifest(manifest_path)?;
    let dir = manifest_dir(manifest_path);
    let entry = LogEntry {
        adds: adds
            .map(|u| describe_segment(&resolve(&dir, u), u))
            .transpose()?,
        dels: dels
            .map(|u| describe_segment(&resolve(&dir, u), u))
            .transpose()?,
    };
    m.log.push(entry);
    m.generation += 1;
    store_manifest(manifest_path, &m)?;
    println!(
        "{manifest_path}: appended log entry #{} (generation {})",
        m.log.len(),
        m.generation
    );
    Ok(())
}

/// `rete manifest status` — the log, and whether every pinned segment still
/// verifies. `--count` additionally runs the full fold for the quad count.
pub(crate) fn status(manifest_path: &str, count: bool, as_json: bool) -> anyhow::Result<()> {
    let m = load_manifest(manifest_path)?;
    let dir = manifest_dir(manifest_path);
    let mut rows = Vec::new();
    let mut all_ok = true;
    for (i, entry) in m.log.iter().enumerate() {
        for (seg, is_dels) in [(&entry.adds, false), (&entry.dels, true)] {
            let Some(seg) = seg else { continue };
            let resolved = resolve(&dir, &seg.url);
            let verified = verify_segment(&resolved, seg);
            all_ok &= verified.is_ok();
            rows.push((i, is_dels, seg.clone(), verified));
        }
    }
    let visible = if count {
        Some(visible_quads(manifest_path)?.len())
    } else {
        None
    };
    if as_json {
        let out = json!({
            "schemaVersion": crate::JSON_SCHEMA_VERSION,
            "name": m.name,
            "generation": m.generation,
            "entries": m.log.len(),
            "verified": all_ok,
            "visible_quads": visible,
            "segments": rows.iter().map(|(i, is_dels, seg, v)| json!({
                "entry": i,
                "role": if *is_dels { "dels" } else { "adds" },
                "url": seg.url,
                "size": seg.size,
                "blake3_16": seg.blake3_16,
                "ok": v.is_ok(),
                "error": v.as_ref().err().map(|e| e.to_string()),
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "{} — generation {}, {} log entr{}",
            m.name,
            m.generation,
            m.log.len(),
            if m.log.len() == 1 { "y" } else { "ies" }
        );
        for (i, is_dels, seg, verified) in &rows {
            let role = if *is_dels { "dels" } else { "adds" };
            let state = match verified {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("MISMATCH — {e}"),
            };
            println!(
                "  [{i}] {role} {} ({} bytes, blake3 {}) {state}",
                seg.url, seg.size, seg.blake3_16
            );
        }
        if let Some(n) = visible {
            println!("  visible quads: {n}");
        }
    }
    if !all_ok {
        anyhow::bail!("one or more segments no longer match their manifest pins");
    }
    Ok(())
}

/// `rete manifest query` — SPARQL over the fold, **as one graph**: one merged
/// dictionary, so joins across segments resolve (`federate`'s per-source UNION
/// cannot do that).
pub(crate) fn query(
    manifest_path: &str,
    query: &str,
    as_json: bool,
    entail: bool,
) -> anyhow::Result<()> {
    let visible = visible_quads(manifest_path)?;
    let quads: Vec<RawQuad> = visible.into_iter().collect();
    let n = quads.len();
    let mut rete = assemble(quads)?;
    rete.set_service_client(Box::new(super::service_http::HttpServiceClient));
    let eval = if entail {
        rete_core::eval_query_reasoned
    } else {
        eval_query
    };
    let result = eval(&rete, query).map_err(|e| anyhow::anyhow!("{e}"))?;
    print_query_output(&result, as_json);
    eprintln!("(fold: {n} visible quads)");
    Ok(())
}

/// Parse one journal line (`+ <nquads>` / `- <nquads>`) — the format
/// `rete serve` appends (see `commands::serve`).
fn parse_journal_line(line: &str, at: &str) -> anyhow::Result<(bool, RawQuad)> {
    let (op, payload) = line.split_at(1);
    let add = match op {
        "+" => true,
        "-" => false,
        other => anyhow::bail!("{at}: unknown journal op {other:?} (expected + or -)"),
    };
    let mut quads = rete_core::ingest::parse_quads(payload.trim_start())
        .map_err(|e| anyhow::anyhow!("{at}: {e}"))?;
    match quads.len() {
        1 => Ok((add, quads.pop().expect("len checked"))),
        n => anyhow::bail!("{at}: journal line held {n} statements"),
    }
}

/// `rete manifest seal` — the WAL checkpoint: net the journal per quad (last
/// op wins), build the net additions as a fresh segment and the net deletions
/// as a tombstone segment, append one log entry, truncate the journal.
///
/// Stop `rete serve` first: seal and serve are both journal writers, and the
/// journal is single-writer by design.
pub(crate) fn seal(
    manifest_path: &str,
    journal: Option<&str>,
    out_dir: Option<&str>,
) -> anyhow::Result<()> {
    let journal_path = journal
        .map(str::to_string)
        .unwrap_or_else(|| format!("{manifest_path}.changes"));
    if !Path::new(&journal_path).exists() {
        println!("nothing to seal: no journal at {journal_path}");
        return Ok(());
    }
    // Net the journal per quad, in order: the LAST op on a quad wins. A quad
    // added then deleted within the chunk nets to a tombstone — conservative
    // and correct whether or not an earlier segment holds it (deleting an
    // absent quad is a no-op in the fold).
    let mut net: BTreeMap<RawQuad, bool> = BTreeMap::new();
    for (i, line) in std::fs::read_to_string(&journal_path)?.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (add, quad) = parse_journal_line(line, &format!("{journal_path}:{}", i + 1))?;
        net.insert(quad, add);
    }
    if net.is_empty() {
        println!("nothing to seal: journal at {journal_path} is empty");
        return Ok(());
    }
    let adds: Vec<RawQuad> = net
        .iter()
        .filter(|(_, &add)| add)
        .map(|(q, _)| q.clone())
        .collect();
    let dels: Vec<RawQuad> = net
        .iter()
        .filter(|(_, &add)| !add)
        .map(|(q, _)| q.clone())
        .collect();

    let mut m = load_manifest(manifest_path)?;
    let dir = out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir(manifest_path));
    std::fs::create_dir_all(&dir)?;
    let gen = m.generation + 1;

    let mut entry = LogEntry {
        adds: None,
        dels: None,
    };
    let mut built = Vec::new();
    for (quads, is_dels) in [(adds, false), (dels, true)] {
        if quads.is_empty() {
            continue;
        }
        let n = quads.len();
        let (image, _stats) =
            assemble_dataset_with_opts(quads, false, false, None, |_, _| Vec::new());
        let header = Header::from_bytes(&image[..HEADER_LEN])
            .map_err(|e| anyhow::anyhow!("built segment has a bad header: {e}"))?;
        let hash = hex16(&header.content_hash);
        let suffix = if is_dels { ".tomb.rete" } else { ".rete" };
        let file_name = format!("{}-g{gen}-{}{suffix}", m.name, &hash[..8]);
        let path = dir.join(&file_name);
        std::fs::write(&path, &image)?;
        let seg = SegmentRef {
            url: file_name.clone(),
            size: image.len() as u64,
            blake3_16: hash,
        };
        if is_dels {
            entry.dels = Some(seg);
        } else {
            entry.adds = Some(seg);
        }
        built.push(format!(
            "  {} {} ({n} quad{})",
            if is_dels { "dels" } else { "adds" },
            file_name,
            if n == 1 { "" } else { "s" }
        ));
    }

    m.log.push(entry);
    m.generation = gen;
    store_manifest(manifest_path, &m)?;
    // The manifest now carries the changes; an interrupted run before this
    // truncation re-seals the same content into identically-hashed segments.
    std::fs::write(&journal_path, "")?;
    println!("sealed journal into generation {gen}:");
    for line in built {
        println!("{line}");
    }
    Ok(())
}

/// `rete manifest compact` — fold the whole log into ONE fresh `.rete`
/// (pyramid on: this is a durable artifact, not a per-query image), then
/// reset the manifest to a single entry. Superseded segments are left on
/// disk — delete them once no reader still uses the old generation.
pub(crate) fn compact(
    manifest_path: &str,
    output: Option<&str>,
    no_pyramid: bool,
) -> anyhow::Result<()> {
    let mut m = load_manifest(manifest_path)?;
    let folded_entries = m.log.len();
    let visible = visible_quads(manifest_path)?;
    let quads: Vec<RawQuad> = visible.into_iter().collect();
    let n = quads.len();
    let (image, _stats) =
        assemble_dataset_with_opts(quads, !no_pyramid, false, None, |_, _| Vec::new());
    let header = Header::from_bytes(&image[..HEADER_LEN])
        .map_err(|e| anyhow::anyhow!("compacted file has a bad header: {e}"))?;
    let hash = hex16(&header.content_hash);
    let gen = m.generation + 1;

    let dir = manifest_dir(manifest_path);
    let (path, url) = match output {
        Some(out) => (PathBuf::from(out), out.to_string()),
        None => {
            let file_name = format!("{}-g{gen}-{}.rete", m.name, &hash[..8]);
            (dir.join(&file_name), file_name)
        }
    };
    std::fs::write(&path, &image)?;

    m.log = vec![LogEntry {
        adds: Some(SegmentRef {
            url,
            size: image.len() as u64,
            blake3_16: hash,
        }),
        dels: None,
    }];
    m.generation = gen;
    store_manifest(manifest_path, &m)?;
    println!(
        "compacted {folded_entries} log entr{} into {} ({n} quads, {} bytes) — generation {gen}\n\
         superseded segments were left in place; remove them once no reader needs them",
        if folded_entries == 1 { "y" } else { "ies" },
        path.display(),
        image.len()
    );
    Ok(())
}
