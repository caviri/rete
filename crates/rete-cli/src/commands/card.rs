//! Dataset Cards — the **CLI's** half: reading a `--card-file` off disk,
//! range-reading a card out of a local or remote `.rete`, rendering the human
//! catalog view, and measuring what the card's starter queries cost.
//!
//! The card *schema* and its *derivation* are not here. They live in
//! [`rete_core::card`] — `card_derive` (the [`DatasetCard`] type and the profile
//! derived from a graph's statements) and `card_queries` (the tiered
//! starter-query library) — because `rete-cli` is a binary-only crate that no
//! client can link, so a card derived here could never be derived anywhere else
//! (#152). Everything below is what genuinely belongs to a command-line tool:
//! `std::fs`, HTTP range reads, terminal formatting, timing.
//!
//! Surfaced by `rete card [--json]`, `rete card-audit`, and folded into
//! `rete info`'s catalog view.

use rete_core::Header;

// The card schema and its derivation, re-exported under the name the rest of
// the CLI has always used (`super::card::DatasetCard`, …). One import site to
// change if the core module is ever renamed.
pub(crate) use rete_core::card::{
    curated_counts_card, derive_card, derive_card_encoded, CardInput, Coherence, DatasetCard,
    ExampleQuery, PermutationsSignal, TextIndexSignal, Tier, CARD_TOP_N,
};

/// `DatasetCard::from_json_bytes` in `anyhow` clothes. `rete-core`'s card
/// errors are plain `String`s (the wasm build has no `anyhow`), and `?` will
/// not lift a `String` into `anyhow::Error` on its own.
fn parse_card(bytes: &[u8]) -> anyhow::Result<DatasetCard> {
    DatasetCard::from_json_bytes(bytes).map_err(|e| anyhow::anyhow!(e))
}

/// The `rete build` card flags, bundled for threading through `build()`.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardArgs {
    /// `--card`: embed a card even with no curated fields (stats only).
    pub enabled: bool,
    /// `--card-file <path>`: JSON document of curated fields.
    pub file: Option<String>,
    pub title: Option<String>,
    pub license: Option<String>,
    pub source: Option<String>,
    pub description: Option<String>,
    pub created: Option<String>,
}

impl CardArgs {
    /// Did the user ask for a card at all? Any flag presence opts in; otherwise
    /// the build stays cardless (byte-identical to a no-card build).
    pub fn requested(&self) -> bool {
        self.enabled
            || self.file.is_some()
            || self.title.is_some()
            || self.license.is_some()
            || self.source.is_some()
            || self.description.is_some()
            || self.created.is_some()
    }
}

/// The versioned JSON envelope used by the local and remote `card --json`
/// commands. Embedded metadata remains backward-readable; the CLI contract adds
/// its schema version at presentation time.
pub(crate) fn card_json(card: &DatasetCard) -> serde_json::Value {
    let mut value = serde_json::to_value(card).expect("DatasetCard serializes");
    value
        .as_object_mut()
        .expect("DatasetCard JSON is an object")
        .insert(
            "schemaVersion".into(),
            serde_json::json!(crate::JSON_SCHEMA_VERSION),
        );
    value
}

/// [`card_json`] plus the build-info record (when the file carries one) under a
/// `"build"` key — an envelope addition, so the card's own stored bytes stay
/// exactly the hashed metadata section.
pub(crate) fn card_json_with_build(
    card: &DatasetCard,
    build: Option<&super::buildinfo::BuildInfo>,
) -> serde_json::Value {
    let mut value = card_json(card);
    if let Some(b) = build {
        value
            .as_object_mut()
            .expect("DatasetCard JSON is an object")
            .insert(
                "build".into(),
                serde_json::to_value(b).expect("BuildInfo serializes"),
            );
    }
    value
}

/// Resolve the curated fields: load `--card-file` (if any), then let explicit
/// flags override individual fields. Custom fields have **no flag** — arbitrary
/// key/values on a command line are a shell-quoting trap with no schema to
/// validate against; they come from the card file's `extra` object only, and
/// the bag is normalized and bounds-checked here, the single choke point every
/// card-writing command (`build`, `merge`, `repyramid`) funnels through.
pub(crate) fn load_curated(args: &CardArgs) -> anyhow::Result<CardInput> {
    let mut c = match &args.file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("reading --card-file {path}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| {
                // The top level is reserved for rete-defined fields; point a
                // stray key at the bag instead of leaving a bare serde error.
                // (The wording is `rete-core`'s, so the CLI, the browser
                // builder and every binding say the same thing.)
                let hint = if e.to_string().contains("unknown field") {
                    rete_core::card::UNKNOWN_FIELD_HINT
                } else {
                    ""
                };
                anyhow::anyhow!("parsing --card-file {path}: {e}{hint}")
            })?
        }
        None => CardInput::default(),
    };
    if args.title.is_some() {
        c.title = args.title.clone();
    }
    if args.license.is_some() {
        c.license = args.license.clone();
    }
    if args.source.is_some() {
        c.source = args.source.clone();
    }
    if args.description.is_some() {
        c.description = args.description.clone();
    }
    if args.created.is_some() {
        c.created = args.created.clone();
    }
    // After the override, so `--description` is bounded exactly like a
    // `--card-file` one. (`--description "$(cat desc.md)"` is the shell-side
    // answer to authoring a multi-line description — see docs/dataset-cards.md.)
    //
    // Every rule below this point — the description cap, the keyword/theme
    // canonicalization, the `extra` bounds — is `rete_core::card::CardInput`'s,
    // so a card written from Python or the browser is held to exactly what
    // `--card-file` is held to. All the CLI adds is the `anyhow` face.
    c.normalize().map_err(|e| anyhow::anyhow!(e))
}

