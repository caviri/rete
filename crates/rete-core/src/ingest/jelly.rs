//! Jelly RDF reader: a streaming decoder for `.jelly` files.
//!
//! Jelly (<https://jelly-rdf.github.io/>) is a Protocol Buffers RDF format: a
//! file is a sequence of varint-length-delimited `RdfStreamFrame` messages.
//! This module reads both layouts of the format without `protoc`, `prost` or
//! any generated code, because only a handful of messages matter and the
//! fields are all varints, strings and nested messages:
//!
//! * the **row layout** of Jelly-RDF 1.0 and 1.1 (version tag 1 and 2): a frame
//!   is a list of rows (options, triples, quads, graph boundaries, lookup
//!   entries, namespace declarations), with repeated terms and a stream-wide
//!   IRI sequence;
//! * the **column layout** of Jelly-RDF 1.2 (version tag 3): a frame carries
//!   one column batch, with packed lookup entries, run-length column layouts
//!   and RDF 1.2 triple terms.
//!
//! Frames are read one at a time, so the file is never resident; memory is
//! the largest frame plus the three lookup tables. Every statement leaves as
//! the same canonical term tokens the N-Quads reader produces (`<iri>`,
//! `_:label`, `"lex"`, `"lex"@tag`, `"lex"^^<dt>`, `<<s p o>>`).
//!
//! What it refuses, by name: version tags above 3 or 0, physical type
//! `UNSPECIFIED`, `GRAPHS` in the column layout, streams of RDF Messages
//! (`stream_type` 1, optional in the specification), generalized statements
//! (a literal as subject, predicate or graph name, or a blank-node predicate),
//! lookup tables above [`MAX_TABLE`] entries, frames above [`MAX_FRAME`] bytes,
//! and every violation the specification says a consumer must reject. A file
//! that is one bare, non-delimited frame is not recognised.

use super::{audit_quad, IngestError, IriAudit, RawQuad};
use std::io::{BufRead, Read};

/// Largest frame accepted, in bytes. The specification recommends frames under
/// 1 MB; this leaves room for producers that ignore it.
pub const MAX_FRAME: u64 = 512 << 20;

/// Largest lookup table (names, prefixes or datatypes) accepted, in entries.
/// A stream asking for more is refused up front, as the specification's
/// security section asks (the conformance suite requires 10,000,000 to fail).
pub const MAX_TABLE: u32 = 1 << 20;

/// Deepest quoted-triple / triple-term nesting accepted.
const MAX_DEPTH: usize = 100;

/// Largest `row_count` of a column batch (the specification's 2^27 - 1).
const MAX_ROWS: u64 = (1 << 27) - 1;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const RDF_DIR_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString";

type Res<T> = Result<T, IngestError>;

fn bad<T>(msg: impl Into<String>) -> Res<T> {
    Err(IngestError::Jelly(msg.into()))
}

// ---------------------------------------------------------------------------
// Protocol Buffers wire format
// ---------------------------------------------------------------------------

/// One decoded field value.
#[derive(Clone, Copy)]
enum Val<'a> {
    Num(u64),
    Bytes(&'a [u8]),
    /// A fixed-width value; nothing in Jelly uses one, so it is skipped.
    Other,
}

/// A cursor over one encoded message.
struct Msg<'a> {
    b: &'a [u8],
}

impl<'a> Msg<'a> {
    fn new(b: &'a [u8]) -> Self {
        Msg { b }
    }

    fn varint(&mut self) -> Res<u64> {
        let mut v = 0u64;
        for shift in (0..70).step_by(7) {
            let Some((&byte, rest)) = self.b.split_first() else {
                return bad("truncated varint");
            };
            self.b = rest;
            if shift == 63 && byte > 1 {
                return bad("varint overflows 64 bits");
            }
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        bad("varint longer than 10 bytes")
    }

    fn take(&mut self, n: u64) -> Res<&'a [u8]> {
        if n > self.b.len() as u64 {
            return bad("length-delimited field runs past the end of its message");
        }
        let (head, tail) = self.b.split_at(n as usize);
        self.b = tail;
        Ok(head)
    }

    /// The next `(field number, value)`, or `None` at the end of the message.
    fn next(&mut self) -> Res<Option<(u32, Val<'a>)>> {
        if self.b.is_empty() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = key >> 3;
        if field == 0 || field > u64::from(u32::MAX >> 3) {
            return bad("invalid protobuf field number");
        }
        let val = match key & 7 {
            0 => Val::Num(self.varint()?),
            1 => {
                self.take(8)?;
                Val::Other
            }
            2 => {
                let n = self.varint()?;
                Val::Bytes(self.take(n)?)
            }
            5 => {
                self.take(4)?;
                Val::Other
            }
            w => return bad(format!("unsupported protobuf wire type {w}")),
        };
        Ok(Some((field as u32, val)))
    }
}

fn utf8(b: &[u8]) -> Res<&str> {
    std::str::from_utf8(b).or_else(|_| bad("string is not valid UTF-8"))
}

fn num(v: Val<'_>, what: &str) -> Res<u64> {
    match v {
        Val::Num(n) => Ok(n),
        _ => bad(format!("{what}: expected a varint")),
    }
}

fn num32(v: Val<'_>, what: &str) -> Res<u32> {
    u32::try_from(num(v, what)?).or_else(|_| bad(format!("{what}: value does not fit 32 bits")))
}

fn bytes<'a>(v: Val<'a>, what: &str) -> Res<&'a [u8]> {
    match v {
        Val::Bytes(b) => Ok(b),
        _ => bad(format!("{what}: expected a length-delimited field")),
    }
}

fn string<'a>(v: Val<'a>, what: &str) -> Res<&'a str> {
    utf8(bytes(v, what)?)
}

