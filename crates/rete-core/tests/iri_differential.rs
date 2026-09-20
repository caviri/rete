//! **Differential test: rete's IRI verdict vs the parser that actually rejects
//! our dumps.**
//!
//! `rete export --format nq` promises N-Quads, and the thing that decides whether
//! it delivered is the loader. So the question "is this IRI valid" is answered
//! here by running the candidate through `oxttl`'s N-Triples parser — the same
//! parser Oxigraph's `convert` and `load` use, already a dependency of this
//! crate — and requiring that [`rete_core::iri::iri_content_defect`] agrees on
//! **every** input.
//!
//! Disagreement in either direction is a bug, and they are different bugs:
//!
//! * *we say valid, the parser says invalid* — a dump is published that will not
//!   load. This is the `<https://::1>` incident, and it is the dangerous one.
//! * *we say invalid, the parser says valid* — a false positive. It blocks a good
//!   dataset, and (if the class were repairable) would percent-encode an IRI
//!   that was already fine.
//!
//! The corpus is three layers: a hand-written table of the cases the design
//! turns on, an exhaustive sweep of every ASCII character in every structural
//! position of an IRI, and proptest-generated strings assembled from the
//! characters that delimit an IRI's parts.

use proptest::prelude::*;
use rete_core::iri::iri_content_defect;

/// What Oxigraph's N-Triples reader makes of `<content>` in subject position.
///
/// The IRI is placed in a complete statement so the real lexer + parser run,
/// exactly as they would over a line of an exported dump. No base IRI is set,
/// so a relative IRI is rejected — which is what a bulk load of our dumps does.
fn oxttl_accepts(content: &str) -> bool {
    let line = format!("<{content}> <http://example.org/p> <http://example.org/o> .\n");
    let mut ok = false;
    for r in oxttl::NTriplesParser::new().for_reader(line.as_bytes()) {
        match r {
            Ok(_) => ok = true,
            Err(_) => return false,
        }
    }
    ok
}

/// The one assertion, in both directions, with the failure mode named.
#[track_caller]
fn assert_agrees(content: &str) {
    let ours = iri_content_defect(content);
    let theirs = oxttl_accepts(content);
    match (ours, theirs) {
        (None, true) | (Some(_), false) => {}
        (None, false) => panic!(
            "UNSOUND: rete calls {content:?} valid, the parser rejects it — \
             a dump carrying this would be published and would not load"
        ),
        (Some(d), true) => panic!(
            "FALSE POSITIVE: rete flags {content:?} as {d:?}, the parser accepts it — \
             this blocks a valid dataset"
        ),
    }
}

#[test]
fn the_cases_the_design_turns_on() {
    // Legal. Every one of these must stay clean.
    for ok in [
        "http://user:pass@host/",
        "http://[::1]:8080/p",
        "urn:isbn:0451450523",
        "mailto:a@b.com",
        "http://host:8080/a:b",
        "http://caf\u{e9}.example/",
        "http://example.org/caf\u{e9}",
        "http://example.org/\u{4e2d}\u{6587}",
        "https://[2001:db8::1]:443/x?q=1#f",
        "http://host/",
        "http://host:/p",
        "ftp://ftp.example.org/pub/",
        "did:example:123456789abcdefghi",
        "tag:example.org,2026:x",
        "file:///tmp/x",
        "http://example.org/(a),b;c=d!e$f*g'h+i",
        "http://example.org/a?b=c&d=%20e#frag",
        "http://example.org/a%20b",
        "http://example.org/\\u00E9",
        "http://example.org/a\\U0001F600b",
        "urn:uuid:2a5c3f60-0000-4000-8000-000000000000",
        "https://example.org/p?q=1&r=2#frag",
        "h://a",
        "a+b-c.d://x",
        "http://192.168.0.1:80/",
        "http://example.org/%E4%B8%AD",
    ] {
        assert_eq!(iri_content_defect(ok), None, "{ok} must be valid");
        assert_agrees(ok);
    }

    // Invalid. Each must be flagged, and the parser must agree.
    for bad in [
        "https://::1",   // the incident
        "http://h:80x/", // non-digit port
        "http://a@b@c/", // two '@'
        "http://[::1/x", // unclosed bracket
        "noscheme/path",
        "",
        "/absolute/path",
        "1http://example.org/",
        "http://example.org/a b",
        "http://example.org/a\"b",
        "http://example.org/a|b",
        "http://example.org/a\\b",
        "http://example.org/a[b]",
        "http://example.org/c#d#e",
        "http://example.org/%x",
        "http://example.org/%",
        "http://example.org/\\uD800",
        "http://example.org/a\\u0020b",
        "http://example.org/a\\u003Cb",
        "://x",
        ":",
        "http://exa mple.org/",
    ] {
        assert!(iri_content_defect(bad).is_some(), "{bad} must be flagged");
        assert_agrees(bad);
    }
}

