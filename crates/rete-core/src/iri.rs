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
//! # Validity is decided by a parser; the classes only say how to repair
//!
//! Two documents apply, and both must hold:
//!
//! * the **N-Triples/N-Quads `IRIREF` production**
//!   (`'<' ([^#x00-#x20<>"{}|^`\] | UCHAR)* '>'`), which excludes a fixed set of
//!   characters outright, and
//! * **RFC 3987**, which the same grammar requires the content to satisfy as an
//!   *absolute* IRI (with an optional fragment).
//!
//! The first is a character set, so a one-pass scan settles it. The second is a
//! grammar, and it is **not** settled by enumerating bad shapes: a taxonomy of
//! known defects can only recognise what has already been seen, so making it the
//! arbiter of validity guarantees that the next unseen shape is published as
//! valid. That is not hypothetical — `<https://::1>` (an IPv6 literal in the
//! authority without the brackets 3987 requires) matched none of the five
//! classes, and a dump carrying it passed the export gate and was rejected by
//! Oxigraph.
//!
//! So the two questions are separated:
//!
//! * **Is it valid?** `oxiri` answers, and nothing else does. That is the
//!   crate Oxigraph's own N-Triples reader validates with — `oxttl`'s lexer
//!   tokenises `<`…`>` per the `IRIREF` production and hands the content to
//!   `oxiri::Iri::parse` — so rete's verdict agrees with the reference loader's
//!   *by construction* rather than by our keeping a list up to date. `oxiri` is
//!   already in this crate's dependency tree via `oxrdf`/`oxttl`, so deciding it
//!   properly costs no new dependency and no new wasm surface.
//! * **Can we repair it, and how?** The classes below answer, and only that.
//!   Each names a shape percent-encoding can fix without inventing information.
//!
//! | class | example | repairable by escaping |
//! |---|---|---|
//! | [`IriDefect::NotAbsolute`]   | `<noscheme/path>`            | **no** |
//! | [`IriDefect::ForbiddenChar`] | `<http://ex/a b>`, `<http://ex/a"b>` | yes |
//! | [`IriDefect::Bracket`]       | `<http://ex/a[b]>`           | yes |
//! | [`IriDefect::ExtraHash`]     | `<http://ex/c#d#e>`          | yes |
//! | [`IriDefect::BadPercent`]    | `<http://ex/%x>`             | yes |
//! | [`IriDefect::Unclassified`]  | `<https://::1>`              | **no** |
//!
//! [`IriDefect::Unclassified`] is the load-bearing one: an IRI the parser
//! rejects that **no class recognises**. It is still counted and it is still
//! unrepairable, so it still blocks. A gap in the taxonomy is now benign — we
//! failed to repair something we might have — instead of harmful, which is a
//! published dump that does not load. A class that is added later only moves
//! occurrences out of this bucket; it never changes a verdict from invalid to
//! valid.
//!
//! A repairable class is only reported when escaping actually **lands a valid
//! IRI**: `<https://::1/a[b]>` has a bracket, but encoding it leaves an IRI the
//! parser still rejects, so it is reported as [`IriDefect::Unclassified`] rather
//! than as a `Bracket` the sanitizer would claim to have fixed.
//!
//! # `UCHAR` escapes are syntax, not content
//!
//! `<http://ex/café>` is a legal `IRIREF` whose IRI is `http://ex/café` —
//! the backslash belongs to N-Triples, not to the IRI. The escapes are therefore
//! resolved *before* the parser sees the string, exactly as `oxttl` does.
//! Handing the raw bracket content to an RFC 3987 parser would reject `\` and
//! make every escaped dump a false positive.
//!
//! # What this deliberately does not judge
//!
//! * **Scheme semantics.** `<nonsense://x>` is syntactically a fine IRI and is
//!   accepted. Whether the scheme is registered, resolvable or meaningful is not
//!   this module's question.
//! * **Whether the IRI names anything.** A 404, a typo'd host and a dead DOI are
//!   all valid IRIs.
//! * **Normalisation.** `<HTTP://EX/a/../b>` is valid and is left exactly as it
//!   was given; rete never case-folds a scheme or removes a dot segment, because
//!   both change the dictionary key and break the round-trip.
//! * **Anything escaping cannot fix.** A relative IRI ([`IriDefect::NotAbsolute`])
//!   and an [`IriDefect::Unclassified`] one are *reported and left alone* —
//!   [`sanitize_iri_content`] returns `None` and the term is emitted verbatim. A
//!   dump containing one is still not valid N-Quads, and `--sanitize-iris` says
//!   so rather than implying a fix it did not make.
//!
//! **Non-ASCII used to be on this list and no longer is.** RFC 3987 admits
//! `ucschar`, so `<http://ex/café>` is valid — but the narrow sub-ranges 3987
//! excludes (surrogates, `iprivate` outside the query) were previously not
//! policed at all. The parser polices them now, because it is the same parser
//! the loader uses: if it accepts a character, so does the dump's reader.

