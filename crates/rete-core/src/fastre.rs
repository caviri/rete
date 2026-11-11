//! Plain string operations for the few regex shapes that need no regex engine.
//!
//! `REGEX` and `REPLACE` with a constant pattern and no flags often ask for
//! nothing more than "the part after the last `/`" or "does it contain this
//! text". Those run as `str` searches here; every other pattern (any flag, any
//! construct not listed below) returns `None` from [`Fast::parse`] and uses the
//! regex engine unchanged. The results are byte-identical to the engine's: see
//! the differential test at the end of this file.
//!
//! Shapes (flags empty; `LIT` is non-metacharacters or `\`-escaped
//! metacharacters, `SET` is 1+ printable ASCII characters other than
//! `\ ^ - [ ] & ~`):
//!
//! | pattern        | match                                             |
//! |----------------|---------------------------------------------------|
//! | `LIT`          | every occurrence (non-overlapping, left to right) |
//! | `^LIT`         | the prefix                                        |
//! | `LIT$`         | the suffix                                        |
//! | `[^SET]*$`     | the suffix after the last SET character (maybe empty) |
//! | `^.*[SET]`, `^.*C` | the prefix up to and including the last SET character; the engine's `.` stops at `\n`, so a text containing one uses the engine |
//!
//! A replacement containing `$` or `\` also uses the engine (group references).

/// Characters of `[...]` this module accepts: no escapes, ranges, nesting or
/// set operations to get wrong.
fn plain_class_char(c: char) -> bool {
    c.is_ascii_graphic() && !"\\^-[]&~".contains(c) || c == ' '
}

const META: &str = ".^$*+?()[]{}|\\";

