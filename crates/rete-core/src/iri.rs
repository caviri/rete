//! **IRI validity** — what may appear between the angle brackets of an
//! N-Triples / N-Quads `IRIREF`, and the one lossy repair the exporter offers.
//!
//! rete's line-based N-Triples/N-Quads reader is deliberately tolerant: it takes
//! whatever sits between `<` and the next `>` and stores it as a dictionary key.
//! That makes the file a faithful container of what it was given, but it also
//! means a `.rete` can hold IRIs that no strict parser accepts — and
//! `rete export --format nq` then names a grammar it does not emit. Oxigraph,
//! Jena and rapper reject such a dump; a bulk loader rejects the *chunk*, not
//! the line, so one bad IRI can cost a hundred thousand good statements.
//!
//! This module is the single definition of "invalid" used by the build-time
//! audit (`rete build`, `rete validate`) and the export-time repair
//! (`rete export --sanitize-iris`).
//!
//! # What counts as invalid
//!
//! Two documents apply, and both must hold:
//!
//! * the **N-Triples/N-Quads `IRIREF` production**
//!   (`'<' ([^#x00-#x20<>"{}|^`\] | UCHAR)* '>'`), which excludes a fixed set of
//!   characters outright, and
//! * **RFC 3987**, which the same grammar requires the content to satisfy as an
//!   *absolute* IRI (with an optional fragment).
//!
//! The five classes below are what that combination rules out in practice. They
//! are the classes measured in the published `scholar/` exports; each one was
//! seen in real data.
//!
//! | class | example | repairable by escaping |
//! |---|---|---|
//! | [`IriDefect::NotAbsolute`]   | `<noscheme/path>`            | **no** |
//! | [`IriDefect::ForbiddenChar`] | `<http://ex/a b>`, `<http://ex/a"b>` | yes |
//! | [`IriDefect::Bracket`]       | `<http://ex/a[b]>`           | yes |
//! | [`IriDefect::ExtraHash`]     | `<http://ex/c#d#e>`          | yes |
//! | [`IriDefect::BadPercent`]    | `<http://ex/%x>`             | yes |
//!
//! # What this deliberately does not judge
//!
//! * **Non-ASCII.** RFC 3987 admits `ucschar`, so `<http://ex/café>` is a valid
//!   IRI. Bytes ≥ `0x80` are passed through untouched. The narrow sub-ranges
//!   3987 excludes (surrogates, `iprivate` outside the query) are not policed:
//!   flagging them risks percent-encoding an IRI that was fine, which is the one
//!   failure mode a sanitizer must not have.
//! * **Scheme semantics.** `<nonsense://x>` is well-formed and accepted.
//! * **A relative IRI** ([`IriDefect::NotAbsolute`]) is *reported and left
//!   alone*. Escaping cannot repair it — resolving it needs a base IRI that the
//!   `.rete` never recorded — so [`sanitize_iri_content`] returns `None` and the
//!   term is emitted verbatim. A dump containing one is still not valid N-Quads,
//!   and `--sanitize-iris` says so rather than implying a fix it did not make.

use std::borrow::Cow;

/// Why an IRI is not one. The classes are ordered by how they are found, not by
/// severity; [`IriDefect::NotAbsolute`] is checked first because it is a
/// property of the whole IRI and is the only one escaping cannot repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IriDefect {
    /// No `scheme:` prefix, so the `IRIREF` is relative. N-Triples requires an
    /// absolute IRI. **Not repairable** — see the module docs.
    NotAbsolute,
    /// A character the `IRIREF` production excludes outright: `U+0000`–`U+0020`
    /// (space and the controls), `U+007F`, and `< > " { } | ^ ` \`. A backslash
    /// that opens a well-formed `UCHAR` (`\uXXXX` / `\UXXXXXXXX`) is fine.
    ForbiddenChar,
    /// `[` or `]` outside an IP-literal host. RFC 3987 reserves the brackets for
    /// `http://[::1]/…`; anywhere else — a path, a query — they must be
    /// percent-encoded. The single largest class in the field data (unescaped
    /// brackets in harvested PDF URLs).
    Bracket,
    /// More than one `#`. The first starts the fragment; a `#` inside a fragment
    /// is not a `pchar` and must be `%23`.
    ExtraHash,
    /// A `%` not followed by two hex digits, so it is not a `pct-encoded`
    /// triplet. Often the trace of a broken string template rather than an
    /// escaping mistake (`%x`, `%p`) — escaping it makes the dump loadable
    /// without making the IRI *right*, which is the publisher's bug to fix.
    BadPercent,
}