use std::borrow::Cow;

/// Why an IRI is not one — a **repair** classification, not the validity
/// verdict. [`iri_content_defect`] decides validity with an RFC 3987 parser and
/// uses these only to say what, if anything, can be done about it.
///
/// The classes are ordered by how they are found, not by severity;
/// [`IriDefect::NotAbsolute`] is checked first because it is a property of the
/// whole IRI, and [`IriDefect::Unclassified`] last because it is what is left
/// when no other class explains a parser rejection.
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
    /// **The RFC 3987 parser rejected it and no class above explains why.**
    /// Either the defect is a shape this taxonomy has never seen (`https://::1`
    /// — an IPv6 literal in the authority without its brackets, the case this
    /// bucket was added for), or escaping the shape that *was* recognised still
    /// leaves an IRI the parser refuses.
    ///
    /// **Not repairable** — we do not know what is wrong, so we cannot claim to
    /// have fixed it. It is counted and it blocks, which is the whole point: a
    /// gap in the taxonomy must never read as a clean bill of health. Adding a
    /// class later only moves occurrences out of here; it never turns an invalid
    /// IRI into a valid one.
    Unclassified,
}

/// Number of [`IriDefect`] classes — the width of a report's counter array.
pub const DEFECT_CLASSES: usize = 6;

impl IriDefect {
    /// Every class, in declaration order — the iteration order of a report.
    pub const ALL: [IriDefect; DEFECT_CLASSES] = [
        IriDefect::NotAbsolute,
        IriDefect::ForbiddenChar,
        IriDefect::Bracket,
        IriDefect::ExtraHash,
        IriDefect::BadPercent,
        IriDefect::Unclassified,
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
            IriDefect::Unclassified => 5,
        }
    }

    /// A short human reason, for a warning line.
    ///
    /// These strings are **parsed downstream** — `scripts/export_scholar_nquads.sh`
    /// matches a stable fragment of each to fill `state.tsv`. Changing one means
    /// changing that parser in the same commit.
    pub fn reason(self) -> &'static str {
        match self {
            IriDefect::NotAbsolute => "no scheme — a relative IRI, not an absolute one",
            IriDefect::ForbiddenChar => {
                "a character the IRIREF grammar excludes (space, control, or one of <>\"{}|^`\\)"
            }
            IriDefect::Bracket => "'[' or ']' outside an IP-literal host",
            IriDefect::ExtraHash => "more than one '#'",
            IriDefect::BadPercent => "'%' not followed by two hex digits",
            IriDefect::Unclassified => {
                "rejected by the RFC 3987 parser, and no repair class recognises it"
            }
        }
    }

    /// Can percent-encoding repair it without inventing information?
    ///
    /// Everything except [`IriDefect::NotAbsolute`], which needs a base IRI the
    /// file does not carry, and [`IriDefect::Unclassified`], where we do not
    /// know what is wrong.
    ///
    /// Callers deciding whether a dump may be published must ask **this**, over
    /// every class — never whether a particular class is present. Hardcoding one
    /// class is how `<https://::1>` got through.
    #[inline]
    pub fn repairable(self) -> bool {
        !matches!(self, IriDefect::NotAbsolute | IriDefect::Unclassified)
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

/// Resolve the `UCHAR` escapes (`\uXXXX` / `\UXXXXXXXX`) an `IRIREF` may carry,
/// yielding the IRI the escapes *denote*.
///
/// The backslash belongs to N-Triples, not to the IRI: `<http://ex/café>`
/// names `http://ex/café`. Handing the raw bracket content to an RFC 3987 parser
/// would reject the `\` and make every escaped dump a false positive, so the
/// escapes are resolved first — which is exactly what `oxttl`'s lexer does
/// before it calls the same parser.
///
/// `None` when an escape is malformed or names a surrogate, both of which a
/// strict reader rejects outright (`char::from_u32` refuses `D800`–`DFFF`).
///
/// Borrows — and so costs nothing — for the overwhelmingly common IRI that
/// carries no backslash at all.
fn resolve_uchars(s: &str) -> Option<Cow<'_, str>> {
    if !s.as_bytes().contains(&b'\\') {
        return Some(Cow::Borrowed(s));
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            let start = i;
            while i < b.len() && b[i] != b'\\' {
                i += 1;
            }
            out.push_str(&s[start..i]);
            continue;
        }
        // Only UCHAR is legal inside an IRIREF — ECHAR (`\n`, `\"`, `\\`) is
        // not, and `uchar_len` accepts nothing else.
        let n = uchar_len(b, i)?;
        let cp = u32::from_str_radix(&s[i + 2..i + n], 16).ok()?;
        out.push(char::from_u32(cp)?);
        i += n;
    }
    Some(Cow::Owned(out))
}