/// Append a `repeated uint32` field, packed or not.
fn push_u32s(out: &mut Vec<u32>, v: Val<'_>, what: &str) -> Res<()> {
    match v {
        Val::Num(n) => out.push(
            u32::try_from(n).or_else(|_| bad(format!("{what}: value does not fit 32 bits")))?,
        ),
        Val::Bytes(b) => {
            let mut m = Msg::new(b);
            while !m.b.is_empty() {
                let n = m.varint()?;
                out.push(
                    u32::try_from(n)
                        .or_else(|_| bad(format!("{what}: value does not fit 32 bits")))?,
                );
            }
        }
        Val::Other => return bad(format!("{what}: unexpected fixed-width value")),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Term tokens
// ---------------------------------------------------------------------------

/// The quoted string of an N-Triples literal, escaped the way `oxrdf` prints it
/// so a Jelly literal and the same literal read from `.nq` are one dictionary
/// term.
fn push_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\0'..='\u{1F}' | '\u{7F}' => {
                out.push_str(&format!("\\u{:04X}", u32::from(c)));
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

fn iri_token(prefix: &str, name: &str) -> String {
    let mut t = String::with_capacity(prefix.len() + name.len() + 2);
    t.push('<');
    t.push_str(prefix);
    t.push_str(name);
    t.push('>');
    t
}

/// A blank-node token. `scope` is 0 when labels are scoped to the whole stream;
/// otherwise the label gets the scope as a `-N` suffix. N never contains a dash,
/// so the suffix cannot be confused with a longer label.
fn bnode_token(label: &str, scope: u64) -> String {
    if scope == 0 {
        format!("_:{label}")
    } else {
        format!("_:{label}-{scope}")
    }
}

fn literal_token(lex: &str, lang: Option<&str>, dir: u32, datatype: Option<&str>) -> Res<String> {
    let mut t = String::with_capacity(lex.len() + 8);
    push_quoted(&mut t, lex);
    if let Some(tag) = lang {
        if tag.is_empty() {
            return bad("empty language tag");
        }
        t.push('@');
        // Language tags are case-insensitive; the Turtle reader (oxrdf) stores them
        // lower-case, so do the same rather than make `@nl-BE` and `@nl-be` two terms.
        t.push_str(&tag.to_ascii_lowercase());
        match dir {
            0 => {}
            1 => t.push_str("--ltr"),
            2 => t.push_str("--rtl"),
            d => return bad(format!("unknown base direction {d}")),
        }
    } else if let Some(dt) = datatype {
        if dt == RDF_LANG_STRING || dt == RDF_DIR_LANG_STRING {
            return bad("a datatype literal must not use rdf:langString or rdf:dirLangString");
        }
        if dt != XSD_STRING {
            t.push_str("^^<");
            t.push_str(dt);
            t.push('>');
        }
    }
    Ok(t)
}

// ---------------------------------------------------------------------------
// Stream options and lookup tables
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Options {
    physical: u32,
    generalized: bool,
    rdf_star: bool,
    stream_type: u32,
    rdf_version: u32,
    max_name: u32,
    max_prefix: u32,
    max_datatype: u32,
    logical: u32,
    version: u32,
}

impl Options {
    fn parse(b: &[u8]) -> Res<Options> {
        let mut o = Options::default();
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => {
                    string(v, "stream_name")?;
                }
                2 => o.physical = num32(v, "physical_type")?,
                3 => o.generalized = num(v, "generalized_statements")? != 0,
                4 => o.rdf_star = num(v, "rdf_star")? != 0,
                5 => o.stream_type = num32(v, "stream_type")?,
                6 => o.rdf_version = num32(v, "rdf_version")?,
                9 => o.max_name = num32(v, "max_name_table_size")?,
                10 => o.max_prefix = num32(v, "max_prefix_table_size")?,
                11 => o.max_datatype = num32(v, "max_datatype_table_size")?,
                14 => o.logical = num32(v, "logical_type")?,
                15 => o.version = num32(v, "version")?,
                _ => {}
            }
        }
        o.validate()?;
        Ok(o)
    }

    fn columns(&self) -> bool {
        self.version == 3
    }

    fn validate(&self) -> Res<()> {
        match self.version {
            0 => return bad("stream options have version 0; the version tag is required"),
            1..=3 => {}
            v => {
                return bad(format!(
                    "Jelly version tag {v} is newer than this reader supports (1, 2 and 3)"
                ))
            }
        }
        let col = self.columns();
        match self.physical {
            1 | 2 => {}
            3 if !col => {}
            3 => {
                return bad("physical type GRAPHS does not exist in the column layout (version 3)")
            }
            0 => return bad("physical stream type is UNSPECIFIED"),
            p => return bad(format!("unknown physical stream type {p}")),
        }
        let min_name = if col { 128 } else { 8 };
        if self.max_name < min_name {
            return bad(format!(
                "max_name_table_size is {}, the minimum is {min_name}",
                self.max_name
            ));
        }
        for (what, v) in [
            ("max_name_table_size", self.max_name),
            ("max_prefix_table_size", self.max_prefix),
            ("max_datatype_table_size", self.max_datatype),
        ] {
            if v > MAX_TABLE {
                return bad(format!(
                    "{what} is {v}, more than the {MAX_TABLE} entries this reader allocates"
                ));
            }
        }
        if col {
            if self.generalized || self.rdf_star || self.logical != 0 {
                return bad(
                    "generalized_statements, rdf_star and logical_type must not be set in a \
                     column-layout stream",
                );
            }
            match self.stream_type {
                0 => {}
                1 => {
                    return bad(
                        "streams of RDF Messages (stream_type MESSAGES) are not supported; \
                         this reader reads one graph or dataset",
                    )
                }
                t => return bad(format!("unknown stream_type {t}")),
            }
            if self.rdf_version > 3 {
                return bad(format!("unknown rdf_version {}", self.rdf_version));
            }
        }
        Ok(())
    }

    /// Whether the two options may both occur in one stream (row layout: they
    /// must be identical).
    fn same_as(&self, other: &Options) -> bool {
        self == other
    }
}

/// One lookup table: 1-based, filled by entries, bounded by the stream options.
struct Lookup {
    what: &'static str,
    max: u32,
    slots: Vec<Option<Box<str>>>,
    last_id: u32,
}

impl Lookup {
    fn new(what: &'static str) -> Self {
        Lookup {
            what,
            max: 0,
            slots: Vec::new(),
            last_id: 0,
        }
    }

    fn reset(&mut self, max: u32) {
        self.max = max;
        self.slots.clear();
        self.last_id = 0;
    }

