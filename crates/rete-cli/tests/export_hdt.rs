//! `rete export --format hdt` at the CLI surface.
//!
//! The external validation — that `hdt-cpp` loads the file, recovers every
//! triple and answers the same patterns — lives in
//! `dev/export-formats/hdt-check.sh`, because it needs a third-party container.
//! What is here instead is everything that can be checked without one:
//!
//! * the **refusal gate**, which is the feature's main safety property and must
//!   fire before any work rather than after;
//! * the **graph-selection ladder**, since HDT is triples-only;
//! * and **structural assertions** on the emitted bytes, via a small reader
//!   written from the same specification the writer was — enough to catch a
//!   wrong CRC, a mis-sized field or a dictionary section that does not
//!   round-trip, without pulling in a dependency.
//!
//! The structural reader deliberately re-derives the format rather than calling
//! into the writer's own helpers: a test that shares code with the thing it
//! tests can only prove self-consistency.

mod common;

const TRIPLES: &str = concat!(
    "<http://ex/alice> <http://ex/knows> <http://ex/bob> .\n",
    "<http://ex/alice> <http://ex/name> \"Alice\" .\n",
    "<http://ex/bob> <http://ex/name> \"Bob\" .\n",
    "<http://ex/bob> <http://ex/age> \"30\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
    "<http://ex/carol> <http://ex/knows> <http://ex/alice> .\n",
);

/// Two named graphs and nothing in the default graph.
const NAMED_ONLY: &str = concat!(
    "<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/g1> .\n",
    "<http://ex/s> <http://ex/p> <http://ex/o2> <http://ex/g2> .\n",
);

fn build(nq: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    let out = dir.path().join("out.rete");
    std::fs::write(&src, nq).unwrap();
    common::build(&src, &out, &["--no-pyramid"]);
    (dir, out)
}

fn run(file: &std::path::Path, args: &[&str]) -> std::process::Output {
    common::rete()
        .arg("export")
        .arg(file)
        .args(args)
        .output()
        .unwrap()
}