/// Every ASCII character, dropped into every structural position of an IRI.
///
/// This is the sweep that would have caught `<https://::1>` before it shipped:
/// the authority is one of the positions, and `:` is one of the characters.
#[test]
fn every_ascii_character_in_every_position() {
    let templates: &[&str] = &[
        "http://example.org/{C}",      // path
        "http://example.org/a{C}b",    // mid-path
        "http://{C}/p",                // whole authority
        "http://ex{C}ample.org/",      // inside the host
        "http://example.org{C}/",      // end of authority
        "http://example.org:8{C}0/",   // inside the port
        "http://u{C}ser@example.org/", // userinfo
        "http://example.org/?q={C}",   // query
        "http://example.org/#f{C}",    // fragment
        "ht{C}tp://example.org/",      // scheme
        "{C}http://example.org/",      // before the scheme
        "http://[::{C}1]/p",           // inside an IP-literal
        "http://example.org/a%{C}0b",  // inside a pct-triplet
    ];
    let mut checked = 0;
    for c in 0u8..=0x7f {
        let ch = c as char;
        for t in templates {
            let content = t.replace("{C}", &ch.to_string());
            // A raw '>' or a newline ends the IRIREF / the line, so the string
            // could never reach a parser in this form; the character check
            // rejects both anyway.
            if content.contains(['>', '\n', '\r']) {
                continue;
            }
            assert_agrees(&content);
            checked += 1;
        }
    }
    assert!(checked > 1000, "only {checked} combinations checked");
}

/// Non-ASCII, which RFC 3987 admits as `ucschar` and RFC 3986 does not. The
/// module docs used to say this was "deliberately not judged"; it is judged now,
/// by the parser, so the two must agree here too.
#[test]
fn non_ascii_in_every_position() {
    let chars = [
        '\u{e9}',
        '\u{4e2d}',
        '\u{1F600}',
        '\u{a0}',
        '\u{200b}',
        '\u{fffd}',
        '\u{e000}',
        '\u{10FFFF}',
        '\u{2028}',
        '\u{3000}',
    ];
    for ch in chars {
        for t in [
            "http://example.org/{C}",
            "http://{C}.example.org/",
            "http://example.org/?q={C}",
            "http://example.org/#f{C}",
            "ht{C}tp://example.org/",
            "http://example.org:80{C}/",
        ] {
            assert_agrees(&t.replace("{C}", &ch.to_string()));
        }
    }
}

// The characters that delimit an IRI's parts, plus the ones the IRIREF
// production excludes, plus a couple of `ucschar` — the alphabet most likely to
// build a string the two implementations could read differently.
const ALPHABET: &[char] = &[
    'a', 'B', '0', '9', ':', '/', '?', '#', '[', ']', '@', '!', '$', '&', '\'', '(', ')', '*', '+',
    ',', ';', '=', '%', '.', '-', '_', '~', ' ', '<', '"', '{', '}', '|', '^', '`', '\\', 'u', 'U',
    'F', '\u{e9}', '\u{4e2d}', '\u{7f}', '\u{1}',
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// Free-form strings over that alphabet.
    #[test]
    fn arbitrary_strings_agree(
        cs in prop::collection::vec(prop::sample::select(ALPHABET), 0..24)
    ) {
        let s: String = cs.into_iter().collect();
        if s.contains(['>', '\n', '\r']) { return Ok(()); }
        assert_agrees(&s);
    }

    /// Strings shaped like an IRI, so the generator spends its budget on the
    /// grammar's interesting corners instead of rediscovering "needs a scheme".
    #[test]
    fn iri_shaped_strings_agree(
        scheme in "[a-z][a-z0-9+.-]{0,4}",
        auth in prop::collection::vec(prop::sample::select(ALPHABET), 0..10),
        path in prop::collection::vec(prop::sample::select(ALPHABET), 0..10),
        slashes in prop::bool::ANY,
    ) {
        let a: String = auth.into_iter().collect();
        let p: String = path.into_iter().collect();
        let s = if slashes {
            format!("{scheme}://{a}/{p}")
        } else {
            format!("{scheme}:{a}{p}")
        };
        if s.contains(['>', '\n', '\r']) { return Ok(()); }
        assert_agrees(&s);
    }

    /// Authorities specifically — userinfo, host, port — which is where the
    /// taxonomy had its hole.
    #[test]
    fn authorities_agree(
        user in prop::option::of("[a-z:@%]{0,6}"),
        host in "[a-z0-9.:\\[\\]-]{0,12}",
        port in prop::option::of("[0-9a-z]{0,5}"),
    ) {
        let mut s = String::from("http://");
        if let Some(u) = user { s.push_str(&u); s.push('@'); }
        s.push_str(&host);
        if let Some(p) = port { s.push(':'); s.push_str(&p); }
        s.push_str("/path");
        assert_agrees(&s);
    }
}