/// Hex-encode a 16-byte content hash (the `.rete` integrity checksum).
pub(crate) fn hex16(bytes: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// [`rete_core::card::load_card`] in `anyhow` clothes — read the card embedded
/// in a `.rete` file image, or `None` if it has no metadata section.
pub(crate) fn load_card(bytes: &[u8]) -> anyhow::Result<Option<DatasetCard>> {
    rete_core::card::load_card(bytes).map_err(|e| anyhow::anyhow!(e))
}

/// Read the dataset card via a [`rete_core::RangeReader`] — fetching only the
/// header and metadata range (the index-free CARD tier), never the
/// dictionary/index/pyramid. The remote/S3 companion to [`load_card`], which
/// needs the whole file in memory. Returns `None` when the file has no card.
pub(crate) fn load_card_ranged<R: rete_core::RangeReader>(
    reader: &R,
) -> anyhow::Result<Option<DatasetCard>> {
    match rete_core::read_metadata_ranged(reader)? {
        None => Ok(None),
        Some(bytes) => Ok(Some(parse_card(&bytes)?)),
    }
}

/// One CARD-tier read of a `.rete`: everything a reader can learn about the
/// file without touching the dictionary, index or pyramid.
pub(crate) struct CardRead {
    /// The 1 KiB header, parsed once and reused (checksum, section directory).
    pub header: Header,
    /// The embedded card, with [`rete_core::card::Signals::text_index`] already measured.
    pub card: Option<DatasetCard>,
    /// The adjacent build-info record, when the file carries one.
    pub build: Option<super::buildinfo::BuildInfo>,
    /// What the header says about the file's full-text index — measured, so it
    /// is an answer even for a file with no card at all.
    pub text_index: TextIndexSignal,
    /// What the card's own bytes claimed about the index before the measurement
    /// replaced it. Normally `None`; `Some(_)` is drift worth reporting.
    pub stored_text_index: Option<TextIndexSignal>,
    /// Which index permutations the file stores — read off the header byte the
    /// CARD tier already holds, so it costs nothing and answers for a cardless
    /// file too.
    pub permutations: PermutationsSignal,
}

/// Read the header, the card, **and** the build-info record in the CARD tier's
/// budget: the 1 KiB header read plus one coalesced range covering both
/// adjacent sections (the same adjacency contract as
/// [`rete_core::read_card_and_build_info_ranged`], done here so the header is
/// fetched exactly once and reused for the checksum). A build-info blob that
/// fails to parse degrades to `None` with a warning — a newer writer's record
/// must never make the card unreadable.
///
/// The header this already holds also answers whether the file carries a
/// full-text index, so the [`TextIndexSignal`] is measured here — the one place
/// every card-reading command funnels through — rather than in each of them. It
/// adds one ≤10-byte range read, and only when there is an index to measure.
pub(crate) fn load_card_and_build_ranged<R: rete_core::RangeReader>(
    reader: &R,
) -> anyhow::Result<CardRead> {
    let head = reader.read_at(0, rete_core::HEADER_LEN as u64)?;
    let header = Header::from_bytes(&head)?;
    let meta = (header.metadata_offset, header.metadata_len);
    let build = (header.build_info_offset, header.build_info_len);
    let (m, b) = if meta.1 > 0 && build.1 > 0 && build.0 == meta.0 + meta.1 {
        // Adjacent (the layout the writers produce): one read spans both.
        let both = reader.read_at(meta.0, meta.1 + build.1)?;
        let (m, b) = both.split_at(meta.1 as usize);
        (Some(m.to_vec()), Some(b.to_vec()))
    } else {
        let fetch = |off: u64, len: u64| -> std::io::Result<Option<Vec<u8>>> {
            if len == 0 {
                Ok(None)
            } else {
                Ok(Some(reader.read_at(off, len)?))
            }
        };
        (fetch(meta.0, meta.1)?, fetch(build.0, build.1)?)
    };
    let mut card = match m {
        None => None,
        Some(bytes) => Some(parse_card(&bytes)?),
    };
    let build = b.and_then(
        |bytes| match super::buildinfo::BuildInfo::from_json_bytes(&bytes) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("warning: unreadable build-info section ({e}); ignoring it");
                None
            }
        },
    );
    let text_index = TextIndexSignal::probe(reader, &header);
    let stored_text_index = card
        .as_mut()
        .and_then(|c| c.observe_text_index(text_index))
        .filter(|stored| *stored != text_index);
    let permutations = PermutationsSignal::probe(&header);
    if let Some(c) = card.as_mut() {
        c.observe_permutations(permutations.clone());
    }
    Ok(CardRead {
        header,
        card,
        build,
        text_index,
        stored_text_index,
        permutations,
    })
}

