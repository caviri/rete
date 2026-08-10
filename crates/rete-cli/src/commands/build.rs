//! The ingest group: parse RDF inputs (N-Triples / N-Quads / Turtle) and either
//! `build` them into a `.rete` dataset or `validate` that they parse. Parsing
//! and assembly live in `rete_core::ingest` (shared with the wasm bindings);
//! this module adds the CLI-only concerns: file/stdin IO, format detection by
//! extension, `--materialize`, and the Dataset Card flags.

use crate::commands::buildinfo;
use crate::commands::card::{self, CardArgs};
use rete_core::card::{claim_of, Claim};
use rete_core::ingest;

/// Run the card's starter queries against the finished image and **remove the
/// ones the run proves worthless**, returning the (possibly re-carded) image
/// and the rows to record.
///
/// The build already ran every starter query to cost it, so for a carded build
/// emptiness is *measured*, not inferred — and a measurement beats every static
/// rule that guessed at it from the profile. The generator's own principle is
/// that a query answering nothing is worse than no query at all, so the honest
/// repair is to not ship it.
///
/// **Why drop rather than fail.** Refusing to build is the right answer for
/// *authored* content — an over-long `extra` bag is the publisher's text, and
/// silently rewriting it destroys an intent only they can restore. A starter
/// query has no author: the generator wrote it moments earlier from this
/// dataset's own profile, and the build is the only party in a position to act.
/// Failing would also make the file unbuildable for a reason the user cannot
/// fix, at the very end of a build that may have taken hours, over a metadata
/// nicety — and would push them onto `--no-card-costs`, which switches off the
/// measurement rather than the problem. The generator already *drops* (rather
/// than fails) when its static `provably_empty` hook fires; measurement is a
/// better oracle for the same question and earns the same consequence. Loud is
/// achieved without fatal: every drop is printed with its reason and recorded
/// in the build record, so the evidence outlives the terminal.
fn drop_measured_empty_queries(
    bytes: Vec<u8>,
    mut card: card::DatasetCard,
) -> anyhow::Result<(Vec<u8>, buildinfo::QueryCosts, Vec<buildinfo::DroppedQuery>)> {
    let image = std::sync::Arc::new(bytes);
    let measured = buildinfo::measure_query_costs(image.clone(), &card.queries);
    let mut bytes = std::sync::Arc::try_unwrap(image).unwrap_or_else(|a| (*a).clone());
    eprintln!(
        "measured {} starter quer{} against the built file",
        measured.len(),
        if measured.len() == 1 { "y" } else { "ies" }
    );

    let mut kept = Vec::with_capacity(measured.len());
    let mut dropped = Vec::new();
    for m in measured {
        match m.useless {
            None => kept.push(m.cost),
            Some(why) => {
                // Only a zero-row result can contradict a template: the
                // non-emptiness claims are about row *count*, and
                // `NonEmpty::Aggregate` says outright that the row's values may
                // be unbound. So a vacuous row is news, not a broken promise —
                // and inside this arm "useless but not vacuous" is exactly
                // "measured zero rows".
                let contradicts_claim = !m.vacuous && claim_of(&m.cost.id) == Claim::CannotBeEmpty;
                dropped.push(buildinfo::DroppedQuery {
                    id: m.cost.id,
                    why,
                    contradicts_claim,
                });
            }
        }
    }

    if !dropped.is_empty() {
        eprintln!(
            "dropping {} starter quer{} the build measured as useless on this file:",
            dropped.len(),
            if dropped.len() == 1 { "y" } else { "ies" }
        );
        for d in &dropped {
            eprintln!("  {:<14} {}", d.id, d.why);
            if d.contradicts_claim {
                // The static rule and the measurement disagree, and the
                // measurement is the one that ran. A template that declares its
                // emitted query cannot be empty and then measures zero is a bug
                // in `queries.rs`, not a property of this dataset.
                eprintln!(
                    "  {:<14} ^ GENERATOR DEFECT: its template claims this query cannot come back \
                     empty. Fix the rule in crates/rete-cli/src/commands/queries.rs.",
                    ""
                );
            }
        }
        let dropped_ids: std::collections::BTreeSet<&str> =
            dropped.iter().map(|d| d.id.as_str()).collect();
        card.queries
            .retain(|q| !dropped_ids.contains(q.id.as_str()));
        // The card is inside the content hash, so this is a new file identity —
        // correctly: the card is part of what the file says about itself. The
        // splice reproduces the writer's layout exactly, so the result is
        // byte-identical to a build that had generated this query set directly.
        bytes = rete_core::replace_metadata(&bytes, &card.to_json_bytes())?;
        eprintln!(
            "re-carded with the {} starter quer{} that answer (content hash follows the card)",
            card.queries.len(),
            if card.queries.len() == 1 { "y" } else { "ies" }
        );
    }
    Ok((bytes, buildinfo::query_costs(kept), dropped))
}