fn export_hdt(file: &std::path::Path, args: &[&str]) -> Vec<u8> {
    let mut a = vec!["--format", "hdt"];
    a.extend_from_slice(args);
    let out = run(file, &a);
    assert!(
        out.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

// ---------------------------------------------------------------------------
// A minimal structural reader, from the format spec
// ---------------------------------------------------------------------------

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }

    /// VByte: 7 bits per byte, little-endian, high bit marks the LAST byte.
    fn vbyte(&mut self) -> u64 {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let byte = self.b[self.p];
            self.p += 1;
            v |= ((byte & 127) as u64) << shift;
            if byte & 0x80 != 0 {
                return v;
            }
            shift += 7;
        }
    }

    /// One control-information block: cookie, type, format, properties, crc16.
    fn control_info(&mut self) -> (u8, String, String) {
        assert_eq!(&self.b[self.p..self.p + 4], b"$HDT", "missing cookie");
        self.p += 4;
        let kind = self.b[self.p];
        self.p += 1;
        let fmt = self.cstr();
        let props = self.cstr();
        self.p += 2; // crc16
        (kind, fmt, props)
    }

    fn cstr(&mut self) -> String {
        let end = self.p + self.b[self.p..].iter().position(|&c| c == 0).unwrap();
        let s = String::from_utf8_lossy(&self.b[self.p..end]).into_owned();
        self.p = end + 1;
        s
    }

    /// A LogSequence2; returns (numbits, numentries) and skips the payload.
    fn log_sequence(&mut self) -> (u8, u64) {
        assert_eq!(self.b[self.p], 0x01, "LogSequence2 type byte");
        self.p += 1;
        let numbits = self.b[self.p];
        self.p += 1;
        let numentries = self.vbyte();
        self.p += 1; // crc8
        let numbytes = (numbits as u64 * numentries).div_ceil(8) as usize;
        self.p += numbytes + 4; // data + crc32
        (numbits, numentries)
    }

    /// A BitSequence375; returns numbits and skips the payload.
    fn bit_sequence(&mut self) -> u64 {
        assert_eq!(self.b[self.p], 0x01, "BitSequence375 type byte");
        self.p += 1;
        let numbits = self.vbyte();
        self.p += 1; // crc8
                     // numBytes(0) == 1 — an empty bitmap still writes one byte.
        let numbytes = if numbits == 0 {
            1
        } else {
            ((numbits - 1) >> 3) as usize + 1
        };
        self.p += numbytes + 4;
        numbits
    }

    /// A front-coded section; returns its decoded strings.
    fn pfc_section(&mut self) -> Vec<Vec<u8>> {
        assert_eq!(self.b[self.p], 0x02, "CSD_PFC type byte");
        self.p += 1;
        let numstrings = self.vbyte();
        let textlen = self.vbyte() as usize;
        let blocksize = self.vbyte();
        self.p += 1; // crc8
        let (_, block_entries) = self.log_sequence();
        assert_eq!(
            block_entries,
            numstrings.div_ceil(blocksize.max(1)) + 1,
            "blocks must be one per block plus a sentinel"
        );
        let text = &self.b[self.p..self.p + textlen];
        self.p += textlen + 4; // text + crc32

        let mut out = Vec::new();
        let mut prev: Vec<u8> = Vec::new();
        let mut t = 0usize;
        for i in 0..numstrings {
            let cur = if i % blocksize == 0 {
                let end = t + text[t..].iter().position(|&c| c == 0).unwrap();
                let s = text[t..end].to_vec();
                t = end + 1;
                s
            } else {
                let mut delta = 0u64;
                let mut shift = 0;
                loop {
                    let byte = text[t];
                    t += 1;
                    delta |= ((byte & 127) as u64) << shift;
                    if byte & 0x80 != 0 {
                        break;
                    }
                    shift += 7;
                }
                let end = t + text[t..].iter().position(|&c| c == 0).unwrap();
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
}

/// Everything the structural reader recovers from a file.
struct Parsed {
    shared: Vec<Vec<u8>>,
    subjects: Vec<Vec<u8>>,
    predicates: Vec<Vec<u8>>,
    objects: Vec<Vec<u8>>,
    bitmap_y: u64,
    bitmap_z: u64,
    array_y: u64,
    array_z: u64,
    consumed: usize,
    total: usize,
}

fn parse(bytes: &[u8]) -> Parsed {
    let mut r = Reader::new(bytes);
    let (k, fmt, _) = r.control_info();
    assert_eq!(k, 1, "global control information");
    assert_eq!(fmt, "<http://purl.org/HDT/hdt#HDTv1>");

    let (k, fmt, props) = r.control_info();
    assert_eq!(k, 2, "header control information");
    assert_eq!(fmt, "ntriples");
    let len: usize = props
        .trim_end_matches(';')
        .strip_prefix("length=")
        .expect("header declares its length")
        .parse()
        .unwrap();
    r.p += len;

    let (k, fmt, _) = r.control_info();
    assert_eq!(k, 3, "dictionary control information");
    assert_eq!(fmt, "<http://purl.org/HDT/hdt#dictionaryFour>");
    // FourSectionDictionary::save order: shared, subjects, PREDICATES, objects.
    let shared = r.pfc_section();
    let subjects = r.pfc_section();
    let predicates = r.pfc_section();
    let objects = r.pfc_section();

    let (k, fmt, props) = r.control_info();
    assert_eq!(k, 4, "triples control information");
    assert_eq!(fmt, "<http://purl.org/HDT/hdt#triplesBitmap>");
    assert_eq!(props, "order=1;", "SPO, and no numTriples");

    // save order: bitmapY, bitmapZ, arrayY, arrayZ.
    let bitmap_y = r.bit_sequence();
    let bitmap_z = r.bit_sequence();
    let (_, array_y) = r.log_sequence();
    let (_, array_z) = r.log_sequence();

    Parsed {
        shared,
        subjects,
        predicates,
        objects,
        bitmap_y,
        bitmap_z,
        array_y,
        array_z,
        consumed: r.p,
        total: bytes.len(),
    }
}

// --- structure --------------------------------------------------------------

#[test]
fn the_file_parses_end_to_end_with_nothing_left_over() {
    let (_dir, file) = build(TRIPLES);
    let p = parse(&export_hdt(&file, &[]));
    assert_eq!(
        p.consumed, p.total,
        "the parse must consume exactly the file: {} of {}",
        p.consumed, p.total
    );
}

#[test]
fn the_bitmaps_match_their_arrays() {
    let (_dir, file) = build(TRIPLES);
    let p = parse(&export_hdt(&file, &[]));
    assert_eq!(p.bitmap_y, p.array_y, "|bitmapY| == |arrayY|");
    assert_eq!(p.bitmap_z, p.array_z, "|bitmapZ| == |arrayZ|");
    assert_eq!(p.array_z, 5, "arrayZ holds one entry per triple");
}

#[test]
fn the_dictionary_holds_the_right_terms_in_the_right_sections() {
    let (_dir, file) = build(TRIPLES);
    let p = parse(&export_hdt(&file, &[]));

    let s = |v: &Vec<Vec<u8>>| -> Vec<String> {
        v.iter()
            .map(|t| String::from_utf8_lossy(t).into_owned())
            .collect()
    };
    // alice and bob are both subjects and objects; carol is subject-only.
    assert_eq!(s(&p.shared), vec!["http://ex/alice", "http://ex/bob"]);
    assert_eq!(s(&p.subjects), vec!["http://ex/carol"]);
    assert_eq!(
        s(&p.predicates),
        vec!["http://ex/age", "http://ex/knows", "http://ex/name"]
    );
    // Object-only: the two literals. Note the IRIs lost their brackets and the
    // literals kept their quotes.
    assert_eq!(
        s(&p.objects),
        vec![
            "\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "\"Alice\"",
            "\"Bob\"",
        ]
    );
}

#[test]
fn every_section_is_sorted_by_raw_bytes() {
    // hdt-cpp binary-searches each section with strcmp, so an unsorted section
    // produces wrong answers rather than an error.
    let (_dir, file) = build(TRIPLES);
    let p = parse(&export_hdt(&file, &[]));
    for (name, sec) in [
        ("shared", &p.shared),
        ("subjects", &p.subjects),
        ("predicates", &p.predicates),
        ("objects", &p.objects),
    ] {
        let mut sorted = sec.clone();
        sorted.sort();
        assert_eq!(*sec, sorted, "{name} is not in byte order");
    }
}

#[test]
fn literals_are_stored_with_their_escapes_resolved() {
    // hdt-cpp stores the characters and re-escapes on output. Storing rete's
    // escaped form instead is a silent data corruption, so it is pinned here as
    // well as in the unit tests.
    let (_dir, file) = build(concat!(
        "<http://ex/a> <http://ex/p> \"line\\nbreak\" .\n",
        "<http://ex/b> <http://ex/p> \"tab\\there\" .\n",
        "<http://ex/c> <http://ex/p> \"quote\\\"inside\" .\n",
    ));
    let p = parse(&export_hdt(&file, &[]));
    let objs: Vec<String> = p
        .objects
        .iter()
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    assert!(objs.contains(&"\"line\nbreak\"".to_string()), "{objs:?}");
    assert!(objs.contains(&"\"tab\there\"".to_string()), "{objs:?}");
    assert!(objs.contains(&"\"quote\"inside\"".to_string()), "{objs:?}");
}

// --- the gate ---------------------------------------------------------------

#[test]
fn a_budget_too_small_refuses_before_doing_any_work() {
    // Big enough that the estimate clears 1 MiB: the gate is sized per distinct
    // term, so the fixture needs terms rather than merely triples.
    let mut nq = String::new();
    for i in 0..20_000 {
        nq.push_str(&format!(
            "<http://ex/subject/{i}> <http://ex/p> <http://ex/object/{i}> .
"
        ));
    }
    let (_dir, file) = build(&nq);
    let out = run(&file, &["--format", "hdt", "--memory-budget-mb", "1"]);
    assert!(!out.status.success(), "must refuse");
    assert!(out.stdout.is_empty(), "and write nothing");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("memory"), "{err}");
    assert!(
        err.contains("--format trig"),
        "the refusal must point at the alternative with no ceiling: {err}"
    );
}

#[test]
fn an_unlimited_budget_is_accepted() {
    let (_dir, file) = build(TRIPLES);
    let bytes = export_hdt(&file, &["--memory-budget-mb", "0"]);
    assert_eq!(&bytes[..4], b"$HDT");
}

#[test]
fn compression_is_refused_with_a_reason() {
    let (_dir, file) = build(TRIPLES);
    for codec in ["zstd", "gzip"] {
        let out = run(&file, &["--format", "hdt", "--compress", codec]);
        assert!(!out.status.success(), "{codec} must be refused");
        assert!(out.stdout.is_empty());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("queried in place"),
            "the refusal must say WHY, not just no: {err}"
        );
    }
}

