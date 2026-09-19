//! The HDT four-section dictionary, and the term partition that produces it.
//!
//! # Why the partition has to be recomputed
//!
//! rete already keeps an "HDT-style" dictionary — `shared` / `subjects` /
//! `objects` / `predicates`, per-section dense ids, shared-first. It is tempting
//! to copy it across. Two things stop that.
//!
//! **The sort order is different.** rete stores an IRI as the N-Triples token
//! `<http://…>`, brackets included; HDT stores it bare. That changes the
//! collation twice over: blank nodes (`_`, 0x5F) sort *after* IRIs (`<`, 0x3C)
//! in rete and *before* them (`h`, 0x68) in HDT, so the two blocks swap
//! wholesale; and within the IRIs, rete effectively sorts on `iri + ">"` where
//! HDT sorts on `iri + "\0"`, so for any pair where one IRI is a prefix of
//! another — the normal shape of hierarchical identifiers — the order inverts
//! (`/` is 0x2F, below `>` at 0x3E). The displacement is unbounded: a term can
//! move past its entire descendant subtree.
//!
//! **The partition is file-wide, not graph-wide.** rete's `shared` set is
//! computed over every quad in the file. An HDT of one named graph must describe
//! exactly that graph's terms, and a term that is subject-only *there* may be
//! shared file-wide.
//!
//! So the sections are rebuilt from the selected graph's triples: collect the
//! distinct subject and object nodes, intersect for `shared`, resolve, strip,
//! sort, and number.
//!
//! # Plain front coding (`CSD_PFC`)
//!
//! `libhdt/src/libdcs/CSD_PFC.cpp:48-124` (build) and `:198-232` (save). Every
//! `blocksize`-th string is stored whole; the rest store a VByte common-prefix
//! length against the **immediately preceding** string plus the differing
//! suffix. Every string, whole or delta-coded, is NUL-terminated. A separate
//! `LogSequence2` holds the byte offset of each block's first string, plus a
//! trailing sentinel equal to the total text length — so it has `nblocks + 1`
//! entries, which is what the loader's `nblocks = numentries - 1` expects.

use super::codec::{bits_for, crc32c, crc8, vbyte, LogSequence};

/// Strings per block. `hdt-cpp` writes 16 and every file examined uses it; the
/// loader reads whatever is in the header, but matching the convention keeps
/// our output comparable with theirs.
pub(crate) const BLOCK_SIZE: u64 = 16;

/// `CSD::PFC`, the section type byte. `libhdt/src/libdcs/CSD.h:46`.
const TYPE_PFC: u8 = 2;

/// A flat arena of term bytes plus their extents.
///
/// One `Vec<u8>` and one `Vec<(u32, u32)>` rather than `Vec<String>`: a dump of
/// 40M terms would otherwise pay three words of `String` header and a separate
/// allocation each, which is both the memory and the allocator traffic we cannot
/// afford at that scale.
#[derive(Default)]
pub(crate) struct TermArena {
    bytes: Vec<u8>,
    spans: Vec<(u32, u32)>,
}

impl TermArena {
    pub(crate) fn with_capacity(terms: usize, bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(bytes),
            spans: Vec::with_capacity(terms),
        }
    }

    /// Store one canonical N-Triples term in the form HDT wants.
    ///
    /// IRIs lose their angle brackets; literals keep their quotes, language tag
    /// and datatype exactly as they are; blank nodes keep `_:`. Returns the
    /// index of the stored term.
    #[cfg(test)]
    pub(crate) fn push_term(&mut self, token: &str) -> usize {
        let text = hdt_form(token);
        self.push_bytes(&text)
    }

    /// Store an already-stripped term, for a caller that interned it earlier.
    pub(crate) fn push_bytes(&mut self, text: &[u8]) -> usize {
        let start = self.bytes.len() as u32;
        self.bytes.extend_from_slice(text);
        self.spans.push((start, text.len() as u32));
        self.spans.len() - 1
    }

    pub(crate) fn get(&self, i: usize) -> &[u8] {
        let (start, len) = self.spans[i];
        &self.bytes[start as usize..(start + len) as usize]
    }

    /// Total bytes of term text — the `sizeStrings` the dictionary control
    /// information reports.
    pub(crate) fn text_len(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// The arena's indices in HDT's order: ascending raw byte comparison.
    ///
    /// `hdt-cpp` binary-searches sections with `strcmp`, so the order is plain
    /// unsigned byte order over the stored form — not codepoint-aware, not
    /// locale-aware, and computed *after* the brackets come off.
    #[cfg(test)]
    pub(crate) fn sorted_indices(&self) -> Vec<u32> {
        let mut idx: Vec<u32> = (0..self.spans.len() as u32).collect();
        idx.sort_unstable_by(|&a, &b| self.get(a as usize).cmp(self.get(b as usize)));
        idx
    }
}