/// A pattern made of literal characters; `\` before a metacharacter is that
/// character. `None` for anything else (including the empty pattern).
fn literal(p: &str) -> Option<String> {
    let mut out = String::new();
    let mut it = p.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            let n = it.next()?;
            if !META.contains(n) {
                return None;
            }
            out.push(n);
        } else if META.contains(c) {
            return None;
        } else {
            out.push(c);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A set of ASCII bytes (or one arbitrary char) to search for.
#[derive(Debug, Clone)]
pub(crate) enum CharSet {
    One(char),
    Ascii(Vec<u8>),
}

impl CharSet {
    /// Byte index just past the last member of the set, if any.
    fn rfind_end(&self, text: &str) -> Option<usize> {
        match self {
            CharSet::One(c) => text.rfind(*c).map(|i| i + c.len_utf8()),
            // Members are ASCII: a UTF-8 continuation byte never equals one.
            CharSet::Ascii(set) => text
                .as_bytes()
                .iter()
                .rposition(|b| set.contains(b))
                .map(|i| i + 1),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Fast {
    Contains(String),
    Prefix(String),
    Suffix(String),
    /// `[^SET]*$`
    Tail(CharSet),
    /// `^.*[SET]`
    Head(CharSet),
}

impl Fast {
    /// The fast shape of `pattern` under `flags`, if it is one.
    pub(crate) fn parse(pattern: &str, flags: &str) -> Option<Fast> {
        if !flags.is_empty() {
            return None;
        }
        if let Some(rest) = pattern.strip_prefix("^.*") {
            if let Some(body) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                return plain_set(body).map(Fast::Head);
            }
            let l = literal(rest)?;
            let mut cs = l.chars();
            return match (cs.next(), cs.next()) {
                (Some(c), None) => Some(Fast::Head(CharSet::One(c))),
                _ => None,
            };
        }
        if let Some(body) = pattern
            .strip_prefix("[^")
            .and_then(|r| r.strip_suffix("]*$"))
        {
            return plain_set(body).map(Fast::Tail);
        }
        if let Some(rest) = pattern.strip_prefix('^') {
            return literal(rest).map(Fast::Prefix);
        }
        if let Some(rest) = pattern.strip_suffix('$') {
            // `a\$` leaves a lone `\` in `rest`: no anchor, a literal `$`
            // (the `Contains` case below).
            if let Some(l) = literal(rest) {
                return Some(Fast::Suffix(l));
            }
        }
        literal(pattern).map(Fast::Contains)
    }

    /// `REGEX`; `None` = this text needs the engine.
    pub(crate) fn is_match(&self, text: &str) -> Option<bool> {
        Some(match self {
            Fast::Contains(l) => text.contains(l.as_str()),
            Fast::Prefix(l) => text.starts_with(l.as_str()),
            Fast::Suffix(l) => text.ends_with(l.as_str()),
            Fast::Tail(_) => true,
            Fast::Head(set) => {
                if text.contains('\n') {
                    return None;
                }
                set.rfind_end(text).is_some()
            }
        })
    }

    /// `REPLACE` with a `$`- and `\`-free `rep`; `None` = use the engine.
    pub(crate) fn replace(&self, text: &str, rep: &str) -> Option<String> {
        if rep.bytes().any(|b| b == b'$' || b == b'\\') {
            return None;
        }
        Some(match self {
            Fast::Contains(l) => text.replace(l.as_str(), rep),
            Fast::Prefix(l) => match text.strip_prefix(l.as_str()) {
                Some(rest) => [rep, rest].concat(),
                None => text.to_string(),
            },
            Fast::Suffix(l) => match text.strip_suffix(l.as_str()) {
                Some(head) => [head, rep].concat(),
                None => text.to_string(),
            },
            // One (possibly empty) match at the end; the engine does not
            // report a second, empty one right after it.
            Fast::Tail(set) => {
                let at = set.rfind_end(text).unwrap_or(0);
                [&text[..at], rep].concat()
            }
            Fast::Head(set) => {
                if text.contains('\n') {
                    return None;
                }
                match set.rfind_end(text) {
                    Some(at) => [rep, &text[at..]].concat(),
                    None => text.to_string(),
                }
            }
        })
    }
}

fn plain_set(body: &str) -> Option<CharSet> {
    if body.is_empty() || !body.chars().all(plain_class_char) {
        return None;
    }
    let mut bytes: Vec<u8> = body.bytes().collect();
    bytes.sort_unstable();
    bytes.dedup();
    Some(CharSet::Ascii(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row::Regex;
    use proptest::prelude::*;

    /// Texts rich in the characters the patterns look for, plus newlines,
    /// non-ASCII and the empty string.
    fn text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                4 => proptest::sample::select(vec!['/', '#', 'a', 'b', '.', '$', ' ', ':']),
                1 => proptest::sample::select(vec!['\n', '\r', 'é', 'ß', '日', '😀', '\u{301}']),
                2 => any::<char>(),
            ],
            0..14,
        )
        .prop_map(|v| v.into_iter().collect())
    }

    fn set_chars() -> impl Strategy<Value = Vec<char>> {
        proptest::collection::vec(
            prop_oneof![
                4 => proptest::sample::select(vec!['/', '#', 'a', ':', '.', '$', ' ', '?', '*', '|', '(', '{']),
                1 => proptest::char::range(' ', '~'),
            ],
            1..4,
        )
        .prop_map(|v| v.into_iter().filter(|c| plain_class_char(*c)).collect::<Vec<_>>())
        .prop_filter("non-empty", |v| !v.is_empty())
    }

    fn lit_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            proptest::sample::select(vec![
                'a', 'b', '/', '.', '$', '^', '(', 'é', '日', '\\', ' ', '-', '\n',
            ]),
            1..4,
        )
        .prop_map(|v| v.into_iter().collect())
    }

    fn escape(l: &str) -> String {
        l.chars()
            .flat_map(|c| {
                if META.contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect()
    }

    fn pattern() -> impl Strategy<Value = String> {
        prop_oneof![
            lit_text().prop_map(|l| escape(&l)),
            lit_text().prop_map(|l| format!("^{}", escape(&l))),
            lit_text().prop_map(|l| format!("{}$", escape(&l))),
            set_chars().prop_map(|s| format!("[^{}]*$", s.iter().collect::<String>())),
            set_chars().prop_map(|s| format!("^.*[{}]", s.iter().collect::<String>())),
            lit_text()
                .prop_map(|l| format!("^.*{}", escape(&l.chars().take(1).collect::<String>()))),
        ]
    }

    fn rep() -> impl Strategy<Value = String> {
        proptest::sample::select(vec!["", "X", "-", "é日", "$1", "a$", "\\", "$0$0", "[$]"])
            .prop_map(String::from)
    }

    fn check(pat: &str, text: &str, rep: &str) -> Result<(), TestCaseError> {
        let re = Regex::new(pat).expect("valid");
        let fast = Fast::parse(pat, "");
        prop_assert!(fast.is_some(), "shape not recognised: {pat:?}");
        let f = fast.unwrap();
        if let Some(m) = f.is_match(text) {
            prop_assert_eq!(m, re.is_match(text), "REGEX {:?} on {:?}", pat, text);
        }
        if let Some(out) = f.replace(text, rep) {
            prop_assert_eq!(
                out,
                re.replace_all(text, rep).into_owned(),
                "REPLACE {:?} on {:?} with {:?}",
                pat,
                text,
                rep
            );
        } else {
            prop_assert!(
                rep.contains(['$', '\\']) || text.contains('\n'),
                "declined {pat:?} on {text:?}"
            );
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(5000))]
        #[test]
        fn fast_paths_match_the_engine(pat in pattern(), t in text(), r in rep()) {
            // Skip patterns the generator made into something else (a `\` or a
            // newline in an anchored literal is still a literal; everything
            // here must parse).
            check(&pat, &t, &r)?;
        }

        /// Texts built from the pattern's own characters hit the interesting
        /// cases (adjacent matches, matches at both ends) far more often.
        #[test]
        fn dense_texts(set in set_chars(), t in proptest::collection::vec(0usize..5, 0..10), r in rep()) {
            let alphabet: Vec<char> = set.iter().copied().chain(['x', 'é']).collect();
            let text: String = t.iter().map(|i| alphabet[i % alphabet.len()]).collect();
            let s: String = set.iter().collect();
            for pat in [format!("[^{s}]*$"), format!("^.*[{s}]")] {
                check(&pat, &text, &r)?;
            }
            let l = escape(&s);
            for pat in [l.clone(), format!("^{l}"), format!("{l}$")] {
                check(&pat, &text, &r)?;
            }
        }
    }

    #[test]
    fn declines_what_it_does_not_handle() {
        for (p, f) in [
            ("a", "i"),
            ("[^/]*$", "s"),
            ("", ""),
            ("^", ""),
            ("$", ""),
            ("a|b", ""),
            ("(a)", ""),
            ("a.b", ""),
            ("a+", ""),
            ("\\d", ""),
            ("\\.", "x"),
            ("[^/]+$", ""),
            ("[^\\/]*$", ""),
            ("[^a-z]*$", ""),
            ("[^]*$", ""),
            ("[^^]*$", ""),
            ("[a-z]", ""),
            ("^.*[a-z]", ""),
            ("^.*[]", ""),
            ("^.*ab", ""),
            ("^.*", ""),
            ("a\\", ""),
            ("\\p{L}", ""),
            ("^a$", ""),
            ("^^a", ""),
            ("^.*[é]", ""),
            ("[^é]*$", ""),
            ("[^a&&b]*$", ""),
        ] {
            assert!(Fast::parse(p, f).is_none(), "{p:?} {f:?}");
        }
    }

    #[test]
    fn known_cases() {
        let f = Fast::parse("[^/]*$", "").unwrap();
        assert_eq!(f.replace("a/b/c", "X").unwrap(), "a/b/X");
        assert_eq!(f.replace("a/", "X").unwrap(), "a/X");
        assert_eq!(f.replace("", "X").unwrap(), "X");
        let g = Fast::parse("^.*[/#]", "").unwrap();
        assert_eq!(g.replace("http://x/y#z", "").unwrap(), "z");
        assert_eq!(g.replace("a\nb/c", ""), None);
        assert_eq!(g.is_match("nothing"), Some(false));
    }
}