/// Number of [`IriDefect`] classes — the width of a report's counter array.
pub const DEFECT_CLASSES: usize = 5;

impl IriDefect {
    /// Every class, in declaration order — the iteration order of a report.
    pub const ALL: [IriDefect; DEFECT_CLASSES] = [
        IriDefect::NotAbsolute,
        IriDefect::ForbiddenChar,
        IriDefect::Bracket,
        IriDefect::ExtraHash,
        IriDefect::BadPercent,
    ];

    /// Index into a report's per-class counters.
    #[inline]
    pub fn index(self) -> usize {
        match self {
            IriDefect::NotAbsolute => 0,
            IriDefect::ForbiddenChar => 1,
            IriDefect::Bracket => 2,
            IriDefect::ExtraHash => 3,
            IriDefect::BadPercent => 4,
        }
    }

    /// A short human reason, for a warning line.
    pub fn reason(self) -> &'static str {
        match self {
            IriDefect::NotAbsolute => "no scheme — a relative IRI, not an absolute one",
            IriDefect::ForbiddenChar => {
                "a character the IRIREF grammar excludes (space, control, or one of <>\"{}|^`\\)"
            }
            IriDefect::Bracket => "'[' or ']' outside an IP-literal host",
            IriDefect::ExtraHash => "more than one '#'",
            IriDefect::BadPercent => "'%' not followed by two hex digits",
        }
    }

    /// Can percent-encoding repair it without inventing information?
    ///
    /// Everything except [`IriDefect::NotAbsolute`], which needs a base IRI the
    /// file does not carry.
    #[inline]
    pub fn repairable(self) -> bool {
        self != IriDefect::NotAbsolute
    }
}

#[inline]
fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Length of the well-formed `UCHAR` starting at `b[i]` (`\uXXXX` = 6,
/// `\UXXXXXXXX` = 10), or `None` if this backslash does not open one.
fn uchar_len(b: &[u8], i: usize) -> Option<usize> {
    let (n, digits) = match b.get(i + 1) {
        Some(b'u') => (6usize, 4usize),
        Some(b'U') => (10usize, 8usize),
        _ => return None,
    };
    if i + n > b.len() {
        return None;
    }
    b[i + 2..i + 2 + digits]
        .iter()
        .all(|&d| is_hex(d))
        .then_some(n)
}

/// The byte index of the scheme's `:`, if `s` opens with a well-formed
/// `scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` followed by `:`.
fn scheme_colon(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if !b.first()?.is_ascii_alphabetic() {
        return None;
    }
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b':' => return Some(i),
            c if c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.' => i += 1,
            _ => return None,
        }
    }
    None
}

/// The `[`/`]` byte positions of an IP-literal host (`scheme://[::1]…`), the one
/// place RFC 3987 allows brackets. `None` when the IRI has no bracketed
/// authority, in which case every bracket is a defect.
fn ip_literal_brackets(s: &str, colon: usize) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    if b.get(colon + 1) != Some(&b'/') || b.get(colon + 2) != Some(&b'/') {
        return None;
    }
    let start = colon + 3;
    if b.get(start) != Some(&b'[') {
        return None;
    }
    // The authority ends at the first '/', '?' or '#'; the ']' must be inside it.
    let end = b[start..]
        .iter()
        .position(|&c| c == b'/' || c == b'?' || c == b'#')
        .map(|p| start + p)
        .unwrap_or(b.len());
    let close = b[start..end].iter().position(|&c| c == b']')? + start;
    Some((start, close))
}