/// Rewrite a canonical N-Triples term into the byte form HDT stores.
///
/// Two transformations, and the second is the one that is easy to miss:
///
/// 1. An IRI loses its angle brackets.
/// 2. **Escape sequences are resolved.** rete keeps a term in its N-Triples
///    lexical form, so a literal containing a newline is stored as the two
///    characters `\` and `n`. HDT stores the *characters* and lets its
///    serializer re-escape them on the way out — verified by round-tripping a
///    file of awkward literals through `rdf2hdt`/`hdt2rdf`, which stores a real
///    tab for `\t`, a bare `"` for `\"`, and one backslash for `\\`.
///
/// Copying rete's escaped form through unchanged produces a file that loads and
/// queries, and quietly hands back `back\\slash` where the graph said
/// `back\slash`. That is a data corruption, not a formatting difference, which
/// is why this is done here rather than left to the reader.
///
/// The structural parts — the quotes around a literal, an `@lang` tag, a
/// `^^<datatype>` suffix — are preserved exactly; only the lexical form between
/// the outer quotes is unescaped. For an IRI, only `\uXXXX`/`\UXXXXXXXX` can
/// legally appear, and they are resolved the same way.
pub(crate) fn hdt_form(token: &str) -> Vec<u8> {
    if let Some(iri) = token.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        return unescape(iri);
    }
    // A literal: find the closing quote of the lexical form, walking escapes so
    // an embedded `\"` does not end it early.
    if token.starts_with('"') {
        if let Some(end) = literal_end(token) {
            let mut out = Vec::with_capacity(token.len());
            out.push(b'"');
            out.extend_from_slice(&unescape(&token[1..end]));
            out.push(b'"');
            // The suffix (`@en`, `^^<…>`) is structural and passes through.
            out.extend_from_slice(&token.as_bytes()[end + 1..]);
            return out;
        }
    }
    // Blank nodes, quoted triples, anything unrecognized: verbatim.
    token.as_bytes().to_vec()
}

