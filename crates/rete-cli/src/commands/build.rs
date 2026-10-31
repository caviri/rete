//! The ingest group: parse RDF inputs (N-Triples / N-Quads / Turtle) and either
//! `build` them into a `.rete` dataset or `validate` that they parse. Parsing
//! and assembly live in `rete_core::ingest` (shared with the wasm bindings);
//! this module adds the CLI-only concerns: file/stdin IO, format detection by
//! extension, `--materialize`, and the Dataset Card flags.

use crate::commands::buildinfo;
use crate::commands::card::{self, CardArgs};
use crate::commands::progress;
use rete_core::build_progress::{self as live, Stat, Unit};
use rete_core::build_trace::Phase;
use rete_core::ingest;
use rete_core::{IntoTextIndex, TextIndexScope};

/// The optional sections added after the generation-1 freeze that a build
/// writes: the LABEL_INDEX (kind 8, `--label-index`) and the SCHEMA (kind 9,
/// on unless `--no-schema`). Whatever is chosen, `rete build` keeps the labels
/// and the schema out of the pyramid-meta, which carries only the community
/// summary and descriptors.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BuildSections {
    /// `--label-index`.
    pub label_index: bool,
    /// Not `--no-schema`.
    pub schema: bool,
}

impl Default for BuildSections {
    /// What `rete build` writes with neither flag: the SCHEMA section, no
    /// label index.
    fn default() -> Self {
        BuildSections {
            label_index: false,
            schema: true,
        }
    }
}

impl BuildSections {
    /// The in-memory assembly options for these sections.
    #[allow(clippy::too_many_arguments)]
    fn assemble_options<'a>(
        self,
        with_pyramid: bool,
        text_index: Option<&'a TextIndexScope>,
        type_override: Option<&'a str>,
        algo: rete_core::PyramidAlgo,
        perms: rete_core::PermSet,
        graphs: rete_core::GraphLayout,
    ) -> ingest::AssembleOptions<'a> {
        ingest::AssembleOptions {
            with_pyramid,
            with_text_index: text_index.is_some(),
            text_index_scope: text_index,
            type_override,
            algo,
            perms,
            graph_layout: graphs,
            pyramid_layout: rete_core::PyramidLayout::CommunityOnly,
            label_index: self.label_index,
            schema: self.schema,
        }
    }
}

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

    let (kept, dropped) = buildinfo::keep_answering(&mut card, measured);
    if !dropped.is_empty() {
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

/// Every flag of a build that shapes the file, as the build record keeps it —
/// assembled from the build function's own arguments, so the record cannot
/// disagree with what the build was asked to do.
pub(crate) struct RecordFlags<'a> {
    /// `build`, `build --memory-budget-mb` or `repyramid`.
    pub command: &'a str,
    pub format: Option<&'a str>,
    pub no_pyramid: bool,
    pub pyramid_algo: rete_core::PyramidAlgo,
    pub text_index: Option<&'a TextIndexScope>,
    pub materialize: bool,
    pub reason: bool,
    pub sections: BuildSections,
    pub perms: rete_core::PermSet,
    pub graphs: rete_core::GraphLayout,
    pub collapse_graphs: bool,
    pub strict: bool,
    pub surface: ingest::QuotedTripleSurface,
    pub type_predicate: Option<&'a str>,
    pub measure_costs: bool,
    pub memory_budget_mb: Option<u64>,
    pub card_args: &'a CardArgs,
}

impl RecordFlags<'_> {
    /// The record's `params`. `codec` is the finished file's own.
    pub(crate) fn params(&self, codec: Option<u8>) -> buildinfo::BuildParams {
        let mut params = buildinfo::BuildParams {
            command: Some(self.command.to_string()),
            format: self.format.map(str::to_string),
            no_pyramid: self.no_pyramid,
            label_index: self.sections.label_index,
            no_schema: !self.sections.schema,
            materialize: self.materialize,
            reason: self.reason,
            pyramid_algo: (!self.no_pyramid).then(|| {
                match self.pyramid_algo {
                    rete_core::PyramidAlgo::Types => "types",
                    _ => "louvain",
                }
                .to_string()
            }),
            memory_budget_mb: self.memory_budget_mb,
            codec: codec.map(|c| buildinfo::codec_name(c).to_string()),
            card_top_n: Some(card::CARD_TOP_N as u32),
            write: buildinfo::write_record(),
            permutations: Some(self.perms.len() as u8),
            named_graphs: Some(
                match self.graphs {
                    rete_core::GraphLayout::Quad => "quad",
                    _ => "per-graph",
                }
                .to_string(),
            ),
            collapse_graphs: Some(self.collapse_graphs),
            strict: Some(self.strict),
            quoted_triple_syntax: Some(
                match self.surface {
                    ingest::QuotedTripleSurface::Rdf12 => "rdf12",
                    _ => "rdf-star",
                }
                .to_string(),
            ),
            type_predicate: self.type_predicate.map(str::to_string),
            card_costs: Some(self.measure_costs),
            card_file: card::card_file_digest(self.card_args),
            ..Default::default()
        };
        params.set_text_index(self.text_index);
        if matches!(self.command, "repyramid" | "merge") {
            // They re-assemble existing files: these flags do not exist there,
            // and "false" would claim a choice nobody made.
            params.collapse_graphs = None;
            params.strict = None;
            params.quoted_triple_syntax = None;
        }
        params
    }
}