    /// Set the value of `id` (0 = previous + 1).
    fn set(&mut self, id: u32, value: &str) -> Res<()> {
        let id = if id == 0 {
            self.last_id.checked_add(1)
        } else {
            Some(id)
        };
        let Some(id) = id.filter(|&i| i >= 1 && i <= self.max) else {
            return bad(format!(
                "{} lookup entry outside the table (size {})",
                self.what, self.max
            ));
        };
        self.last_id = id;
        let i = id as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, None);
        }
        self.slots[i] = Some(value.into());
        Ok(())
    }

    fn get(&self, id: u32) -> Res<&str> {
        if id == 0 || id > self.max {
            return bad(format!(
                "{} id {id} is outside the table (size {})",
                self.what, self.max
            ));
        }
        match self.slots.get(id as usize) {
            Some(Some(s)) => Ok(s),
            _ => bad(format!("{} id {id} was never defined", self.what)),
        }
    }
}

/// The three tables plus the options they were sized from.
struct Tables {
    names: Lookup,
    prefixes: Lookup,
    datatypes: Lookup,
}

impl Tables {
    fn new() -> Self {
        Tables {
            names: Lookup::new("name"),
            prefixes: Lookup::new("prefix"),
            datatypes: Lookup::new("datatype"),
        }
    }

    fn reset(&mut self, o: &Options) {
        self.names.reset(o.max_name);
        self.prefixes.reset(o.max_prefix);
        self.datatypes.reset(o.max_datatype);
    }
}

/// The `prefix_id` / `name_id` inference state of one IRI sequence.
#[derive(Default, Clone, Copy)]
struct IriSeq {
    prefix: u32,
    name: u32,
}

impl IriSeq {
    /// Resolve a `(prefix_id, name_id)` pair against the tables, updating the
    /// sequence: 0 means "same prefix" and "previous name + 1".
    fn resolve(&mut self, t: &Tables, prefix_id: u32, name_id: u32) -> Res<String> {
        let name = if name_id == 0 {
            self.name.checked_add(1).unwrap_or(0)
        } else {
            name_id
        };
        self.name = name;
        let prefix = if prefix_id == 0 {
            self.prefix
        } else {
            prefix_id
        };
        self.prefix = prefix;
        let pre = if prefix == 0 {
            ""
        } else {
            t.prefixes.get(prefix)?
        };
        Ok(iri_token(pre, t.names.get(name)?))
    }

    /// An `RdfIri` message.
    fn message(&mut self, t: &Tables, b: &[u8]) -> Res<String> {
        let (mut p, mut n) = (0, 0);
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => p = num32(v, "prefix_id")?,
                2 => n = num32(v, "name_id")?,
                _ => {}
            }
        }
        self.resolve(t, p, n)
    }
}

// ---------------------------------------------------------------------------
// The decoder
// ---------------------------------------------------------------------------

struct Decoder<'a, 'b> {
    opts: Option<Options>,
    tables: Tables,
    /// Row layout: the stream-wide IRI sequence.
    seq: IriSeq,
    /// Row layout: the last s, p, o and g, for repeated terms.
    prev: [Option<String>; 3],
    prev_g: Option<Option<String>>,
    /// GRAPHS streams: the graph the triples belong to (`Some(None)` = default).
    graph: Option<Option<String>>,
    /// Blank-node scope: 0 for the whole stream, else the frame number.
    scope: u64,
    frame: u64,
    audit: Option<&'a mut IriAudit>,
    f: &'b mut dyn FnMut(RawQuad),
}

/// Which term position a oneof field belongs to, and the kind it carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Iri,
    Bnode,
    Literal,
    Triple,
    DefaultGraph,
}

/// Field number to `(position 0..4, kind)` in `RdfTriple` / `RdfQuad`.
fn slot_of(field: u32) -> Option<(usize, Kind)> {
    Some(match field {
        1 => (0, Kind::Iri),
        2 => (0, Kind::Bnode),
        3 => (0, Kind::Literal),
        4 => (0, Kind::Triple),
        5 => (1, Kind::Iri),
        6 => (1, Kind::Bnode),
        7 => (1, Kind::Literal),
        8 => (1, Kind::Triple),
        9 => (2, Kind::Iri),
        10 => (2, Kind::Bnode),
        11 => (2, Kind::Literal),
        12 => (2, Kind::Triple),
        13 => (3, Kind::Iri),
        14 => (3, Kind::Bnode),
        15 => (3, Kind::DefaultGraph),
        16 => (3, Kind::Literal),
        _ => return None,
    })
}

type Slots<'a> = [Option<(Kind, &'a [u8])>; 4];

/// Split an `RdfTriple` / `RdfQuad` / `RdfGraphStart` message into its term
/// slots, so the terms can be decoded in subject, predicate, object, graph
/// order whatever order they were written in. `graph_start` uses field numbers
/// 1 to 4 for the graph, which `slot_of` would read as a subject.
fn split_slots<'a>(b: &'a [u8], graph_start: bool) -> Res<Slots<'a>> {
    let mut slots: Slots<'a> = [None; 4];
    let mut m = Msg::new(b);
    while let Some((f, v)) = m.next()? {
        let (pos, kind) = if graph_start {
            match f {
                1 => (3, Kind::Iri),
                2 => (3, Kind::Bnode),
                3 => (3, Kind::DefaultGraph),
                4 => (3, Kind::Literal),
                _ => continue,
            }
        } else {
            match slot_of(f) {
                Some(s) => s,
                None => continue,
            }
        };
        let Val::Bytes(payload) = v else {
            return bad("term field is not length-delimited");
        };
        if let Some((prev, _)) = slots[pos] {
            if prev != kind {
                return bad("two fields of one term oneof are set");
            }
        }
        slots[pos] = Some((kind, payload));
    }
    Ok(slots)
}

impl<'a, 'b> Decoder<'a, 'b> {
    fn emit(&mut self, s: String, p: String, o: String, g: Option<String>) -> Res<()> {
        let frame = self.frame;
        audit_quad(self.audit.as_deref_mut(), &s, &p, &o, g.as_deref(), || {
            format!("jelly frame {frame}")
        })?;
        (self.f)((s, p, o, g));
        Ok(())
    }

    fn opts(&self) -> Res<&Options> {
        match &self.opts {
            Some(o) => Ok(o),
            None => bad("a statement or lookup entry precedes the stream options"),
        }
    }

    // ----- frames -----