// --- the graph ladder -------------------------------------------------------

#[test]
fn several_named_graphs_and_an_empty_default_is_refused() {
    let (_dir, file) = build(NAMED_ONLY);
    let out = run(&file, &["--format", "hdt"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    for want in [
        "<http://ex/g1>",
        "<http://ex/g2>",
        "--graph",
        "--format trig",
    ] {
        assert!(err.contains(want), "must mention {want}: {err}");
    }
}

#[test]
fn a_named_graph_can_be_selected() {
    let (_dir, file) = build(NAMED_ONLY);
    let p = parse(&export_hdt(&file, &["--graph", "http://ex/g1"]));
    assert_eq!(p.array_z, 1, "graph g1 has exactly one triple");
    let objs: Vec<String> = p
        .objects
        .iter()
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    assert_eq!(objs, vec!["http://ex/o"], "only g1's object, not g2's");
}

#[test]
fn the_chosen_graph_is_reported_on_stderr() {
    let (_dir, file) = build(TRIPLES);
    let out = run(&file, &["--format", "hdt"]);
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("HDT export of the default graph"), "{err}");
    assert!(
        err.contains("cannot be written in one pass"),
        "the memory note explains why there is a ceiling at all: {err}"
    );
}

#[test]
fn stdout_is_the_file_and_diagnostics_stay_on_stderr() {
    let (_dir, file) = build(TRIPLES);
    let out = run(&file, &["--format", "hdt"]);
    assert_eq!(&out.stdout[..4], b"$HDT", "the cookie starts the stream");
    assert!(!out.stderr.is_empty(), "notes went to stderr");
}
