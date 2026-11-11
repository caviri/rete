//! The order of the terms inside a dictionary section (SPEC.md §5).
//!
//! Dictionary ids are lexical ranks, so *which* lexical order a section uses
//! decides what id order means.
//!
//! * [`TermOrder::Bytewise`] is the order of formats `0x05` to `0x07`: the
//!   UTF-8 bytes of the stored token (`<iri>`, `_:b`, `"lit"…`). It is not the
//!   SPARQL order: `<…Ng8130_provenance>` sorts before `<…Ng8>` because `1`
//!   is below `>`, and literals sort before IRIs before blank nodes.
//! * [`TermOrder::Sparql`] is the order of format `0x08`. Terms group by kind,
//!   in the order SPARQL 1.1 §15.1 gives (blank nodes, IRIs, literals), and
//!   within a group sort so that **id order is the engine's `ORDER BY` order**
//!   wherever the engine's order is a function of the term alone and cheap to
//!   state:
//!
//!   | group | terms | sorted by |
//!   |---|---|---|
//!   | 0 blank | `_:…` | the token |
//!   | 1 IRI | `<…>` | the IRI text without `<` and `>`, by code point (UTF-8 byte order) |
//!   | 2 text | simple, `xsd:string` and language-tagged literals | lexical form unescaped, then datatype, then lower-cased tag, then the token |
//!   | 3 typed | every other literal (numbers, dates, booleans, other datatypes) and anything else | the token, bytewise |
//!   | 4 triple | `<< … >>` | the token |
//!
//!   Groups 0, 1, 2 and 4 are **order-exact**: two terms of one section in
//!   those groups compare as their ids do. Group 3 is not: `ORDER BY` puts
//!   numbers by value, and the dictionary does not.
//!
//! One function, [`TermOrder::cmp`], is the order; every writer sorts with it
//! and every lookup searches with it, so a file and its reader cannot disagree.

use std::cmp::Ordering;

/// How a dictionary section orders its terms. The format generation says
/// which: `0x05`-`0x07` are [`Bytewise`](Self::Bytewise), `0x08` is
/// [`Sparql`](Self::Sparql).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TermOrder {
    /// The bytes of the stored token.
    #[default]
    Bytewise,
    /// Kind, then the SPARQL `ORDER BY` order; see the module docs.
    Sparql,
}

thread_local! {
    static BUILDER_ORDER: std::cell::Cell<Option<TermOrder>> = const { std::cell::Cell::new(None) };
}

/// Run `f` with every [`crate::DictionaryBuilder::new`] on this thread sorting in
/// `order`. The tests that need an input in an older generation (`0x05` to
/// `0x07` are byte-ordered) build it this way; a builder made on another
/// thread is not affected.
#[doc(hidden)]
pub fn with_builder_order<R>(order: TermOrder, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<TermOrder>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BUILDER_ORDER.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(BUILDER_ORDER.with(|c| c.replace(Some(order))));
    f()
}

/// The order [`with_builder_order`] set on this thread, if any.
pub(crate) fn builder_order_override() -> Option<TermOrder> {
    BUILDER_ORDER.with(|c| c.get())
}

/// Term group (the kind ranks of [`TermOrder::Sparql`]).
pub const G_BLANK: u8 = 0;
/// IRIs.
pub const G_IRI: u8 = 1;
/// Simple, `xsd:string` and language-tagged literals.
pub const G_TEXT: u8 = 2;
/// Literals of any other datatype, and anything that is no term at all.
pub const G_TYPED: u8 = 3;
/// RDF-star quoted triples.
pub const G_TRIPLE: u8 = 4;
/// Number of groups.
pub const GROUPS: usize = 5;

const XSD_STRING_SUFFIX: &str = "^^<http://www.w3.org/2001/XMLSchema#string>";

/// The group of a term token.
pub fn group(t: &[u8]) -> u8 {
    match t.first() {
        Some(b'<') => {
            if t.starts_with(b"<<") && t.ends_with(b">>") {
                G_TRIPLE
            } else {
                G_IRI
            }
        }
        Some(b'_') if t.get(1) == Some(&b':') => G_BLANK,
        Some(b'"') => match std::str::from_utf8(t) {
            Ok(s) => literal_group(s),
            Err(_) => G_TYPED,
        },
        _ => G_TYPED,
    }
}

fn literal_group(s: &str) -> u8 {
    match crate::terms::literal_parts(s) {
        Some((_, "")) => G_TEXT,
        Some((_, suffix)) if suffix.starts_with('@') || suffix == XSD_STRING_SUFFIX => G_TEXT,
        _ => G_TYPED,
    }
}