    fn frame(&mut self, b: &[u8]) -> Res<()> {
        let mut rows: Vec<&[u8]> = Vec::new();
        let mut columns: Option<Val<'_>> = None;
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => rows.push(bytes(v, "rows")?),
                2 => columns = Some(v),
                _ => {} // metadata (15) and anything newer
            }
        }
        self.update_scope();
        let row_count = rows.len();
        for row in rows {
            self.row(row)?;
        }
        if row_count > 1 && self.opts.as_ref().is_some_and(Options::columns) {
            return bad(
                "a column-layout frame has more than one row; only the stream options belong there",
            );
        }
        if let Some(c) = columns {
            self.batch(c)?;
        }
        Ok(())
    }

    /// Blank-node scope. A stream of graphs or datasets (logical type 3 or 4 and
    /// their subtypes) has one element per frame, so its labels do not carry
    /// over between frames; every other stream is one scope.
    fn update_scope(&mut self) {
        self.scope = match &self.opts {
            Some(o) if !o.columns() && matches!(o.logical % 10, 3 | 4) => self.frame,
            _ => 0,
        };
    }

    fn set_options(&mut self, b: &[u8]) -> Res<()> {
        let new = Options::parse(b)?;
        match self.opts.take() {
            None => {
                self.tables.reset(&new);
            }
            Some(old) => {
                if new.columns() != old.columns() {
                    return bad("the stream options changed the protocol version mid-stream");
                }
                if old.columns() {
                    if new.physical != old.physical || new.stream_type != old.stream_type {
                        return bad("restated stream options changed physical_type or stream_type");
                    }
                    // Restating empties the lookups.
                    self.tables.reset(&new);
                } else if !new.same_as(&old) {
                    return bad("repeated stream options differ from the first ones");
                }
            }
        }
        self.opts = Some(new);
        self.update_scope();
        Ok(())
    }

    // ----- row layout -----

    fn row(&mut self, b: &[u8]) -> Res<()> {
        let mut m = Msg::new(b);
        let mut seen = false;
        while let Some((f, v)) = m.next()? {
            if !matches!(f, 1..=6 | 9..=11) {
                continue;
            }
            if seen {
                return bad("a stream row has more than one field set");
            }
            seen = true;
            let body = bytes(v, "stream row")?;
            if f == 1 {
                self.set_options(body)?;
                continue;
            }
            let o = self.opts()?.clone();
            if o.columns() {
                return bad(
                    "a column-layout (version 3) frame must carry only stream options in rows",
                );
            }
            match f {
                2 => self.triple(body, &o)?,
                3 => self.quad(body, &o)?,
                4 => self.graph_start(body, &o)?,
                5 => self.graph_end(&o)?,
                6 => self.namespace(body)?,
                9 => self.entry(body, 0)?,
                10 => self.entry(body, 1)?,
                11 => self.entry(body, 2)?,
                _ => unreachable!(),
            }
        }
        if !seen {
            return bad("a stream row has no field set");
        }
        Ok(())
    }

    /// An `RdfNameEntry` (0), `RdfPrefixEntry` (1) or `RdfDatatypeEntry` (2).
    fn entry(&mut self, b: &[u8], which: u8) -> Res<()> {
        let (mut id, mut value) = (0u32, "");
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => id = num32(v, "lookup entry id")?,
                2 => value = string(v, "lookup entry value")?,
                _ => {}
            }
        }
        let t = &mut self.tables;
        match which {
            0 => t.names.set(id, value),
            1 => t.prefixes.set(id, value),
            _ => t.datatypes.set(id, value),
        }
    }

    fn namespace(&mut self, b: &[u8]) -> Res<()> {
        let mut value = None;
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => {
                    string(v, "namespace name")?;
                }
                2 => value = Some(bytes(v, "namespace value")?),
                _ => {}
            }
        }
        let Some(value) = value else {
            return bad("a namespace declaration has no IRI");
        };
        // Its IRI takes part in the stream's IRI sequence like any other.
        self.seq.message(&self.tables, value)?;
        Ok(())
    }

    /// One row-layout term. `None` means the slot was empty (a repeated term).
    fn term(
        &mut self,
        slot: Option<(Kind, &[u8])>,
        pos: usize,
        depth: usize,
    ) -> Res<Option<String>> {
        fn generalized<T>(what: &str) -> Res<T> {
            bad(format!(
                "generalized RDF is not supported ({what}); rete stores RDF 1.1 / RDF-star terms"
            ))
        }
        let Some((kind, b)) = slot else {
            return Ok(None);
        };
        Ok(Some(match kind {
            Kind::Iri => self.seq.message(&self.tables, b)?,
            Kind::Bnode if pos == 1 => return generalized("a blank node as predicate"),
            Kind::Bnode => bnode_token(utf8(b)?, self.scope),
            Kind::Literal if pos != 2 => {
                return generalized("a literal as subject, predicate or graph name")
            }
            Kind::Literal => self.literal(b)?,
            Kind::Triple if pos == 1 => return generalized("a quoted triple as predicate"),
            Kind::Triple => self.quoted(b, depth + 1)?,
            Kind::DefaultGraph => return Ok(None),
        }))
    }

    fn literal(&self, b: &[u8]) -> Res<String> {
        let (mut lex, mut lang, mut dt) = ("", None, None);
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => lex = string(v, "lex")?,
                2 => lang = Some(string(v, "langtag")?),
                3 => dt = Some(num32(v, "datatype")?),
                _ => {} // direction (4) is a version-3 field; ignored in rows
            }
        }
        if lang.is_some() && dt.is_some() {
            return bad("a literal has both a language tag and a datatype");
        }
        let dt = match dt {
            Some(id) => Some(self.tables.datatypes.get(id)?),
            None => None,
        };
        literal_token(lex, lang, 0, dt)
    }

    /// A quoted triple: every term present, none repeated.
    fn quoted(&mut self, b: &[u8], depth: usize) -> Res<String> {
        if depth > MAX_DEPTH {
            return bad("quoted triples nested too deeply");
        }
        let slots = split_slots(b, false)?;
        if slots[3].is_some() {
            return bad("a quoted triple has a graph term");
        }
        let mut t: [String; 3] = Default::default();
        for pos in 0..3 {
            match self.term(slots[pos], pos, depth)? {
                Some(x) => t[pos] = x,
                None => return bad("a repeated term occurs in a quoted triple"),
            }
        }
        Ok(format!("<<{} {} {}>>", t[0], t[1], t[2]))
    }

    /// Decode the s, p, o slots of a row, filling empty ones from the previous
    /// statement. Returns the three terms.
    fn spo(&mut self, slots: &Slots<'_>) -> Res<[String; 3]> {
        let mut out: [String; 3] = Default::default();
        for pos in 0..3 {
            let fresh = self.term(slots[pos], pos, 0)?;
            out[pos] = match fresh {
                Some(t) => {
                    self.prev[pos] = Some(t.clone());
                    t
                }
                None => match &self.prev[pos] {
                    Some(t) => t.clone(),
                    None => return bad("a repeated term occurs before any statement set it"),
                },
            };
        }
        Ok(out)
    }

    fn triple(&mut self, b: &[u8], o: &Options) -> Res<()> {
        if o.physical == 2 {
            return bad("a triple row in a QUADS stream");
        }
        let slots = split_slots(b, false)?;
        if slots[3].is_some() {
            return bad("a triple row has a graph term");
        }
        let [s, p, ob] = self.spo(&slots)?;
        let g = if o.physical == 3 {
            match &self.graph {
                Some(g) => g.clone(),
                None => return bad("a triple outside a graph in a GRAPHS stream"),
            }
        } else {
            None
        };
        self.emit(s, p, ob, g)
    }

    fn quad(&mut self, b: &[u8], o: &Options) -> Res<()> {
        if o.physical != 2 {
            return bad("a quad row outside a QUADS stream");
        }
        let slots = split_slots(b, false)?;
        let [s, p, ob] = self.spo(&slots)?;
        let g = self.graph_node(slots[3], true)?;
        self.emit(s, p, ob, g)
    }

    /// A graph name: `Some(g)` where `g` is `None` for the default graph.
    fn graph_node(&mut self, slot: Option<(Kind, &[u8])>, may_repeat: bool) -> Res<Option<String>> {
        match slot {
            None if may_repeat => match &self.prev_g {
                Some(g) => Ok(g.clone()),
                None => bad("a repeated graph occurs before any statement set it"),
            },
            None => bad("a graph start has no graph"),
            Some((Kind::DefaultGraph, _)) => {
                self.prev_g = Some(None);
                Ok(None)
            }
            Some((k, b)) => {
                let g = match k {
                    Kind::Iri => self.seq.message(&self.tables, b)?,
                    Kind::Bnode => bnode_token(utf8(b)?, self.scope),
                    _ => return bad("generalized RDF is not supported (a literal as graph name)"),
                };
                self.prev_g = Some(Some(g.clone()));
                Ok(Some(g))
            }
        }
    }

    fn graph_start(&mut self, b: &[u8], o: &Options) -> Res<()> {
        if o.physical != 3 {
            return bad("a graph_start row outside a GRAPHS stream");
        }
        let slots = split_slots(b, true)?;
        // A graph start inside a graph closes it: older producers write no
        // graph_end rows.
        self.graph = Some(self.graph_node(slots[3], false)?);
        Ok(())
    }

    fn graph_end(&mut self, o: &Options) -> Res<()> {
        if o.physical != 3 {
            return bad("a graph_end row outside a GRAPHS stream");
        }
        if self.graph.take().is_none() {
            return bad("a graph_end row outside a graph");
        }
        Ok(())
    }

    // ----- column layout -----

    fn batch(&mut self, v: Val<'_>) -> Res<()> {
        let o = self.opts()?.clone();
        if !o.columns() {
            return Ok(()); // a version 1/2 stream may ignore the version 3 field
        }
        let b = bytes(v, "columns")?;
        let mut row_count = 0u64;
        let mut lookups: [Vec<&[u8]>; 3] = Default::default();
        let mut namespaces: Vec<&[u8]> = Vec::new();
        let mut cols: [Option<&[u8]>; 4] = [None; 4];
        let mut lengths = Vec::new();
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => row_count = num(v, "row_count")?,
                2..=4 => lookups[(f - 2) as usize].push(bytes(v, "lookup entries")?),
                5 => namespaces.push(bytes(v, "namespaces")?),
                6..=9 => cols[(f - 6) as usize] = Some(bytes(v, "column")?),
                15 => push_u32s(&mut lengths, v, "message_lengths")?,
                _ => {}
            }
        }
        if row_count > MAX_ROWS {
            return bad(format!("row_count {row_count} exceeds {MAX_ROWS}"));
        }
        if !lengths.is_empty() {
            return bad("message_lengths in a FLAT stream");
        }
        if o.physical == 1 && cols[3].is_some() {
            return bad("a graphs column in a TRIPLES stream");
        }
        // 1. lookup entries, before anything else in the batch.
        for (which, entries) in lookups.iter().enumerate() {
            for e in entries {
                self.packed(e, which)?;
            }
        }
        // 2. namespace declarations, one IRI sequence per batch.
        let mut seq = IriSeq::default();
        for ns in namespaces {
            let mut value = None;
            let mut m = Msg::new(ns);
            while let Some((f, v)) = m.next()? {
                match f {
                    1 => {
                        string(v, "namespace name")?;
                    }
                    2 => value = Some(bytes(v, "namespace value")?),
                    _ => {}
                }
            }
            let Some(value) = value else {
                return bad("a namespace declaration has no IRI");
            };
            seq.message(&self.tables, value)?;
        }
        // 3. statements.
        let n = row_count;
        let mut subjects = Column::decode(cols[0], n, &self.tables, &o, self.scope)?;
        let mut predicates = Column::decode(cols[1], n, &self.tables, &o, self.scope)?;
        let mut objects = Column::decode(cols[2], n, &self.tables, &o, self.scope)?;
        let mut graphs = Column::decode(cols[3], n, &self.tables, &o, self.scope)?;
        for _ in 0..n {
            let s = subjects.next();
            let p = predicates.next();
            let ob = objects.next();
            let g = graphs.next();
            let (Some(s), Some(p), Some(ob)) = (s, p, ob) else {
                return bad("a subject, predicate or object cell is unbound");
            };
            if !matches!(s.0, Kind::Iri | Kind::Bnode) {
                return bad("a subject is not an IRI or blank node");
            }
            if p.0 != Kind::Iri {
                return bad("a predicate is not an IRI");
            }
            let g = match g {
                None => None,
                Some((Kind::Iri | Kind::Bnode, t)) => Some(t.to_string()),
                Some(_) => return bad("a graph name is not an IRI or blank node"),
            };
            self.emit(s.1.to_string(), p.1.to_string(), ob.1.to_string(), g)?;
        }
        Ok(())
    }

    /// An `RdfLookupEntryPacked`.
    fn packed(&mut self, b: &[u8], which: usize) -> Res<()> {
        let mut id = 0u32;
        let mut values: Vec<&str> = Vec::new();
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => id = num32(v, "packed lookup id")?,
                2 => values.push(string(v, "packed lookup value")?),
                _ => {}
            }
        }
        if values.is_empty() {
            return bad("a packed lookup entry has no values");
        }
        let table = match which {
            0 => &mut self.tables.names,
            1 => &mut self.tables.prefixes,
            _ => &mut self.tables.datatypes,
        };
        let mut first = id;
        for v in values {
            table.set(first, v)?;
            first = 0; // each next value takes last_id + 1
        }
        Ok(())
    }
}