/// **The validity verdict.** Does `s` parse as an absolute RFC 3987 IRI?
///
/// Decided by `oxiri`, the crate Oxigraph's own N-Triples reader validates with,
/// so the answer agrees with the loader that will read the dump. Nothing else in
/// this module decides validity; the [`IriDefect`] classes only describe repairs.
fn parses_as_absolute_iri(s: &str) -> bool {
    match resolve_uchars(s) {
        Some(resolved) => oxiri::Iri::parse(resolved.as_ref()).is_ok(),
        None => false,
    }
}

/// The **repair** classification: the N-Triples `IRIREF` character set plus the
/// whole-IRI shapes escaping is known to fix. Says nothing about validity —
/// [`iri_content_defect`] is the entry point, and it asks the parser.
///
/// One pass over the bytes with no allocation.
fn shape_defect(s: &str) -> Option<IriDefect> {
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

/// Classify the **content of an `IRIREF`** — what sits between `<` and `>` —
/// returning `None` when it is a valid absolute IRI, and otherwise the class
/// that says what can be done about it.
///
/// **Validity is the parser's call, not the taxonomy's.** The order is:
///
/// 1. the cheap `shape_defect` scan, which settles the `IRIREF` character set
///    and names a repair when it recognises one;
/// 2. a repairable shape is only *reported* as repairable when escaping it
///    verifiably lands an IRI the parser accepts — otherwise the IRI carries a
///    second defect no class names, and calling it `Bracket` would have the
///    sanitizer claim a fix it did not make;
/// 3. anything the scan finds nothing wrong with still has to satisfy RFC 3987,
///    and [`IriDefect::Unclassified`] is what a rejection with no known shape is
///    called. It is counted, it is unrepairable, and it blocks.
///
/// The common case — a valid IRI — costs one byte scan plus one `oxiri` parse,
/// with no allocation. The re-escape in step 2 runs only for an IRI that is
/// already known to be broken, which is the rare case by construction.
pub fn iri_content_defect(s: &str) -> Option<IriDefect> {
    match shape_defect(s) {
        // A shape we know how to escape — but only if escaping it works.
        Some(d) if d.repairable() => {
            let fixed = escape_shape_defects(s);
            match fixed {
                Some(f) if shape_defect(&f).is_none() && parses_as_absolute_iri(&f) => Some(d),
                _ => Some(IriDefect::Unclassified),
            }
        }
        // NotAbsolute: no scheme, so there is nothing for a parser to add.
        Some(d) => Some(d),
        None => {
            if parses_as_absolute_iri(s) {
                None
            } else {
                Some(IriDefect::Unclassified)
            }
        }
    }
}

fn push_pct(out: &mut Vec<u8>, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'%');
    out.push(HEX[(byte >> 4) as usize]);
    out.push(HEX[(byte & 0x0f) as usize]);
}

