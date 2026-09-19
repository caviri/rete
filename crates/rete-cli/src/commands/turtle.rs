//! Streaming, prefix-compressed Turtle / TriG serialization.
//!
//! # What this is for
//!
//! `rete export --format ttl` used to collect every triple into a `Vec`, build a
//! `BTreeMap` over all of it, render one giant `String` and print that. Peak
//! memory was O(graph), which on the files this tool exists for (tens of GB) is
//! not a serialization strategy, it is an OOM. It also emitted no `@prefix`
//! declarations at all, so its output was *larger* than the N-Triples it was
//! supposedly an abbreviation of.
//!
//! This module replaces both halves: a writer that holds **one statement** of
//! state, and a prefix table that turns the repeated namespace bytes — the bulk
//! of an RDF dump — into two-or-three-character QNames.
//!
//! # Constant memory
//!
//! [`TurtleWriter`] keeps the current subject and predicate (two `String`s,
//! reused) and three scratch buffers for the abbreviated terms. Nothing else
//! grows with the graph. Subject grouping — the `s p o ; p o .` shape that makes
//! Turtle smaller than N-Triples — works because the index scan arrives already
//! grouped by subject, so "have we changed subject?" is a string compare against
//! one held value rather than a map over the whole graph.
//!
//! That grouping is a property of the *routed permutation*, not a law, so the
//! caller passes [`Grouping`] after asking the engine which permutation the scan
//! will use (see `export::subject_grouped`). When the answer is "not grouped"
//! the writer degrades to one statement per line, which is still valid Turtle
//! and still prefix-compressed — just without the subject sharing.
//!
//! # Where the `@prefix` lines go
//!
//! A streaming writer cannot know which prefixes a graph uses before it has
//! written it, and buffering the whole document to find out is the thing this
//! module exists to avoid. Turtle and TriG both allow a directive **anywhere a
//! statement may appear**, and a binding takes effect for everything after it
//! (Turtle 1.1 §2.4, `turtleDoc ::= statement*`, `statement ::= directive |
//! triples '.'`). So a prefix is declared lazily, at the moment it is first
//! needed, and never at all if it is not — which is exactly the "only emit a
//! `@prefix` line for a prefix actually used" rule, obtained in one pass.
//!
//! In practice the declarations still end up in a block at the top, because the
//! caller primes the table from a bounded sample of the scan before writing any
//! data (see `export::sample_namespaces`). The lazy path is what catches a
//! namespace that only appears after the sample.
//!
//! TriG is stricter: `trigDoc ::= (directive | block)*`, so a directive may not
//! sit **inside** a `GRAPH … { }` block. When a new prefix is needed mid-block
//! the writer therefore closes the block, emits the directive, and reopens a
//! block for the same graph. Repeating a graph label is legal TriG — the blocks
//! union — and it can happen at most once per prefix in the table, so the cost
//! is bounded by the table size (tens of lines), not by the data.
//!
//! # Conservative QNames
//!
//! An IRI is only abbreviated when the part after the namespace is legal
//! `PN_LOCAL` (Turtle 1.1 §6.5) *without* needing backslash escapes. Anything
//! else is written as a full `<iri>`. A wrong QName produces a file that will
//! not parse, and a missed one produces a file that is merely slightly larger;
//! those costs are not symmetric, so [`is_pn_local`] errs toward the full form.

use std::io::{self, Write};

/// `rdf:type`, as the canonical N-Triples token the dictionary stores. Turtle
/// abbreviates it to the keyword `a`, which is both idiomatic and the single
/// biggest per-statement saving in a type-heavy graph.
pub(crate) const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

/// Namespaces that get a conventional prefix name when they appear.
///
/// These are seeded rather than derived so that a dump of a well-known
/// vocabulary comes out with the prefix a reader expects (`rdfs:label`, not
/// `ns7:label`). Namespaces *not* in this table are picked up from the data
/// instead — see [`PrefixTable::from_sample`] — because on real files the
/// dataset's own namespace is usually an order of magnitude more frequent than
/// any standard vocabulary, and that is where the compression actually is.
///
/// Order is irrelevant (the table is sorted on construction); duplicates on the
/// *namespace* are not allowed and are caught by a test.
pub(crate) const WELL_KNOWN: &[(&str, &str)] = &[
    // RDF core.
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("sh", "http://www.w3.org/ns/shacl#"),
    // Description / cataloguing.
    ("dc", "http://purl.org/dc/elements/1.1/"),
    ("dct", "http://purl.org/dc/terms/"),
    ("dcat", "http://www.w3.org/ns/dcat#"),
    ("void", "http://rdfs.org/ns/void#"),
    ("skos", "http://www.w3.org/2004/02/skos/core#"),
    ("prov", "http://www.w3.org/ns/prov#"),
    ("foaf", "http://xmlns.com/foaf/0.1/"),
    ("vcard", "http://www.w3.org/2006/vcard/ns#"),
    ("org", "http://www.w3.org/ns/org#"),
    ("adms", "http://www.w3.org/ns/adms#"),
    ("time", "http://www.w3.org/2006/time#"),
    ("geo", "http://www.w3.org/2003/01/geo/wgs84_pos#"),
    ("oa", "http://www.w3.org/ns/oa#"),
    ("schema", "http://schema.org/"),
    ("schemas", "https://schema.org/"),
    // Bibliographic / scholarly vocabularies.
    ("bibo", "http://purl.org/ontology/bibo/"),
    ("cito", "http://purl.org/spar/cito/"),
    ("fabio", "http://purl.org/spar/fabio/"),
    ("datacite", "http://purl.org/spar/datacite/"),
    ("pro", "http://purl.org/spar/pro/"),
    ("frapo", "http://purl.org/cerif/frapo/"),
    // Scholarly identifier namespaces: these are *entity* namespaces, so they
    // abbreviate subjects and objects rather than predicates — which on a
    // citation or author graph is most of the bytes.
    ("doi", "https://doi.org/"),
    ("orcid", "https://orcid.org/"),
    ("ror", "https://ror.org/"),
    ("openalex", "https://openalex.org/"),
    // Wikibase / Wikidata, the most common shape of a large public dump.
    ("wd", "http://www.wikidata.org/entity/"),
    ("wdt", "http://www.wikidata.org/prop/direct/"),
    ("wds", "http://www.wikidata.org/entity/statement/"),
    ("wdref", "http://www.wikidata.org/reference/"),
    ("wdv", "http://www.wikidata.org/value/"),
    ("p", "http://www.wikidata.org/prop/"),
    ("ps", "http://www.wikidata.org/prop/statement/"),
    ("pq", "http://www.wikidata.org/prop/qualifier/"),
    ("wikibase", "http://wikiba.se/ontology#"),
];