// ----- columns -----

/// A decoded column: its run values and the segments that lay them over cells.
struct Column {
    runs: Vec<(Kind, String)>,
    segs: Vec<Seg>,
    seg: usize,
    used: u64,
}

#[derive(Clone, Copy)]
enum SegKind {
    /// `count` cells, one run value each, from run `start`.
    Singles(usize),
    /// One run value in `count` cells.
    Repeat(usize),
    Unbound,
}

#[derive(Clone, Copy)]
struct Seg {
    kind: SegKind,
    count: u64,
}

impl Column {
    /// An all-unbound column (an absent field).
    fn empty() -> Column {
        Column {
            runs: Vec::new(),
            segs: Vec::new(),
            seg: 0,
            used: 0,
        }
    }

    /// The next cell, or `None` for an unbound one.
    fn next(&mut self) -> Option<(Kind, &str)> {
        loop {
            let seg = *self.segs.get(self.seg)?;
            if self.used >= seg.count {
                self.seg += 1;
                self.used = 0;
                continue;
            }
            let k = self.used;
            self.used += 1;
            let idx = match seg.kind {
                SegKind::Unbound => return None,
                SegKind::Singles(start) => start + k as usize,
                SegKind::Repeat(i) => i,
            };
            let (kind, t) = &self.runs[idx];
            return Some((*kind, t));
        }
    }