/// Render a card as a human-readable catalog. `checksum` is the file's content
/// hash in hex (the integrity checksum surfaced by `rete verify`).
pub(crate) fn format_card(card: &DatasetCard, checksum: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "Dataset Card");
    let field = |out: &mut String, label: &str, value: &Option<String>| {
        if let Some(v) = value {
            let _ = writeln!(out, "  {label:<13}: {v}");
        }
    };
    field(&mut out, "title", &card.title);
    // The description may be Markdown, and Markdown has line breaks. Indent the
    // continuation lines to the value column so a multi-line description stays
    // inside the catalog's layout instead of falling out of it at column 0.
    if let Some(d) = &card.description {
        for (n, line) in d.split('\n').enumerate() {
            if n == 0 {
                let _ = writeln!(out, "  {:<13}: {line}", "description");
            } else {
                let _ = writeln!(out, "  {:<13}  {line}", "");
            }
        }
    }
    field(&mut out, "license", &card.license);
    field(&mut out, "source", &card.source);
    field(&mut out, "created", &card.created);
    if !card.keywords.is_empty() {
        let _ = writeln!(out, "  {:<13}: {}", "keywords", card.keywords.join(", "));
    }
    for t in &card.theme {
        let _ = writeln!(out, "  {:<13}: {t}", "theme");
    }
    field(&mut out, "version", &card.version);
    field(&mut out, "source date", &card.source_date);
    for c in &card.creators {
        let orcid = c
            .orcid
            .as_deref()
            .map(|o| format!("  ({o})"))
            .unwrap_or_default();
        let _ = writeln!(out, "  {:<13}: {}{orcid}", "creator", c.name);
    }
    if let Some(p) = &card.publisher {
        let ror = p
            .ror
            .as_deref()
            .map(|r| format!("  ({r})"))
            .unwrap_or_default();
        let _ = writeln!(out, "  {:<13}: {}{ror}", "publisher", p.name);
    }
    field(&mut out, "canonical URL", &card.canonical_url);
    field(&mut out, "endpoint", &card.sparql_endpoint);
    field(&mut out, "doi", &card.doi);
    field(&mut out, "cite as", &card.cite_as);
    for d in &card.derived_from {
        let _ = writeln!(out, "  {:<13}: {d}", "derived from");
    }
    if !card.extra.is_empty() {
        let _ = writeln!(out, "  custom fields ({}):", card.extra.len());
        for (k, v) in &card.extra {
            // `Value`'s Display is compact JSON — strings keep their quotes,
            // so a value is always unambiguous about its type.
            let _ = writeln!(out, "      {k} = {v}");
        }
    }

    let _ = writeln!(out, "  {:<13}: {}", "triples", card.triple_count);
    if card.named_graph_count > 0 {
        let _ = writeln!(
            out,
            "  {:<13}: {} total, {} named graph(s)",
            "quads", card.quad_count, card.named_graph_count
        );
    }
    let _ = writeln!(out, "  {:<13}: {}", "terms", card.term_count);
    let _ = writeln!(
        out,
        "  {:<13}: {checksum}  (blake3-16 content hash)",
        "checksum"
    );

    if !card.vocabularies.is_empty() {
        let _ = writeln!(out, "  {:<13}: {}", "vocabularies", card.vocabularies.len());
        for ns in &card.vocabularies {
            let _ = writeln!(out, "      {ns}");
        }
    }
    if !card.predicates.is_empty() {
        let _ = writeln!(out, "  predicates ({}):", card.predicates.len());
        for (iri, count) in &card.predicates {
            let _ = writeln!(out, "      {count:>8}  {iri}");
        }
    }
    if !card.classes.is_empty() {
        let _ = writeln!(out, "  classes ({}):", card.classes.len());
        for (iri, count) in &card.classes {
            let _ = writeln!(out, "      {count:>8}  {iri}");
        }
    }
    if !card.datatypes.is_empty() {
        let _ = writeln!(out, "  datatypes ({}):", card.datatypes.len());
        for (dt, count) in &card.datatypes {
            let _ = writeln!(out, "      {count:>8}  {dt}");
        }
    }
    if !card.languages.is_empty() {
        let _ = writeln!(out, "  languages ({}):", card.languages.len());
        for (lang, count) in &card.languages {
            let shown = if lang.is_empty() { "(untagged)" } else { lang };
            let _ = writeln!(out, "      {count:>8}  {shown}");
        }
    }
    if !card.class_links.is_empty() {
        let _ = writeln!(out, "  class links ({}):", card.class_links.len());
        for l in &card.class_links {
            let _ = writeln!(
                out,
                "      {:>8}  {} --{}-> {}",
                l.count, l.s_class, l.predicate, l.o_class
            );
        }
    }
    if !card.top_hubs.is_empty() {
        let _ = writeln!(out, "  top hubs (out-degree):");
        for (iri, d) in &card.top_hubs {
            let _ = writeln!(out, "      {d:>8}  {iri}");
        }
    }
    if !card.in_hubs.is_empty() {
        let _ = writeln!(out, "  top hubs (in-degree):");
        for (iri, d) in &card.in_hubs {
            let _ = writeln!(out, "      {d:>8}  {iri}");
        }
    }
    if !card.signals.is_empty() {
        let s = &card.signals;
        let _ = writeln!(out, "  signals:");
        let opt = |out: &mut String, label: &str, v: &Option<String>| {
            if let Some(v) = v {
                let _ = writeln!(out, "      {label:<11}: {v}");
            }
        };
        opt(&mut out, "label pred", &s.label_predicate);
        opt(&mut out, "base IRI", &s.base_iri);
        opt(&mut out, "default lang", &s.default_lang);
        if !s.time_predicates.is_empty() {
            let _ = writeln!(out, "      time preds : {}", s.time_predicates.join(", "));
        }
        if !s.link_predicates.is_empty() {
            let _ = writeln!(out, "      link preds : {}", s.link_predicates.join(", "));
        }
        if let Some((from, to)) = &s.temporal_extent {
            let _ = writeln!(out, "      time extent: {from} … {to}");
        }
        if let Some([min_lon, min_lat, max_lon, max_lat]) = s.spatial_bbox {
            let _ = writeln!(
                out,
                "      bbox       : lon [{min_lon}, {max_lon}], lat [{min_lat}, {max_lat}] (CRS84 lon/lat)"
            );
        }
        if s.geo_wkt {
            let _ = writeln!(out, "      geometry   : geo:asWKT present");
        }
        // Stated in both directions, and last because it is the one signal
        // measured from the file's sections rather than from its triples. A
        // card read from a saved JSON document has nothing to measure, so it
        // says nothing at all rather than guessing "no".
        if let Some(ti) = &s.text_index {
            let _ = writeln!(out, "      full text  : {}", ti.describe());
        }
        if let Some(p) = &s.permutations {
            let _ = writeln!(out, "      index      : {}", p.describe());
        }
    }
    if !card.coherence.is_empty() {
        let c = &card.coherence;
        let verdict = if c.coherent {
            "coherent".to_string()
        } else {
            format!("{} incoherent point(s)", c.inconsistency_count)
        };
        let _ = writeln!(out, "  coherence:");
        let _ = writeln!(out, "      verdict    : {verdict}");
        if !c.by_kind.is_empty() {
            let kinds: Vec<String> = c.by_kind.iter().map(|(k, n)| format!("{k}×{n}")).collect();
            let _ = writeln!(out, "      by kind    : {}", kinds.join(", "));
        }
        let _ = writeln!(
            out,
            "      scope      : {} · rules {} · {}",
            c.scope,
            c.rules,
            if c.materialized {
                "materialized"
            } else {
                "not materialized"
            }
        );
    }
    if !card.queries.is_empty() {
        let _ = writeln!(out, "  starter queries ({}):", card.queries.len());
        for q in &card.queries {
            let tier = match q.tier {
                Tier::Card => "card",
                Tier::Summary => "summary",
                Tier::Index => "index",
            };
            let _ = writeln!(out, "      [{tier:^7}] {} — {}", q.id, q.question);
        }
    }
    if !card.example_queries.is_empty() {
        let _ = writeln!(out, "  curated queries:");
        for q in &card.example_queries {
            let _ = writeln!(out, "      {q}");
        }
    }
    // Trim the trailing newline so callers control spacing.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Output shape of `rete card` / `rete card-url`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardFormat {
    /// Human catalog view (default).
    Text,
    /// The raw card JSON envelope (`--json`).
    Json,
    /// JSON-LD projection: VoID + schema.org + PROV (`--format jsonld`).
    JsonLd,
    /// Croissant projection of the honestly-mappable subset
    /// (`--format croissant`).
    Croissant,
}

