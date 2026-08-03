//! The ingest group: parse RDF inputs (N-Triples / N-Quads / Turtle) and either
//! `build` them into a `.rete` dataset or `validate` that they parse. Parsing
//! and assembly live in `rete_core::ingest` (shared with the wasm bindings);
//! this module adds the CLI-only concerns: file/stdin IO, format detection by
//! extension, `--materialize`, and the Dataset Card flags.

use crate::commands::buildinfo;
use crate::commands::card::{self, CardArgs};
use rete_core::ingest;

/// Attach the **build-info** section (kind 7, outside the content hash) to a
/// finished card-carrying image: timestamp, builder version, the parameters in
/// force, and — when `measure_costs` — each starter query's measured cost
/// (bytes / range requests / rows / reference ms), run cold against this very
/// image. Cardless builds skip this entirely, staying byte-identical to
/// pre-build-info output.
#[allow(clippy::too_many_arguments)]
fn attach_build_record(
    bytes: Vec<u8>,
    command: &str,
    format: Option<&str>,
    no_pyramid: bool,
    text_index: bool,
    materialize: bool,
    reason: bool,
    pyramid_algo: rete_core::PyramidAlgo,
    measure_costs: bool,
) -> anyhow::Result<Vec<u8>> {
    let header = rete_core::Header::from_bytes(&bytes)?;
    let mut info = buildinfo::new_build_info(buildinfo::BuildParams {
        command: Some(command.to_string()),
        format: format.map(str::to_string),
        no_pyramid,
        text_index,
        materialize,
        reason,
        pyramid_algo: if no_pyramid {
            None
        } else {
            Some(
                match pyramid_algo {
                    rete_core::PyramidAlgo::Types => "types",
                    _ => "louvain",
                }
                .to_string(),
            )
        },
        memory_budget_mb: None,
        codec: Some(buildinfo::codec_name(header.dict_codec).to_string()),
        card_top_n: Some(card::CARD_TOP_N as u32),
    });
    let bytes = if measure_costs {
        let queries = card::load_card(&bytes)?
            .map(|c| c.queries)
            .unwrap_or_default();
        if queries.is_empty() {
            bytes
        } else {
            let image = std::sync::Arc::new(bytes);
            let costs = buildinfo::measure_query_costs(image.clone(), &queries);
            eprintln!(
                "measured {} starter quer{} against the built file",
                costs.queries.len(),
                if costs.queries.len() == 1 { "y" } else { "ies" }
            );
            info.query_costs = Some(costs);
            std::sync::Arc::try_unwrap(image).unwrap_or_else(|a| (*a).clone())
        }
    } else {
        bytes
    };
    let blob = info.to_json_bytes();
    let out = rete_core::attach_build_info(&bytes, &blob)?;
    eprintln!(
        "embedded build info ({} bytes, outside the content hash)",
        blob.len()
    );
    Ok(out)
}

/// Read an input source: a file path, or `-` for stdin.
fn read_input(path: &str) -> anyhow::Result<String> {
    if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(std::fs::read_to_string(path)?)
    }
}

/// The parser to use for an input: explicit `--format` wins, else by extension,
/// else (no extension / stdin) N-Triples.
pub(crate) fn input_format(path: &str, override_fmt: Option<&str>) -> &'static str {
    if let Some(f) = override_fmt {
        return match f {
            "nq" => "nq",
            "ttl" => "ttl",
            "rdfxml" | "rdf" | "owl" | "xml" => "rdfxml",
            _ => "nt",
        };
    }
    let p = path.to_ascii_lowercase();
    if p.ends_with(".nq") || p.ends_with(".nquads") {
        "nq"
    } else if p.ends_with(".ttl") || p.ends_with(".turtle") {
        "ttl"
    } else if p.ends_with(".rdf") || p.ends_with(".owl") || p.ends_with(".rdfxml") {
        // RDF/XML — the common OWL serialization (an `rdf:RDF` root). `.xml` is left
        // to default to N-Triples since it's ambiguous; pass `--format rdfxml` for it.
        "rdfxml"
    } else {
        "nt"
    }
}