    fn decode(b: Option<&[u8]>, rows: u64, t: &Tables, o: &Options, scope: u64) -> Res<Column> {
        let Some(b) = b else {
            return Ok(Column::empty());
        };
        let mut kinds: &[u8] = &[];
        let mut layouts: Vec<u32> = Vec::new();
        let (mut name_ids, mut prefix_ids, mut lit_kinds, mut dirs) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut lex: Vec<&str> = Vec::new();
        let mut langtags: Vec<&str> = Vec::new();
        let mut bnodes: Vec<&str> = Vec::new();
        let mut triples: Vec<&[u8]> = Vec::new();
        let mut m = Msg::new(b);
        while let Some((f, v)) = m.next()? {
            match f {
                1 => kinds = bytes(v, "kinds")?,
                2 => push_u32s(&mut layouts, v, "layouts")?,
                3 => push_u32s(&mut name_ids, v, "name_ids")?,
                4 => push_u32s(&mut prefix_ids, v, "prefix_ids")?,
                5 => lex.push(string(v, "lex_values")?),
                6 => push_u32s(&mut lit_kinds, v, "literal_kinds")?,
                7 => langtags.push(string(v, "langtags")?),
                8 => push_u32s(&mut dirs, v, "langtag_directions")?,
                9 => bnodes.push(string(v, "bnodes")?),
                10 => triples.push(bytes(v, "triple_terms")?),
                _ => {}
            }
        }
        let counts = [name_ids.len(), lex.len(), bnodes.len(), triples.len()];
        let m_runs: usize = counts.iter().sum();

        // The kind of each run value.
        let order: Vec<u8> = if kinds.is_empty() {
            if counts.iter().filter(|&&c| c > 0).count() > 1 {
                return bad("a column mixes term types but has no kinds field");
            }
            let k = counts.iter().position(|&c| c > 0).unwrap_or(0) as u8;
            vec![k; m_runs]
        } else {
            if kinds.len() != m_runs.div_ceil(4) {
                return bad("the kinds field has the wrong length");
            }
            if !m_runs.is_multiple_of(4) && kinds[kinds.len() - 1] >> (2 * (m_runs % 4)) != 0 {
                return bad("unused bits of the kinds field are not zero");
            }
            (0..m_runs)
                .map(|i| (kinds[i / 4] >> (2 * (i % 4))) & 3)
                .collect()
        };
        let mut used = [0usize; 4];
        for &k in &order {
            used[k as usize] += 1;
        }
        if used.iter().zip(counts).any(|(u, c)| *u > c) {
            return bad("the kinds field refers to more values than the column has");
        }

        // The IRIs of the column, and of its triple terms, are separate sequences.
        if !matches!(prefix_ids.len(), 0 | 1) && prefix_ids.len() != name_ids.len() {
            return bad("prefix_ids has a length other than 0, 1 or one per IRI");
        }
        if !matches!(lit_kinds.len(), 0 | 1) && lit_kinds.len() != lex.len() {
            return bad("literal_kinds has a length other than 0, 1 or one per literal");
        }
        if !dirs.is_empty() && dirs.len() != langtags.len() {
            return bad("langtag_directions does not match langtags");
        }
        if !triples.is_empty() && matches!(o.rdf_version, 1 | 2) {
            return bad("a triple term in a stream that declares RDF 1.1 or RDF 1.2 Basic");
        }
        let (mut iri, mut lit, mut bn, mut tr) = (0usize, 0usize, 0usize, 0usize);
        let mut iri_seq = IriSeq::default();
        let mut tt_seq = IriSeq::default();
        let mut runs = Vec::with_capacity(m_runs);
        for &k in &order {
            runs.push(match k {
                0 => {
                    let name = name_ids[iri];
                    let prefix = match prefix_ids.len() {
                        0 => 0,
                        1 => prefix_ids[0],
                        _ => prefix_ids[iri],
                    };
                    iri += 1;
                    (Kind::Iri, iri_seq.resolve(t, prefix, name)?)
                }
                1 => {
                    let kind = match lit_kinds.len() {
                        0 => 0,
                        1 => lit_kinds[0],
                        _ => lit_kinds[lit],
                    };
                    let l = lex[lit];
                    lit += 1;
                    (
                        Kind::Literal,
                        column_literal(l, kind, &langtags, &dirs, t, o)?,
                    )
                }
                2 => {
                    bn += 1;
                    (Kind::Bnode, bnode_token(bnodes[bn - 1], scope))
                }
                _ => {
                    tr += 1;
                    (
                        Kind::Triple,
                        triple_term(triples[tr - 1], t, &mut tt_seq, o, scope, 1)?,
                    )
                }
            });
        }

        // Segments from the layout tokens.
        let mut segs = Vec::new();
        let (mut i, mut pos) = (0usize, 0u64);
        let mut it = layouts.iter();
        while let Some(&tok) = it.next() {
            let skip = u64::from(tok >> 5) as usize;
            let kind = (tok >> 4) & 1;
            let code = tok & 15;
            let len = if code == 15 {
                match it.next() {
                    Some(&e) => 15 + u64::from(e),
                    None => return bad("a layout token is missing its extension varint"),
                }
            } else {
                u64::from(code)
            };
            if i + skip > m_runs {
                return bad("a layout skip runs past the last run value");
            }
            if skip > 0 {
                segs.push(Seg {
                    kind: SegKind::Singles(i),
                    count: skip as u64,
                });
                i += skip;
                pos += skip as u64;
            }
            if kind == 0 {
                if i == m_runs {
                    return bad("a repeat run points past the last run value");
                }
                segs.push(Seg {
                    kind: SegKind::Repeat(i),
                    count: len + 2,
                });
                i += 1;
                pos += len + 2;
            } else {
                segs.push(Seg {
                    kind: SegKind::Unbound,
                    count: len + 1,
                });
                pos += len + 1;
            }
            if pos > rows {
                return bad("a column decodes to more cells than row_count");
            }
        }
        if i < m_runs {
            segs.push(Seg {
                kind: SegKind::Singles(i),
                count: (m_runs - i) as u64,
            });
            pos += (m_runs - i) as u64;
        }
        if pos > rows {
            return bad("a column decodes to more cells than row_count");
        }
        Ok(Column {
            runs,
            segs,
            seg: 0,
            used: 0,
        })
    }
}