// ---------------------------------------------------------------------------
// PN_LOCAL / PN_PREFIX — the Turtle 1.1 §6.5 terminals, conservatively applied
// ---------------------------------------------------------------------------

/// `PN_CHARS_BASE` — the letter class Turtle names may start with.
fn is_pn_chars_base(c: char) -> bool {
    matches!(c,
        'A'..='Z' | 'a'..='z'
        | '\u{00C0}'..='\u{00D6}' | '\u{00D8}'..='\u{00F6}' | '\u{00F8}'..='\u{02FF}'
        | '\u{0370}'..='\u{037D}' | '\u{037F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// `PN_CHARS_U` = `PN_CHARS_BASE | '_'`.
fn is_pn_chars_u(c: char) -> bool {
    is_pn_chars_base(c) || c == '_'
}

/// `PN_CHARS` = `PN_CHARS_U | '-' | [0-9] | #xB7 | [#x300-#x36F] | [#x203F-#x2040]`.
fn is_pn_chars(c: char) -> bool {
    is_pn_chars_u(c)
        || c == '-'
        || c.is_ascii_digit()
        || c == '\u{00B7}'
        || matches!(c, '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}

/// Is `s` a legal `PN_LOCAL` that needs **no** backslash escaping?
///
/// ```text
/// PN_LOCAL ::= (PN_CHARS_U | ':' | [0-9] | PLX)
///              ((PN_CHARS | '.' | ':' | PLX)* (PN_CHARS | ':' | PLX))?
/// PLX      ::= PERCENT | PN_LOCAL_ESC
/// PERCENT  ::= '%' HEX HEX
/// ```
///
/// Two deliberate narrowings of the grammar, both in the safe direction:
///
/// * `PLX` is accepted only in its `PERCENT` form. A percent escape is extremely
///   common in real IRIs (`.../Maseka%27s_paper`) and passes through a QName
///   unchanged, so accepting it is free compression. `PN_LOCAL_ESC` — writing
///   `\-` or `\,` — is *legal* Turtle but is the shape most likely to trip a
///   lenient-but-not-conformant consumer, and the alternative (a full `<iri>`)
///   costs only bytes. So we never emit one, and therefore never accept a local
///   part that would need one.
/// * The empty local part is rejected. `ex:` alone is grammatical, but an IRI
///   equal to a namespace is rare enough that it is not worth the special case.
///
/// The last character may not be `.` — that is the grammar's own rule (the final
/// alternative excludes `.`), and it exists because a trailing dot would be read
/// as the statement terminator.
pub(crate) fn is_pn_local(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let b = s.as_bytes();
    let mut i = 0usize;
    let mut first = true;
    // Whether the character just consumed is one the grammar allows LAST.
    let mut final_ok = false;
    while i < b.len() {
        if b[i] == b'%' {
            // PERCENT ::= '%' HEX HEX — all three bytes are ASCII, so indexing
            // by byte is safe and cannot split a code point.
            if i + 2 >= b.len() || !b[i + 1].is_ascii_hexdigit() || !b[i + 2].is_ascii_hexdigit() {
                return false;
            }
            i += 3;
            first = false;
            final_ok = true;
            continue;
        }
        // Not a percent escape: decode one character.
        let c = match s[i..].chars().next() {
            Some(c) => c,
            None => return false,
        };
        let ok = if first {
            is_pn_chars_u(c) || c == ':' || c.is_ascii_digit()
        } else {
            is_pn_chars(c) || c == '.' || c == ':'
        };
        if !ok {
            return false;
        }
        final_ok = is_pn_chars(c) || c == ':';
        i += c.len_utf8();
        first = false;
    }
    final_ok
}

/// Is `s` a legal `PN_PREFIX` — the name to the left of the colon?
///
/// ```text
/// PN_PREFIX ::= PN_CHARS_BASE ((PN_CHARS | '.')* PN_CHARS)?
/// ```
///
/// Note this is stricter than `PN_LOCAL` at both ends: it must start with a
/// *letter* (no `_`, no digit, no colon) and may not end with `.`.
pub(crate) fn is_pn_prefix(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if is_pn_chars_base(c) => {}
        _ => return false,
    }
    let mut last = 'a';
    for c in chars {
        if !(is_pn_chars(c) || c == '.') {
            return false;
        }
        last = c;
    }
    last != '.'
}

// ---------------------------------------------------------------------------
// Literal parsing
// ---------------------------------------------------------------------------

/// Byte index of the closing `"` of an N-Triples literal that starts at index 0,
/// or `None` if the token is not a well-formed quoted string.
///
/// This walks the escapes. The naive `token[1..].find('"')` truncates at the
/// first *embedded* quote — the exact bug fixed for the SPARQL string functions
/// in #242/#243 — which here would misread `"he said \"no\""^^<xsd:string>` as
/// the literal `he said \` with trailing garbage, and emit a file that does not
/// parse. A backslash always escapes the next byte in N-Triples, so skipping two
/// bytes on `\` is sufficient and needs no table of which escapes are legal:
/// the token came out of the dictionary already valid, and this only has to find
/// its end, not re-validate it.
pub(crate) fn literal_end(token: &str) -> Option<usize> {
    let b = token.as_bytes();
    if b.first() != Some(&b'"') {
        return None;
    }
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The prefix table
// ---------------------------------------------------------------------------

/// One prefix binding, plus whether its `@prefix` line has been written yet.
struct Entry {
    prefix: String,
    ns: String,
    declared: bool,
}

/// The bindings available to a writer, sorted by namespace so the longest
/// matching namespace for an IRI is found by binary search.
pub(crate) struct PrefixTable {
    entries: Vec<Entry>,
}

/// How many distinct namespaces the bounded sample will track before it stops
/// learning new ones. A dump with more than this many distinct namespaces in its
/// sample is not one prefix compression can help much anyway, and the cap is
/// what keeps the sample's memory a constant rather than a function of the data.
pub(crate) const MAX_SAMPLED_NAMESPACES: usize = 4096;

/// Namespaces observed in the bounded sample, with how often each appeared.
#[derive(Default)]
pub(crate) struct NamespaceSample {
    counts: std::collections::HashMap<String, u64>,
    /// Terms examined — the denominator for the relative-frequency threshold.
    pub(crate) terms: u64,
}

impl NamespaceSample {
    /// Record one canonical N-Triples term token.
    ///
    /// Only `<iri>` tokens whose local part would actually abbreviate are
    /// counted: a namespace whose locals all fail [`is_pn_local`] would earn a
    /// `@prefix` line that nothing could ever use.
    pub(crate) fn observe(&mut self, token: &str) {
        self.terms += 1;
        if let Some(iri) = token
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
        {
            self.observe_iri(iri);
            return;
        }
        // A typed literal carries an IRI too, and it is one of the most
        // repeated IRIs in a graph — every `xsd:integer` in a numeric-heavy
        // dump is 40-odd bytes that `^^xsd:integer` turns into 13. Skipping the
        // token because it does not start with `<` would leave that on the table.
        if let Some(end) = literal_end(token) {
            if let Some(dt) = token[end + 1..]
                .strip_prefix("^^<")
                .and_then(|rest| rest.strip_suffix('>'))
            {
                self.observe_iri(dt);
            }
        }
    }

    /// Count one bare IRI, if a prefix binding could ever abbreviate it.
    fn observe_iri(&mut self, iri: &str) {
        let Some(split) = namespace_split(iri) else {
            return;
        };
        // A namespace whose local parts never form a legal QName would earn a
        // `@prefix` line that nothing could use.
        if !is_pn_local(&iri[split..]) {
            return;
        }
        let ns = &iri[..split];
        if let Some(c) = self.counts.get_mut(ns) {
            *c += 1;
        } else if self.counts.len() < MAX_SAMPLED_NAMESPACES {
            self.counts.insert(ns.to_string(), 1);
        }
    }
}

/// Where a namespace ends inside a bare IRI: just past the last `#` or `/`.
///
/// Returns `None` when there is nothing usable — no separator, or a separator so
/// late that the local part would be empty, or so early that the "namespace"
/// would be a bare scheme (`http://`), which abbreviates nothing and produces a
/// prefix name nobody wants.
fn namespace_split(iri: &str) -> Option<usize> {
    let cut = iri.rfind(['#', '/'])? + 1;
    if cut >= iri.len() {
        return None;
    }
    // `http://` is 7 bytes; anything shorter than that plus a character is not a
    // namespace worth binding.
    if cut < 8 {
        return None;
    }
    Some(cut)
}

impl PrefixTable {
    /// An empty table — no abbreviation at all. Used by `--no-prefixes`.
    pub(crate) fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Build the table for a dump from what a bounded sample of it contained.
    ///
    /// Two sources, in priority order:
    ///
    /// 1. every [`WELL_KNOWN`] namespace that appeared at all, so standard
    ///    vocabularies keep their conventional names;
    /// 2. the most frequent remaining namespaces, up to `max_auto`, each given a
    ///    name derived from its own last path segment.
    ///
    /// Source 2 is where the compression is. On the files this was measured
    /// against, the dataset's own entity namespace outnumbers every standard
    /// vocabulary by 10:1 or more — a table of nothing but well-known prefixes
    /// abbreviates the rarest terms and leaves the common ones at full length.
    ///
    /// `min_count` is an absolute floor: a namespace seen once or twice in the
    /// sample would spend a whole `@prefix` line to save a few bytes.
    pub(crate) fn from_sample(sample: &NamespaceSample, max_auto: usize, min_count: u64) -> Self {
        let mut entries: Vec<Entry> = Vec::new();
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (prefix, ns) in WELL_KNOWN {
            if sample.counts.contains_key(*ns) {
                taken.insert((*prefix).to_string());
                entries.push(Entry {
                    prefix: (*prefix).to_string(),
                    ns: (*ns).to_string(),
                    declared: false,
                });
            }
        }
        let known: std::collections::HashSet<&str> = WELL_KNOWN.iter().map(|(_, ns)| *ns).collect();

        // Sort candidates by descending count, then by namespace, so the table —
        // and therefore the output — is a deterministic function of the sample
        // and does not depend on hash iteration order.
        let mut candidates: Vec<(&String, &u64)> = sample
            .counts
            .iter()
            .filter(|(ns, c)| **c >= min_count && !known.contains(ns.as_str()))
            .collect();
        candidates.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

        for (ns, _) in candidates.into_iter().take(max_auto) {
            let name = unique_prefix_name(ns, &mut taken);
            entries.push(Entry {
                prefix: name,
                ns: ns.clone(),
                declared: false,
            });
        }

        entries.sort_by(|a, b| a.ns.cmp(&b.ns));
        Self { entries }
    }

    /// The table's bindings, as `(prefix, namespace)` — for tests and for the
    /// `--show-prefixes` diagnostic.
    pub(crate) fn bindings(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|e| (e.prefix.as_str(), e.ns.as_str()))
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The entry whose namespace is the longest prefix of `iri`, if any.
    ///
    /// The entries are sorted by namespace, so the candidate is at or just below
    /// `iri`'s insertion point; walking back from there finds the longest match
    /// first. In practice this stops on the first or second probe — namespaces
    /// that are prefixes of one another are rare.
    fn lookup(&self, iri: &str) -> Option<usize> {
        let at = self.entries.partition_point(|e| e.ns.as_str() <= iri);
        (0..at)
            .rev()
            .find(|&i| iri.starts_with(&self.entries[i].ns))
    }
}

/// A readable, legal `PN_PREFIX` derived from a namespace, unique within `taken`.
///
/// `http://data.europa.eu/s66#` becomes `s66`; `.../resource/authors/` becomes
/// `authors`. The derived name is cosmetic — any name would serialize the same
/// bytes of *data* — but a dump a person has to read is worth the few lines.
fn unique_prefix_name(ns: &str, taken: &mut std::collections::HashSet<String>) -> String {
    let base = derive_prefix_name(ns);
    if taken.insert(base.clone()) {
        return base;
    }
    for n in 2u32.. {
        let candidate = format!("{base}{n}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the integers are not exhausted")
}

/// The un-deduplicated stem of [`unique_prefix_name`].
fn derive_prefix_name(ns: &str) -> String {
    // Strip the trailing separator, then take the last path/fragment segment.
    let trimmed = ns.trim_end_matches(['#', '/']);
    let seg = trimmed
        .rsplit(['#', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or("");
    // Keep only characters that can appear in a prefix name, lowercase them, and
    // drop a leading run that is not a letter (PN_PREFIX must start with one).
    let mut out = String::new();
    for c in seg.chars() {
        let c = c.to_ascii_lowercase();
        if out.is_empty() {
            if is_pn_chars_base(c) {
                out.push(c);
            }
        } else if is_pn_chars(c) {
            out.push(c);
        }
    }
    while out.ends_with('.') {
        out.pop();
    }
    out.truncate(24);
    if out.is_empty() || !is_pn_prefix(&out) {
        "ns".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// The writer
// ---------------------------------------------------------------------------

/// Whether the incoming statement stream is grouped by subject.
///
/// This is not a preference — it is a fact about the scan the caller is about to
/// run, and getting it wrong produces either a wrong document (claiming grouping
/// that is not there splits one subject across many blocks, which is merely
/// verbose) or unbounded memory (buffering to *create* grouping, which is the
/// thing this module refuses to do).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Grouping {
    /// Consecutive statements share a subject: emit `s\n  p o ;\n  p o .`.
    BySubject,
    /// No order guarantee: emit one `s p o .` per line. Still valid Turtle, and
    /// still prefix-compressed — just without the subject sharing.
    PerStatement,
}

/// Streaming Turtle / TriG writer.
pub(crate) struct TurtleWriter<W: Write> {
    out: W,
    table: PrefixTable,
    grouping: Grouping,
    /// The subject of the block currently open, if one is.
    subject: String,
    /// The predicate of the object list currently open.
    predicate: String,
    open: bool,
    /// The graph label of the `GRAPH … { }` block currently open (TriG).
    graph: Option<String>,
    /// Abbreviated forms of the statement being written. Reused every row so a
    /// 100M-statement dump performs no per-statement allocation.
    sb: String,
    pb: String,
    ob: String,
    /// Table entries this statement needs that have not been declared yet.
    pending: Vec<usize>,
}

impl<W: Write> TurtleWriter<W> {
    pub(crate) fn new(out: W, table: PrefixTable, grouping: Grouping) -> Self {
        Self {
            out,
            table,
            grouping,
            subject: String::new(),
            predicate: String::new(),
            open: false,
            graph: None,
            sb: String::new(),
            pb: String::new(),
            ob: String::new(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn table(&self) -> &PrefixTable {
        &self.table
    }

    /// Write the `@prefix` line for every binding in the table, now.
    ///
    /// Called before any data so the declarations form the conventional block at
    /// the top of the document. It is an optimisation of presentation only: a
    /// binding left undeclared here is declared lazily the first time it is used,
    /// and a binding declared here that is never used costs one line.
    ///
    /// The caller primes the table from a bounded sample of the very scan it is
    /// about to write, so "declared here but unused" is possible in principle
    /// (the sample saw a namespace the rest of the scan does not) but does not
    /// happen in practice — the sample is a prefix of the scan.
    pub(crate) fn declare_all(&mut self) -> io::Result<()> {
        for i in 0..self.table.entries.len() {
            self.declare(i)?;
        }
        Ok(())
    }

    fn declare(&mut self, i: usize) -> io::Result<()> {
        if self.table.entries[i].declared {
            return Ok(());
        }
        let e = &self.table.entries[i];
        writeln!(self.out, "@prefix {}: <{}> .", e.prefix, e.ns)?;
        self.table.entries[i].declared = true;
        Ok(())
    }

    /// Open a `GRAPH <label> { … }` block. `label` is a canonical N-Triples term
    /// (`<iri>` or `_:b`), which is already valid TriG `labelOrSubject` syntax.
    pub(crate) fn begin_graph(&mut self, label: &str) -> io::Result<()> {
        self.close_statement()?;
        writeln!(self.out, "GRAPH {label} {{")?;
        self.graph = Some(label.to_string());
        Ok(())
    }

    /// Close the open `GRAPH` block.
    pub(crate) fn end_graph(&mut self) -> io::Result<()> {
        self.close_statement()?;
        if self.graph.take().is_some() {
            writeln!(self.out, "}}")?;
        }
        Ok(())
    }

    /// Terminate the statement currently being built, if any.
    fn close_statement(&mut self) -> io::Result<()> {
        if self.open {
            self.out.write_all(b" .\n")?;
            self.open = false;
            self.subject.clear();
            self.predicate.clear();
        }
        Ok(())
    }

    /// Write one statement. `s`, `p` and `o` are canonical N-Triples term tokens.
    pub(crate) fn write_triple(&mut self, s: &str, p: &str, o: &str) -> io::Result<()> {
        self.pending.clear();
        abbreviate(&self.table, &mut self.pending, &mut self.sb, s);
        if p == RDF_TYPE {
            self.pb.clear();
            self.pb.push('a');
        } else {
            abbreviate(&self.table, &mut self.pending, &mut self.pb, p);
        }
        abbreviate(&self.table, &mut self.pending, &mut self.ob, o);

        if !self.pending.is_empty() {
            self.flush_declarations()?;
        }

        // The buffers cannot be borrowed across `write!` on `self.out` (both are
        // `self`), so the writes go through raw byte slices taken one at a time.
        match self.grouping {
            Grouping::PerStatement => {
                self.out.write_all(self.sb.as_bytes())?;
                self.out.write_all(b" ")?;
                self.out.write_all(self.pb.as_bytes())?;
                self.out.write_all(b" ")?;
                self.out.write_all(self.ob.as_bytes())?;
                self.out.write_all(b" .\n")?;
            }
            Grouping::BySubject => {
                if !self.open || self.subject != self.sb {
                    self.close_statement()?;
                    self.out.write_all(self.sb.as_bytes())?;
                    self.out.write_all(b"\n  ")?;
                    self.out.write_all(self.pb.as_bytes())?;
                    self.out.write_all(b" ")?;
                    self.out.write_all(self.ob.as_bytes())?;
                    self.subject.clear();
                    self.subject.push_str(&self.sb);
                    self.predicate.clear();
                    self.predicate.push_str(&self.pb);
                    self.open = true;
                } else if self.predicate != self.pb {
                    self.out.write_all(b" ;\n  ")?;
                    self.out.write_all(self.pb.as_bytes())?;
                    self.out.write_all(b" ")?;
                    self.out.write_all(self.ob.as_bytes())?;
                    self.predicate.clear();
                    self.predicate.push_str(&self.pb);
                } else {
                    self.out.write_all(b" ,\n    ")?;
                    self.out.write_all(self.ob.as_bytes())?;
                }
            }
        }
        Ok(())
    }

    /// Emit the `@prefix` lines this statement needs.
    ///
    /// A directive may not appear inside a TriG `GRAPH` block, so when one is
    /// open it is closed and reopened around the declarations. Two blocks for the
    /// same graph are legal TriG and denote the union, and this happens at most
    /// once per table entry over the whole document.
    fn flush_declarations(&mut self) -> io::Result<()> {
        self.close_statement()?;
        let reopen = self.graph.clone();
        if reopen.is_some() {
            self.out.write_all(b"}\n")?;
        }
        let pending = std::mem::take(&mut self.pending);
        for i in pending {
            self.declare(i)?;
        }
        if let Some(g) = reopen {
            writeln!(self.out, "GRAPH {g} {{")?;
        }
        Ok(())
    }

    /// Terminate the document and hand back the sink.
    ///
    /// The sink is returned rather than dropped so the caller can flush it and
    /// *propagate* the error: a `BufWriter` dropped on the floor swallows the
    /// failure of its final write, which is how a truncated dump gets mistaken
    /// for a complete one.
    pub(crate) fn finish(mut self) -> io::Result<W> {
        self.end_graph()?;
        self.close_statement()?;
        Ok(self.out)
    }
}

/// Render one canonical N-Triples term into `buf` as Turtle, recording in
/// `pending` any table entry whose declaration the result depends on.
///
/// Three shapes:
///
/// * `<iri>` — a QName when the table has a namespace for it and the remainder
///   is legal `PN_LOCAL`, else the token verbatim (already valid Turtle).
/// * `"lit"`, `"lit"@en`, `"lit"^^<dt>` — the lexical form and any language tag
///   pass through byte for byte (N-Triples literal escaping is a subset of
///   Turtle's), and only the datatype IRI is considered for abbreviation. The
///   literal's own quoting is never re-derived, so no escaping bug can be
///   introduced here.
/// * anything else (`_:b`, a quoted triple) — verbatim.
fn abbreviate(table: &PrefixTable, pending: &mut Vec<usize>, buf: &mut String, token: &str) {
    buf.clear();
    // An RDF-star quoted triple (`<< s p o >>`) also starts with `<` and ends
    // with `>`, and its inner terms are a whole statement, not an IRI. It is
    // written verbatim: the token is already valid Turtle-star, and abbreviating
    // inside it would mean re-tokenizing a nested statement for a saving that no
    // file this serializer targets is made of.
    if rete_core::terms::is_quoted_triple(token) {
        buf.push_str(token);
        return;
    }
    if let Some(iri) = token
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
    {
        if !push_qname(table, pending, buf, iri) {
            buf.push_str(token);
        }
        return;
    }
    // A literal with a datatype: abbreviate the datatype IRI only.
    if let Some(end) = literal_end(token) {
        let rest = &token[end + 1..];
        if let Some(dt) = rest
            .strip_prefix("^^<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            buf.push_str(&token[..=end]);
            buf.push_str("^^");
            if !push_qname(table, pending, buf, dt) {
                buf.push('<');
                buf.push_str(dt);
                buf.push('>');
            }
            return;
        }
    }
    buf.push_str(token);
}

/// Append `iri` to `buf` as `prefix:local`, or return `false` and leave `buf`
/// unchanged from its entry length when no safe QName exists.
fn push_qname(table: &PrefixTable, pending: &mut Vec<usize>, buf: &mut String, iri: &str) -> bool {
    let Some(i) = table.lookup(iri) else {
        return false;
    };
    let local = &iri[table.entries[i].ns.len()..];
    if !is_pn_local(local) {
        return false;
    }
    if !table.entries[i].declared && !pending.contains(&i) {
        pending.push(i);
    }
    buf.push_str(&table.entries[i].prefix);
    buf.push(':');
    buf.push_str(local);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_of(pairs: &[(&str, &str)]) -> PrefixTable {
        let mut entries: Vec<Entry> = pairs
            .iter()
            .map(|(p, n)| Entry {
                prefix: (*p).to_string(),
                ns: (*n).to_string(),
                declared: false,
            })
            .collect();
        entries.sort_by(|a, b| a.ns.cmp(&b.ns));
        PrefixTable { entries }
    }

    fn render(table: PrefixTable, grouping: Grouping, rows: &[(&str, &str, &str)]) -> String {
        let mut w = TurtleWriter::new(Vec::new(), table, grouping);
        for (s, p, o) in rows {
            w.write_triple(s, p, o).unwrap();
        }
        String::from_utf8(w.finish().unwrap()).unwrap()
    }

    // --- PN_LOCAL ---------------------------------------------------------

    #[test]
    fn pn_local_accepts_the_ordinary_shapes() {
        for s in [
            "type",
            "Person",
            "_under",
            "a1",
            "1starts_with_a_digit", // legal: the first-char class includes [0-9]
            "has.dot.inside",
            "has-dash",
            "co:lon",
            "Q42",
            "caf\u{e9}",    // PN_CHARS_BASE covers Latin-1 letters
            "pct%20escape", // PLX in its PERCENT form
            "%41",          // a local part that is nothing but an escape
            "ends_with_colon:",
        ] {
            assert!(is_pn_local(s), "should accept {s:?}");
        }
    }

    #[test]
    fn pn_local_rejects_what_would_not_parse() {
        for s in [
            "",              // empty
            ".leading.dot",  // '.' is not in the first-char class
            "-leading-dash", // nor is '-'
            "trailing.",     // the last char may not be '.'
            "has space",
            "has\"quote",
            "has<angle>",
            "has,comma", // legal only via PN_LOCAL_ESC, which we never emit
            "has;semi",
            "has(paren)",
            "pct%2",  // truncated escape
            "pct%zz", // not hex
            "pct%",   // bare '%'
            "a/slash",
            "a\\backslash",
        ] {
            assert!(!is_pn_local(s), "should reject {s:?}");
        }
    }

    #[test]
    fn pn_prefix_is_stricter_than_pn_local_at_both_ends() {
        assert!(is_pn_prefix("rdf"));
        assert!(is_pn_prefix("s66"));
        assert!(is_pn_prefix("a.b"));
        assert!(!is_pn_prefix(""));
        assert!(
            !is_pn_prefix("_leading"),
            "PN_PREFIX may not start with '_'"
        );
        assert!(!is_pn_prefix("1abc"), "nor with a digit");
        assert!(!is_pn_prefix("abc."), "nor end with '.'");
        assert!(!is_pn_prefix("a:b"), "a colon is not a prefix character");
    }

    // --- literal scanning -------------------------------------------------

    #[test]
    fn literal_end_walks_escapes_rather_than_stopping_at_the_first_quote() {
        assert_eq!(literal_end(r#""plain""#), Some(6));
        // The bug this exists to prevent: an embedded, escaped quote.
        let t = r#""he said \"no\"""#;
        assert_eq!(literal_end(t), Some(t.len() - 1));
        // An escaped backslash immediately before the terminator.
        let t = r#""ends with a backslash \\""#;
        assert_eq!(literal_end(t), Some(t.len() - 1));
        // Not a literal at all.
        assert_eq!(literal_end("<http://ex/x>"), None);
        assert_eq!(literal_end("_:b0"), None);
        // Unterminated.
        assert_eq!(literal_end(r#""no end"#), None);
    }

    #[test]
    fn datatype_is_abbreviated_without_touching_the_lexical_form() {
        let table = table_of(&[("xsd", "http://www.w3.org/2001/XMLSchema#")]);
        let out = render(
            table,
            Grouping::PerStatement,
            &[(
                "<http://ex/s>",
                "<http://ex/p>",
                r#""he said \"no\""^^<http://www.w3.org/2001/XMLSchema#string>"#,
            )],
        );
        assert!(
            out.contains(r#""he said \"no\""^^xsd:string"#),
            "got: {out}"
        );
    }

    #[test]
    fn a_literal_containing_the_datatype_marker_is_not_misparsed() {
        // The lexical form itself contains `"^^<`, which a non-escape-aware
        // scanner would mistake for the datatype separator.
        let table = table_of(&[("xsd", "http://www.w3.org/2001/XMLSchema#")]);
        let token = r#""a \"^^<http://evil>\" b"@en"#;
        let out = render(
            table,
            Grouping::PerStatement,
            &[("<http://ex/s>", "<http://ex/p>", token)],
        );
        assert!(out.contains(token), "literal must pass through: {out}");
        assert!(!out.contains("^^xsd:"), "no datatype here: {out}");
    }

    // --- QName formation --------------------------------------------------

    #[test]
    fn an_unsafe_local_part_falls_back_to_the_full_iri() {
        let table = table_of(&[("ex", "http://ex/")]);
        let out = render(
            table,
            Grouping::PerStatement,
            &[
                ("<http://ex/ok>", "<http://ex/p>", "<http://ex/has space>"),
                ("<http://ex/a>", "<http://ex/b>", "<http://ex/trailing.>"),
            ],
        );
        assert!(out.contains("ex:ok"), "got: {out}");
        assert!(out.contains("<http://ex/has space>"), "got: {out}");
        assert!(out.contains("<http://ex/trailing.>"), "got: {out}");
    }

    #[test]
    fn the_longest_matching_namespace_wins() {
        let table = table_of(&[("short", "http://ex/"), ("long", "http://ex/deep/")]);
        let out = render(
            table,
            Grouping::PerStatement,
            &[("<http://ex/deep/x>", "<http://ex/p>", "<http://ex/y>")],
        );
        assert!(out.contains("long:x"), "got: {out}");
        assert!(out.contains("short:y"), "got: {out}");
    }

    #[test]
    fn rdf_type_becomes_a_and_needs_no_prefix() {
        let out = render(
            PrefixTable::empty(),
            Grouping::PerStatement,
            &[("<http://ex/s>", RDF_TYPE, "<http://ex/T>")],
        );
        assert_eq!(out, "<http://ex/s> a <http://ex/T> .\n");
    }

    #[test]
    fn an_rdf_star_quoted_triple_passes_through() {
        let table = table_of(&[("ex", "http://ex/")]);
        let token = "<<<http://ex/s> <http://ex/p> <http://ex/o>>>";
        let out = render(
            table,
            Grouping::PerStatement,
            &[(token, "<http://ex/said>", token)],
        );
        assert_eq!(
            out,
            format!("@prefix ex: <http://ex/> .
{token} ex:said {token} .
")
        );
    }

    #[test]
    fn a_blank_node_passes_through() {
        let out = render(
            PrefixTable::empty(),
            Grouping::PerStatement,
            &[("_:b0", "<http://ex/p>", "_:b1")],
        );
        assert_eq!(out, "_:b0 <http://ex/p> _:b1 .\n");
    }

    // --- declarations -----------------------------------------------------

    #[test]
    fn only_used_prefixes_are_declared() {
        let table = table_of(&[("used", "http://used/"), ("unused", "http://unused/")]);
        let out = render(
            table,
            Grouping::PerStatement,
            &[("<http://used/s>", "<http://used/p>", "<http://used/o>")],
        );
        assert!(out.contains("@prefix used: <http://used/> ."), "got: {out}");
        assert!(!out.contains("unused"), "got: {out}");
    }

    #[test]
    fn a_declaration_precedes_its_first_use() {
        let table = table_of(&[("ex", "http://ex/")]);
        let out = render(
            table,
            Grouping::PerStatement,
            &[
                ("<http://other/s>", "<http://other/p>", "<http://other/o>"),
                ("<http://ex/s>", "<http://ex/p>", "<http://ex/o>"),
            ],
        );
        let decl = out.find("@prefix ex:").expect("declared");
        let use_ = out.find("ex:s").expect("used");
        assert!(decl < use_, "declaration must come first:\n{out}");
    }

    // --- grouping ---------------------------------------------------------

    #[test]
    fn subject_grouping_shares_subject_and_predicate() {
        let out = render(
            PrefixTable::empty(),
            Grouping::BySubject,
            &[
                ("<http://ex/a>", RDF_TYPE, "<http://ex/T>"),
                ("<http://ex/a>", "<http://ex/p>", "<http://ex/1>"),
                ("<http://ex/a>", "<http://ex/p>", "<http://ex/2>"),
                ("<http://ex/b>", "<http://ex/p>", "<http://ex/3>"),
            ],
        );
        assert_eq!(
            out,
            "<http://ex/a>\n  a <http://ex/T> ;\n  <http://ex/p> <http://ex/1> ,\n    <http://ex/2> .\n\
             <http://ex/b>\n  <http://ex/p> <http://ex/3> .\n"
        );
    }

    #[test]
    fn per_statement_grouping_repeats_the_subject() {
        let out = render(
            PrefixTable::empty(),
            Grouping::PerStatement,
            &[
                ("<http://ex/a>", "<http://ex/p>", "<http://ex/1>"),
                ("<http://ex/a>", "<http://ex/p>", "<http://ex/2>"),
            ],
        );
        assert_eq!(
            out,
            "<http://ex/a> <http://ex/p> <http://ex/1> .\n\
             <http://ex/a> <http://ex/p> <http://ex/2> .\n"
        );
    }

    // --- TriG -------------------------------------------------------------

    #[test]
    fn trig_wraps_statements_in_a_graph_block() {
        let mut w = TurtleWriter::new(Vec::new(), PrefixTable::empty(), Grouping::BySubject);
        w.write_triple("<http://ex/d>", "<http://ex/p>", "<http://ex/o>")
            .unwrap();
        w.begin_graph("<http://g/1>").unwrap();
        w.write_triple("<http://ex/a>", "<http://ex/p>", "<http://ex/o>")
            .unwrap();
        w.end_graph().unwrap();
        let out = String::from_utf8(w.finish().unwrap()).unwrap();
        assert_eq!(
            out,
            "<http://ex/d>\n  <http://ex/p> <http://ex/o> .\n\
             GRAPH <http://g/1> {\n\
             <http://ex/a>\n  <http://ex/p> <http://ex/o> .\n}\n"
        );
    }

    #[test]
    fn a_prefix_first_needed_inside_a_graph_block_closes_and_reopens_it() {
        // TriG forbids a directive inside a block, so the writer must leave the
        // block, declare, and re-enter. Two blocks for one graph are legal and
        // denote the union.
        let table = table_of(&[("late", "http://late/")]);
        let mut w = TurtleWriter::new(Vec::new(), table, Grouping::BySubject);
        w.begin_graph("<http://g/1>").unwrap();
        w.write_triple("<http://early/s>", "<http://early/p>", "<http://early/o>")
            .unwrap();
        w.write_triple("<http://late/s>", "<http://late/p>", "<http://late/o>")
            .unwrap();
        w.end_graph().unwrap();
        let out = String::from_utf8(w.finish().unwrap()).unwrap();
        assert_eq!(
            out,
            "GRAPH <http://g/1> {\n\
             <http://early/s>\n  <http://early/p> <http://early/o> .\n\
             }\n\
             @prefix late: <http://late/> .\n\
             GRAPH <http://g/1> {\n\
             late:s\n  late:p late:o .\n\
             }\n"
        );
    }

    // --- table construction ----------------------------------------------

    #[test]
    fn well_known_namespaces_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (p, ns) in WELL_KNOWN {
            assert!(seen.insert(*ns), "duplicate namespace {ns}");
            assert!(is_pn_prefix(p), "{p} is not a legal PN_PREFIX");
        }
    }

    #[test]
    fn the_sample_prefers_frequent_namespaces_and_names_them_readably() {
        let mut sample = NamespaceSample::default();
        for i in 0..100 {
            sample.observe(&format!("<http://data.europa.eu/s66/resource/grants/{i}>"));
        }
        for i in 0..10 {
            sample.observe(&format!("<http://www.w3.org/2000/01/rdf-schema#p{i}>"));
        }
        // Below the floor: should not earn a line.
        sample.observe("<http://rare.example/x>");

        let table = PrefixTable::from_sample(&sample, 8, 5);
        let got: Vec<(&str, &str)> = table.bindings().collect();
        assert!(
            got.contains(&("rdfs", "http://www.w3.org/2000/01/rdf-schema#")),
            "well-known keeps its conventional name: {got:?}"
        );
        assert!(
            got.contains(&("grants", "http://data.europa.eu/s66/resource/grants/")),
            "the dataset namespace is named from its last segment: {got:?}"
        );
        assert!(
            !got.iter().any(|(_, ns)| *ns == "http://rare.example/"),
            "below the floor: {got:?}"
        );
    }

    #[test]
    fn generated_prefix_names_never_collide() {
        let mut sample = NamespaceSample::default();
        // Two different namespaces whose last segment is the same word.
        for i in 0..50 {
            sample.observe(&format!("<http://a.example/v1/authors/x{i}>"));
            sample.observe(&format!("<http://b.example/v2/authors/x{i}>"));
        }
        let table = PrefixTable::from_sample(&sample, 8, 5);
        let names: Vec<&str> = table.bindings().map(|(p, _)| p).collect();
        let unique: std::collections::HashSet<&&str> = names.iter().collect();
        assert_eq!(names.len(), unique.len(), "collision in {names:?}");
        assert!(names.contains(&"authors"), "{names:?}");
        assert!(names.contains(&"authors2"), "{names:?}");
    }

    #[test]
    fn a_namespace_whose_locals_never_abbreviate_is_not_sampled() {
        let mut sample = NamespaceSample::default();
        for i in 0..50 {
            // Every local part has a space, so no QName could ever be emitted.
            sample.observe(&format!("<http://bad.example/ns/has space {i}>"));
        }
        let table = PrefixTable::from_sample(&sample, 8, 5);
        assert_eq!(table.len(), 0, "{:?}", table.bindings().collect::<Vec<_>>());
    }

    #[test]
    fn the_sample_is_capped() {
        let mut sample = NamespaceSample::default();
        for i in 0..(MAX_SAMPLED_NAMESPACES + 500) {
            sample.observe(&format!("<http://ex{i}.example/ns/local>"));
        }
        assert_eq!(sample.counts.len(), MAX_SAMPLED_NAMESPACES);
    }

    #[test]
    fn a_typed_literal_contributes_its_datatype_namespace() {
        let mut sample = NamespaceSample::default();
        for i in 0..20 {
            sample.observe(&format!(
                r#""{i}"^^<http://www.w3.org/2001/XMLSchema#integer>"#
            ));
        }
        let table = PrefixTable::from_sample(&sample, 8, 5);
        let got: Vec<(&str, &str)> = table.bindings().collect();
        assert!(
            got.contains(&("xsd", "http://www.w3.org/2001/XMLSchema#")),
            "{got:?}"
        );
    }

    #[test]
    fn a_plain_or_lang_tagged_literal_contributes_nothing() {
        let mut sample = NamespaceSample::default();
        for _ in 0..50 {
            sample.observe(r#""just text""#);
            sample.observe(r#""texte"@fr"#);
        }
        assert_eq!(sample.counts.len(), 0);
        assert_eq!(sample.terms, 100);
    }

    #[test]
    fn a_bare_scheme_is_not_a_namespace() {
        assert_eq!(namespace_split("http://ex"), None);
        assert_eq!(namespace_split("http://"), None);
        assert_eq!(namespace_split("urn:x"), None);
        assert_eq!(namespace_split("http://ex/"), None, "empty local part");
        assert_eq!(namespace_split("http://ex/a"), Some(10));
    }
}