/// True if `text` smells like a non-RDF OWL serialization (OWL/XML's `<Ontology>`
/// document element, or OWL Functional Syntax's `Ontology(...)`) rather than
/// RDF/XML — used only to enrich the parse error with a conversion hint.
fn looks_like_non_rdf_owl(text: &str) -> bool {
    let head = &text[..text.len().min(4096)];
    // RDF/XML's document element is rdf:RDF; OWL/XML's is <Ontology …>. Functional
    // Syntax has no XML prolog and opens with `Ontology(` (after optional prefixes).
    (!head.contains("rdf:RDF") && head.contains("<Ontology"))
        || (!head.trim_start().starts_with('<') && head.contains("Ontology("))
}

/// Parse one or more RDF inputs into quads (triples → default graph). Shared by
/// `build` and `validate`. Returns a parse error (which input, what went wrong)
/// if any input is malformed.
fn parse_inputs(inputs: &[String], format: Option<&str>) -> anyhow::Result<Vec<ingest::RawQuad>> {
    let mut quads: Vec<ingest::RawQuad> = Vec::new();
    for input in inputs {
        let fmt = input_format(input, format);
        // Stream N-Triples / N-Quads files line by line so the whole file text is
        // never resident — the big-build memory win. Pre-size from the file length
        // (~64 B/line) to avoid Vec doublings. stdin and Turtle take the text path
        // (oxttl needs the whole input).
        let parsed = if input != "-" && (fmt == "nt" || fmt == "nq") {
            let file = std::fs::File::open(input).map_err(|e| anyhow::anyhow!("{input}: {e}"))?;
            let cap = file
                .metadata()
                .map(|m| (m.len() / 64) as usize)
                .unwrap_or(0);
            ingest::parse_reader(std::io::BufReader::new(file), fmt, cap)
                .map_err(|e| anyhow::anyhow!("{input}: {e}"))?
        } else {
            let text = read_input(input)?;
            ingest::parse_statements(&text, fmt).map_err(|e| {
                // OWL/XML and Functional Syntax look like ".owl" but are NOT RDF, so
                // the RDF/XML reader rejects them. Point the user at the fix instead
                // of leaving a cryptic XML error.
                if fmt == "rdfxml" && looks_like_non_rdf_owl(&text) {
                    anyhow::anyhow!(
                        "{input}: {e}\n\
                         hint: this looks like OWL/XML or OWL Functional Syntax, which \
                         are not RDF. Convert to RDF/XML or Turtle first (e.g. owlready2, \
                         `robot convert`, or Protégé → Save as → RDF/XML), then build that."
                    )
                } else {
                    anyhow::anyhow!("{input}: {e}")
                }
            })?
        };
        // Move the first input's vec in; only pay an extend-copy when merging more.
        if quads.is_empty() {
            quads = parsed;
        } else {
            quads.extend(parsed);
        }
    }
    Ok(quads)
}

/// Parse inputs without building — report triple/quad and named-graph counts, or
/// fail with a clear parse error. The way to check an RDF file (N-Triples /
/// N-Quads / Turtle) is well-formed before ingesting it.
pub(crate) fn validate(inputs: &[String], format: Option<&str>) -> anyhow::Result<()> {
    let quads = parse_inputs(inputs, format)?;
    let named: std::collections::BTreeSet<&String> =
        quads.iter().filter_map(|(_, _, _, g)| g.as_ref()).collect();
    let in_default = quads.iter().filter(|(_, _, _, g)| g.is_none()).count();
    println!(
        "valid: {} statement(s) — {} in the default graph, {} named graph(s)",
        quads.len(),
        in_default,
        named.len()
    );
    Ok(())
}