fn column_literal(
    lex: &str,
    kind: u32,
    langtags: &[&str],
    dirs: &[u32],
    t: &Tables,
    o: &Options,
) -> Res<String> {
    if kind == 0 {
        return literal_token(lex, None, 0, None);
    }
    if kind % 2 == 1 {
        let dt = t.datatypes.get(kind.div_ceil(2))?;
        return literal_token(lex, None, 0, Some(dt));
    }
    let idx = (kind / 2 - 1) as usize;
    let Some(tag) = langtags.get(idx) else {
        return bad("a literal kind refers past the end of langtags");
    };
    let dir = dirs.get(idx).copied().unwrap_or(0);
    if dir != 0 && o.rdf_version == 1 {
        return bad("a base direction in a stream that declares RDF 1.1");
    }
    literal_token(lex, Some(tag), dir, None)
}

/// An `RdfTripleTerm`: subject, predicate and object all present, only the
/// object may nest.
fn triple_term(
    b: &[u8],
    t: &Tables,
    seq: &mut IriSeq,
    o: &Options,
    scope: u64,
    depth: usize,
) -> Res<String> {
    if depth > MAX_DEPTH {
        return bad("triple terms nested too deeply");
    }
    let mut s = None;
    let mut p = None;
    let mut ob: Option<(u32, &[u8])> = None;
    let mut m = Msg::new(b);
    while let Some((f, v)) = m.next()? {
        match f {
            1 | 2 => {
                if s.is_some() {
                    return bad("a triple term has two subjects");
                }
                s = Some((f, bytes(v, "triple term subject")?));
            }
            5 => p = Some(bytes(v, "triple term predicate")?),
            9..=12 => {
                if ob.is_some() {
                    return bad("a triple term has two objects");
                }
                ob = Some((f, bytes(v, "triple term object")?));
            }
            _ => {}
        }
    }
    let (Some(s), Some(p), Some(ob)) = (s, p, ob) else {
        return bad("a triple term is missing its subject, predicate or object");
    };
    let st = if s.0 == 1 {
        seq.message(t, s.1)?
    } else {
        bnode_token(utf8(s.1)?, scope)
    };
    let pt = seq.message(t, p)?;
    let ot = match ob.0 {
        9 => seq.message(t, ob.1)?,
        10 => bnode_token(utf8(ob.1)?, scope),
        11 => {
            let (mut lex, mut lang, mut dt, mut dir) = ("", None, None, 0u32);
            let mut m = Msg::new(ob.1);
            while let Some((f, v)) = m.next()? {
                match f {
                    1 => lex = string(v, "lex")?,
                    2 => lang = Some(string(v, "langtag")?),
                    3 => dt = Some(num32(v, "datatype")?),
                    4 => dir = num32(v, "direction")?,
                    _ => {}
                }
            }
            if lang.is_some() && dt.is_some() {
                return bad("a literal has both a language tag and a datatype");
            }
            if dir != 0 && lang.is_none() {
                return bad("a base direction without a language tag");
            }
            if dir != 0 && o.rdf_version == 1 {
                return bad("a base direction in a stream that declares RDF 1.1");
            }
            let dt = match dt {
                Some(id) => Some(t.datatypes.get(id)?),
                None => None,
            };
            literal_token(lex, lang, dir, dt)?
        }
        _ => triple_term(ob.1, t, seq, o, scope, depth + 1)?,
    };
    Ok(format!("<<{st} {pt} {ot}>>"))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Read the length prefix of the next frame; `None` at a clean end of input.
fn frame_len<R: BufRead>(r: &mut R) -> Res<Option<u64>> {
    let mut v = 0u64;
    for (i, shift) in (0..70).step_by(7).enumerate() {
        let mut byte = [0u8; 1];
        match r.read(&mut byte) {
            Ok(0) if i == 0 => return Ok(None),
            Ok(0) => return bad("input ends inside a frame length"),
            Ok(_) => {}
            Err(e) => return Err(IngestError::Io(e.to_string())),
        }
        if shift == 63 && byte[0] > 1 {
            return bad("frame length overflows 64 bits");
        }
        v |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(Some(v));
        }
    }
    bad("frame length longer than 10 bytes")
}