impl CardFormat {
    /// Resolve the `--json` / `--format` flags (`--format` wins).
    pub(crate) fn resolve(json: bool, format: Option<&str>) -> anyhow::Result<Self> {
        match format {
            None => Ok(if json {
                CardFormat::Json
            } else {
                CardFormat::Text
            }),
            Some("json") => Ok(CardFormat::Json),
            Some("jsonld") => Ok(CardFormat::JsonLd),
            Some("croissant") => Ok(CardFormat::Croissant),
            Some(other) => anyhow::bail!("unknown card format {other:?} (json|jsonld|croissant)"),
        }
    }
}

/// Shared presentation for `rete card` and `rete card-url`: render one card (+
/// optional build info) in the chosen format. `source` is the file's own
/// path/URL, used as the dataset IRI fallback in the projections.
pub(crate) fn print_card(
    card: &DatasetCard,
    build: Option<&super::buildinfo::BuildInfo>,
    checksum: &str,
    source: &str,
    format: CardFormat,
    sha256: Option<&str>,
) -> anyhow::Result<()> {
    match format {
        CardFormat::Text => {
            println!("{}", format_card(card, checksum));
            if let Some(b) = build {
                println!("{}", super::buildinfo::format_build_info(b));
            }
        }
        CardFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&card_json_with_build(card, build))?
        ),
        CardFormat::JsonLd => println!(
            "{}",
            serde_json::to_string_pretty(&super::card_jsonld::to_jsonld(
                card, build, checksum, source
            ))?
        ),
        CardFormat::Croissant => println!(
            "{}",
            serde_json::to_string_pretty(&super::card_jsonld::to_croissant(
                card, checksum, source, sha256
            ))?
        ),
    }
    Ok(())
}

/// `rete card <file> [--json|--format jsonld|croissant]`: print the embedded
/// dataset card (catalog view), the raw JSON, or an RDF projection. Prints
/// `(no dataset card — …)` when absent, still naming what the header alone
/// decides (whether the file carries a full-text index).
pub(crate) fn card_cmd(
    file: &str,
    json: bool,
    format: Option<&str>,
    sha256: Option<&str>,
) -> anyhow::Result<()> {
    let format = CardFormat::resolve(json, format)?;
    // The CARD tier: header + one coalesced metadata+build-info range — the
    // same reads `card-url` does over HTTP, so a 50 GB local file costs KBs to
    // describe.
    let reader = crate::commands::range_source::LocalRangeReader::open(file)?;
    let read = load_card_and_build_ranged(&reader)?;
    match &read.card {
        // A cardless file can still answer the one question the header alone
        // decides — and staying silent about it is what #189 was about.
        None => println!(
            "(no dataset card — {}; {})",
            read.text_index.describe(),
            read.permutations.describe()
        ),
        Some(card) => print_card(
            card,
            read.build.as_ref(),
            &hex16(&read.header.content_hash),
            file,
            format,
            sha256,
        )?,
    }
    Ok(())
}

/// `rete card-audit` — do the starter queries a card **already ships** still
/// answer on the file that carries them?
///
/// A published `.rete` cannot be re-carded for free (a 250 GB catalog is not a
/// thing to rebuild on a hunch), so this reads the card — two range requests,
/// tens of KB, whatever the file's size — and decides each query's fate from
/// the profile the card carries. The judgement is
/// [`rete_core::card::audit`], which shares its one co-occurrence test
/// with the generator itself.
///
/// Input is a `.rete` file (local path or `http(s)://` URL) or a card JSON
/// document as written by `rete card --json` / `rete card-url --json`, so a
/// survey that already fetched the cards need not fetch them twice.
///
/// `--measure` goes further and **runs** them. That is a different kind of
/// answer: the static pass leaves whole templates undecidable by construction
/// (nothing in a card ties a subject to a predicate, so `top-reach` cannot be
/// decided from one), and no amount of card-only reasoning closes that. A run
/// closes it, and records what the answer cost.
pub(crate) fn card_audit_cmd(path: &str, opts: &AuditOptions) -> anyhow::Result<()> {
    if !opts.measure && (!opts.only.is_empty() || opts.max_mb > 0.0) {
        // Silently ignoring them would report a static audit under a flag that
        // says a run was bounded.
        anyhow::bail!("--only and --max-mb bound a measurement; add --measure");
    }
    let read = read_card_for_audit(path)?;
    let (build, measured, stored_ti) = (read.build, read.text_index, read.stored_text_index);
    let Some(card) = read.card else {
        if opts.measure || opts.write_costs {
            anyhow::bail!("{path}: no dataset card, so there are no starter queries to measure");
        }
        if opts.json {
            println!(
                "{{\"path\":{},\"card\":false,\"text_index\":{},\"findings\":[]}}",
                quote(path),
                text_index_json(measured, stored_ti)
            );
        } else {
            println!("(no dataset card)");
            if let Some(ti) = measured {
                println!("  full text     {}", ti.describe());
            }
        }
        return Ok(());
    };
    let mut findings = rete_core::card::audit(&card);
    let run = if opts.measure || opts.write_costs {
        Some(measure_card(
            path,
            &card,
            build.as_ref(),
            &mut findings,
            opts,
        )?)
    } else {
        None
    };
    if opts.write_costs {
        let run = run.as_ref().expect("--write-costs implies a measurement");
        let bytes = write_costs(path, &card, run, opts)?;
        eprintln!(
            "wrote {} query cost(s) into {path} — {} bytes, content hash unchanged",
            run.costs.len(),
            bytes
        );
    }

    if opts.json {
        let doc = serde_json::json!({
            "path": path,
            "card": true,
            "title": card.title,
            "triples": card.triple_count,
            "quads": card.quad_count,
            "named_graphs": card.named_graph_count,
            "queries": card.queries.len(),
            "truncated": card.truncated,
            "text_index": text_index_json(measured, stored_ti),
            "measurement": run.as_ref().map(|r| serde_json::json!({
                "transport": r.transport,
                "engine": crate::commands::buildinfo::builder_version(),
                "queries_run": r.costs.len(),
                "total_bytes": r.costs.iter().map(|c| c.bytes).sum::<u64>(),
                "total_requests": r.costs.iter().map(|c| c.requests).sum::<u64>(),
                "written": opts.write_costs,
                "note": COST_NOTE,
            })),
            "findings": findings,
        });
        println!("{}", serde_json::to_string(&doc)?);
        return Ok(());
    }

    println!("{}  ({} starter queries)", path, card.queries.len());
    // The full-text verdict leads the report: it is the one line that is true of
    // the FILE rather than of the card, and it is the question `FILTER(CONTAINS)`
    // cannot answer for you — the same query returns the same rows either way.
    match measured {
        Some(ti) => println!("  full text     {}", ti.describe()),
        None => println!(
            "  full text     unknown — a card document has no sections to measure; \
             pass the .rete itself"
        ),
    }
    if let Some(stored) = stored_ti {
        println!(
            "  full text     <- the card's own bytes claim {} — the file disagrees; re-card it",
            stored.describe()
        );
    }
    if let Some(run) = &run {
        // The transport goes ABOVE the numbers, not in a footnote: a byte
        // figure without the thing that fetched the bytes is not a cost.
        println!("  measured over: {}", run.transport);
        println!("  {COST_NOTE}");
        println!(
            "  {:<12} {:<10} {:<14} {:>7}       {:>13}   {:>6}     {:>8}",
            "card says", "run says", "query", "rows", "bytes", "req", "ms"
        );
    }
    for f in &findings {
        match &f.observed {
            None => println!(
                "  {:<12} {:<14} {:<11} {}",
                f.verdict.as_str(),
                f.id,
                f.revision,
                f.why
            ),
            Some(o) => {
                let mut note = String::new();
                if let Some(r) = &o.recorded {
                    note = if r.agrees {
                        "  = build record".to_string()
                    } else {
                        format!(
                            "  != build record ({} B, {} req, {} rows)",
                            r.bytes, r.requests, r.rows
                        )
                    };
                }
                if o.contradicts(f.verdict) {
                    note.push_str("  <- the run disagrees with the card");
                }
                println!(
                    "  {:<12} {:<10} {:<14} {:>7} row(s) {:>13} B {:>6} req {:>8} ms{note}",
                    f.verdict.as_str(),
                    o.outcome,
                    f.id,
                    o.rows,
                    o.bytes,
                    o.requests,
                    o.debug_ms,
                );
                if let Some(e) = &o.error {
                    println!("  {:<12} {:<10} {:<14} {e}", "", "", "");
                }
            }
        }
    }
    if let Some(run) = &run {
        println!(
            "  == {} quer{} run, {} bytes in {} range request(s){}",
            run.costs.len(),
            if run.costs.len() == 1 { "y" } else { "ies" },
            run.costs.iter().map(|c| c.bytes).sum::<u64>(),
            run.costs.iter().map(|c| c.requests).sum::<u64>(),
            match findings
                .iter()
                .filter_map(|f| f.observed.as_ref()?.recorded.as_ref())
                .filter(|r| !r.agrees)
                .count()
            {
                0 => String::new(),
                n => format!("  — {n} disagree with the file's own build record"),
            }
        );
    }
    Ok(())
}