/// The IRI text of an `<…>` token (the whole token when it is not bracketed,
/// as the engine's `ORDER BY` reads it).
fn iri_text(t: &[u8]) -> &[u8] {
    if t.len() >= 2 && t[0] == b'<' && t[t.len() - 1] == b'>' {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

fn lexical(body: &str) -> std::borrow::Cow<'_, str> {
    if body.contains('\\') {
        std::borrow::Cow::Owned(crate::terms::unescape_literal(body))
    } else {
        std::borrow::Cow::Borrowed(body)
    }
}

/// 0 `rdf:dirLangString`, 1 `rdf:langString`, 2 `xsd:string`: the order of
/// those datatype IRIs as strings.
fn datatype_rank(suffix: &str) -> u8 {
    if let Some(tag) = suffix.strip_prefix('@') {
        if tag.contains("--") {
            0
        } else {
            1
        }
    } else {
        2
    }
}

/// Order two group-2 tokens: `(lexical form, datatype, lower-cased tag, token)`,
/// the engine's text-literal order (`sparql::value::order_cmp`).
fn text_cmp(a: &[u8], b: &[u8]) -> Ordering {
    // Both passed `literal_group`, so both are UTF-8 and well formed.
    let (Ok(sa), Ok(sb)) = (std::str::from_utf8(a), std::str::from_utf8(b)) else {
        return a.cmp(b);
    };
    let (Some((ba, xa)), Some((bb, xb))) = (
        crate::terms::literal_parts(sa),
        crate::terms::literal_parts(sb),
    ) else {
        return a.cmp(b);
    };
    lexical(ba)
        .as_ref()
        .cmp(lexical(bb).as_ref())
        .then_with(|| datatype_rank(xa).cmp(&datatype_rank(xb)))
        .then_with(|| {
            let ta = xa.strip_prefix('@').unwrap_or("").bytes();
            let tb = xb.strip_prefix('@').unwrap_or("").bytes();
            ta.map(|c| c.to_ascii_lowercase())
                .cmp(tb.map(|c| c.to_ascii_lowercase()))
        })
        .then_with(|| a.cmp(b))
}

impl TermOrder {
    /// Compare two term tokens in this order. A total order: two tokens
    /// compare equal only when they are the same bytes. The empty slice sorts
    /// before everything (a chunk directory's first key is empty).
    pub fn cmp(self, a: &[u8], b: &[u8]) -> Ordering {
        match self {
            TermOrder::Bytewise => a.cmp(b),
            TermOrder::Sparql => {
                if a.is_empty() || b.is_empty() {
                    return a.len().cmp(&b.len());
                }
                let (ga, gb) = (group(a), group(b));
                if ga != gb {
                    return ga.cmp(&gb);
                }
                match ga {
                    G_IRI => iri_text(a).cmp(iri_text(b)).then_with(|| a.cmp(b)),
                    G_TEXT => text_cmp(a, b),
                    _ => a.cmp(b),
                }
            }
        }
    }

    /// `self.cmp(a, b)` on strings.
    pub fn cmp_str(self, a: &str, b: &str) -> Ordering {
        self.cmp(a.as_bytes(), b.as_bytes())
    }

    /// A chunk directory's routing key between two neighbouring chunks: a
    /// short byte string `k` with `prev_last < k <= first` in this order.
    ///
    /// Bytewise it is [`crate::dict::shortest_separator`]. In the SPARQL order
    /// two IRIs get the shortest separator of their texts, written as an IRI
    /// token; every other boundary keeps `first` whole.
    pub fn separator(self, prev_last: &[u8], first: &[u8]) -> Vec<u8> {
        if self == TermOrder::Sparql {
            let iri = |t: &[u8]| group(t) == G_IRI && t.len() >= 2 && t[t.len() - 1] == b'>';
            if iri(prev_last) && iri(first) && self.cmp(prev_last, first) == Ordering::Less {
                let s = crate::dict::shortest_separator(iri_text(prev_last), iri_text(first));
                let mut k = Vec::with_capacity(s.len() + 2);
                k.push(b'<');
                k.extend_from_slice(&s);
                k.push(b'>');
                return k;
            }
            if let Some(k) = self.text_separator(prev_last, first) {
                return k;
            }
            return first.to_vec();
        }
        crate::dict::shortest_separator(prev_last, first)
    }
}

impl TermOrder {
    /// The separator of two text literals: a simple literal whose lexical form
    /// is the shortest string strictly between the two lexical forms (so no
    /// datatype or tag decides). `None` when there is none (equal or prefix
    /// lexical forms): the caller keeps `first` whole.
    fn text_separator(self, prev_last: &[u8], first: &[u8]) -> Option<Vec<u8>> {
        if group(prev_last) != G_TEXT || group(first) != G_TEXT {
            return None;
        }
        let (sp, sf) = (
            std::str::from_utf8(prev_last).ok()?,
            std::str::from_utf8(first).ok()?,
        );
        let (bp, _) = crate::terms::literal_parts(sp)?;
        let (bf, _) = crate::terms::literal_parts(sf)?;
        let (lp, lf) = (lexical(bp), lexical(bf));
        if lp.as_ref() >= lf.as_ref() {
            return None;
        }
        let shared = lp
            .as_bytes()
            .iter()
            .zip(lf.as_bytes())
            .take_while(|(a, b)| a == b)
            .count();
        // One byte past the shared prefix, then to a whole character.
        let mut end = shared + 1;
        while end < lf.len() && !lf.is_char_boundary(end) {
            end += 1;
        }
        if end >= lf.len() {
            return None; // the whole of `first`'s lexical form: no shorter key
        }
        let key = crate::terms::make_literal(&lf[..end], None, None);
        // Belt and braces: the key must route between the two.
        (self.cmp(prev_last, key.as_bytes()) == Ordering::Less
            && self.cmp(key.as_bytes(), first) != Ordering::Greater)
            .then(|| key.into_bytes())
    }
}

/// How many terms of a section are in each group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupCounts(pub [u32; GROUPS]);

impl GroupCounts {
    /// Count one term.
    pub fn add(&mut self, t: &[u8]) {
        self.0[group(t) as usize] += 1;
    }

    /// Ids `1..=lit_start()` are blank nodes and IRIs.
    pub fn lit_start(&self) -> u32 {
        self.0[G_BLANK as usize].saturating_add(self.0[G_IRI as usize])
    }

    /// Ids `lit_start()+1..=text_end()` are text literals.
    pub fn text_end(&self) -> u32 {
        self.lit_start().saturating_add(self.0[G_TEXT as usize])
    }

    /// Ids `text_end()+1..=lit_end()` are typed literals; everything after
    /// is a quoted triple.
    pub fn lit_end(&self) -> u32 {
        self.text_end().saturating_add(self.0[G_TYPED as usize])
    }

    /// Terms in all groups.
    pub fn total(&self) -> u64 {
        self.0.iter().map(|&n| n as u64).sum()
    }

    /// Is section-local id `id` in an order-exact group, so that it compares
    /// with another exact id of the same section as the terms do?
    pub fn is_exact(&self, id: u32) -> bool {
        !(id > self.text_end() && id <= self.lit_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(order: TermOrder, mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| order.cmp_str(a, b));
        v
    }

    #[test]
    fn iri_prefix_sorts_before_its_extensions() {
        let v = vec![
            "<http://x/Ng8130_provenance>",
            "<http://x/Ng8>",
            "<http://x/Ng8->",
        ];
        assert_eq!(
            sorted(TermOrder::Sparql, v.clone()),
            vec![
                "<http://x/Ng8>",
                "<http://x/Ng8->",
                "<http://x/Ng8130_provenance>"
            ]
        );
        // the bytewise order is the one that put the extension first
        assert_eq!(
            TermOrder::Bytewise.cmp_str("<http://x/Ng8130_provenance>", "<http://x/Ng8>"),
            Ordering::Less
        );
    }

    #[test]
    fn kinds_group_in_sparql_order() {
        let v = vec![
            "\"z\"",
            "<http://a>",
            "_:b",
            "\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "<<_:a <http://p> _:b>>",
            "\"a\"@en",
        ];
        assert_eq!(
            sorted(TermOrder::Sparql, v),
            vec![
                "_:b",
                "<http://a>",
                "\"a\"@en",
                "\"z\"",
                "\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>",
                "<<_:a <http://p> _:b>>",
            ]
        );
    }

    #[test]
    fn text_literals_order_by_lexical_form_not_by_closing_quote() {
        let v = vec!["\"a b\"", "\"a\"", "\"a\"@en", "\"a!\"", "\"B\""];
        // lexical: "B" < "a" < "a b" < "a!"; at "a": rdf:langString < xsd:string
        assert_eq!(
            sorted(TermOrder::Sparql, v),
            vec!["\"B\"", "\"a\"@en", "\"a\"", "\"a b\"", "\"a!\""]
        );
    }

    #[test]
    fn escapes_compare_unescaped() {
        // `a"b` unescaped sorts between `a!` and `a#`; escaped (`a\"b`) the
        // backslash would put it after `a#`.
        let v = vec!["\"a#\"", "\"a\\\"b\"", "\"a!\""];
        assert_eq!(
            sorted(TermOrder::Sparql, v),
            vec!["\"a!\"", "\"a\\\"b\"", "\"a#\""]
        );
    }

    #[test]
    fn non_ascii_is_codepoint_order() {
        let v = vec!["<http://x/\u{e9}>", "<http://x/z>", "<http://x/\u{1f600}>"];
        assert_eq!(
            sorted(TermOrder::Sparql, v),
            vec!["<http://x/z>", "<http://x/\u{e9}>", "<http://x/\u{1f600}>"]
        );
    }

    #[test]
    fn xsd_string_is_text_and_other_datatypes_are_typed() {
        assert_eq!(
            group(b"\"x\"^^<http://www.w3.org/2001/XMLSchema#string>"),
            G_TEXT
        );
        assert_eq!(group(b"\"x\"^^<http://example/dt>"), G_TYPED);
        assert_eq!(group(b"\"x\""), G_TEXT);
        assert_eq!(group(b"\"x\"@en-GB"), G_TEXT);
        assert_eq!(group(b"\"x\"@en--ltr"), G_TEXT);
        assert_eq!(group(b"_:b"), G_BLANK);
        assert_eq!(group(b"<<a b c>>"), G_TRIPLE);
        assert_eq!(group(b"<x>"), G_IRI);
        assert_eq!(group(b""), G_TYPED);
    }

    #[test]
    fn it_is_a_total_order() {
        let terms = [
            "_:a",
            "_:b",
            "<a>",
            "<a",
            "<ab>",
            "\"x\"",
            "\"x\"^^<http://www.w3.org/2001/XMLSchema#string>",
            "\"X\"@EN",
            "\"X\"@en",
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "<<a b c>>",
            "junk",
        ];
        for o in [TermOrder::Bytewise, TermOrder::Sparql] {
            for a in terms {
                for b in terms {
                    let ab = o.cmp_str(a, b);
                    assert_eq!(ab, o.cmp_str(b, a).reverse(), "{a} {b}");
                    assert_eq!(ab == Ordering::Equal, a == b, "{a} {b}");
                    for c in terms {
                        if ab == Ordering::Less && o.cmp_str(b, c) == Ordering::Less {
                            assert_eq!(o.cmp_str(a, c), Ordering::Less, "{a} {b} {c}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_separator_routes_between_its_neighbours() {
        let o = TermOrder::Sparql;
        let pairs = [
            ("<http://x/Ng8>", "<http://x/Ng8130_provenance>"),
            ("<http://x/a>", "<http://x/b>"),
            ("<http://x/z>", "\"lit\""),
            ("\"a\"", "\"b\""),
            ("\"aaa\"@en", "\"abcdef\"@en"),
            ("\"caf\u{e9}1\"", "\"caf\u{e9}2 and much more text\""),
        ];
        for (p, f) in pairs {
            let k = o.separator(p.as_bytes(), f.as_bytes());
            assert_eq!(o.cmp(p.as_bytes(), &k), Ordering::Less, "{p} < key {k:?}");
            assert_ne!(
                o.cmp(&k, f.as_bytes()),
                Ordering::Greater,
                "key {k:?} <= {f}"
            );
        }
        // and for text literals
        let k = o.separator(b"\"aaa\"", b"\"abcdef ghij klmn\"");
        assert_eq!(k, b"\"ab\"");
        // shorter than the whole term for IRIs
        let k = o.separator(b"<http://example.org/aaa/1>", b"<http://example.org/bbb/2>");
        assert!(k.len() < b"<http://example.org/bbb/2>".len());
    }

    #[test]
    fn counts_name_the_exact_ranges() {
        let mut c = GroupCounts::default();
        for t in [
            "_:a",
            "<a>",
            "<b>",
            "\"x\"",
            "\"1\"^^<http://e/d>",
            "\"2\"^^<http://e/d>",
        ] {
            c.add(t.as_bytes());
        }
        assert_eq!((c.lit_start(), c.text_end(), c.lit_end()), (3, 4, 6));
        assert!(c.is_exact(1) && c.is_exact(4));
        assert!(!c.is_exact(5) && !c.is_exact(6));
        assert!(c.is_exact(7));
    }
}