/// Write a finished card-carrying image to `output` with its **build record**
/// (the kind-7 build-info section, outside the content hash): the flags, the
/// engine and host, the inputs and their digests, the statement counts, and —
/// when `flags.measure_costs` — each starter query's measured cost (bytes /
/// range requests / rows / reference ms), run cold against this very image.
/// Cardless builds skip this entirely, staying byte-identical to pre-build-info
/// output.
///
/// The record is written last-but-one: the image goes to disk with the record's
/// widest form reserved ([`buildinfo::BuildInfo::widest`]), and the sealed
/// record — whose wall time and peak RSS now cover the write — is patched into
/// that reservation in place.
///
/// `measure_costs` is `--no-card-costs` inverted, and it decides more than
/// whether a table of numbers is recorded: the same run is what proves each
/// starter query answers ([`drop_measured_empty_queries`]). Opting out of the
/// measurement therefore opts out of that protection — the card keeps whatever
/// the generator's static reasoning produced, unchecked.
#[allow(clippy::too_many_arguments)]
fn write_carded(
    output: &str,
    bytes: Vec<u8>,
    flags: &RecordFlags<'_>,
    rec: &buildinfo::Recorder,
    stats: &ingest::BuildStats,
    read: u64,
    inferred: Option<u64>,
    iri: &rete_core::IriReport,
) -> anyhow::Result<usize> {
    let header = rete_core::Header::from_bytes(&bytes)?;
    let mut info = buildinfo::new_build_info(flags.params(Some(header.dict_codec)));
    rec.begin(&mut info);
    if !flags.measure_costs {
        eprintln!(
            "--no-card-costs: the starter queries were NOT run, so nothing checked that they \
             answer on this file"
        );
    }
    let card = card::load_card(&bytes)?.filter(|c| flags.measure_costs && !c.queries.is_empty());
    let bytes = match card {
        Some(card) => {
            live::begin(
                Phase::StarterQueries,
                Some(card.queries.len() as u64),
                Unit::Queries,
            );
            let (bytes, costs, dropped) = drop_measured_empty_queries(bytes, card)?;
            info.query_costs = Some(costs);
            info.dropped_queries = dropped;
            rec.lap(rete_core::build_trace::Phase::StarterQueries);
            bytes
        }
        None => bytes,
    };
    info.counts = Some(buildinfo::counts(stats, read, inferred, iri));
    info.inputs = rec.inputs();
    if rec.is_reproducible() {
        buildinfo::zero_debug_ms(&mut info);
    }
    let path = std::path::Path::new(output);
    let v06 = rete_core::v06::open::is_v06(&bytes);
    let reserve = if v06 && info.query_costs.is_some() {
        // 0x06: the costs are a property of the final layout, card region
        // included — reserve the record with every cost at its widest, measure
        // again on the image as it will be written, and seal into that.
        buildinfo::with_widest_costs(&info).reserve_bytes(buildinfo::Pending::Write, rec)
    } else {
        info.reserve_bytes(buildinfo::Pending::Write, rec)
    };
    live::begin(Phase::Write, None, Unit::None);
    let len = if v06 {
        let image = std::sync::Arc::new(rete_core::attach_build_info(&bytes, &reserve)?);
        drop(bytes);
        if let (Some(costs), Some(card)) = (info.query_costs.as_mut(), card::load_card(&image)?) {
            let source: std::sync::Arc<dyn rete_core::RangeReader + Send + Sync> =
                std::sync::Arc::new(rete_core::BytesReader(std::sync::Arc::from(&image[..])));
            buildinfo::remeasure_costs(source, &card.queries, costs);
            if rec.is_reproducible() {
                buildinfo::zero_debug_ms(&mut info);
            }
        }
        std::fs::write(path, &*image)?;
        image.len() as u64
    } else {
        let len = buildinfo::write_image_with_build_info(path, &bytes, &reserve)?;
        drop(bytes);
        len
    };
    rec.lap(rete_core::build_trace::Phase::Write);
    rec.seal(&mut info);
    let blob = info.to_json_bytes();
    buildinfo::patch_build_info(path, &blob)?;
    eprintln!(
        "embedded build info ({} bytes in a {}-byte section, outside the content hash)",
        blob.len(),
        reserve.len()
    );
    Ok(len as usize)
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
    /// The build record's running BLAKE3 of the input — fed here, under the
    /// gzip decoder, so it is the digest of the file as stored, in the one pass
    /// the build makes anyway.
    hasher: Option<std::rc::Rc<std::cell::RefCell<blake3::Hasher>>>,
}

impl<R: std::io::Read> std::io::Read for CountedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.limit.is_some_and(|l| self.counter.get() >= l) {
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        self.counter.set(self.counter.get() + n as u64);
        // `--progress`: a parse step's done, in input bytes as stored.
        live::advance(n as u64);
        if let Some(h) = &self.hasher {
            h.borrow_mut().update(&buf[..n]);
        }
        Ok(n)
    }
}

/// How many input bytes a reader from [`open_reader_counted`] has consumed —
/// shared with the caller so it can watch progress against a sampling limit.
pub(crate) type ByteCounter = std::rc::Rc<std::cell::Cell<u64>>;

/// [`open_reader_tapped`] plus a live byte counter (and an optional early-EOF limit).
pub(crate) fn open_reader_counted(
    path: &str,
    limit: Option<u64>,
) -> anyhow::Result<(Box<dyn std::io::BufRead>, ByteCounter)> {
    open_reader_inner(path, limit, None)
}

/// Open an input for streaming — a file path, or `-` for stdin — **transparently
/// gunzipped**. Detection is by content, not name: a gzip member starts with the
/// two magic bytes `1f 8b`, so `dump.ttl.gz`, a `.gz` that was renamed, and a
/// piped `gzip -dc` stream all do the right thing. `MultiGzDecoder` (not
/// `GzDecoder`) so a concatenation of members — what `cat *.gz` produces, and
/// what several public dumps ship as — reads through to the end instead of
/// stopping silently after the first member.
///
/// Given a build-record [`buildinfo::InputTap`], the bytes read and their
/// BLAKE3 accumulate in the tap as the input streams (under the decoder, so of
/// the file as stored), and the tap learns whether the input was gzip.
pub(crate) fn open_reader_tapped(
    path: &str,
    tap: Option<&buildinfo::InputTap>,
) -> anyhow::Result<Box<dyn std::io::BufRead>> {
    Ok(open_reader_inner(path, None, tap)?.0)
}