/// Attach the **build-info** section (kind 7, outside the content hash) to a
/// finished card-carrying image: timestamp, builder version, the parameters in
/// force, and — when `measure_costs` — each starter query's measured cost
/// (bytes / range requests / rows / reference ms), run cold against this very
/// image. Cardless builds skip this entirely, staying byte-identical to
/// pre-build-info output.
///
/// `measure_costs` is `--no-card-costs` inverted, and it now decides more than
/// whether a table of numbers is recorded: the same run is what proves each
/// starter query answers ([`drop_measured_empty_queries`]). Opting out of the
/// measurement therefore opts out of that protection — the card keeps whatever
/// the generator's static reasoning produced, unchecked.
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
    if !measure_costs {
        eprintln!(
            "--no-card-costs: the starter queries were NOT run, so nothing checked that they \
             answer on this file"
        );
    }
    let card = card::load_card(&bytes)?.filter(|c| measure_costs && !c.queries.is_empty());
    let bytes = match card {
        Some(card) => {
            let (bytes, costs, dropped) = drop_measured_empty_queries(bytes, card)?;
            info.query_costs = Some(costs);
            info.dropped_queries = dropped;
            bytes
        }
        None => bytes,
    };
    let blob = info.to_json_bytes();
    let out = rete_core::attach_build_info(&bytes, &blob)?;
    eprintln!(
        "embedded build info ({} bytes, outside the content hash)",
        blob.len()
    );
    Ok(out)
}

/// Open an input for streaming — a file path, or `-` for stdin — **transparently
/// gunzipped**. Detection is by content, not name: a gzip member starts with the
/// two magic bytes `1f 8b`, so `dump.ttl.gz`, a `.gz` that was renamed, and a
/// piped `gzip -dc` stream all do the right thing. `MultiGzDecoder` (not
/// `GzDecoder`) so a concatenation of members — what `cat *.gz` produces, and
/// what several public dumps ship as — reads through to the end instead of
/// stopping silently after the first member.
fn open_reader(path: &str) -> anyhow::Result<Box<dyn std::io::BufRead>> {
    Ok(open_reader_counted(path, None)?.0)
}

/// Counts the bytes pulled from an input — and, given a `limit`, reports EOF once
/// that many have been read. It sits **under** any gzip decoder, so the count is
/// of the file as it exists on disk: the same units as `fs::metadata().len()`,
/// which is what `estimate --sample-mb` extrapolates against. Counting the
/// decompressed side instead would make a sampled ratio silently wrong by the
/// compression factor.
struct CountedRead<R> {
    inner: R,
    counter: std::rc::Rc<std::cell::Cell<u64>>,
    limit: Option<u64>,
}

impl<R: std::io::Read> std::io::Read for CountedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.limit.is_some_and(|l| self.counter.get() >= l) {
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        self.counter.set(self.counter.get() + n as u64);
        Ok(n)
    }
}

/// How many input bytes a reader from [`open_reader_counted`] has consumed —
/// shared with the caller so it can watch progress against a sampling limit.
pub(crate) type ByteCounter = std::rc::Rc<std::cell::Cell<u64>>;