/// Build a `.rete` from one or more inputs, merged under one shared dictionary.
/// N-Triples/Turtle contribute to the default graph; N-Quads may carry named
/// graphs. The output uses the dataset layout iff any named graph appears (which
/// is byte-identical to the plain triple file when none do).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    inputs: &[String],
    output: &str,
    format: Option<&str>,
    materialize: bool,
    no_pyramid: bool,
    reason: bool,
    pyramid_algo: rete_core::PyramidAlgo,
    text_index: bool,
    type_predicate: Option<&str>,
    card_args: CardArgs,
    no_card_costs: bool,
) -> anyhow::Result<()> {
    // Fast low-RAM path: when every input is an N-Triples / N-Quads FILE and no
    // reasoning is requested, assemble by STREAMING the inputs twice instead of
    // materializing every parsed quad. Peak RAM drops from the (huge, heavily
    // duplicated) string-quad multiset to just the dictionary + id-triples +
    // index — the difference between a ~44 GB and a ~6 GB build on an 88 M-triple
    // graph. Output is byte-identical to the in-memory path. `--materialize` /
    // `--reason` need the whole quad set resident (to run the reasoner) and stdin
    // can't be re-read, so those fall through to the in-memory path below.
    let streamable = !materialize
        && !reason
        && inputs
            .iter()
            .all(|i| i != "-" && matches!(input_format(i, format), "nt" | "nq"));
    if streamable {
        let curated = if card_args.requested() {
            Some(card::load_curated(&card_args)?)
        } else {
            None
        };
        let card_requested = curated.is_some();
        let inputs_fmt: Vec<(&str, &'static str)> = inputs
            .iter()
            .map(|i| (i.as_str(), input_format(i, format)))
            .collect();
        // Re-readable source: each call re-opens and streams every input file. The
        // two-pass assembler invokes it once to observe terms, once to encode.
        let stream = |visit: &mut dyn FnMut(ingest::RawQuad)| -> Result<(), ingest::IngestError> {
            for (path, fmt) in &inputs_fmt {
                let file = std::fs::File::open(path)
                    .map_err(|e| ingest::IngestError::Io(format!("{path}: {e}")))?;
                ingest::stream_reader(std::io::BufReader::new(file), fmt, visit)?;
            }
            Ok(())
        };
        let (bytes, stats) = ingest::assemble_dataset_streaming_algo(
            stream,
            !no_pyramid,
            text_index,
            type_predicate,
            pyramid_algo,
            |stats, dict, triples| match curated {
                Some(curated) => {
                    let blob = card::derive_card_encoded(
                        dict,
                        triples,
                        stats.statements as u64,
                        stats.terms as u64,
                        stats.named_graphs as u64,
                        curated,
                    )
                    .to_json_bytes();
                    eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
                    blob
                }
                None => Vec::new(),
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let bytes = if card_requested {
            attach_build_record(
                bytes,
                "build",
                format,
                no_pyramid,
                text_index,
                false,
                false,
                pyramid_algo,
                !no_card_costs,
            )?
        } else {
            bytes
        };
        std::fs::write(output, &bytes)?;
        print_build_summary(output, &stats, bytes.len());
        return Ok(());
    }

    // 1. Parse every input into quads (triples → default graph, `None`).
    let mut quads = parse_inputs(inputs, format)?;

    // 1b. Reason over the default graph once if either flag needs it: `--materialize`
    // folds the inferred triples in (deduped later by the index builder, so they ship
    // in the file and need no query-time reasoning) and ABORTS on an incoherent graph
    // rather than baking in a contradiction; `--reason` stamps the verdict into the
    // card (recording `coherent: false` honestly, without aborting). `reason()`
    // materializes internally, so the verdict is the same whether computed here or
    // after the bake — coherence is invariant under adding entailments.
    let reasoning = if materialize || reason {
        let base: Vec<(String, String, String)> = quads
            .iter()
            .filter(|(_, _, _, g)| g.is_none())
            .map(|(s, p, o, _)| (s.clone(), p.clone(), o.clone()))
            .collect();
        Some(rete_core::reason(&base))
    } else {
        None
    };

    if materialize {
        let r = reasoning
            .as_ref()
            .expect("reasoning computed when materialize");
        if !r.inconsistencies.is_empty() {
            for inc in &r.inconsistencies {
                eprintln!("  incoherent [{}] {}", inc.kind, inc.detail);
            }
            anyhow::bail!(
                "{} inconsistency(ies) — refusing to materialize an incoherent graph",
                r.inconsistencies.len()
            );
        }
        let inferred = r.inferred.len();
        quads.extend(r.inferred.iter().cloned().map(|(s, p, o)| (s, p, o, None)));
        eprintln!("materialized {inferred} inferred triple(s) into the default graph");
    }

    // 2. Assemble dictionary + indexes + pyramid into the file image, optionally
    // deriving + embedding a Dataset Card (data-catalog metadata) from the final
    // counts. Without a card flag the metadata payload is empty, which is
    // byte-identical to a metadata-free build. `--reason` always embeds a card (to
    // carry the coherence stamp), and stamps the verdict computed above.
    let curated = if card_args.requested() || reason {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    let card_requested = curated.is_some();
    let (bytes, stats) = ingest::assemble_dataset_with_opts_algo(
        quads,
        !no_pyramid,
        text_index,
        type_predicate,
        pyramid_algo,
        |stats, quads| match curated {
            Some(curated) => {
                let mut dataset_card = card::derive_card(
                    quads,
                    stats.terms as u64,
                    stats.named_graphs as u64,
                    curated,
                );
                if let Some(r) = reasoning.as_ref() {
                    dataset_card = dataset_card.with_coherence(r, materialize);
                }
                let blob = dataset_card.to_json_bytes();
                eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
                blob
            }
            None => Vec::new(),
        },
    );
    let bytes = if card_requested {
        attach_build_record(
            bytes,
            "build",
            format,
            no_pyramid,
            text_index,
            materialize,
            reason,
            pyramid_algo,
            !no_card_costs,
        )?
    } else {
        bytes
    };
    std::fs::write(output, &bytes)?;
    print_build_summary(output, &stats, bytes.len());
    Ok(())
}

/// `rete build --memory-budget-mb <N>`: the **memory-bounded external build** —
/// chunk the input to disk, merge, and stream the final file, holding roughly the
/// budget in RAM regardless of graph size (`rete_core::extbuild`). Output is
/// byte-identical to a standard `--no-pyramid` build of the same input.
///
/// v1 constraints (explicit errors): N-Triples/N-Quads *files* only (the input
/// is streamed once; stdin/Turtle can't be), default graph only, no pyramid, no
/// text index, no reasoning. `--card` embeds curated fields + counts (the
/// derived profile lists need unbounded RAM, so they are omitted here).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_external_cmd(
    inputs: &[String],
    output: &str,
    format: Option<&str>,
    memory_budget_mb: u64,
    tmp_dir: Option<&str>,
    materialize: bool,
    reason: bool,
    text_index: bool,
    card_args: CardArgs,
) -> anyhow::Result<()> {
    if materialize || reason {
        anyhow::bail!(
            "--memory-budget-mb is incompatible with --materialize/--reason \
             (they need the whole graph resident); run them via the standard build"
        );
    }
    if text_index {
        anyhow::bail!("--memory-budget-mb does not support --text-index yet");
    }
    let inputs_fmt: Vec<(&str, &'static str)> = inputs
        .iter()
        .map(|i| (i.as_str(), input_format(i, format)))
        .collect();
    // stdin IS allowed here — unlike the two-pass streaming assembler, the
    // external build consumes its input exactly ONCE, so a non-rewindable pipe
    // works. It must be the only input and carry an explicit --format.
    let uses_stdin = inputs.iter().any(|i| i == "-");
    if uses_stdin && (inputs.len() != 1 || format.is_none()) {
        anyhow::bail!(
            "--memory-budget-mb with stdin: `-` must be the only input and \
             requires an explicit --format nt|nq"
        );
    }
    if let Some((bad, fmt)) = inputs_fmt
        .iter()
        .find(|(i, f)| *i != "-" && !matches!(*f, "nt" | "nq"))
    {
        anyhow::bail!(
            "--memory-budget-mb streams N-Triples/N-Quads only ({bad} is {fmt}); \
             convert Turtle/RDF-XML inputs to .nt first"
        );
    }

    let curated = if card_args.requested() {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    // Build conditions for the external build: the parameters are all known up
    // front (there are no starter queries to measure — the external card has no
    // derived profile), so the section is written natively during the stream.
    let build_info = if curated.is_some() {
        buildinfo::new_build_info(buildinfo::BuildParams {
            command: Some("build --memory-budget-mb".to_string()),
            format: format.map(str::to_string),
            no_pyramid: true,
            memory_budget_mb: Some(memory_budget_mb),
            ..Default::default()
        })
        .to_json_bytes()
    } else {
        Vec::new()
    };
    let out_path = std::path::Path::new(output).to_path_buf();
    let stats = rete_core::extbuild::build_external(
        |visit| {
            for (path, fmt) in &inputs_fmt {
                let mut err: Option<rete_core::extbuild::ExtBuildError> = None;
                let mut on_quad = |q: ingest::RawQuad| {
                    if err.is_none() {
                        if let Err(e) = visit(q) {
                            err = Some(e);
                        }
                    }
                };
                let res = if *path == "-" {
                    let stdin = std::io::stdin();
                    ingest::stream_reader(
                        std::io::BufReader::with_capacity(1 << 20, stdin.lock()),
                        fmt,
                        &mut on_quad,
                    )
                } else {
                    let file = std::fs::File::open(path).map_err(|e| {
                        rete_core::extbuild::ExtBuildError::Ingest(ingest::IngestError::Io(
                            format!("{path}: {e}"),
                        ))
                    })?;
                    ingest::stream_reader(
                        std::io::BufReader::with_capacity(1 << 20, file),
                        fmt,
                        &mut on_quad,
                    )
                };
                if let Some(e) = err {
                    return Err(e);
                }
                res.map_err(rete_core::extbuild::ExtBuildError::Ingest)?;
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
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let byte_len = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0) as usize;
    print_build_summary(output, &stats, byte_len);
    Ok(())
}

/// Print the `wrote …` summary for a finished build — quad form when the file has
/// named graphs, triple form otherwise. Shared by the streaming and in-memory
/// build paths.
fn print_build_summary(output: &str, stats: &ingest::BuildStats, byte_len: usize) {
    if stats.named_graphs > 0 {
        println!(
            "wrote {output}: {} quads ({} default + {} named graph(s)), {} terms, {} bytes",
            stats.statements, stats.default_triples, stats.named_graphs, stats.terms, byte_len
        );
    } else {
        println!(
            "wrote {output}: {} triples, {} terms, {} pyramid level(s), {} bytes",
            stats.default_triples, stats.terms, stats.pyramid_levels, byte_len
        );
    }
}

/// Rebuild a `.rete`'s pyramid **in place**: read every triple straight from the
/// file and re-assemble it — with a schema pyramid, optionally keyed on a custom
/// type predicate — without the N-Quads text round-trip that `export | build`
/// needs. For giving a large, pre-pyramid `.rete` (e.g. a 1 GB Wikidata slice
/// built before the schema pyramid existed) its pyramid in one pass.
pub(crate) fn repyramid(
    input: &str,
    output: &str,
    text_index: bool,
    type_predicate: Option<&str>,
    pyramid_algo: rete_core::PyramidAlgo,
    card_args: CardArgs,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(input)?;
    let rete = rete_core::Rete::open(&bytes)?;
    // The decoded graph owns its sections, so the raw file image is done — free
    // it before building so the (large) assembly has the headroom.
    drop(bytes);

    let mut quads: Vec<ingest::RawQuad> = rete
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
    drop(rete); // only the quads are needed to re-assemble.

    // Optionally derive + embed a Dataset Card from the rebuilt counts (same as
    // `build --card`); without a card flag the metadata payload stays empty.
    let curated = if card_args.requested() {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    let card_requested = curated.is_some();
    let (out_bytes, stats) = ingest::assemble_dataset_with_opts_algo(
        quads,
        true,
        text_index,
        type_predicate,
        pyramid_algo,
        |stats, quads| match curated {
            Some(curated) => {
                let blob = card::derive_card(
                    quads,
                    stats.terms as u64,
                    stats.named_graphs as u64,
                    curated,
                )
                .to_json_bytes();
                eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
                blob
            }
            None => Vec::new(),
        },
    );
    let out_bytes = if card_requested {
        attach_build_record(
            out_bytes,
            "repyramid",
            None,
            false,
            text_index,
            false,
            false,
            pyramid_algo,
            true,
        )?
    } else {
        out_bytes
    };
    std::fs::write(output, &out_bytes)?;
    if stats.named_graphs > 0 {
        println!(
            "repyramid: wrote {output} — {} quads ({} default + {} named), {} terms, {} bytes",
            stats.statements,
            stats.default_triples,
            stats.named_graphs,
            stats.terms,
            out_bytes.len()
        );
    } else {
        println!(
            "repyramid: wrote {output} — {} triples, {} terms, {} pyramid level(s), {} bytes",
            stats.default_triples,
            stats.terms,
            stats.pyramid_levels,
            out_bytes.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rete_core::Rete;

    /// The determinism contract of issue #153: a card build carries a
    /// build-info section (timestamp, builder, params, measured costs) and two
    /// builds of identical data STILL produce equal content hashes — the
    /// volatile facts sit outside the hash, and stripping them yields
    /// byte-identical images.
    #[test]
    fn card_build_is_hash_deterministic_with_build_info_outside() {
        let nt = "<http://x/a> <http://x/p> <http://x/b> .\n\
                  <http://x/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://x/C> .\n";
        let pid = std::process::id();
        let inp = std::env::temp_dir().join(format!("rete_bi_in_{pid}.nt"));
        let out1 = std::env::temp_dir().join(format!("rete_bi_out1_{pid}.rete"));
        let out2 = std::env::temp_dir().join(format!("rete_bi_out2_{pid}.rete"));
        std::fs::write(&inp, nt).unwrap();
        let run = |out: &std::path::Path| {
            build(
                &[inp.to_str().unwrap().to_string()],
                out.to_str().unwrap(),
                None,
                false,
                false,
                false,
                rete_core::PyramidAlgo::Louvain,
                false,
                None,
                CardArgs {
                    enabled: true,
                    title: Some("BI test".into()),
                    ..Default::default()
                },
                false,
            )
            .unwrap();
            std::fs::read(out).unwrap()
        };
        let a = run(&out1);
        let b = run(&out2);
        std::fs::remove_file(&inp).ok();
        std::fs::remove_file(&out1).ok();
        std::fs::remove_file(&out2).ok();

        // Both carry a parseable build-info record with the promised facts.
        let info = buildinfo::BuildInfo::from_json_bytes(
            &rete_core::read_build_info(&a).unwrap().unwrap(),
        )
        .unwrap();
        assert!(info.built_at.is_some(), "timestamp stamped");
        assert!(
            info.builder.as_deref().unwrap().starts_with("rete-cli "),
            "builder version stamped"
        );
        assert_eq!(info.params.command.as_deref(), Some("build"));
        assert_eq!(info.params.card_top_n, Some(card::CARD_TOP_N as u32));
        let costs = info.query_costs.clone().expect("costs measured by default");
        assert!(costs.context.engine.is_some() && costs.context.note.is_some());
        let smoke = costs
            .queries
            .iter()
            .find(|c| c.id == "ov-one-row")
            .expect("one-row smoke query measured");
        assert_eq!(
            smoke.rows, 1,
            "the smoke query answers with exactly one row"
        );
        assert!(smoke.bytes > 0 && smoke.requests > 0);

        // Hash equality across the two builds, despite differing build-info.
        let ha = rete_core::Header::from_bytes(&a).unwrap();
        let hb = rete_core::Header::from_bytes(&b).unwrap();
        assert_eq!(
            ha.content_hash, hb.content_hash,
            "content hash reproducible"
        );
        assert!(rete_core::verify(&a).unwrap() && rete_core::verify(&b).unwrap());

        // Strip the (only volatile) section: the images are byte-identical.
        assert_eq!(
            rete_core::attach_build_info(&a, &[]).unwrap(),
            rete_core::attach_build_info(&b, &[]).unwrap(),
            "everything except build-info is reproducible byte-for-byte"
        );

        // The measured byte/request figures are themselves deterministic (a
        // property of layout + query, not of the run).
        let info_b = buildinfo::BuildInfo::from_json_bytes(
            &rete_core::read_build_info(&b).unwrap().unwrap(),
        )
        .unwrap();
        let strip_ms = |c: &buildinfo::QueryCosts| {
            c.queries
                .iter()
                .map(|q| (q.id.clone(), q.bytes, q.requests, q.rows))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            strip_ms(&costs),
            strip_ms(info_b.query_costs.as_ref().unwrap()),
        );
    }

    /// A cardless build must stay byte-identical to pre-build-info output — no
    /// metadata, no build-info, nothing volatile.
    #[test]
    fn cardless_build_gets_no_build_info() {
        let nt = "<http://x/a> <http://x/p> <http://x/b> .\n";
        let pid = std::process::id();
        let inp = std::env::temp_dir().join(format!("rete_nobi_in_{pid}.nt"));
        let out = std::env::temp_dir().join(format!("rete_nobi_out_{pid}.rete"));
        std::fs::write(&inp, nt).unwrap();
        build(
            &[inp.to_str().unwrap().to_string()],
            out.to_str().unwrap(),
            None,
            false,
            false,
            false,
            rete_core::PyramidAlgo::Louvain,
            false,
            None,
            CardArgs::default(),
            false,
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        std::fs::remove_file(&inp).ok();
        std::fs::remove_file(&out).ok();
        assert!(rete_core::read_build_info(&bytes).unwrap().is_none());
        let h = rete_core::Header::from_bytes(&bytes).unwrap();
        assert_eq!(h.metadata_len, 0);
        assert_eq!(h.build_info_len, 0);
    }

    /// `build --materialize` runs the reasoner and bakes RDFS entailments into the
    /// file: `x a C` + `C subClassOf D` yields a queryable `x a D`.
    #[test]
    fn build_materialize_bakes_in_entailments() {
        const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        const SUBCLASS: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
        let nt = format!("<http://x> {TYPE} <http://C> .\n<http://C> {SUBCLASS} <http://D> .\n");
        let pid = std::process::id();
        let inp = std::env::temp_dir().join(format!("rete_mat_in_{pid}.nt"));
        let out = std::env::temp_dir().join(format!("rete_mat_out_{pid}.rete"));
        std::fs::write(&inp, nt).unwrap();

        build(
            &[inp.to_str().unwrap().to_string()],
            out.to_str().unwrap(),
            None,
            true,
            false,
            false,
            rete_core::PyramidAlgo::Louvain,
            false,
            None,
            CardArgs::default(),
            false,
        )
        .unwrap();

        let bytes = std::fs::read(&out).unwrap();
        let rete = Rete::open(&bytes).unwrap();
        let inferred = rete.query(Some("<http://x>"), Some(TYPE), Some("<http://D>"));
        assert_eq!(inferred.len(), 1, "x a D should be materialized");

        // Without --materialize the entailed triple is absent.
        build(
            &[inp.to_str().unwrap().to_string()],
            out.to_str().unwrap(),
            None,
            false,
            false,
            false,
            rete_core::PyramidAlgo::Louvain,
            false,
            None,
            CardArgs::default(),
            false,
        )
        .unwrap();
        let plain = Rete::open(&std::fs::read(&out).unwrap()).unwrap();
        assert!(plain
            .query(Some("<http://x>"), Some(TYPE), Some("<http://D>"))
            .is_empty());

        std::fs::remove_file(&inp).ok();
        std::fs::remove_file(&out).ok();
    }
}