fn open_reader_inner(
    path: &str,
    limit: Option<u64>,
    tap: Option<&buildinfo::InputTap>,
) -> anyhow::Result<(Box<dyn std::io::BufRead>, ByteCounter)> {
    use std::io::BufRead;
    let inner: Box<dyn std::io::Read> = if path == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?)
    };
    let counter = tap
        .map(|t| t.bytes.clone())
        .unwrap_or_else(|| std::rc::Rc::new(std::cell::Cell::new(0u64)));
    let counted = CountedRead {
        inner,
        counter: counter.clone(),
        limit,
        hasher: tap.map(|t| t.hasher.clone()),
    };
    let mut buf = std::io::BufReader::with_capacity(1 << 20, counted);
    let gzipped = {
        let head = buf.fill_buf().map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
        head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b
    };
    if let Some(t) = tap {
        t.gzip.set(gzipped);
    }
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
/// transparently, like [`open_reader_tapped`].
fn read_input(path: &str, tap: Option<&buildinfo::InputTap>) -> anyhow::Result<String> {
    use std::io::Read;
    let mut s = String::new();
    open_reader_tapped(path, tap)?
        .read_to_string(&mut s)
        .map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    Ok(s)
}

/// The parser to use for an input: explicit `--format` wins, else by extension,
/// else (no extension / stdin) N-Triples. A trailing `.gz` is stripped before
/// the extension is read, so `authors.ttl.gz` is Turtle — the compression is
/// detected from the bytes ([`open_reader_tapped`]), never from the name.
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

/// What `--strict` refused, and the three ways forward. Printed under the
/// parser's own message, which already names the offending IRI and the rule it
/// breaks.
const STRICT_HINT: &str = "hint: `--strict` refuses input carrying IRIs the N-Triples/N-Quads \
     IRIREF grammar (RFC 3987) disallows.\n      Build without it to store them verbatim — the \
     build then reports how many it ingested — or repair the source.\n      `rete export \
     --sanitize-iris` percent-encodes the repairable classes on the way out.";

/// The invalid-IRI audit a build runs: counting by default, refusing under
/// `--strict`.
fn iri_audit(strict: bool) -> ingest::IriAudit {
    if strict {
        ingest::IriAudit::refusing()
    } else {
        ingest::IriAudit::counting()
    }
}

/// Present an ingest error, adding the `--strict` explanation when the refusal
/// came from the invalid-IRI audit rather than from a malformed line.
/// `IngestError::InvalidIri` is only ever produced by a strict audit, so the
/// flag itself need not be threaded here.
fn ingest_error(input: Option<&str>, e: ingest::IngestError) -> anyhow::Error {
    let where_ = input.map(|i| format!("{i}: ")).unwrap_or_default();
    match e {
        ingest::IngestError::InvalidIri { .. } => anyhow::anyhow!("{where_}{e}\n{STRICT_HINT}"),
        _ => anyhow::anyhow!("{where_}{e}"),
    }
}