/// The audit's `text_index` verdict as JSON: the measured signal, `null` when
/// nothing was measured (a card *document* has no sections), plus the card's own
/// stale claim under `card_said` when the two disagree. `null` is deliberately
/// distinguishable from `{"present": false}` — "I could not look" is not "there
/// is no index".
fn text_index_json(
    measured: Option<TextIndexSignal>,
    stored: Option<TextIndexSignal>,
) -> serde_json::Value {
    let Some(measured) = measured else {
        return serde_json::Value::Null;
    };
    let mut v = serde_json::to_value(measured).expect("TextIndexSignal serializes");
    if let Some(stored) = stored {
        v.as_object_mut().expect("an object").insert(
            "card_said".into(),
            serde_json::to_value(stored).expect("TextIndexSignal serializes"),
        );
    }
    v
}

/// What `rete card-audit` was asked to do beyond reading the card.
#[derive(Debug, Clone, Default)]
pub(crate) struct AuditOptions {
    pub json: bool,
    /// Run the queries instead of only reasoning about them.
    pub measure: bool,
    /// Measure only these ids — the way to spend 3 MB instead of 8 GB.
    pub only: Vec<String>,
    /// Give up on a query once it has asked for this many MB (0 = no cap).
    pub max_mb: f64,
    /// Record the measurement in the file's build-info section.
    pub write_costs: bool,
    /// Write even though a query measured zero rows.
    pub allow_empty: bool,
}

use crate::commands::buildinfo::COST_NOTE;

/// One measurement run: what it went through, and what each query cost.
pub(crate) struct CostRun {
    pub transport: String,
    pub costs: Vec<crate::commands::buildinfo::QueryCost>,
    /// True when only a subset of the card's queries was run (`--only`).
    pub partial: bool,
    /// Ids whose run did not finish — a byte budget bit, or the query failed.
    pub failed: Vec<String>,
}