/// Percent-encode the offending characters of an `IRIREF`'s content.
///
/// Returns `None` when there is nothing to do (`s` is already a valid IRI) and
/// when the defect is not repairable by escaping — [`IriDefect::NotAbsolute`],
/// which needs a base IRI the file never recorded, and
/// [`IriDefect::Unclassified`], where we do not know what is wrong.
///
/// **A `Some` result is guaranteed to satisfy [`iri_content_defect`]** — and,
/// since that now asks a real RFC 3987 parser, guaranteed to be an IRI the
/// loader accepts. `iri_content_defect` establishes this before it names a
/// repairable class, so the check is not repeated here.
///
/// **`None` for an IRI that was already fine** is the other half of the
/// contract, and the one failure mode a sanitizer must not have: this function
/// never percent-encodes something valid. It is `iri_content_defect` that
/// decides "fine", and it decides it with the parser.
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
    escape_shape_defects(s)
}

/// The escaping pass itself: percent-encode every character `shape_defect`
/// objects to, and nothing else. Pure syntax — it does not decide whether the
/// result is valid, which is [`iri_content_defect`]'s job.
///
/// `None` only when `s` has no scheme, where there is no authority to locate and
/// so no way to tell a host's brackets from a path's.
fn escape_shape_defects(s: &str) -> Option<String> {
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

/// **Every** defect a term carries, not only the first.
///
/// A term is not always one IRI: an RDF-star quoted triple holds three, each
/// able to be broken in its own way. [`term_defect`] answers "is this term
/// invalid" and stops at the first reason, which is all a strict caller needs —
/// but a *report* that stopped there would count `<<<http://ex/a[b]> <http://ex/p>
/// <relative>>>` as a repairable `Bracket` and never record the relative IRI that
/// actually blocks the dump. So the report counts per IRI, and this is what it
/// walks.
///
/// Returns an empty `Vec` — which does not allocate — for a clean term, so the
/// hot path pays nothing.
pub fn term_defects(token: &str) -> Vec<IriDefect> {
    let mut out = Vec::new();
    collect_term_defects(token, &mut out);
    out
}

fn collect_term_defects(token: &str, out: &mut Vec<IriDefect>) {
    if let Some((s, p, o)) = crate::ingest::quoted_triple_parts(token) {
        collect_term_defects(&s, out);
        collect_term_defects(&p, out);
        collect_term_defects(&o, out);
        return;
    }
    if crate::terms::is_iri(token) {
        out.extend(iri_content_defect(&token[1..token.len() - 1]));
    } else if token.starts_with('"') {
        out.extend(literal_datatype_content(token).and_then(iri_content_defect));
    }
}

/// Percent-encode every repairable IRI inside a canonical term token, returning
/// `None` when nothing changed. The token-level counterpart of
/// [`sanitize_iri_content`]; it rebuilds quoted triples and literal datatypes
/// around the repaired IRI so the result is still a canonical token.
///
/// **A `Some` result carries no defect**, exactly as for
/// [`sanitize_iri_content`]. A quoted triple whose subject cannot be repaired is
/// therefore returned as `None` even when its object could be: a *partial*
/// repair changes IRIs without making the term loadable, and the report would
/// count it as fixed. Found by the `iri` fuzz target.
pub fn sanitize_term(token: &str) -> Option<String> {
    if let Some((s, p, o)) = crate::ingest::quoted_triple_parts(token) {
        let (rs, rp, ro) = (sanitize_term(&s), sanitize_term(&p), sanitize_term(&o));
        if rs.is_none() && rp.is_none() && ro.is_none() {
            return None;
        }
        let pick = |r: Option<String>, orig: String| r.unwrap_or(orig);
        let rebuilt = format!("<<{} {} {}>>", pick(rs, s), pick(rp, p), pick(ro, o));
        // All or nothing: a component we could not fix leaves the whole term
        // invalid, and claiming the repair would be the lie this module exists
        // to prevent.
        return term_defect(&rebuilt).is_none().then_some(rebuilt);
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

    /// Classify one term token and record **every** defect it carries — an
    /// RDF-star quoted triple holds three IRIs and can be broken in three ways.
    /// Returns the first, so a strict caller can fail on the spot.
    pub fn observe_term(&mut self, token: &str) -> Option<IriDefect> {
        let defects = term_defects(token);
        let first = defects.first().copied();
        for d in defects {
            self.note(d, token);
        }
        first
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
        let defects = term_defects(token);
        if defects.is_empty() {
            return Cow::Borrowed(token);
        }
        for &d in &defects {
            self.note(d, token);
        }
        match sanitize_term(token) {
            // `sanitize_term` only returns `Some` for a term that is now clean,
            // so every defect above really was repaired.
            Some(fixed) => {
                self.repaired += defects.len() as u64;
                Cow::Owned(fixed)
            }
            None => Cow::Borrowed(token),
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

    /// Occurrences no escaping can repair — **every class** for which
    /// [`IriDefect::repairable`] is false, asked class by class rather than
    /// named one at a time.
    ///
    /// This is the number a publication gate must look at. `> 0` means the dump
    /// contains an IRI that is still invalid after everything the sanitizer
    /// could do, so a strict loader will reject it. Gating on one specific class
    /// instead is how an unrecognised defect gets published.
    pub fn unrepairable(&self) -> u64 {
        IriDefect::ALL
            .iter()
            .filter(|d| !d.repairable())
            .map(|d| self.counts[d.index()])
            .sum()
    }

    /// Occurrences escaping *can* repair. Complements [`IriReport::unrepairable`]
    /// — the two partition [`IriReport::occurrences`].
    pub fn repairable(&self) -> u64 {
        IriDefect::ALL
            .iter()
            .filter(|d| d.repairable())
            .map(|d| self.counts[d.index()])
            .sum()
    }

    /// Occurrences the RFC 3987 parser rejected that **no repair class
    /// recognised** — [`IriDefect::Unclassified`].
    ///
    /// Reported separately because it is the number that says the taxonomy has a
    /// gap. It is already inside [`IriReport::unrepairable`], so a gate that
    /// reads that one does not need this; it is for the human who has to decide
    /// whether a new class is worth adding.
    pub fn unclassified(&self) -> u64 {
        self.counts[IriDefect::Unclassified.index()]
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

    /// The false-positive set. A wrong flag blocks a valid dump, so these are
    /// the critical cases: every one is legal RFC 3987 and must stay clean.
    #[test]
    fn legal_iris_are_never_flagged() {
        for ok in [
            "http://user:pass@host/",    // ':' inside userinfo
            "http://[::1]:8080/p",       // bracketed IPv6 with a port
            "urn:isbn:0451450523",       // no authority; colons are path
            "mailto:a@b.com",            // no authority; '@' is path
            "http://host:8080/a:b",      // colons in the path
            "http://caf\u{e9}.example/", // RFC 3987 admits `ucschar`
            "http://example.org/caf\u{e9}",
            "https://[2001:db8::1]:443/x?q=1#f",
            "http://host/", // empty port is legal
            "http://host:/p",
            "ftp://ftp.example.org/pub/",
            "did:example:123456789abcdefghi",
            "http://example.org/a?b=c&d=%20e#frag",
        ] {
            assert_eq!(iri_content_defect(ok), None, "{ok} must not be flagged");
            assert_eq!(
                sanitize_iri_content(ok),
                None,
                "{ok} was already fine and must not be rewritten"
            );
        }
    }

    /// The authority defects the five-class taxonomy could not see. Each must be
    /// flagged, and flagged as something that BLOCKS.
    #[test]
    fn a_bad_authority_is_flagged_and_unrepairable() {
        for bad in [
            "https://::1",   // the incident: unbracketed IPv6
            "http://h:80x/", // non-digit port
            "http://a@b@c/", // two '@' in the authority
            "http://[::1/x", // unclosed bracket
            "http://ho st/", // (space is ForbiddenChar first — see below)
        ] {
            let d = iri_content_defect(bad).unwrap_or_else(|| panic!("{bad} not flagged"));
            assert!(
                d == IriDefect::Unclassified || !d.repairable() || d == IriDefect::ForbiddenChar,
                "{bad} -> {d:?}"
            );
        }
        // Specifically: the incident IRI, and it does not repair.
        assert_eq!(
            iri_content_defect("https://::1"),
            Some(IriDefect::Unclassified)
        );
        assert_eq!(sanitize_iri_content("https://::1"), None);
        assert_eq!(
            iri_content_defect("http://h:80x/"),
            Some(IriDefect::Unclassified)
        );
        assert_eq!(
            iri_content_defect("http://a@b@c/"),
            Some(IriDefect::Unclassified)
        );
    }

    /// A repairable shape wrapped around an unrepairable one must NOT be
    /// reported as repairable: escaping the bracket leaves an IRI the parser
    /// still rejects, and claiming a repair there is how a broken dump gets
    /// marked publishable.
    #[test]
    fn a_repair_that_would_not_land_valid_is_not_claimed() {
        let bad = "https://::1/a[b]";
        assert_eq!(shape_defect(bad), Some(IriDefect::Bracket));
        assert_eq!(iri_content_defect(bad), Some(IriDefect::Unclassified));
        assert_eq!(sanitize_iri_content(bad), None);
    }

    /// `UCHAR` is N-Triples syntax, not IRI content. Resolving it before the
    /// parser sees it is what keeps an escaped dump from being a false positive.
    #[test]
    fn uchar_escapes_are_resolved_before_the_parser_sees_them() {
        // `é` denotes `é`, which RFC 3987 allows.
        assert_eq!(iri_content_defect("http://example.org/\\u00E9"), None);
        assert_eq!(sanitize_iri_content("http://example.org/\\u00E9"), None);
        assert_eq!(iri_content_defect("http://example.org/a\\U0001F600b"), None);
        // An escape can smuggle a character the raw grammar forbids. The parser
        // is the only thing that catches it, and it must.
        for smuggled in [
            "http://example.org/a\\u0020b", // space
            "http://example.org/a\\u003Eb", // '>'
            "http://example.org/a\\u005Cb", // '\'
            "http://example.org/a\\u0022b", // '"'
        ] {
            assert_eq!(
                iri_content_defect(smuggled),
                Some(IriDefect::Unclassified),
                "{smuggled}"
            );
        }
        // A surrogate is not a character; a strict reader refuses it.
        assert_eq!(
            iri_content_defect("http://example.org/\\uD800"),
            Some(IriDefect::Unclassified)
        );
        // `#` and `?` arrive legally through an escape.
        assert_eq!(iri_content_defect("http://example.org/a\\u003Fq"), None);
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

    /// Found by the `iri` fuzz target. A quoted triple whose subject is a
    /// relative IRI and whose object is merely badly escaped used to be
    /// "repaired" into a term that was still invalid — and, worse, counted as
    /// repaired, with only the *first* defect recorded, so the relative IRI
    /// never reached `unrepairable()` and never blocked.
    #[test]
    fn a_partly_repairable_quoted_triple_is_not_claimed_as_repaired() {
        let token = "<<<relative/x> <http://ex/p> <http://ex/o[1]>>>";
        assert_eq!(sanitize_term(token), None, "a partial repair was claimed");

        let mut r = IriReport::default();
        assert_eq!(r.sanitize(token), token, "the term must be left verbatim");
        assert_eq!(r.repaired(), 0);
        // BOTH defects are recorded, not just the first.
        assert_eq!(r.occurrences(), 2);
        assert_eq!(r.count(IriDefect::NotAbsolute), 1);
        assert_eq!(r.count(IriDefect::Bracket), 1);
        // And the one that blocks is visible to the gate.
        assert_eq!(r.unrepairable(), 1);
    }

    /// The other half: when every component *can* be repaired, it is.
    #[test]
    fn a_fully_repairable_quoted_triple_still_repairs() {
        let token = "<<<http://ex/a[b]> <http://ex/p> <http://ex/c#d#e>>>";
        let mut r = IriReport::default();
        assert_eq!(
            r.sanitize(token),
            "<<<http://ex/a%5Bb%5D> <http://ex/p> <http://ex/c#d%23e>>>"
        );
        assert_eq!(r.occurrences(), 2);
        assert_eq!(r.repaired(), 2);
        assert_eq!(r.unrepairable(), 0);
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