/// Parse one or more RDF inputs into quads (triples → default graph). Shared by
/// `build` and `validate`. Returns a parse error (which input, what went wrong)
/// if any input is malformed.
///
/// `audit` is the invalid-IRI tally (see `rete_core::iri`): every input feeds the
/// same one, so a multi-input build reports a single total. With
/// `audit.strict` the first offending statement aborts the parse.
/// `surface` decides what a Turtle/TriG `<< s p o >>` means — a quoted triple
/// or an RDF 1.2 reifier. It reaches the line-based readers too and is ignored
/// there by design: rete's own N-Triples/N-Quads tokenizer takes both spellings
/// and RDF 1.2 N-Triples has no reifier syntax, so there is nothing to choose.
fn parse_inputs(
    inputs: &[String],
    format: Option<&str>,
    audit: &mut ingest::IriAudit,
    surface: ingest::QuotedTripleSurface,
    taps: &[buildinfo::InputTap],
) -> anyhow::Result<Vec<ingest::RawQuad>> {
    let mut quads: Vec<ingest::RawQuad> = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let tap = taps.get(i);
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
            ingest::parse_reader_audited(open_reader_tapped(input, tap)?, fmt, cap, Some(audit))
                .map_err(|e| ingest_error(Some(input), e))?
        } else {
            let text = read_input(input, tap)?;
            ingest::parse_statements_audited_surface(&text, fmt, Some(audit), surface).map_err(
                |e| {
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
                        ingest_error(Some(input), e)
                    }
                },
            )?
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
pub(crate) fn validate(
    inputs: &[String],
    format: Option<&str>,
    strict: bool,
    surface: ingest::QuotedTripleSurface,
) -> anyhow::Result<()> {
    let mut audit = iri_audit(strict);
    let quads = parse_inputs(inputs, format, &mut audit, surface, &[])?;
    let named: std::collections::BTreeSet<&String> =
        quads.iter().filter_map(|(_, _, _, g)| g.as_ref()).collect();
    let in_default = quads.iter().filter(|(_, _, _, g)| g.is_none()).count();
    println!(
        "valid: {} statement(s) — {} in the default graph, {} named graph(s)",
        quads.len(),
        in_default,
        named.len()
    );
    // "Parses" and "is valid RDF" are not the same claim, and `validate` is
    // exactly where the difference should be visible.
    crate::commands::iri_report::warn_after_build(&audit.report);
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
    text_index: impl IntoTextIndex,
    type_predicate: Option<&str>,
    collapse_graphs: bool,
    card_args: CardArgs,
    no_card_costs: bool,
    perms: rete_core::PermSet,
    graphs: rete_core::GraphLayout,
    strict: bool,
    surface: ingest::QuotedTripleSurface,
    sections: BuildSections,
) -> anyhow::Result<()> {
    let text_index = text_index.into_text_index();
    // Format 0x06 stores named graphs in the quad index only.
    let graphs = if super::v06::writes_legacy() {
        graphs
    } else {
        rete_core::GraphLayout::Quad
    };
    let inputs_fmt: Vec<(&str, &'static str)> = inputs
        .iter()
        .map(|i| (i.as_str(), input_format(i, format)))
        .collect();
    // The build record's clock and input taps start here, before any input is
    // opened. A cardless build carries no record, so it taps nothing: its
    // inputs are not hashed.
    let recording = card_args.requested() || reason;
    let rec = buildinfo::Recorder::start(if recording { &inputs_fmt[..] } else { &[] });
    let flags = RecordFlags {
        command: "build",
        format,
        no_pyramid,
        pyramid_algo,
        text_index: text_index.as_ref(),
        materialize,
        reason,
        sections,
        perms,
        graphs,
        collapse_graphs,
        strict,
        surface,
        type_predicate,
        measure_costs: !no_card_costs,
        memory_budget_mb: None,
        card_args: &card_args,
    };
    let opts = sections.assemble_options(
        !no_pyramid,
        text_index.as_ref(),
        type_predicate,
        pyramid_algo,
        perms,
        graphs,
    );
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
    progress::plan(in_ram_plan(
        streamable,
        materialize || reason,
        card_args.requested() || reason,
        !no_card_costs,
        !no_pyramid,
        text_index.is_some(),
        sections,
        perms,
    ));
    if streamable {
        let curated = if card_args.requested() {
            Some(card::load_curated(&card_args)?)
        } else {
            None
        };
        let card_requested = curated.is_some();
        // Invalid-IRI audit, shared across every input so the build reports one
        // total. The assembler reads the inputs TWICE (observe terms, then
        // encode), so the audit runs on the first pass only — auditing both
        // would double every count. `--strict` aborts from inside that first
        // pass, before any index work. The build record's input digests and
        // statement count are taken on that same first pass.
        let audit = std::cell::RefCell::new(iri_audit(strict));
        let pass = std::cell::Cell::new(0u32);
        let read = rec.statements_read.clone();
        // Re-readable source: each call re-opens and streams every input file. The
        // two-pass assembler invokes it once to observe terms, once to encode.
        let stream = |visit: &mut dyn FnMut(ingest::RawQuad)| -> Result<(), ingest::IngestError> {
            let first = pass.replace(pass.get() + 1) == 0;
            // Each pass reads every input once: its progress is in input bytes.
            live::set_total(progress::input_bytes(inputs), Unit::Bytes);
            let mut guard = audit.borrow_mut();
            let mut sink = if first { Some(&mut *guard) } else { None };
            for (i, (path, fmt)) in inputs_fmt.iter().enumerate() {
                let tap = if first { rec.taps.get(i) } else { None };
                let rd = open_reader_tapped(path, tap)
                    .map_err(|e| ingest::IngestError::Io(format!("{path}: {e}")))?;
                // No surface is threaded here on purpose: `streamable` above
                // admits only `nt`/`nq` files, whose reader takes both spellings
                // unconditionally. There is nothing for the flag to decide.
                let mut counted = |q: ingest::RawQuad| {
                    if first {
                        read.set(read.get() + 1);
                        if read.get() & 0xFFF == 0 {
                            live::set_stat(Stat::Statements, read.get());
                        }
                    }
                    if collapse_graphs {
                        visit((q.0, q.1, q.2, None))
                    } else {
                        visit(q)
                    }
                };
                ingest::stream_reader_audited(rd, fmt, sink.as_deref_mut(), &mut counted)?;
            }
            live::set_stat(Stat::Statements, read.get());
            Ok(())
        };
        let (bytes, stats) =
            ingest::assemble_dataset_streaming_opts(stream, &opts, |stats, dict, triples| {
                match curated {
                    Some(curated) => {
                        live::begin(Phase::CardDerive, None, Unit::None);
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
                }
            })
            .map_err(|e| ingest_error(None, e))?;
        let bytes = super::v06::finish(bytes)?;
        let audit = audit.into_inner();
        let len = if card_requested {
            write_carded(
                output,
                bytes,
                &flags,
                &rec,
                &stats,
                rec.statements_read.get(),
                None,
                &audit.report,
            )?
        } else {
            live::begin(Phase::Write, None, Unit::None);
            std::fs::write(output, &bytes)?;
            bytes.len()
        };
        progress::finish(stats.statements as u64, output);
        print_build_summary(output, &stats, len);
        crate::commands::iri_report::warn_after_build(&audit.report);
        return Ok(());
    }

    // 1. Parse every input into quads (triples → default graph, `None`).
    // Progress is in input bytes; they bound the parse only when every input
    // is a line-based file, parsed as it is read.
    let line_files = inputs
        .iter()
        .all(|i| i != "-" && matches!(input_format(i, format), "nt" | "nq"));
    live::begin(
        Phase::Parse,
        if line_files {
            progress::input_bytes(inputs)
        } else {
            None
        },
        Unit::Bytes,
    );
    let mut audit = iri_audit(strict);
    let mut quads = parse_inputs(inputs, format, &mut audit, surface, &rec.taps)?;
    let read = quads.len() as u64;
    live::set_stat(Stat::Statements, read);
    // 1a. `--collapse-graphs`: drop the graph term so everything lands in the
    // default graph. Done before reasoning, which only sees the default graph —
    // so collapsing a named-graph dump is also what makes it reasonable over.
    if collapse_graphs {
        for q in quads.iter_mut() {
            q.3 = None;
        }
    }
    rec.lap(rete_core::build_trace::Phase::Parse);

    // 1b. Reason over the default graph once if either flag needs it: `--materialize`
    // folds the inferred triples in (deduped later by the index builder, so they ship
    // in the file and need no query-time reasoning) and ABORTS on an incoherent graph
    // rather than baking in a contradiction; `--reason` stamps the verdict into the
    // card (recording `coherent: false` honestly, without aborting). `reason()`
    // materializes internally, so the verdict is the same whether computed here or
    // after the bake — coherence is invariant under adding entailments.
    let reasoning = if materialize || reason {
        live::begin(Phase::Reason, None, Unit::None);
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
    let inferred = materialize.then(|| (quads.len() as u64).saturating_sub(read));
    if reasoning.is_some() {
        rec.lap(rete_core::build_trace::Phase::Reason);
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
    let (bytes, stats) =
        ingest::assemble_dataset_opts(quads, &opts, |stats, quads| match curated {
            Some(curated) => {
                live::begin(Phase::CardDerive, None, Unit::None);
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
        });
    let bytes = super::v06::finish(bytes)?;
    let len = if card_requested {
        write_carded(
            output,
            bytes,
            &flags,
            &rec,
            &stats,
            read,
            inferred,
            &audit.report,
        )?
    } else {
        live::begin(Phase::Write, None, Unit::None);
        std::fs::write(output, &bytes)?;
        bytes.len()
    };
    progress::finish(stats.statements as u64, output);
    print_build_summary(output, &stats, len);
    crate::commands::iri_report::warn_after_build(&audit.report);
    Ok(())
}

/// The steps an in-RAM build takes, in order, for `--progress`.
#[allow(clippy::too_many_arguments)]
fn in_ram_plan(
    streamable: bool,
    reasoning: bool,
    card: bool,
    measure_costs: bool,
    pyramid: bool,
    text_index: bool,
    sections: BuildSections,
    perms: rete_core::PermSet,
) -> progress::Plan {
    let mut phases = if streamable {
        vec![Phase::ParseDictionary, Phase::ParseEncode]
    } else {
        let mut p = vec![Phase::Parse];
        if reasoning {
            p.push(Phase::Reason);
        }
        p.extend([Phase::Dictionary, Phase::Encode]);
        p
    };
    if card {
        phases.push(Phase::CardDerive);
    }
    if pyramid {
        phases.push(Phase::Pyramid);
    }
    if text_index {
        phases.push(Phase::TextIndex);
    }
    if sections.label_index || sections.schema {
        phases.push(Phase::LabelsSchema);
    }
    phases.extend([
        Phase::DictionarySection,
        Phase::IndexPermutations,
        Phase::Serialize,
    ]);
    if card && measure_costs {
        phases.push(Phase::StarterQueries);
    }
    phases.push(Phase::Write);
    progress::Plan {
        kind: if streamable {
            progress::Kind::InRamStream
        } else {
            progress::Kind::InRamText
        },
        phases,
        perms: perms.len(),
        label_index: sections.label_index,
    }
}

/// The steps of the memory-bounded external builder, in order, for
/// `--progress`: `rete build --memory-budget-mb` and `rete merge` (`read`
/// is the merge hashing its shards for the build record).
#[allow(clippy::too_many_arguments)]
pub(crate) fn external_plan(
    kind: progress::Kind,
    read: bool,
    named_graphs: bool,
    text_index: bool,
    sections: BuildSections,
    card: bool,
    measure_costs: bool,
    perms: rete_core::PermSet,
) -> progress::Plan {
    let mut phases = Vec::new();
    if read {
        phases.push(Phase::Read);
    }
    phases.push(Phase::ParseChunk);
    if rete_core::write_opts::frequent_objects() > 0 {
        phases.push(Phase::FrequentObjects);
    }
    phases.extend([Phase::Dictionary, Phase::Encode]);
    if text_index {
        phases.push(Phase::TextIndex);
    }
    phases.push(Phase::IndexPermutations);
    if named_graphs {
        phases.push(Phase::NamedGraphs);
    }
    phases.push(Phase::Write);
    if sections.label_index || sections.schema {
        phases.push(Phase::LabelsSchema);
    }
    if card {
        phases.push(Phase::CardDerive);
        if measure_costs {
            phases.push(Phase::StarterQueries);
        }
        phases.push(Phase::ContentHash);
    }
    progress::Plan {
        kind,
        phases,
        perms: perms.len(),
        label_index: sections.label_index,
    }
}

/// `rete build --memory-budget-mb <N>`: the **memory-bounded external build** —
/// chunk the input to disk, merge, and stream the final file, holding roughly the
/// budget in RAM regardless of graph size (`rete_core::extbuild`). Its sections
/// are byte-identical to a standard `--no-pyramid` build's of the same input,
/// the SCHEMA section and `--label-index` included (their passes are external
/// sorts sized from the same budget).
///
/// Named graphs are supported: the graph ordinal leads the external sort key, so
/// each graph comes out as one contiguous run and is indexed from that run alone
/// (`--collapse-graphs` still folds them into the default graph when that is what
/// you want).
///
/// `--text-index` (any scope) is a second external sort over the literal
/// words, inside the same budget, and writes the same TEXT_INDEX bytes as the
/// in-RAM build (`rete_core::text_index_spill`).
///
/// Constraints (explicit errors): no pyramid, no reasoning. Every input syntax
/// streams, including gzipped ones, so the source never has to be decompressed
/// to disk.
///
/// A card is the **full** card, derived the way `rete recard` derives it: once
/// the file is written, its sorted sections are streamed into the profile and
/// the starter queries are run against it ([`card::derive_full_card`], inside
/// the same budget), and the card and the build record are written **after the
/// data** ([`card::append_card`]) — the header is the directory, so a card need
/// not sit at the front, and putting it there would mean copying the file. The
/// content hash is the one the same card at the front would give: the hash
/// folds the card first, wherever it lies, so finishing it costs one read of
/// the file.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_external_cmd(
    inputs: &[String],
    output: &str,
    format: Option<&str>,
    memory_budget_mb: u64,
    tmp_dir: Option<&str>,
    materialize: bool,
    reason: bool,
    text_index: Option<TextIndexScope>,
    collapse_graphs: bool,
    card_args: CardArgs,
    measure_costs: bool,
    perms: rete_core::PermSet,
    graphs: rete_core::GraphLayout,
    strict: bool,
    surface: ingest::QuotedTripleSurface,
    type_predicate: Option<&str>,
    sections: BuildSections,
) -> anyhow::Result<()> {
    if materialize || reason {
        anyhow::bail!(
            "--memory-budget-mb is incompatible with --materialize/--reason \
             (they need the whole graph resident); run them via the standard build"
        );
    }
    // Format 0x06 stores named graphs in the quad index only.
    let graphs = if super::v06::writes_legacy() {
        graphs
    } else {
        rete_core::GraphLayout::Quad
    };
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

    let rec = buildinfo::Recorder::start(if card_args.requested() {
        &inputs_fmt[..]
    } else {
        &[]
    });
    let curated = if card_args.requested() {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    // Build conditions for the external build. What is known up front goes in
    // now; the rest — digests, counts, spill, the card's costs, timings — once
    // the file is complete. The record is written after the data with the
    // card, sealed, so nothing is reserved during the stream.
    let flags = RecordFlags {
        command: "build --memory-budget-mb",
        format,
        no_pyramid: true,
        pyramid_algo: rete_core::PyramidAlgo::Louvain,
        text_index: text_index.as_ref(),
        materialize: false,
        reason: false,
        sections,
        perms,
        graphs,
        collapse_graphs,
        strict,
        surface,
        type_predicate,
        measure_costs,
        memory_budget_mb: Some(memory_budget_mb),
        card_args: &card_args,
    };
    let mut info = curated.is_some().then(|| {
        let mut info = buildinfo::new_build_info(flags.params(None));
        rec.begin(&mut info);
        info
    });
    let out_path = std::path::Path::new(output).to_path_buf();
    // Named graphs come from N-Quads and TriG unless they are folded away.
    let named = !collapse_graphs && inputs_fmt.iter().any(|(_, f)| matches!(*f, "nq" | "trig"));
    progress::plan(external_plan(
        progress::Kind::Bounded,
        false,
        named,
        text_index.is_some(),
        sections,
        curated.is_some(),
        measure_costs,
        perms,
    ));
    // The external build reads its input exactly once, so — unlike the two-pass
    // streaming assembler — the audit needs no pass guard.
    let audit = std::cell::RefCell::new(iri_audit(strict));
    let read = rec.statements_read.clone();
    let stats = rete_core::extbuild::build_external(
        |visit| {
            live::set_total(progress::input_bytes(inputs), Unit::Bytes);
            let mut guard = audit.borrow_mut();
            for (i, (path, fmt)) in inputs_fmt.iter().enumerate() {
                let mut err: Option<rete_core::extbuild::ExtBuildError> = None;
                let mut on_quad = |q: ingest::RawQuad| {
                    if err.is_none() {
                        read.set(read.get() + 1);
                        if read.get() & 0xFFF == 0 {
                            live::set_stat(Stat::Statements, read.get());
                        }
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
                let rd = open_reader_tapped(path, rec.taps.get(i)).map_err(|e| {
                    rete_core::extbuild::ExtBuildError::Ingest(ingest::IngestError::Io(
                        e.to_string(),
                    ))
                })?;
                let res = ingest::stream_reader_audited_surface(
                    rd,
                    fmt,
                    Some(&mut *guard),
                    surface,
                    &mut on_quad,
                );
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
            // The card is derived from the finished file and appended after
            // it (below), so the stream writes neither a card nor a record.
            metadata: Box::new(|_| Vec::new()),
            build_info: Vec::new(),
            perms,
            label_index: sections.label_index,
            schema: sections.schema,
            type_override: type_predicate.map(str::to_string),
            text_index,
            graphs,
            // A parser's buffers are small: the chunks get the whole budget.
            input_memory: 0,
        },
    )
    .map_err(|e| match e {
        rete_core::extbuild::ExtBuildError::Ingest(inner) => ingest_error(None, inner),
        other => anyhow::anyhow!("{other}"),
    })?;
    super::v06::finish_file(output)?;
    let audit = audit.into_inner();
    if let (Some(info), Some(curated)) = (info.as_mut(), curated) {
        append_full_card(
            output,
            info,
            curated,
            measure_costs,
            memory_budget_mb,
            &stats,
            &audit.report,
            &rec,
            "build",
        )?;
    }
    let byte_len = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0) as usize;
    progress::finish(stats.statements as u64, output);
    print_build_summary(output, &stats, byte_len);
    crate::commands::iri_report::warn_after_build(&audit.report);
    Ok(())
}

/// Give the file the memory-bounded external builder has just written at
/// `output` its **full** card, and its build record: what `rete build
/// --memory-budget-mb` and `rete merge` both do once their stream is on disk.
///
/// `info` is the record begun before the stream ([`buildinfo::Recorder::begin`]);
/// this adds what only the finished file knows — its codec, the counts
/// (`stats`, the statements the stream read, the IRI audit), the spill — then
/// derives the card from the file exactly as `rete recard` does
/// ([`card::derive_full_card`], inside `memory_budget_mb` all told), runs and
/// records its starter queries unless `!measure_costs`, and writes the card
/// and the sealed record after the data ([`card::append_card`]). `who`
/// prefixes the progress lines.
///
/// A failure past the stream leaves a complete, cardless file; the error says
/// how to card it without a second build.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_full_card(
    output: &str,
    info: &mut buildinfo::BuildInfo,
    curated: card::CardInput,
    measure_costs: bool,
    memory_budget_mb: u64,
    stats: &ingest::BuildStats,
    iri: &rete_core::IriReport,
    rec: &buildinfo::Recorder,
    who: &str,
) -> anyhow::Result<()> {
    let codec = {
        use std::io::Read;
        let mut head = vec![0u8; rete_core::format::HEADER_LEN];
        std::fs::File::open(output)?.read_exact(&mut head)?;
        rete_core::Header::from_bytes(&head)?.dict_codec
    };
    info.params.codec = Some(buildinfo::codec_name(codec).to_string());
    info.counts = Some(buildinfo::counts(
        stats,
        rec.statements_read.get(),
        None,
        iri,
    ));
    info.spill = Some(rec.spill());
    let unfinished = |e: anyhow::Error| {
        anyhow::anyhow!(
            "{e}\n{output} is complete but has no card; `rete recard {output} --in-place \
             --card-file …` gives it one without rebuilding"
        )
    };
    let full = card::derive_full_card(
        output,
        curated,
        None,
        measure_costs,
        card::DeriveMemory::Total(memory_budget_mb.saturating_mul(1 << 20)),
        Some(stats.default_triples as u64),
        rec,
        who,
    )
    .map_err(unfinished)?;
    info.query_costs = full.query_costs;
    info.dropped_queries = full.dropped;
    let meta = full.card.to_json_bytes();
    if super::v06::file_is_v06(std::path::Path::new(output))? {
        // 0x06: the card region sits in front of the data, and a cardless
        // file has none — the pair goes in by one copy (phase 2 streams the
        // data once, after the card).
        rete_core::build_progress::begin(
            rete_core::build_trace::Phase::ContentHash,
            None,
            rete_core::build_progress::Unit::Bytes,
        );
        rec.lap(rete_core::build_trace::Phase::ContentHash);
        rec.seal(info);
        let reserve = buildinfo::with_widest_costs(info).to_json_bytes();
        super::v06::recard_file(
            output,
            rete_core::v06::write::NewCard::Set(Some(&meta)),
            Some(&reserve),
        )
        .map_err(unfinished)?;
        buildinfo::remeasure_on_file(output, info, rec).map_err(unfinished)?;
        let record = info.to_json_bytes();
        eprintln!(
            "embedded dataset card ({} bytes of metadata) and build info ({} bytes, outside the \
             content hash) in the card region",
            meta.len(),
            record.len()
        );
        return Ok(());
    }
    let (_, _) = card::append_card(output, &meta, false, rec, who, || {
        rec.seal(info);
        info.to_json_bytes()
    })
    .map_err(unfinished)?;
    eprintln!(
        "embedded dataset card ({} bytes of metadata) and build info ({} bytes, outside the \
         content hash) after the data",
        meta.len(),
        info.to_json_bytes().len()
    );
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
    text_index: Option<TextIndexScope>,
    type_predicate: Option<&str>,
    pyramid_algo: rete_core::PyramidAlgo,
    card_args: CardArgs,
    label_index: bool,
) -> anyhow::Result<()> {
    let tapped = [(input, "rete")];
    let rec = buildinfo::Recorder::start(if card_args.requested() {
        &tapped[..]
    } else {
        &[]
    });
    let bytes = std::fs::read(input)?;
    if let Some(tap) = rec.taps.first() {
        tap.bytes.set(bytes.len() as u64);
        tap.hasher.borrow_mut().update(&bytes);
    }
    let rete = rete_core::Rete::open(&bytes)?;
    // Rebuilding must not silently CHANGE what the file carries: a three-
    // permutation input stays three-permutation, a six stays six. `repyramid`
    // has no `--permutations` of its own for the same reason it has no
    // `--no-pyramid` — it re-assembles an existing file, it does not re-decide
    // how it was built. Rebuild from source to change the set.
    let perms = rete.header().perms;
    // The named-graph layout is the same kind of fact: a quad-index input
    // stays a quad-index file.
    let graphs = if rete.header().has_quad_index() || !super::v06::writes_legacy() {
        rete_core::GraphLayout::Quad
    } else {
        rete_core::GraphLayout::PerGraph
    };
    if perms != rete_core::PermSet::ALL {
        eprintln!(
            "preserving the input's {} permutation(s): {}",
            perms.len(),
            perms.names().join(", ")
        );
    }
    // The same rule for the label index: kept when the input has one, added
    // with `--label-index`. The SCHEMA section is always written, as by `build`.
    let sections = BuildSections {
        label_index: label_index || rete.has_label_index(),
        schema: true,
    };
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
    let read = quads.len() as u64;
    rec.lap(rete_core::build_trace::Phase::Read);

    // Optionally derive + embed a Dataset Card from the rebuilt counts (same as
    // `build --card`); without a card flag the metadata payload stays empty.
    let curated = if card_args.requested() {
        Some(card::load_curated(&card_args)?)
    } else {
        None
    };
    let card_requested = curated.is_some();
    let opts = sections.assemble_options(
        true,
        text_index.as_ref(),
        type_predicate,
        pyramid_algo,
        perms,
        graphs,
    );
    let (out_bytes, stats) =
        ingest::assemble_dataset_opts(quads, &opts, |stats, quads| match curated {
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
        });
    let out_bytes = super::v06::finish(out_bytes)?;
    let len = if card_requested {
        let flags = RecordFlags {
            command: "repyramid",
            format: None,
            no_pyramid: false,
            pyramid_algo,
            text_index: text_index.as_ref(),
            materialize: false,
            reason: false,
            sections,
            perms,
            graphs,
            collapse_graphs: false,
            strict: false,
            surface: ingest::QuotedTripleSurface::default(),
            type_predicate,
            measure_costs: true,
            memory_budget_mb: None,
            card_args: &card_args,
        };
        write_carded(
            output,
            out_bytes,
            &flags,
            &rec,
            &stats,
            read,
            None,
            &rete_core::IriReport::default(),
        )?
    } else {
        std::fs::write(output, &out_bytes)?;
        out_bytes.len()
    };
    if stats.named_graphs > 0 {
        println!(
            "repyramid: wrote {output} — {} quads ({} default + {} named), {} terms, {} bytes",
            stats.statements, stats.default_triples, stats.named_graphs, stats.terms, len
        );
    } else {
        println!(
            "repyramid: wrote {output} — {} triples, {} terms, {} pyramid level(s), {} bytes",
            stats.default_triples, stats.terms, stats.pyramid_levels, len
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
            rete_core::GraphLayout::PerGraph,
            false,
            ingest::QuotedTripleSurface::default(),
            BuildSections::default(),
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
                rete_core::GraphLayout::PerGraph,
                false,
                ingest::QuotedTripleSurface::default(),
                BuildSections::default(),
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
            rete_core::GraphLayout::PerGraph,
            false,
            ingest::QuotedTripleSurface::default(),
            BuildSections::default(),
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
            rete_core::GraphLayout::PerGraph,
            false,
            ingest::QuotedTripleSurface::default(),
            BuildSections::default(),
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
            rete_core::GraphLayout::PerGraph,
            false,
            ingest::QuotedTripleSurface::default(),
            BuildSections::default(),
        )
        .unwrap();
        let plain = Rete::open(&std::fs::read(&out).unwrap()).unwrap();
        assert!(plain
            .query(Some("<http://x>"), Some(TYPE), Some("<http://D>"))
            .is_empty());

        std::fs::remove_file(&inp).ok();
        std::fs::remove_file(&out).ok();
    }

    /// `--text-index` with `--memory-budget-mb` (B1): no longer refused, and the
    /// TEXT_INDEX section is the bytes the in-RAM build writes — for the
    /// default scope and for a scoped one, whose flags build-info records.
    #[test]
    fn memory_bounded_text_index_matches_the_in_ram_section() {
        let nq = "<http://x/e1> <http://x/title> \"Glucose metabolism étude\" .\n\
                  <http://x/e1> <http://x/desc> \"The entity has been modified.\" .\n\
                  <http://x/e2> <http://x/title> \"Benzene ring\"@en .\n\
                  <http://x/p1> <http://x/desc> \"The entity has been created.\" <http://x/g1> .\n\
                  <http://x/p1> <http://x/note> \"created by an agent\" <http://x/g1> .\n";
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("rete_tiext_{pid}"));
        std::fs::create_dir_all(&dir).unwrap();
        let inp = dir.join("in.nq");
        std::fs::write(&inp, nq).unwrap();
        let input = [inp.to_str().unwrap().to_string()];
        let none: [&str; 0] = [];
        let scopes = [
            TextIndexScope::default(),
            TextIndexScope::from_options(rete_core::TextIndexGraphs::All, none, ["http://x/desc"])
                .unwrap(),
        ];
        let section = |b: &[u8]| -> (Vec<u8>, bool) {
            if rete_core::v06::open::is_v06(b) {
                // 0x06: the text unit root, its leaves and its postings.
                use rete_core::v06::{kind, tag, P_TEXT_PARTIAL};
                let (h, root) =
                    rete_core::v06::open::read_front(&rete_core::SliceReader::new(b)).unwrap();
                let mut out = root.unit(tag::TEXT).unwrap().to_vec();
                for k in [kind::TEXT_LEAVES, kind::TEXT_DATA] {
                    let r = root.section(k).unwrap().abs(&h);
                    out.extend_from_slice(&b[r.offset as usize..(r.offset + r.len) as usize]);
                }
                return (out, h.presence & P_TEXT_PARTIAL != 0);
            }
            let h = rete_core::Header::from_bytes(b).unwrap();
            let off = h.text_index_offset as usize;
            (
                b[off..off + h.text_index_len as usize].to_vec(),
                h.text_index_partial,
            )
        };
        for scope in scopes {
            let ram = dir.join("ram.rete");
            let ext = dir.join("ext.rete");
            build(
                &input,
                ram.to_str().unwrap(),
                None,
                false,
                true,
                false,
                rete_core::PyramidAlgo::Louvain,
                Some(scope.clone()),
                None,
                false,
                CardArgs::default(),
                false,
                rete_core::PermSet::ALL,
                rete_core::GraphLayout::PerGraph,
                false,
                ingest::QuotedTripleSurface::default(),
                BuildSections::default(),
            )
            .unwrap();
            build_external_cmd(
                &input,
                ext.to_str().unwrap(),
                None,
                64,
                Some(dir.to_str().unwrap()),
                false,
                false,
                Some(scope.clone()),
                false,
                CardArgs {
                    enabled: true,
                    ..CardArgs::default()
                },
                true,
                rete_core::PermSet::ALL,
                rete_core::GraphLayout::PerGraph,
                false,
                ingest::QuotedTripleSurface::default(),
                None,
                BuildSections::default(),
            )
            .unwrap();
            let ram_bytes = std::fs::read(&ram).unwrap();
            let ext_bytes = std::fs::read(&ext).unwrap();
            let (ram_sec, ram_partial) = section(&ram_bytes);
            let (ext_sec, ext_partial) = section(&ext_bytes);
            assert!(!ram_sec.is_empty());
            assert_eq!(ext_sec, ram_sec, "{scope:?}: TEXT_INDEX sections differ");
            assert_eq!(ext_partial, scope.restricts_predicates());
            assert_eq!(ram_partial, ext_partial);

            // The carded external build records the scope it was given.
            let info = buildinfo::BuildInfo::from_json_bytes(
                &rete_core::read_build_info(&ext_bytes).unwrap().unwrap(),
            )
            .unwrap();
            assert!(info.params.text_index);
            if scope.is_default() {
                assert_eq!(info.params.text_index_graphs, None);
                assert!(info.params.text_index_exclude.is_empty());
            } else {
                assert_eq!(info.params.text_index_graphs.as_deref(), Some("all"));
                assert_eq!(info.params.text_index_exclude, ["<http://x/desc>"]);
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