/// Run the card's starter queries against the file that carries them and hang
/// each result on its finding.
///
/// The measurement itself is [`crate::commands::buildinfo::measure_query`] —
/// the same function `rete build` uses to fill in the build record. That is
/// deliberate and load-bearing: a re-measurement that used its own loop could
/// not be compared against a recorded one, and comparing them is the entire
/// point of re-measuring a published file.
fn measure_card(
    path: &str,
    card: &DatasetCard,
    build: Option<&super::buildinfo::BuildInfo>,
    findings: &mut [rete_core::card::Finding],
    opts: &AuditOptions,
) -> anyhow::Result<CostRun> {
    use crate::commands::buildinfo::{measure_query, BudgetReader};
    use crate::commands::range_source::{is_url, LocalRangeReader};

    let remote = is_url(path);
    if !remote && !path.ends_with(".rete") {
        anyhow::bail!(
            "{path}: --measure needs the .rete file itself (a card document has no data to run \
             against) — pass the local path or the http(s):// URL"
        );
    }
    // A mistyped id must not quietly narrow the run: the report would then
    // describe fewer queries than the caller asked about, and say nothing.
    let unknown: Vec<&str> = opts
        .only
        .iter()
        .map(String::as_str)
        .filter(|id| !card.queries.iter().any(|q| &q.id == id))
        .collect();
    if !unknown.is_empty() {
        anyhow::bail!(
            "no starter query matches --only {} (the card ships: {})",
            unknown.join(", "),
            card.queries
                .iter()
                .map(|q| q.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let wanted: Vec<&ExampleQuery> = card
        .queries
        .iter()
        .filter(|q| opts.only.is_empty() || opts.only.iter().any(|id| id == &q.id))
        .collect();
    if wanted.is_empty() {
        anyhow::bail!("this card ships no starter queries to measure");
    }
    let budget = if opts.max_mb > 0.0 {
        (opts.max_mb * (1u64 << 20) as f64) as u64
    } else {
        u64::MAX
    };

    // One source reader for the whole run: neither backing store caches
    // anything (no block cache is in the stack, by design — see the transport
    // string), so sharing it costs nothing and saves one HTTP HEAD per query.
    // The per-query CountingReader is what makes each run cold.
    let source: std::sync::Arc<dyn rete_core::RangeReader + Send + Sync> = if remote {
        std::sync::Arc::new(crate::http::HttpRangeReader::open(path)?)
    } else {
        std::sync::Arc::new(LocalRangeReader::open(path)?)
    };
    let transport = format!(
        "{}; cold lazy open per query; logical range reads, no block cache; reader fan-out {}",
        if remote {
            format!("HTTP range requests to {path}")
        } else {
            format!("local file {path}")
        },
        source.concurrency()
    );

    // The file's own build record, if it has one: the known answer this run
    // gets to check itself against, query by query.
    let recorded = |id: &str| -> Option<&crate::commands::buildinfo::QueryCost> {
        build?
            .query_costs
            .as_ref()?
            .queries
            .iter()
            .find(|c| c.id == id)
    };

    let mut costs = Vec::with_capacity(wanted.len());
    let mut failed = Vec::new();
    for q in &wanted {
        let src = source.clone();
        let m = measure_query(move || Ok(BudgetReader::new(src, budget)), q)?;
        if m.error.is_some() {
            failed.push(q.id.clone());
        }
        let outcome = match (&m.error, m.cost.rows, m.vacuous) {
            (Some(_), _, _) => "error",
            (None, 0, _) => "empty",
            (None, _, true) => "vacuous",
            _ => "answers",
        };
        if let Some(f) = findings.iter_mut().find(|f| f.id == q.id) {
            f.observed = Some(rete_core::card::Observation {
                outcome,
                rows: m.cost.rows,
                bytes: m.cost.bytes,
                requests: m.cost.requests,
                debug_ms: m.cost.debug_ms,
                error: m.error.clone(),
                recorded: recorded(&q.id).map(|r| rete_core::card::Recorded {
                    bytes: r.bytes,
                    requests: r.requests,
                    rows: r.rows,
                    agrees: r.bytes == m.cost.bytes
                        && r.requests == m.cost.requests
                        && r.rows == m.cost.rows,
                }),
            });
        }
        costs.push(m.cost);
    }
    Ok(CostRun {
        transport,
        costs,
        partial: wanted.len() != card.queries.len(),
        failed,
    })
}

/// Record a measurement in the file's build-info section.
///
/// Two guards, both about not making a file worse:
///
/// * a **partial** run would store a cost list a reader reads as complete;
/// * a query that measured **zero rows** does not need a cost, it needs a
///   re-card. Recording "this greeting query costs 1 GB and answers nothing"
///   into a published file, at the price of rewriting it, is work spent making
///   the wrong thing durable. `--allow-empty` is there for the templates that
///   are honestly empty (`top-dangling` on a fully-described graph).
fn write_costs(
    path: &str,
    card: &DatasetCard,
    run: &CostRun,
    opts: &AuditOptions,
) -> anyhow::Result<u64> {
    use crate::commands::buildinfo::{
        cost_context, write_build_info_streaming, BuildInfo, QueryCosts, BUILD_INFO_SCHEMA,
    };
    use crate::commands::range_source::is_url;

    if is_url(path) {
        anyhow::bail!("--write-costs needs a local file; {path} is a URL");
    }
    if run.partial {
        anyhow::bail!(
            "--write-costs refuses a partial run: --only measured {} of the card's {} starter \
             queries, and a stored cost list reads as complete",
            run.costs.len(),
            card.queries.len()
        );
    }
    if let Some(bad) = run.failed.first() {
        anyhow::bail!(
            "refusing to write: {} of the starter queries did not finish ({bad}) — a cost record \
             is only worth storing when every figure in it is a completed run",
            run.failed.len()
        );
    }
    let empty: Vec<&str> = run
        .costs
        .iter()
        .filter(|c| c.rows == 0)
        .map(|c| c.id.as_str())
        .collect();
    if !empty.is_empty() && !opts.allow_empty {
        anyhow::bail!(
            "refusing to write: {} starter quer{} returned zero rows ({}). A file whose greeting \
             queries answer nothing needs a re-card (scripts/recard), which rewrites it anyway \
             and fixes the queries too — pass --allow-empty if the emptiness is expected",
            empty.len(),
            if empty.len() == 1 { "y" } else { "ies" },
            empty.join(", ")
        );
    }

    // Carry whatever build record the file already has; only the costs change.
    // A file built before build records existed gets a record whose honest
    // content is *just* the costs — no invented timestamp, no invented builder,
    // because this tool did not build it.
    let reader = crate::commands::range_source::LocalRangeReader::open(path)?;
    let mut info = load_card_and_build_ranged(&reader)?
        .build
        .unwrap_or(BuildInfo {
            schema: BUILD_INFO_SCHEMA,
            ..Default::default()
        });
    info.query_costs = Some(QueryCosts {
        context: cost_context(&run.transport),
        queries: run.costs.clone(),
    });
    write_build_info_streaming(std::path::Path::new(path), &info.to_json_bytes())
}

/// JSON-quote a string for the one hand-built object above.
fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Read a card — and the build record that sits in the same coalesced range —
/// from a `.rete` (local or remote, CARD tier only), or a card from a card JSON
/// document. `(no dataset card)` — what the CLI prints for a cardless file — is
/// accepted and reported as no card, so a survey's saved output can be piped
/// straight back in. A card document carries no build record; that is what
/// makes `--measure` need the file.
///
/// A `.rete` also yields the measured [`TextIndexSignal`] and any drift between
/// it and what the card's bytes claimed; a card *document* yields neither —
/// there is no file to measure, so the audit reports the index as unknown
/// rather than asserting it from a document that may be years old.
struct CardForAudit {
    card: Option<DatasetCard>,
    build: Option<super::buildinfo::BuildInfo>,
    /// `None` for a card document (nothing was measured).
    text_index: Option<TextIndexSignal>,
    stored_text_index: Option<TextIndexSignal>,
}

fn read_card_for_audit(path: &str) -> anyhow::Result<CardForAudit> {
    let from_file = |read: CardRead| CardForAudit {
        card: read.card,
        build: read.build,
        text_index: Some(read.text_index),
        stored_text_index: read.stored_text_index,
    };
    if path.starts_with("http://") || path.starts_with("https://") {
        let reader = rete_core::CountingReader::new(
            crate::commands::range_source::RangedSourceReader::open(path)?,
        );
        let out = load_card_and_build_ranged(&reader).map(from_file);
        eprintln!(
            "fetched {} bytes in {} range request(s)",
            reader.bytes_read(),
            reader.requests()
        );
        return out;
    }
    if path.ends_with(".rete") {
        let reader = crate::commands::range_source::LocalRangeReader::open(path)?;
        return load_card_and_build_ranged(&reader).map(from_file);
    }
    let text = if path == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(path)?
    };
    let none = CardForAudit {
        card: None,
        build: None,
        text_index: None,
        stored_text_index: None,
    };
    let Some(start) = text.find('{') else {
        return Ok(none);
    };
    let card = serde_json::from_str(&text[start..])
        .map_err(|e| anyhow::anyhow!("{path}: not a dataset card document: {e}"))?;
    Ok(CardForAudit {
        card: Some(card),
        ..none
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rete_core::card::{normalize_string_list, normalize_themes};

    const TYPE: &str = rete_core::RDF_TYPE;

    fn q(s: &str, p: &str, o: &str) -> (String, String, String, Option<String>) {
        (s.into(), p.into(), o.into(), None)
    }

    /// A literal-bearing triple helper for the enriched-profile tests.
    const LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
    const SAMEAS: &str = "<http://www.w3.org/2002/07/owl#sameAs>";
    const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#integer";
    const XSD_GYEAR: &str = "http://www.w3.org/2001/XMLSchema#gYear";

    fn enriched_fixture() -> Vec<(String, String, String, Option<String>)> {
        vec![
            q("<http://ex/a>", TYPE, "<http://ex/Person>"),
            q("<http://ex/b>", TYPE, "<http://ex/Person>"),
            q("<http://ex/c>", TYPE, "<http://ex/City>"),
            q("<http://ex/a>", LABEL, "\"Alice\"@en"),
            q("<http://ex/b>", LABEL, "\"Bob\"@en"),
            q(
                "<http://ex/a>",
                "<http://ex/age>",
                &format!("\"30\"^^<{XSD_INT}>"),
            ),
            q(
                "<http://ex/a>",
                "<http://ex/birthYear>",
                &format!("\"1850\"^^<{XSD_GYEAR}>"),
            ),
            q(
                "<http://ex/b>",
                "<http://ex/birthYear>",
                &format!("\"1875\"^^<{XSD_GYEAR}>"),
            ),
            q("<http://ex/a>", "<http://ex/knows>", "<http://ex/b>"),
            q("<http://ex/a>", "<http://ex/livesIn>", "<http://ex/c>"),
            q("<http://ex/a>", SAMEAS, "<http://other/a>"),
        ]
    }

    /// The curated discovery lists (`keywords`, `theme`) are canonicalized
    /// at write time — trimmed, sorted, deduped — so two builds whose card
    /// files list the same entries in different order produce byte-identical
    /// cards; an empty entry is a loud error, never a silent drop; and a
    /// free-text `theme` is rejected (that is what `keywords` is for — the
    /// controlled-vocabulary IRI is `theme`'s whole point).
    #[test]
    fn curated_lists_are_canonicalized_and_bad_entries_rejected() {
        assert_eq!(
            normalize_string_list(
                "keywords",
                vec![" open data ".into(), "catalog".into(), "open data".into()]
            )
            .unwrap(),
            vec!["catalog", "open data"],
            "trimmed, sorted, deduplicated"
        );
        let err = normalize_string_list("keywords", vec!["ok".into(), "   ".into()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("keywords"), "names the field: {err}");

        // Themes must be controlled-vocabulary IRIs.
        normalize_themes(vec![
            "http://publications.europa.eu/resource/authority/data-theme/GOVE".into(),
        ])
        .expect("an IRI theme is accepted");
        let err = normalize_themes(vec!["government".into()])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("keywords"),
            "points free text at `keywords`: {err}"
        );

        // The canonical form is what serializes: reordered authoring yields
        // byte-identical cards (the same property the `extra` bag pins for
        // the content hash).
        let build = |json: &str| {
            let mut input: CardInput = serde_json::from_str(json).unwrap();
            input.keywords =
                normalize_string_list("keywords", std::mem::take(&mut input.keywords)).unwrap();
            curated_counts_card(3, 5, input).to_json_bytes()
        };
        assert_eq!(
            build(r#"{"keywords":["b","a"]}"#),
            build(r#"{"keywords":["a"," b "]}"#)
        );

        // And the text catalog renders them.
        let card = curated_counts_card(
            3,
            5,
            serde_json::from_str(
                r#"{"keywords":["catalog","open data"],
                    "theme":["http://publications.europa.eu/resource/authority/data-theme/GOVE"]}"#,
            )
            .unwrap(),
        );
        let text = format_card(&card, "aa");
        assert!(text.contains("keywords     : catalog, open data"));
        assert!(text.contains(
            "theme        : http://publications.europa.eu/resource/authority/data-theme/GOVE"
        ));
    }

    /// The CARD tier's budget survives the bag: header + ONE coalesced range
    /// still fetches card (custom fields included) and build info — the
    /// 2-request property `card-url` states, pinned with `extra` present.
    #[test]
    fn card_tier_stays_two_requests_with_extra_present() {
        use std::sync::Mutex;
        struct Counting {
            data: Vec<u8>,
            reads: Mutex<Vec<(u64, u64)>>,
        }
        impl rete_core::RangeReader for Counting {
            fn len(&self) -> u64 {
                self.data.len() as u64
            }
            fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
                self.reads.lock().unwrap().push((offset, len));
                let start = offset as usize;
                let end = start
                    .checked_add(len as usize)
                    .filter(|&e| e <= self.data.len())
                    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))?;
                Ok(self.data[start..end].to_vec())
            }
        }

        let input: CardInput = serde_json::from_str(
            r#"{"title":"T","extra":{"atlas:layer":"84","review":{"by":"dg","status":"ok"}}}"#,
        )
        .unwrap();
        let quads = enriched_fixture();
        let card = derive_card(&quads, 12, 0, input);
        let (image, _) =
            rete_core::ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| {
                card.to_json_bytes()
            });
        // Plus an adjacent build-info section, like every real card build.
        let image =
            rete_core::attach_build_info(&image, br#"{"schema":1,"builder":"test"}"#).unwrap();

        let reader = Counting {
            data: image,
            reads: Mutex::new(Vec::new()),
        };
        let read = load_card_and_build_ranged(&reader).unwrap();
        let got = read.card.expect("card present");
        assert_eq!(got.extra.get("atlas:layer"), Some(&serde_json::json!("84")));
        assert!(
            read.build.is_some(),
            "build info came out of the same range"
        );
        // No TEXT_INDEX section: the signal is answered by the header alone.
        assert_eq!(got.signals.text_index, Some(TextIndexSignal::default()));

        let reads = reader.reads.lock().unwrap();
        assert_eq!(
            reads.len(),
            2,
            "CARD tier = header + ONE coalesced range, extra included: {reads:?}"
        );
        assert_eq!(reads[0], (0, rete_core::HEADER_LEN as u64));
    }

    /// A card whose own bytes claim a full-text index the file does not have is
    /// the exact drift #189 had to repair by hand, one level down. Our writers
    /// cannot produce it ([`DatasetCard::to_json_bytes`] strips the field), but a
    /// third-party writer can — so the reader measures, overrides, and hands the
    /// stale claim back for `card-audit` to report.
    #[test]
    fn a_card_claiming_an_index_the_file_lacks_is_reported_as_drift() {
        let quads = enriched_fixture();
        let mut card = derive_card(&quads, 12, 0, CardInput::default());
        // Serialized the way a FOREIGN writer would — `to_json_bytes` would
        // strip exactly the field this test needs to smuggle in.
        card.signals.text_index = Some(TextIndexSignal {
            present: true,
            bytes: 999_999,
            token_table_bytes: Some(1_234),
        });
        let blob = serde_json::to_vec(&card).unwrap();
        assert!(String::from_utf8_lossy(&blob).contains("text_index"));

        let (image, _) =
            rete_core::ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| {
                blob.clone()
            });
        let read = load_card_and_build_ranged(&rete_core::SliceReader::new(&image)).unwrap();

        // The file has no kind-6 section, so that is what the card now says…
        assert_eq!(
            read.card.unwrap().signals.text_index,
            Some(TextIndexSignal::default())
        );
        // …and the claim it displaced is reported rather than silently dropped.
        let stale = read.stored_text_index.expect("the drift is reported");
        assert!(stale.present);
        assert_eq!(stale.bytes, 999_999);
        // `card-audit --json` surfaces it under `card_said`, beside the measurement.
        let doc = text_index_json(Some(read.text_index), read.stored_text_index);
        assert_eq!(doc["present"], serde_json::json!(false));
        assert_eq!(doc["card_said"]["present"], serde_json::json!(true));
    }

    /// The same budget with a TEXT_INDEX present: one extra read, of **≤10
    /// bytes**, at the section's own offset — the leading length varint and
    /// nothing else. Measuring a 1.88 GB index must not cost 1.88 GB, nor even
    /// the token table it measures.
    #[test]
    fn measuring_the_text_index_costs_one_tiny_read() {
        use std::sync::Mutex;

        struct Counting {
            data: Vec<u8>,
            reads: Mutex<Vec<(u64, u64)>>,
        }
        impl rete_core::RangeReader for Counting {
            fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
                self.reads.lock().unwrap().push((offset, len));
                let start = offset as usize;
                let end = (start + len as usize).min(self.data.len());
                Ok(self.data[start..end].to_vec())
            }
            fn len(&self) -> u64 {
                self.data.len() as u64
            }
        }

        let quads = enriched_fixture();
        let card = derive_card(&quads, 12, 0, CardInput::default());
        let (image, _) =
            rete_core::ingest::assemble_dataset_with_opts(quads, true, true, None, |_, _| {
                card.to_json_bytes()
            });
        let header = Header::from_bytes(&image).unwrap();
        assert!(header.text_index_len > 0, "the fixture is indexed");

        let reader = Counting {
            data: image,
            reads: Mutex::new(Vec::new()),
        };
        let read = load_card_and_build_ranged(&reader).unwrap();
        let signal = read.text_index;
        assert!(signal.present);
        assert_eq!(signal.bytes, header.text_index_len);
        let table = signal.token_table_bytes.expect("token table measured");
        assert!(table > 0 && table < signal.bytes);

        let reads = reader.reads.lock().unwrap();
        assert_eq!(reads.len(), 3, "header + card range + the probe: {reads:?}");
        let (offset, len) = reads[2];
        assert_eq!(
            offset, header.text_index_offset,
            "the probe reads the section's FIRST bytes"
        );
        assert!(len <= 10, "a uvarint is at most 10 bytes, not {len}");
    }

    /// `--description` overrides the card file, so the cap has to be applied to
    /// the text that actually lands in the card — after the override, not before.
    #[test]
    fn the_description_flag_is_bounded_like_the_card_file() {
        let args = CardArgs {
            description: Some("x".repeat(rete_core::card::CARD_DESCRIPTION_MAX_BYTES + 1)),
            ..Default::default()
        };
        let err = load_curated(&args).unwrap_err().to_string();
        assert!(err.contains("`description`"), "{err}");
        assert!(err.contains("over the"), "{err}");

        let ok = load_curated(&CardArgs {
            description: Some("A short one.".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(ok.description.as_deref(), Some("A short one."));
    }

    #[test]
    fn flags_override_card_file_fields() {
        // No file, just flags → flags populate curated fields.
        let args = CardArgs {
            title: Some("Flag Title".into()),
            license: Some("MIT".into()),
            ..Default::default()
        };
        assert!(args.requested());
        let curated = load_curated(&args).unwrap();
        assert_eq!(curated.title.as_deref(), Some("Flag Title"));
        assert_eq!(curated.license.as_deref(), Some("MIT"));

        // No flags at all → not requested.
        assert!(!CardArgs::default().requested());
    }

    #[test]
    fn hex16_formats_lowercase_padded() {
        let mut b = [0u8; 16];
        b[0] = 0x0a;
        b[15] = 0xff;
        let h = hex16(&b);
        assert_eq!(h.len(), 32);
        assert!(h.starts_with("0a"));
        assert!(h.ends_with("ff"));
    }
}