/// Stream every statement of a delimited Jelly file through `f`, one frame at a
/// time. See the module documentation for what is accepted.
pub(super) fn stream<R: BufRead>(
    mut reader: R,
    audit: Option<&mut IriAudit>,
    f: &mut dyn FnMut(RawQuad),
) -> Result<(), IngestError> {
    let mut d = Decoder {
        opts: None,
        tables: Tables::new(),
        seq: IriSeq::default(),
        prev: [None, None, None],
        prev_g: None,
        graph: None,
        scope: 0,
        frame: 0,
        audit,
        f,
    };
    let buf: Vec<u8> = Vec::new();
    // A file may be one bare, non-delimited frame (the specification asks
    // consumers to read it, though producers must not write it). Such a file
    // starts with the tag byte of `rows` (0x0a), which is also the length
    // prefix of a 10-byte frame, so look at the start before deciding.
    if reader
        .fill_buf()
        .map_err(|e| IngestError::Io(e.to_string()))?
        .first()
        == Some(&0x0a)
    {
        let mut head = Vec::new();
        reader
            .by_ref()
            .take(MAX_BARE_FRAME + 1)
            .read_to_end(&mut head)
            .map_err(|e| IngestError::Io(e.to_string()))?;
        if head.len() as u64 <= MAX_BARE_FRAME && is_bare_frame(&head) {
            d.frame = 1;
            return d.frame(&head).map_err(|e| match e {
                IngestError::Jelly(m) => IngestError::Jelly(format!("frame 1: {m}")),
                other => other,
            });
        }
        return stream_frames(
            std::io::BufReader::new(std::io::Cursor::new(head).chain(reader)),
            d,
            buf,
        );
    }
    stream_frames(reader, d, buf)
}

/// Largest non-delimited frame looked for at the start of a file.
const MAX_BARE_FRAME: u64 = 64 << 20;

/// Whether `buf`, the whole input, is one bare `RdfStreamFrame` rather than a
/// run of delimited ones: it is not a clean sequence of length-prefixed
/// messages, and it parses as a message with a `rows` field.
fn is_bare_frame(buf: &[u8]) -> bool {
    let mut m = Msg::new(buf);
    let mut delimited = true;
    while !m.b.is_empty() {
        match m.varint() {
            Ok(l) if l <= m.b.len() as u64 => m.b = &m.b[l as usize..],
            _ => {
                delimited = false;
                break;
            }
        }
    }
    if delimited {
        return false;
    }
    let mut m = Msg::new(buf);
    let mut rows = 0;
    loop {
        match m.next() {
            Ok(Some((1, Val::Bytes(_)))) => rows += 1,
            Ok(Some(_)) => {}
            Ok(None) => return rows > 0,
            Err(_) => return false,
        }
    }
}

fn stream_frames<R: BufRead>(
    mut reader: R,
    mut d: Decoder<'_, '_>,
    mut buf: Vec<u8>,
) -> Result<(), IngestError> {
    while let Some(len) = frame_len(&mut reader)? {
        d.frame += 1;
        let frame_no = d.frame;
        if len > MAX_FRAME {
            return bad(format!(
                "frame {frame_no} is {len} bytes, more than the {MAX_FRAME} this reader accepts"
            ));
        }
        buf.clear();
        reader
            .by_ref()
            .take(len)
            .read_to_end(&mut buf)
            .map_err(|e| IngestError::Io(e.to_string()))?;
        if (buf.len() as u64) != len {
            return bad(format!("frame {frame_no} is truncated"));
        }
        d.frame(&buf).map_err(|e| match e {
            IngestError::Jelly(m) => IngestError::Jelly(format!("frame {frame_no}: {m}")),
            other => other,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{parse_jelly_bytes, stream_reader, IngestError};

    /// 3 quads in Jelly 1.1 row layout, written by hand and checked against
    /// jelly-cli: an IRI triple in the default graph, a language literal and a
    /// typed literal in <g1>, with repeated subject and graph terms.
    const TINY: &str = "bb010a0e0a0c1002481050085808700278020a1752151213687474703a2f2f6578616d706c652e6f72672f0a094a071205616c6963650a094a0712056b6e6f77730a074a051203626f620a084a0612046e616d650a064a04120267310a2c5a2a1228687474703a2f2f7777772e77332e6f72672f323030312f584d4c536368656d6123696e74656765720a0c1a0a0a0208012a004a007a000a171a152a0210045a0b0a05416c6963651202656e6a0210050a0a1a085a060a0234321801";

    fn tiny() -> Vec<u8> {
        (0..TINY.len() / 2)
            .map(|i| u8::from_str_radix(&TINY[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    fn quad(s: &str, p: &str, o: &str, g: Option<&str>) -> super::RawQuad {
        (s.into(), p.into(), o.into(), g.map(Into::into))
    }

    #[test]
    fn reads_a_tiny_row_layout_file() {
        let q = parse_jelly_bytes(&tiny()).unwrap();
        let ex = |n: &str| format!("<http://example.org/{n}>");
        assert_eq!(
            q,
            vec![
                quad(&ex("alice"), &ex("knows"), &ex("bob"), None),
                quad(&ex("alice"), &ex("name"), "\"Alice\"@en", Some(&ex("g1"))),
                quad(
                    &ex("alice"),
                    &ex("name"),
                    "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>",
                    Some(&ex("g1"))
                ),
            ]
        );
    }

    #[test]
    fn streams_through_the_reader_entry_point() {
        let mut n = 0;
        stream_reader(std::io::Cursor::new(tiny()), "jelly", &mut |_| n += 1).unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn refuses_what_is_not_a_valid_stream() {
        let ok = tiny();
        // truncated inside the frame
        assert!(matches!(
            parse_jelly_bytes(&ok[..ok.len() - 3]),
            Err(IngestError::Jelly(m)) if m.contains("truncated")
        ));
        // not a Jelly file at all
        assert!(parse_jelly_bytes(b"<a> <b> <c> .\n").is_err());
        // a version tag this reader does not know (the options' last field, 15)
        let mut v4 = ok.clone();
        let at = v4.iter().position(|&b| b == 0x78).unwrap();
        v4[at + 1] = 4;
        assert!(matches!(
            parse_jelly_bytes(&v4),
            Err(IngestError::Jelly(m)) if m.contains("version tag 4")
        ));
        // text is not an input for a binary format
        assert!(super::super::parse_statements("x", "jelly").is_err());
    }

    #[test]
    fn an_empty_file_is_an_empty_stream() {
        assert!(parse_jelly_bytes(&[]).unwrap().is_empty());
    }
}