/// Byte index of the closing `"` of a literal starting at index 0.
fn literal_end(token: &str) -> Option<usize> {
    let b = token.as_bytes();
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

/// Resolve the N-Triples escape sequences in `s`.
fn unescape(s: &str) -> Vec<u8> {
    if !s.contains('\\') {
        return s.as_bytes().to_vec();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let (repl, used): (Option<char>, usize) = match b[i + 1] {
            b't' => (Some('\t'), 2),
            b'b' => (Some('\u{8}'), 2),
            b'n' => (Some('\n'), 2),
            b'r' => (Some('\r'), 2),
            b'f' => (Some('\u{c}'), 2),
            b'"' => (Some('"'), 2),
            b'\'' => (Some('\''), 2),
            b'\\' => (Some('\\'), 2),
            b'u' => (hex_char(s, i + 2, 4), 6),
            b'U' => (hex_char(s, i + 2, 8), 10),
            _ => (None, 0),
        };
        match repl {
            Some(c) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                i += used;
            }
            // Not a sequence we recognize: keep the backslash as data rather
            // than dropping it, so nothing is silently lost.
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    out
}

/// Decode `n` hex digits at `at` into a character.
fn hex_char(s: &str, at: usize, n: usize) -> Option<char> {
    let digits = s.get(at..at + n)?;
    if !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    char::from_u32(u32::from_str_radix(digits, 16).ok()?)
}

/// Serialize one front-coded section from `arena`, visiting `order` in sequence.
///
/// `order` must already be in ascending byte order; this does not re-check,
/// because the caller has just sorted it and the check would cost another pass
/// over the whole section.
pub(crate) fn write_pfc_section(out: &mut Vec<u8>, arena: &TermArena, order: &[u32]) {
    let mut text: Vec<u8> = Vec::new();
    let mut blocks: Vec<u64> = Vec::with_capacity(order.len() / BLOCK_SIZE as usize + 2);
    let mut prev: &[u8] = &[];

    for (i, &term_idx) in order.iter().enumerate() {
        let cur = arena.get(term_idx as usize);
        if (i as u64).is_multiple_of(BLOCK_SIZE) {
            // First string of a block: stored whole, so the block can be found
            // without decoding the one before it.
            blocks.push(text.len() as u64);
            text.extend_from_slice(cur);
        } else {
            let delta = common_prefix(prev, cur);
            vbyte(&mut text, delta as u64);
            text.extend_from_slice(&cur[delta..]);
        }
        text.push(0);
        prev = cur;
    }
    // The sentinel: the loader takes `nblocks = numentries - 1`, and uses the
    // last entry as the end of the final block.
    blocks.push(text.len() as u64);

    let mut head = Vec::with_capacity(32);
    head.push(TYPE_PFC);
    vbyte(&mut head, order.len() as u64);
    vbyte(&mut head, text.len() as u64);
    vbyte(&mut head, BLOCK_SIZE);
    out.extend_from_slice(&head);
    out.push(crc8(&head));

    // The block offsets are sized to the text length, which is what
    // `reduceBits()` leaves behind on the hdt-cpp side.
    LogSequence::with_bits(&blocks, bits_for(text.len() as u64)).write(out);

    out.extend_from_slice(&text);
    out.extend_from_slice(&crc32c(&text).to_le_bytes());
}

/// Length of the longest common prefix of two byte strings.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arena_of(terms: &[&str]) -> TermArena {
        let mut a = TermArena::default();
        for t in terms {
            a.push_term(t);
        }
        a
    }

    #[test]
    fn iris_lose_their_brackets_and_literals_keep_their_quotes() {
        let a = arena_of(&[
            "<http://ex/s>",
            "\"Alice\"",
            "\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "\"bonjour\"@fr",
            "_:b0",
        ]);
        assert_eq!(a.get(0), b"http://ex/s");
        assert_eq!(a.get(1), b"\"Alice\"");
        assert_eq!(
            a.get(2),
            b"\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>".as_slice(),
            "only the OUTER brackets of a bare IRI come off"
        );
        assert_eq!(a.get(3), b"\"bonjour\"@fr");
        assert_eq!(a.get(4), b"_:b0");
    }

    /// The collation difference that makes the whole remap necessary, pinned as
    /// a test so the reasoning survives.
    /// The escape handling, pinned against what `rdf2hdt` actually stores.
    ///
    /// Established empirically, by round-tripping this exact set through
    /// hdt-cpp: it stores a real tab for \t, a bare quote for \", one
    /// backslash for \\, and the character itself for \uXXXX.
    #[test]
    fn literals_are_stored_unescaped_with_their_structure_intact() {
        let cases: &[(&str, &[u8])] = &[
            (r#""line\nbreak""#, b"\"line\nbreak\""),
            (r#""tab\there""#, b"\"tab\there\""),
            (r#""quote\"inside""#, b"\"quote\"inside\""),
            (r#""back\\slash""#, b"\"back\\slash\""),
            (r#""plain"@en"#, b"\"plain\"@en"),
        ];
        for (token, want) in cases {
            assert_eq!(&hdt_form(token)[..], *want, "hdt_form({token})");
        }
    }

    #[test]
    fn a_unicode_escape_becomes_the_character() {
        assert_eq!(
            hdt_form(r#""uni\u00C1code""#),
            "\"uni\u{c1}code\"".as_bytes()
        );
        assert_eq!(hdt_form(r#""\U0001F600""#), "\"\u{1F600}\"".as_bytes());
        // In an IRI too, where UCHAR is the only escape the grammar allows.
        assert_eq!(
            hdt_form(r"<http://ex/caf\u00E9>"),
            "http://ex/caf\u{e9}".as_bytes()
        );
    }

    #[test]
    fn a_datatype_suffix_is_structural_and_survives() {
        assert_eq!(
            hdt_form(r#""30"^^<http://www.w3.org/2001/XMLSchema#integer>"#),
            b"\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>".as_slice(),
            "the datatype keeps its angle brackets, unlike a bare IRI term"
        );
    }

    #[test]
    fn an_unrecognized_escape_keeps_its_backslash() {
        // Better to carry the byte through than to drop it silently.
        assert_eq!(hdt_form(r#""a\qb""#), b"\"a\\qb\"".as_slice());
    }

    #[test]
    fn blank_nodes_pass_through_untouched() {
        assert_eq!(hdt_form("_:b0"), b"_:b0");
    }

    #[test]
    fn stripping_brackets_changes_the_order() {
        // rete stores these as `<http://ex/a>` and `<http://ex/a/b>`. Comparing
        // the stored tokens puts `/a/b` first, because '/' (0x2F) < '>' (0x3E).
        let rete_order = {
            let mut v = vec!["<http://ex/a>", "<http://ex/a/b>"];
            v.sort_unstable();
            v
        };
        assert_eq!(rete_order, vec!["<http://ex/a/b>", "<http://ex/a>"]);

        // HDT stores them bare, and then `a` sorts first.
        let a = arena_of(&["<http://ex/a>", "<http://ex/a/b>"]);
        let order = a.sorted_indices();
        assert_eq!(a.get(order[0] as usize), b"http://ex/a");
        assert_eq!(a.get(order[1] as usize), b"http://ex/a/b");
    }

    #[test]
    fn blank_nodes_and_iris_swap_blocks() {
        // In rete's stored form `_:b` (0x5F) sorts after `<http://…>` (0x3C).
        // Bare, `_:b` sorts BEFORE `http://…` (0x68).
        let a = arena_of(&["<http://ex/s>", "_:b0"]);
        let order = a.sorted_indices();
        assert_eq!(a.get(order[0] as usize), b"_:b0");
        assert_eq!(a.get(order[1] as usize), b"http://ex/s");
    }

    #[test]
    fn a_literal_sorts_before_everything() {
        // '"' is 0x22, below both '_' and any scheme letter — same in both
        // formats, so this part of the order is preserved.
        let a = arena_of(&["<http://ex/s>", "_:b0", "\"Alice\""]);
        let order = a.sorted_indices();
        assert_eq!(a.get(order[0] as usize), b"\"Alice\"");
    }

    /// Decode a section back, the way `hdt-cpp` does, and check it round-trips.
    fn decode_pfc(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut p = 0usize;
        assert_eq!(bytes[p], TYPE_PFC);
        p += 1;
        let read_vbyte = |p: &mut usize| -> u64 {
            let mut v = 0u64;
            let mut shift = 0;
            loop {
                let b = bytes[*p];
                *p += 1;
                v |= ((b & 127) as u64) << shift;
                if b & 0x80 != 0 {
                    return v;
                }
                shift += 7;
            }
        };
        let numstrings = read_vbyte(&mut p);
        let textlen = read_vbyte(&mut p) as usize;
        let blocksize = read_vbyte(&mut p);
        p += 1; // crc8
                // blocks LogSequence2
        assert_eq!(bytes[p], 0x01);
        let numbits = bytes[p + 1] as u64;
        p += 2;
        let numentries = read_vbyte(&mut p);
        p += 1; // crc8
        let databytes = (numbits * numentries).div_ceil(8) as usize;
        p += databytes + 4; // data + crc32
        let text = &bytes[p..p + textlen];

        let mut out = Vec::new();
        let mut prev: Vec<u8> = Vec::new();
        let mut t = 0usize;
        for i in 0..numstrings {
            let cur = if i % blocksize == 0 {
                let end = t + text[t..].iter().position(|&b| b == 0).unwrap();
                let s = text[t..end].to_vec();
                t = end + 1;
                s
            } else {
                let mut delta = 0u64;
                let mut shift = 0;
                loop {
                    let b = text[t];
                    t += 1;
                    delta |= ((b & 127) as u64) << shift;
                    if b & 0x80 != 0 {
                        break;
                    }
                    shift += 7;
                }
                let end = t + text[t..].iter().position(|&b| b == 0).unwrap();
                let mut s = prev[..delta as usize].to_vec();
                s.extend_from_slice(&text[t..end]);
                t = end + 1;
                s
            };
            prev = cur.clone();
            out.push(cur);
        }
        out
    }

    #[test]
    fn a_front_coded_section_decodes_back_to_its_terms() {
        // More than one block, with long shared prefixes — the case front coding
        // exists for, and the case where an off-by-one in the delta shows up.
        let terms: Vec<String> = (0..40)
            .map(|i| format!("<http://dblp.example/resource/authors/Person{i:04}>"))
            .collect();
        let refs: Vec<&str> = terms.iter().map(String::as_str).collect();
        let arena = arena_of(&refs);
        let order = arena.sorted_indices();

        let mut out = Vec::new();
        write_pfc_section(&mut out, &arena, &order);
        let got = decode_pfc(&out);

        assert_eq!(got.len(), 40);
        for (i, &oi) in order.iter().enumerate() {
            assert_eq!(got[i], arena.get(oi as usize), "term {i}");
        }
    }

    #[test]
    fn a_section_with_no_shared_prefixes_still_round_trips() {
        let arena = arena_of(&["\"zebra\"", "_:x", "<http://a/1>", "\"apple\""]);
        let order = arena.sorted_indices();
        let mut out = Vec::new();
        write_pfc_section(&mut out, &arena, &order);
        let got = decode_pfc(&out);
        assert_eq!(
            got,
            vec![
                b"\"apple\"".to_vec(),
                b"\"zebra\"".to_vec(),
                b"_:x".to_vec(),
                b"http://a/1".to_vec(),
            ]
        );
    }

    #[test]
    fn an_empty_section_is_well_formed() {
        let arena = TermArena::default();
        let mut out = Vec::new();
        write_pfc_section(&mut out, &arena, &[]);
        // type + vbyte(0) + vbyte(0) + vbyte(16) + crc8, then a blocks sequence
        // holding just the sentinel, then no text and its crc32.
        assert_eq!(out[0], TYPE_PFC);
        assert_eq!(decode_pfc(&out).len(), 0);
    }

    #[test]
    fn the_block_sequence_has_one_entry_per_block_plus_a_sentinel() {
        // 40 terms at blocksize 16 -> 3 blocks -> 4 entries.
        let terms: Vec<String> = (0..40).map(|i| format!("<http://ex/{i:04}>")).collect();
        let refs: Vec<&str> = terms.iter().map(String::as_str).collect();
        let arena = arena_of(&refs);
        let order = arena.sorted_indices();
        let mut out = Vec::new();
        write_pfc_section(&mut out, &arena, &order);

        // Walk to the blocks LogSequence2 and read its entry count.
        let mut p = 1usize;
        let read_vbyte = |p: &mut usize| -> u64 {
            let mut v = 0u64;
            let mut shift = 0;
            loop {
                let b = out[*p];
                *p += 1;
                v |= ((b & 127) as u64) << shift;
                if b & 0x80 != 0 {
                    return v;
                }
                shift += 7;
            }
        };
        read_vbyte(&mut p); // numstrings
        read_vbyte(&mut p); // bytes
        read_vbyte(&mut p); // blocksize
        p += 1; // crc8
        assert_eq!(out[p], 0x01);
        p += 2; // type + numbits
        let numentries = read_vbyte(&mut p);
        assert_eq!(numentries, 4, "3 blocks + 1 sentinel");
    }

    #[test]
    fn common_prefix_is_a_byte_count_not_a_char_count() {
        // A multi-byte character split across the boundary must not produce a
        // delta that slices a code point — the suffix is raw bytes either way,
        // so byte semantics are what keeps the concatenation valid.
        assert_eq!(common_prefix("café".as_bytes(), "cafx".as_bytes()), 3);
        assert_eq!(common_prefix("café".as_bytes(), "café!".as_bytes()), 5);
        assert_eq!(common_prefix(b"", b"abc"), 0);
    }
}