/// [`open_reader`] plus a live byte counter (and an optional early-EOF limit).
pub(crate) fn open_reader_counted(
    path: &str,
    limit: Option<u64>,
) -> anyhow::Result<(Box<dyn std::io::BufRead>, ByteCounter)> {
    use std::io::BufRead;
    let inner: Box<dyn std::io::Read> = if path == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?)
    };
    let counter = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let counted = CountedRead {
        inner,
        counter: counter.clone(),
        limit,
    };
    let mut buf = std::io::BufReader::with_capacity(1 << 20, counted);
    let gzipped = {
        let head = buf.fill_buf().map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
        head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b
    };
    if gzipped {
        Ok((
            Box::new(std::io::BufReader::with_capacity(
                1 << 20,
                flate2::read::MultiGzDecoder::new(buf),
            )),
            counter,
        ))
    } else {
        Ok((Box::new(buf), counter))
    }
}

/// Read an input source whole: a file path, or `-` for stdin. Gunzips
/// transparently, like [`open_reader`].
fn read_input(path: &str) -> anyhow::Result<String> {
    use std::io::Read;
    let mut s = String::new();
    open_reader(path)?
        .read_to_string(&mut s)
        .map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    Ok(s)
}

/// The parser to use for an input: explicit `--format` wins, else by extension,
/// else (no extension / stdin) N-Triples. A trailing `.gz` is stripped before
/// the extension is read, so `authors.ttl.gz` is Turtle — the compression is
/// detected from the bytes ([`open_reader`]), never from the name.
pub(crate) fn input_format(path: &str, override_fmt: Option<&str>) -> &'static str {
    if let Some(f) = override_fmt {
        return match f {
            "nq" => "nq",
            "ttl" => "ttl",
            "trig" => "trig",
            "rdfxml" | "rdf" | "owl" | "xml" => "rdfxml",
            _ => "nt",
        };
    }
    let lower = path.to_ascii_lowercase();
    let p = lower
        .strip_suffix(".gz")
        .or_else(|| lower.strip_suffix(".gzip"))
        .unwrap_or(lower.as_str());
    if p.ends_with(".nq") || p.ends_with(".nquads") {
        "nq"
    } else if p.ends_with(".ttl") || p.ends_with(".turtle") {
        "ttl"
    } else if p.ends_with(".trig") {
        "trig"
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
        // (~64 B/line) to avoid Vec doublings; for a compressed input that hint is
        // the *compressed* length, which under-shoots harmlessly. stdin and the
        // structured syntaxes take the text path below.
        let parsed = if input != "-" && (fmt == "nt" || fmt == "nq") {
            let cap = std::fs::metadata(input)
                .map(|m| (m.len() / 64) as usize)
                .unwrap_or(0);
            ingest::parse_reader(open_reader(input)?, fmt, cap)
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
    collapse_graphs: bool,
    card_args: CardArgs,
    no_card_costs: bool,
    perms: rete_core::PermSet,
) -> anyhow::Result<()> {
    // Fast low-RAM path: when every input is an N-Triples / N-Quads FILE and no
    // reasoning is requested, assemble by STREAMING the inputs twice instead of
    // materializing every parsed quad. Peak RAM drops from the (huge, heavily
    // duplicated) string-quad multiset to just the dictionary + id-triples +
    // index — the difference between a ~44 GB and a ~6 GB build on an 88 M-triple
    // graph. Output is byte-identical to the in-memory path. `--materialize` /
    // `--reason` need the whole quad set resident (to run the reasoner) and stdin
    // can't be re-read, so those fall through to the in-memory path below.
    //
    // **Turtle and TriG cannot take this path, even though they now stream.**
    // The assembler parses the input TWICE — once to observe every term, once to
    // encode — and oxttl labels anonymous blank nodes (`[ … ]`, collections) with
    // a fresh random id on each parse. The second pass would therefore present
    // terms the first pass never observed. The external build reads its input
    // exactly once, so it has no such constraint and does accept them.
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
                let rd = open_reader(path)
                    .map_err(|e| ingest::IngestError::Io(format!("{path}: {e}")))?;
                if collapse_graphs {
                    let mut flatten = |q: ingest::RawQuad| visit((q.0, q.1, q.2, None));
                    ingest::stream_reader(rd, fmt, &mut flatten)?;
                } else {
                    ingest::stream_reader(rd, fmt, visit)?;
                }
            }
            Ok(())
        };
        let (bytes, stats) = ingest::assemble_dataset_streaming_with_perms(
            stream,
            !no_pyramid,
            text_index,
            type_predicate,
            pyramid_algo,
            perms,
            |stats, dict, triples| match curated {
                Some(curated) => {
                    // Derive NOW (the dictionary and id-triples are resident and
                    // about to be consumed), stamp the counts LATER: the number
                    // of quads the file actually holds is only known once the
                    // indexes have deduplicated the input, and a card that
                    // reports the ingested count instead over-states every
                    // dataset built from overlapping harvest pages.
                    let card = card::derive_card_encoded(
                        dict,
                        triples,
                        stats.statements as u64,
                        stats.terms as u64,
                        stats.named_graphs as u64,
                        curated,
                    );
                    rete_core::ingest::DeferredMetadata::new(move |counts| {
                        let blob = card.with_final_counts(counts).to_json_bytes();
                        eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
                        blob
                    })
                }
                None => rete_core::ingest::DeferredMetadata::none(),
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
    // 1a. `--collapse-graphs`: drop the graph term so everything lands in the
    // default graph. Done before reasoning, which only sees the default graph —
    // so collapsing a named-graph dump is also what makes it reasonable over.
    if collapse_graphs {
        for q in quads.iter_mut() {
            q.3 = None;
        }
    }

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
    let (bytes, stats) = ingest::assemble_dataset_with_perms(
        quads,
        !no_pyramid,
        text_index,
        type_predicate,
        pyramid_algo,
        perms,
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
                // See the streaming path: the counts are stamped once the
                // indexes have deduplicated the input.
                rete_core::ingest::DeferredMetadata::new(move |counts| {
                    let blob = dataset_card.with_final_counts(counts).to_json_bytes();
                    eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
                    blob
                })
            }
            None => rete_core::ingest::DeferredMetadata::none(),
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
/// Constraints (explicit errors): default graph only — pass `--collapse-graphs`
/// for an input whose data lives in named graphs — no pyramid, no text index, no
/// reasoning. `--card` embeds curated fields + counts (the derived profile lists
/// need unbounded RAM, so they are omitted here). Every input syntax streams,
/// including gzipped ones, so the source never has to be decompressed to disk.
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
    collapse_graphs: bool,
    card_args: CardArgs,
    perms: rete_core::PermSet,
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
             requires an explicit --format nt|nq|ttl|trig"
        );
    }
    // Every syntax except RDF/XML streams from a reader, so the external build
    // accepts them all — `.ttl.gz` and `.trig.gz` included, decompressed on the
    // fly. RDF/XML is the one holdout (its reader parser is not incremental
    // enough to be worth the surprise on a multi-GB input).
    if let Some((bad, fmt)) = inputs_fmt
        .iter()
        .find(|(i, f)| *i != "-" && !matches!(*f, "nt" | "nq" | "ttl" | "trig"))
    {
        anyhow::bail!(
            "--memory-budget-mb streams N-Triples/N-Quads/Turtle/TriG only \
             ({bad} is {fmt}); convert RDF/XML inputs to .ttl or .nt first"
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
                        let q = if collapse_graphs {
                            (q.0, q.1, q.2, None)
                        } else {
                            q
                        };
                        if let Err(e) = visit(q) {
                            err = Some(e);
                        }
                    }
                };
                let rd = open_reader(path).map_err(|e| {
                    rete_core::extbuild::ExtBuildError::Ingest(ingest::IngestError::Io(
                        e.to_string(),
                    ))
                })?;
                let res = ingest::stream_reader(rd, fmt, &mut on_quad);
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
            perms,
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
    // Rebuilding must not silently CHANGE what the file carries: a three-
    // permutation input stays three-permutation, a six stays six. `repyramid`
    // has no `--permutations` of its own for the same reason it has no
    // `--no-pyramid` — it re-assembles an existing file, it does not re-decide
    // how it was built. Rebuild from source to change the set.
    let perms = rete.header().perms;
    if perms != rete_core::PermSet::ALL {
        eprintln!(
            "preserving the input's {} permutation(s): {}",
            perms.len(),
            perms.names().join(", ")
        );
    }
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
    let (out_bytes, stats) = ingest::assemble_dataset_with_perms(
        quads,
        true,
        text_index,
        type_predicate,
        pyramid_algo,
        perms,
        |stats, quads| match curated {
            Some(curated) => {
                let card = card::derive_card(
                    quads,
                    stats.terms as u64,
                    stats.named_graphs as u64,
                    curated,
                );
                rete_core::ingest::DeferredMetadata::new(move |counts| {
                    let blob = card.with_final_counts(counts).to_json_bytes();
                    eprintln!("embedded dataset card ({} bytes of metadata)", blob.len());
                    blob
                })
            }
            None => rete_core::ingest::DeferredMetadata::none(),
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

    /// Build `nt` with a card, at a unique path, and return the file image.
    /// `measure` is `--no-card-costs` inverted.
    fn build_carded(tag: &str, nt: &str, measure: bool) -> Vec<u8> {
        let pid = std::process::id();
        let dir = std::env::temp_dir();
        let inp = dir.join(format!("rete_{tag}_{pid}.nt"));
        let out = dir.join(format!("rete_{tag}_{pid}.rete"));
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
            false,
            CardArgs {
                enabled: true,
                title: Some("measured".into()),
                ..Default::default()
            },
            !measure,
            rete_core::PermSet::ALL,
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        std::fs::remove_file(&inp).ok();
        std::fs::remove_file(&out).ok();
        bytes
    }

    fn card_of(bytes: &[u8]) -> card::DatasetCard {
        card::load_card(bytes).unwrap().expect("card embedded")
    }

    fn build_info_of(bytes: &[u8]) -> buildinfo::BuildInfo {
        buildinfo::BuildInfo::from_json_bytes(&rete_core::read_build_info(bytes).unwrap().unwrap())
            .unwrap()
    }

    /// A graph in which every IRI object is also a subject, so `top-dangling`
    /// ("referenced but never described") has nothing to find — while
    /// `card.in_hubs` is non-empty, so the generator's static `provably_empty`
    /// hook does **not** fire and the query is emitted.
    const NO_DANGLING_IRI: &str = "\
<http://x/a> <http://x/p> <http://x/b> .
<http://x/b> <http://x/p> <http://x/a> .
<http://x/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://x/C> .
<http://x/b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://x/C> .
<http://x/C> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://x/C> .
";

    /// The point of the whole change: a build that would have emitted a query
    /// answering zero rows **does not ship it**, because the build ran it.
    ///
    /// `top-dangling` is the case the static machinery cannot close — its
    /// template declares itself `NonEmpty::Undecidable` precisely because the
    /// card does not record which objects are also subjects. Measurement does
    /// not need to know: it ran the query.
    #[test]
    fn a_starter_query_measured_at_zero_rows_is_not_shipped() {
        let bytes = build_carded("drop_empty", NO_DANGLING_IRI, true);
        let card = card_of(&bytes);
        let info = build_info_of(&bytes);

        assert!(
            !card.queries.iter().any(|q| q.id == "top-dangling"),
            "the card must not ship a query the build measured at zero rows"
        );
        let dropped = &info.dropped_queries;
        assert_eq!(
            dropped.len(),
            1,
            "exactly one query was useless: {dropped:?}"
        );
        assert_eq!(dropped[0].id, "top-dangling");
        assert!(dropped[0].why.contains("0 rows"), "{:?}", dropped[0].why);
        assert!(
            !dropped[0].contradicts_claim,
            "its template declared itself undecidable, so this is news, not a defect"
        );

        // What remains is measured, kept in step with the card, and non-empty.
        let costs = info.query_costs.expect("costs measured");
        let ids: Vec<&str> = costs.queries.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            card.queries
                .iter()
                .map(|q| q.id.as_str())
                .collect::<Vec<_>>(),
            "the cost table stays aligned with the queries the card ships"
        );
        for c in &costs.queries {
            assert!(c.rows > 0, "{} shipped with {} rows", c.id, c.rows);
        }
        // The card moved, so the identity did: the file still verifies against
        // its own (new) hash.
        assert!(rete_core::verify(&bytes).unwrap());
    }

    /// The other half a run can see and no row count can: `sp-bbox` conjoins
    /// `wgs:lat` and `wgs:long` on one subject while its gate tallies them
    /// independently. When they sit on different subjects the un-grouped
    /// aggregate still returns exactly one row — of four unbound variables. Its
    /// template says the card "cannot do better"; the measurement can.
    #[test]
    fn a_starter_query_measured_as_a_row_binding_nothing_is_not_shipped() {
        const LAT_AND_LONG_APART: &str = "\
<http://x/a> <http://www.w3.org/2003/01/geo/wgs84_pos#lat> \"46.5\" .
<http://x/b> <http://www.w3.org/2003/01/geo/wgs84_pos#long> \"6.6\" .
<http://x/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://x/C> .
<http://x/b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://x/C> .
";
        let bytes = build_carded("drop_vacuous", LAT_AND_LONG_APART, true);
        let card = card_of(&bytes);
        let info = build_info_of(&bytes);

        assert!(
            !card.queries.iter().any(|q| q.id == "sp-bbox"),
            "a one-row answer that binds nothing is not an answer"
        );
        let d = info
            .dropped_queries
            .iter()
            .find(|d| d.id == "sp-bbox")
            .expect("sp-bbox dropped");
        assert!(d.why.contains("binding no variable"), "{}", d.why);
        assert!(
            !d.contradicts_claim,
            "NonEmpty::Aggregate promises a row, not a bound one — no claim is broken"
        );
    }

    /// **No churn.** Running the starter queries is an observation, not an
    /// edit: on a dataset where they all answer, the file the build writes is
    /// byte-identical to the one it writes when the measurement is skipped
    /// entirely (`--no-card-costs`), build-info aside. Nothing about the drop
    /// path can perturb a healthy build.
    #[test]
    fn measuring_does_not_touch_a_file_whose_queries_all_answer() {
        // Plain graph: `<http://x/C>` is referenced but never described, so
        // even `top-dangling` answers.
        let nt = "<http://x/a> <http://x/p> <http://x/b> .\n\
                  <http://x/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://x/C> .\n";
        let measured = build_carded("nochurn_yes", nt, true);
        let unmeasured = build_carded("nochurn_no", nt, false);

        assert!(
            build_info_of(&measured).dropped_queries.is_empty(),
            "nothing to drop on this graph"
        );
        assert_eq!(
            rete_core::attach_build_info(&measured, &[]).unwrap(),
            rete_core::attach_build_info(&unmeasured, &[]).unwrap(),
            "measuring must not change a single byte when nothing is dropped"
        );
        // …and it really did measure.
        let costs = build_info_of(&measured).query_costs.expect("measured");
        assert!(costs.queries.iter().all(|c| c.rows > 0));
        assert!(build_info_of(&unmeasured).query_costs.is_none());
    }

    /// `--no-card-costs` skips the run, so it also skips the protection the run
    /// provides: the same graph keeps the empty query. That is the flag's cost,
    /// stated as a test so it cannot drift into a surprise.
    #[test]
    fn no_card_costs_opts_out_of_the_emptiness_check_too() {
        let bytes = build_carded("optout", NO_DANGLING_IRI, false);
        assert!(
            card_of(&bytes)
                .queries
                .iter()
                .any(|q| q.id == "top-dangling"),
            "without the measurement the generator's static reasoning is all there is"
        );
        assert!(build_info_of(&bytes).dropped_queries.is_empty());
    }

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
                false,
                CardArgs {
                    enabled: true,
                    title: Some("BI test".into()),
                    ..Default::default()
                },
                false,
                rete_core::PermSet::ALL,
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
            false,
            CardArgs::default(),
            false,
            rete_core::PermSet::ALL,
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
            false,
            CardArgs::default(),
            false,
            rete_core::PermSet::ALL,
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
            false,
            CardArgs::default(),
            false,
            rete_core::PermSet::ALL,
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