/// Classify the **content of an `IRIREF`** — what sits between `<` and `>` —
/// returning the first defect found, or `None` when it is a valid absolute IRI.
///
/// The scan is one pass over the bytes with no allocation, so it is affordable
/// on every term of a billion-statement ingest.
pub fn iri_content_defect(s: &str) -> Option<IriDefect> {
    let colon = match scheme_colon(s) {
        Some(c) => c,
        // A whole-IRI property, and the unrepairable one: report it before any
        // character defect, so a relative IRI is never mistaken for something a
        // sanitizer could fix.
        None => return Some(IriDefect::NotAbsolute),
    };
    let brackets = ip_literal_brackets(s, colon);
    let b = s.as_bytes();
    let mut i = 0;
    let mut seen_hash = false;
    while i < b.len() {
        let c = b[i];
        if c >= 0x80 {
            // RFC 3987 `ucschar` — a legal IRI character. Never touched.
            i += 1;
            continue;
        }
        match c {
            0x00..=0x20 | 0x7f => return Some(IriDefect::ForbiddenChar),
            b'<' | b'>' | b'"' | b'{' | b'}' | b'|' | b'^' | b'`' => {
                return Some(IriDefect::ForbiddenChar)
            }
            b'\\' => match uchar_len(b, i) {
                Some(n) => {
                    i += n;
                    continue;
                }
                None => return Some(IriDefect::ForbiddenChar),
            },
            b'[' | b']' => {
                let ok = matches!(brackets, Some((o, c2)) if i == o || i == c2);
                if !ok {
                    return Some(IriDefect::Bracket);
                }
            }
            b'#' => {
                if seen_hash {
                    return Some(IriDefect::ExtraHash);
                }
                seen_hash = true;
            }
            b'%' => {
                if !(i + 2 < b.len() && is_hex(b[i + 1]) && is_hex(b[i + 2])) {
                    return Some(IriDefect::BadPercent);
                }
                i += 3;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn push_pct(out: &mut Vec<u8>, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'%');
    out.push(HEX[(byte >> 4) as usize]);
    out.push(HEX[(byte & 0x0f) as usize]);
}

/// Percent-encode the offending characters of an `IRIREF`'s content.
///
/// Returns `None` when there is nothing to do (`s` is already valid) **and**
/// when the defect is [`IriDefect::NotAbsolute`], which escaping cannot repair.
/// A `Some` result is guaranteed to satisfy [`iri_content_defect`].
///
/// This changes the IRI. `<http://ex/a[b]>` becomes `<http://ex/a%5Bb%5D>`,
/// which is a *different* IRI: a sanitized dump no longer joins against the
/// source graph, and `rete → store → rete` stops being the identity. That is why
/// it is behind a flag and never the default.
pub fn sanitize_iri_content(s: &str) -> Option<String> {
    let defect = iri_content_defect(s)?;
    if !defect.repairable() {
        return None;
    }
    let colon = scheme_colon(s)?;
    let brackets = ip_literal_brackets(s, colon);
    let b = s.as_bytes();
    // Built as bytes: every branch below appends either ASCII or a verbatim run
    // of the input's own bytes, so the result is valid UTF-8 by construction —
    // whereas a `char`-wise copy would have to re-decode each multi-byte
    // `ucschar` only to re-encode it.
    let mut out: Vec<u8> = Vec::with_capacity(s.len() + 8);
    let mut i = 0;
    let mut seen_hash = false;
    while i < b.len() {
        let c = b[i];
        if c >= 0x80 {
            out.push(c); // RFC 3987 `ucschar` byte — copied untouched.
            i += 1;
            continue;
        }
        match c {
            0x00..=0x20 | 0x7f | b'<' | b'>' | b'"' | b'{' | b'}' | b'|' | b'^' | b'`' => {
                push_pct(&mut out, c)
            }
            b'\\' => match uchar_len(b, i) {
                Some(n) => {
                    out.extend_from_slice(&b[i..i + n]);
                    i += n;
                    continue;
                }
                None => push_pct(&mut out, c),
            },
            b'[' | b']' => {
                if matches!(brackets, Some((o, c2)) if i == o || i == c2) {
                    out.push(c);
                } else {
                    push_pct(&mut out, c);
                }
            }
            b'#' => {
                if seen_hash {
                    push_pct(&mut out, c);
                } else {
                    seen_hash = true;
                    out.push(b'#');
                }
            }
            b'%' => {
                if i + 2 < b.len() && is_hex(b[i + 1]) && is_hex(b[i + 2]) {
                    out.extend_from_slice(&b[i..i + 3]);
                    i += 3;
                    continue;
                }
                push_pct(&mut out, c);
            }
            _ => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// The `^^<datatype>` IRI of a literal token, without its angle brackets.
fn literal_datatype_content(token: &str) -> Option<&str> {
    let inner = token.strip_suffix('>')?;
    let at = inner.rfind("^^<")?;
    Some(&inner[at + 3..])
}

/// Classify a **canonical N-Triples term token** — `<iri>`, `_:b`, `"lit"`,
/// `"lit"^^<dt>`, `"lit"@en`, or an RDF-star `<<s p o>>` — returning the first
/// IRI defect it carries, or `None`.
///
/// Every IRI a term can hide is checked: the term itself, a literal's datatype,
/// and (recursively) the three components of a quoted triple.
pub fn term_defect(token: &str) -> Option<IriDefect> {
    if let Some((s, p, o)) = crate::ingest::quoted_triple_parts(token) {
        return term_defect(&s)
            .or_else(|| term_defect(&p))
            .or_else(|| term_defect(&o));
    }
    if crate::terms::is_iri(token) {
        return iri_content_defect(&token[1..token.len() - 1]);
    }
    if token.starts_with('"') {
        return literal_datatype_content(token).and_then(iri_content_defect);
    }
    None
}

/// Percent-encode every repairable IRI inside a canonical term token, returning
/// `None` when nothing changed. The token-level counterpart of
/// [`sanitize_iri_content`]; it rebuilds quoted triples and literal datatypes
/// around the repaired IRI so the result is still a canonical token.
pub fn sanitize_term(token: &str) -> Option<String> {
    if let Some((s, p, o)) = crate::ingest::quoted_triple_parts(token) {
        let (rs, rp, ro) = (sanitize_term(&s), sanitize_term(&p), sanitize_term(&o));
        if rs.is_none() && rp.is_none() && ro.is_none() {
            return None;
        }
        let pick = |r: Option<String>, orig: String| r.unwrap_or(orig);
        return Some(format!(
            "<<{} {} {}>>",
            pick(rs, s),
            pick(rp, p),
            pick(ro, o)
        ));
    }
    if crate::terms::is_iri(token) {
        return sanitize_iri_content(&token[1..token.len() - 1]).map(|c| format!("<{c}>"));
    }
    if token.starts_with('"') {
        let dt = literal_datatype_content(token)?;
        let fixed = sanitize_iri_content(dt)?;
        let head = &token[..token.len() - 1 - dt.len()];
        return Some(format!("{head}{fixed}>"));
    }
    None
}

/// A bounded tally of invalid IRIs — the shape both the build-time audit and the
/// export-time sanitizer report.
///
/// Memory is **constant**, not proportional to the damage: per-class occurrence
/// counters plus at most one example string per class. A graph where the same
/// bad IRI appears ten million times costs five sample strings, not ten million.
#[derive(Debug, Default, Clone)]
pub struct IriReport {
    statements: u64,
    occurrences: u64,
    repaired: u64,
    counts: [u64; DEFECT_CLASSES],
    samples: [Option<String>; DEFECT_CLASSES],
}

impl IriReport {
    /// Record one offending term. Prefer [`IriReport::observe_term`] /
    /// [`IriReport::sanitize`], which classify first.
    fn note(&mut self, defect: IriDefect, token: &str) {
        let i = defect.index();
        self.counts[i] += 1;
        self.occurrences += 1;
        if self.samples[i].is_none() {
            self.samples[i] = Some(token.to_string());
        }
    }

    /// Classify one term token and record any defect. Returns it, so a strict
    /// caller can fail on the spot.
    pub fn observe_term(&mut self, token: &str) -> Option<IriDefect> {
        let d = term_defect(token)?;
        self.note(d, token);
        Some(d)
    }

    /// Classify a whole statement — subject, predicate, object and (for
    /// N-Quads) the graph — recording every offending term and counting the
    /// statement once. Returns the first defect found.
    pub fn observe_quad(
        &mut self,
        s: &str,
        p: &str,
        o: &str,
        g: Option<&str>,
    ) -> Option<IriDefect> {
        let mut first = None;
        for t in [Some(s), Some(p), Some(o), g].into_iter().flatten() {
            if let Some(d) = self.observe_term(t) {
                first.get_or_insert(d);
            }
        }
        if first.is_some() {
            self.statements += 1;
        }
        first
    }

    /// Repair one term for export: percent-encode what escaping can fix, record
    /// what it found, and return the token to emit. An unrepairable IRI is
    /// counted and returned **unchanged** — the caller is told, and the data is
    /// not silently dropped or invented.
    pub fn sanitize<'a>(&mut self, token: &'a str) -> Cow<'a, str> {
        match term_defect(token) {
            None => Cow::Borrowed(token),
            Some(d) => {
                self.note(d, token);
                match sanitize_term(token) {
                    Some(fixed) => {
                        self.repaired += 1;
                        Cow::Owned(fixed)
                    }
                    None => Cow::Borrowed(token),
                }
            }
        }
    }

    /// Nothing invalid was seen.
    pub fn is_empty(&self) -> bool {
        self.occurrences == 0
    }

    /// Statements carrying at least one invalid IRI (build audit only; the
    /// export sanitizer works term by term and leaves this at zero).
    pub fn statements(&self) -> u64 {
        self.statements
    }

    /// Invalid IRI **term occurrences** — the same IRI in a million statements
    /// counts a million times.
    pub fn occurrences(&self) -> u64 {
        self.occurrences
    }

    /// Occurrences [`IriReport::sanitize`] actually rewrote.
    pub fn repaired(&self) -> u64 {
        self.repaired
    }

    /// Occurrences no escaping can repair — every [`IriDefect::NotAbsolute`].
    pub fn unrepairable(&self) -> u64 {
        IriDefect::ALL
            .iter()
            .filter(|d| !d.repairable())
            .map(|d| self.counts[d.index()])
            .sum()
    }

    /// Per-class occurrence count.
    pub fn count(&self, defect: IriDefect) -> u64 {
        self.counts[defect.index()]
    }

    /// The first term seen in this class, if any.
    pub fn sample(&self, defect: IriDefect) -> Option<&str> {
        self.samples[defect.index()].as_deref()
    }

    /// The non-empty classes, in declaration order, with their count and example
    /// — the rows of a warning block.
    pub fn classes(&self) -> impl Iterator<Item = (IriDefect, u64, Option<&str>)> + '_ {
        IriDefect::ALL
            .into_iter()
            .filter(move |d| self.counts[d.index()] > 0)
            .map(move |d| (d, self.counts[d.index()], self.sample(d)))
    }

    /// Fold another report in — used to total the per-input audits of a
    /// multi-input build.
    pub fn merge(&mut self, other: &IriReport) {
        self.statements += other.statements;
        self.occurrences += other.occurrences;
        self.repaired += other.repaired;
        for i in 0..DEFECT_CLASSES {
            self.counts[i] += other.counts[i];
            if self.samples[i].is_none() {
                self.samples[i].clone_from(&other.samples[i]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_iris_are_left_alone() {
        for ok in [
            "http://example.org/ok",
            "https://example.org/p?q=1&r=2#frag",
            "http://[::1]:7878/sparql",
            "http://[2001:db8::1]/a",
            "urn:uuid:2a5c3f60-0000-4000-8000-000000000000",
            "mailto:someone@example.org",
            "http://example.org/caf\u{e9}",
            "http://example.org/a%20b",
            "http://example.org/\\u00E9",
            "tag:example.org,2026:x",
            "http://example.org/(a),b;c=d!e$f*g'h+i",
            "http://example.org/@at:colon",
            "file:///tmp/x",
        ] {
            assert_eq!(iri_content_defect(ok), None, "{ok} should be valid");
            assert_eq!(sanitize_iri_content(ok), None, "{ok} should need no repair");
        }
    }

    #[test]
    fn the_five_classes_are_recognized() {
        let cases = [
            ("noscheme/path", IriDefect::NotAbsolute),
            ("", IriDefect::NotAbsolute),
            ("/absolute/path", IriDefect::NotAbsolute),
            ("1http://example.org/", IriDefect::NotAbsolute),
            ("http://example.org/a b", IriDefect::ForbiddenChar),
            ("http://example.org/a\"b", IriDefect::ForbiddenChar),
            ("http://example.org/a|b", IriDefect::ForbiddenChar),
            ("http://example.org/a\\b", IriDefect::ForbiddenChar),
            ("http://example.org/a\u{7f}b", IriDefect::ForbiddenChar),
            ("http://example.org/a[b]", IriDefect::Bracket),
            ("http://example.org/?q=[1]", IriDefect::Bracket),
            ("http://example.org/c#d#e", IriDefect::ExtraHash),
            ("http://example.org/%x", IriDefect::BadPercent),
            ("http://example.org/%", IriDefect::BadPercent),
            ("http://example.org/%2", IriDefect::BadPercent),
        ];
        for (bad, want) in cases {
            assert_eq!(iri_content_defect(bad), Some(want), "{bad}");
        }
    }

    #[test]
    fn repairs_are_idempotent_and_land_valid() {
        for bad in [
            "http://example.org/a[b]",
            "http://example.org/c#d#e",
            "http://example.org/%x",
            "http://example.org/a b",
            "http://example.org/a\"b|c{d}e^f`g",
            "http://example.org/caf\u{e9}[x]",
            "http://[::1]/a[b]",
            "http://example.org/a\\b",
        ] {
            let fixed = sanitize_iri_content(bad).unwrap_or_else(|| panic!("{bad} unrepaired"));
            assert_eq!(iri_content_defect(&fixed), None, "{bad} -> {fixed}");
            assert_eq!(
                sanitize_iri_content(&fixed),
                None,
                "not idempotent: {fixed}"
            );
        }
    }

    #[test]
    fn the_issue_examples_repair_to_the_expected_iris() {
        assert_eq!(
            sanitize_iri_content("http://example.org/a[b]").as_deref(),
            Some("http://example.org/a%5Bb%5D")
        );
        assert_eq!(
            sanitize_iri_content("http://example.org/c#d#e").as_deref(),
            Some("http://example.org/c#d%23e")
        );
        // No scheme: reported, never rewritten.
        assert_eq!(sanitize_iri_content("noscheme/path"), None);
        assert_eq!(
            iri_content_defect("noscheme/path"),
            Some(IriDefect::NotAbsolute)
        );
    }

    #[test]
    fn an_ip_literal_host_keeps_its_brackets_but_a_path_does_not() {
        assert_eq!(iri_content_defect("http://[::1]/x"), None);
        assert_eq!(
            sanitize_iri_content("http://[::1]/x[y]").as_deref(),
            Some("http://[::1]/x%5By%5D")
        );
        // A bracket that is not the authority's is still a defect.
        assert_eq!(
            iri_content_defect("http://ex/[::1]"),
            Some(IriDefect::Bracket)
        );
    }

    #[test]
    fn non_ascii_survives_a_repair_byte_for_byte() {
        let fixed = sanitize_iri_content("http://example.org/\u{4e2d}\u{6587}[x]").unwrap();
        assert_eq!(fixed, "http://example.org/\u{4e2d}\u{6587}%5Bx%5D");
    }

    #[test]
    fn term_level_checks_reach_datatypes_and_quoted_triples() {
        assert_eq!(term_defect("<http://ex/ok>"), None);
        assert_eq!(term_defect("_:b0"), None);
        assert_eq!(term_defect("\"plain\""), None);
        assert_eq!(term_defect("\"x\"@en"), None);
        assert_eq!(
            term_defect("\"x\"^^<http://ex/dt[1]>"),
            Some(IriDefect::Bracket)
        );
        assert_eq!(
            sanitize_term("\"x\"^^<http://ex/dt[1]>").as_deref(),
            Some("\"x\"^^<http://ex/dt%5B1%5D>")
        );
        assert_eq!(
            term_defect("<<<http://ex/a[b]> <http://ex/p> \"o\">>"),
            Some(IriDefect::Bracket)
        );
        assert_eq!(
            sanitize_term("<<<http://ex/a[b]> <http://ex/p> \"o\">>").as_deref(),
            Some("<<<http://ex/a%5Bb%5D> <http://ex/p> \"o\">>")
        );
    }

    #[test]
    fn a_report_counts_occurrences_and_keeps_one_sample_per_class() {
        let mut r = IriReport::default();
        r.observe_quad(
            "<http://ex/a[b]>",
            "<http://ex/p>",
            "<http://ex/c#d#e>",
            None,
        );
        r.observe_quad("<noscheme/x>", "<http://ex/p>", "\"lit\"", None);
        r.observe_quad("<http://ex/a[b]>", "<http://ex/p>", "\"lit\"", None);
        assert_eq!(r.statements(), 3);
        assert_eq!(r.occurrences(), 4);
        assert_eq!(r.count(IriDefect::Bracket), 2);
        assert_eq!(r.count(IriDefect::ExtraHash), 1);
        assert_eq!(r.unrepairable(), 1);
        assert_eq!(r.sample(IriDefect::Bracket), Some("<http://ex/a[b]>"));
        assert_eq!(r.classes().count(), 3);
    }

    #[test]
    fn sanitizing_reports_what_it_could_not_repair() {
        let mut r = IriReport::default();
        assert_eq!(r.sanitize("<http://ex/a[b]>"), "<http://ex/a%5Bb%5D>");
        assert_eq!(r.sanitize("<noscheme/x>"), "<noscheme/x>");
        assert_eq!(r.sanitize("<http://ex/fine>"), "<http://ex/fine>");
        assert_eq!(r.occurrences(), 2);
        assert_eq!(r.repaired(), 1);
        assert_eq!(r.unrepairable(), 1);
    }
}
