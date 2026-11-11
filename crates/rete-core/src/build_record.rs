//! The build record every writer leaves in its file (the kind-7 `BuildInfo`
//! section; in `0x06`, beside the card), and the one step that turns an in-RAM
//! build into the file every front end writes.
//!
//! The CLI keeps its own, richer structures for the record (it measures phases,
//! spill and starter-query costs). What is here is what a *binding or the
//! browser* knows: the engine (version, commit, target, profile, features), the
//! host's OS and architecture, the options of the build, the inputs where it
//! knows them, the counts, the writer knobs, and **which front end built it**
//! (`cli`, `python`, `r`, `java`, `wasm`). The JSON has the CLI's schema: `rete
//! card` and every reader of the record read both.
//!
//! `RETE_BUILD_RECORD=reproducible` (the CLI's switch, honoured here too where
//! there is an environment) leaves out the facts that change from run to run:
//! the timestamp, unless `SOURCE_DATE_EPOCH` pins it.
//!
//! A browser engine is a tracked, byte-compared artifact, so its `engine.commit`
//! is `unknown` (see `build.rs`); `docs/wasm-build.json` carries the commit.

use serde_json::{json, Map, Value};

use crate::file::FileError;
use crate::ingest::BuildStats;

/// The front ends that write files. The record says which one did.
pub mod front_end {
    /// `rete build`, `rete merge`, `rete convert`, `rete recard`.
    pub const CLI: &str = "cli";
    /// The Python client (`rete-graph`).
    pub const PYTHON: &str = "python";
    /// The R client.
    pub const R: &str = "r";
    /// The Java client.
    pub const JAVA: &str = "java";
    /// The browser engine (the playground's builder, the npm and MCPB packages).
    pub const WASM: &str = "wasm";
}

/// What a build asked for, as the record states it.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The entry point: `build`, `build_dataset`, `from_text`, …
    pub command: String,
    /// Input syntax, when one syntax was read.
    pub format: Option<String>,
    /// No community pyramid.
    pub no_pyramid: bool,
    /// A full-text index was written.
    pub text_index: bool,
    /// A label index was written.
    pub label_index: bool,
    /// `louvain` or `types`, when a pyramid was built.
    pub pyramid_algo: Option<String>,
    /// 6 or 3 stored permutations.
    pub permutations: Option<u8>,
}

/// One thing the build read.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// A file name (never a path) or a label like `string`.
    pub name: String,
    /// Syntax: `nt`, `nq`, `ttl`, `trig`, `rdfxml`, `rete`.
    pub format: String,
    /// Bytes read.
    pub bytes: Option<u64>,
    /// BLAKE3 (hex) of exactly those bytes.
    pub blake3: Option<String>,
}

impl Input {
    /// An input held in memory: its length and digest are known.
    pub fn from_bytes(name: &str, format: &str, data: &[u8]) -> Self {
        Input {
            name: name.to_string(),
            format: format.to_string(),
            bytes: Some(data.len() as u64),
            blake3: Some(blake3::hash(data).to_hex().to_string()),
        }
    }
}

/// Everything a record is made of.
#[derive(Debug, Clone, Default)]
pub struct Record {
    /// One of [`front_end`].
    pub front_end: String,
    /// The front end's own package version (`rete-python 0.4.2`).
    pub front_end_version: String,
    /// When the file was written, RFC 3339 UTC, whole seconds. `None` to leave
    /// it out (a browser with no clock to read, or a reproducible record).
    pub built_at: Option<String>,
    /// The options of the build.
    pub options: Options,
    /// What it read.
    pub inputs: Vec<Input>,
    /// The statements the parser handed over, when known (else the unique count).
    pub statements_read: Option<u64>,
}

/// `RETE_BUILD_RECORD=reproducible`.
pub fn reproducible() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("RETE_BUILD_RECORD").is_ok_and(|v| v.trim() == "reproducible")
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// `RETE_WRITE_FORMAT=5`: write the legacy `0x05` layout — the test hook the
/// identity tests use, not a user option.
pub fn writes_legacy() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("RETE_WRITE_FORMAT").is_ok_and(|v| v.trim() == "5")
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// The timestamp a record stamps now: the clock (whole seconds, UTC), or
/// `SOURCE_DATE_EPOCH` when set; `None` for a reproducible record without one,
/// and always `None` where there is no clock to read (the browser passes its
/// own through [`Record::built_at`]).
pub fn timestamp() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let epoch = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|v| v.parse::<i64>().ok());
        if reproducible() && epoch.is_none() {
            return None;
        }
        let secs = epoch.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
        Some(rfc3339(secs))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// RFC 3339 UTC for an epoch second (Howard Hinnant's `civil_from_days`).
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The engine this `rete-core` is: enough to rebuild it.
pub fn engine() -> Value {
    json!({
        "name": "rete-core",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": env!("RETE_GIT_COMMIT"),
        "target": env!("RETE_TARGET"),
        "profile": env!("RETE_PROFILE"),
        "features": crate::build_trace::enabled_features(),
    })
}

/// The record as JSON bytes, in the CLI's schema. `stats` are the build's
/// counts (absent: no `counts` block).
pub fn to_json(r: &Record, stats: Option<&BuildStats>) -> Vec<u8> {
    let mut o = Map::new();
    o.insert("schema".into(), json!(1));
    if let Some(t) = &r.built_at {
        o.insert("built_at".into(), json!(t));
    }
    o.insert(
        "builder".into(),
        json!(format!("rete-{} {}", r.front_end, r.front_end_version)),
    );
    o.insert("front_end".into(), json!(r.front_end));
    o.insert("engine".into(), engine());
    #[cfg(not(target_arch = "wasm32"))]
    let host = json!({"os": std::env::consts::OS, "arch": std::env::consts::ARCH});
    #[cfg(target_arch = "wasm32")]
    let host = json!({"os": "browser", "arch": "wasm32"});
    o.insert("host".into(), host);

    let mut p = Map::new();
    p.insert("command".into(), json!(r.options.command));
    if let Some(f) = &r.options.format {
        p.insert("format".into(), json!(f));
    }
    for (k, v) in [
        ("no_pyramid", r.options.no_pyramid),
        ("text_index", r.options.text_index),
        (
            "label_index",
            r.options.label_index || stats.is_some_and(|s| s.label_index),
        ),
    ] {
        if v {
            p.insert(k.into(), json!(true));
        }
    }
    if let Some(a) = &r.options.pyramid_algo {
        p.insert("pyramid_algo".into(), json!(a));
    }
    p.insert(
        "codec".into(),
        json!(if crate::file::writer_codec() == crate::CODEC_ZSTD {
            "zstd"
        } else {
            "none"
        }),
    );
    p.insert(
        "permutations".into(),
        json!(r.options.permutations.unwrap_or(6)),
    );
    p.insert("named_graphs".into(), json!("quad"));
    let w = crate::write_opts::write_options();
    p.insert(
        "write".into(),
        json!({
            "tile_kib": w.tile_budget / 1024,
            "dict_chunk_kib": w.dict_chunk_budget / 1024,
            "zstd_level": w.zstd_level,
            "zstd_frame": match w.frame {
                crate::write_opts::FrameShape::Sized => "sized",
                crate::write_opts::FrameShape::Stream => "stream",
            },
        }),
    );
    o.insert("params".into(), Value::Object(p));

    if !r.inputs.is_empty() {
        let inputs: Vec<Value> = r
            .inputs
            .iter()
            .map(|i| {
                let mut m = Map::new();
                m.insert("name".into(), json!(i.name));
                m.insert("format".into(), json!(i.format));
                if let Some(b) = i.bytes {
                    m.insert("bytes".into(), json!(b));
                }
                if let Some(h) = &i.blake3 {
                    m.insert("blake3".into(), json!(h));
                }
                Value::Object(m)
            })
            .collect();
        o.insert("inputs".into(), Value::Array(inputs));
    }
    if let Some(s) = stats {
        let unique = s.statements as u64;
        let default = s.default_triples as u64;
        let read = r.statements_read.unwrap_or(unique);
        o.insert(
            "counts".into(),
            json!({
                "statements_read": read,
                "statements_unique": unique,
                "duplicates_dropped": read.saturating_sub(unique),
                "default_graph_triples": default,
                "named_graphs": s.named_graphs as u64,
                "named_graph_quads": unique.saturating_sub(default),
                "terms": s.terms as u64,
            }),
        );
    }
    serde_json::to_vec(&Value::Object(o)).unwrap_or_default()
}

/// The file every front end writes from an in-RAM `0x05` build image: the same
/// sections, laid out as `0x06`, with `record` (the build record) in the card
/// region beside the card. An image that is already `0x06` gets the record
/// only. With `RETE_WRITE_FORMAT=5` the image stays `0x05` and the record is
/// attached the `0x05` way.
pub fn finish_in_ram(image: Vec<u8>, record: Option<&[u8]>) -> Result<Vec<u8>, FileError> {
    finish_in_ram_with(image, record, crate::v06::write::WriteOpts::default())
}

/// [`finish_in_ram`] with the writer's options (derived-IRI detection on or off).
pub fn finish_in_ram_with(
    image: Vec<u8>,
    record: Option<&[u8]>,
    opts: crate::v06::write::WriteOpts,
) -> Result<Vec<u8>, FileError> {
    let record = record.filter(|r| !r.is_empty());
    if writes_legacy() {
        return match record {
            Some(r) => crate::file::attach_build_info(&image, r),
            None => Ok(image),
        };
    }
    if crate::v06::open::is_v06(&image) {
        return match record {
            Some(r) => crate::file::attach_build_info(&image, r),
            None => Ok(image),
        };
    }
    let src = crate::reader::SliceReader::new(&image);
    let mut parts = match crate::v06::parts::from_v05(&src) {
        Ok(p) => p,
        // A per-graph (kind 5) image: the named graphs are re-encoded as the
        // quad index, the only layout `0x06` has, and the result finished.
        Err(FileError::Container(m)) if m.contains("kind 5") => {
            let requad = reencode_per_graph(&image)?;
            return finish_in_ram_with(requad, record, opts);
        }
        Err(e) => return Err(e),
    };
    if let Some(r) = record {
        parts.build = Some(r.to_vec());
    }
    let mut out = std::io::Cursor::new(Vec::with_capacity(image.len()));
    crate::v06::write::assemble(&parts, &src, &mut out, opts)?;
    Ok(out.into_inner())
}

/// A `0x05` image whose named graphs are stored per graph (kind 5), rebuilt as
/// a `0x05` image with the quad index: every statement is read back out and
/// assembled again with what the file carries (pyramid, text index and its
/// scope, label index, permutations, card). What `rete convert` does for such a
/// file, and what the public per-graph writers fall back to.
pub fn reencode_per_graph(image: &[u8]) -> Result<Vec<u8>, FileError> {
    let rete = crate::Rete::open(image)?;
    let card = rete.metadata().map(<[u8]>::to_vec);
    let mut quads: Vec<crate::format::RawQuad> = rete
        .dump(None)
        .into_iter()
        .map(|(s, p, o)| (s, p, o, None))
        .collect();
    let named: Vec<String> = rete.graph_names().iter().map(|g| g.to_string()).collect();
    for g in &named {
        for (s, p, o) in rete.dump(Some(g)) {
            quads.push((s, p, o, Some(g.clone())));
        }
    }
    let scope = rete.text_index_scope();
    let mut opts = crate::ingest::AssembleOptions::sectioned();
    opts.graph_layout = crate::GraphLayout::Quad;
    opts.with_pyramid = rete.header().pyramid_meta_len > 0;
    opts.with_text_index = rete.has_text_index();
    opts.text_index_scope = scope.as_ref();
    opts.label_index = rete.has_label_index();
    opts.perms = rete.header().perms;
    let (out, _) =
        crate::ingest::assemble_dataset_opts(quads, &opts, |_, _| card.clone().unwrap_or_default());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339(1_785_801_600), "2026-08-04T00:00:00Z");
    }

    #[test]
    fn the_record_names_its_front_end_and_the_engine() {
        let r = Record {
            front_end: front_end::PYTHON.into(),
            front_end_version: "9.9.9".into(),
            built_at: Some("2026-01-02T03:04:05Z".into()),
            options: Options {
                command: "build".into(),
                format: Some("nt".into()),
                ..Default::default()
            },
            inputs: vec![Input::from_bytes("string", "nt", b"<a> <b> <c> .\n")],
            statements_read: None,
        };
        let v: Value = serde_json::from_slice(&to_json(&r, None)).unwrap();
        assert_eq!(v["front_end"], "python");
        assert_eq!(v["builder"], "rete-python 9.9.9");
        assert_eq!(v["engine"]["name"], "rete-core");
        assert_eq!(v["engine"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["params"]["command"], "build");
        assert_eq!(v["inputs"][0]["bytes"], 14);
        assert_eq!(v["schema"], 1);
    }
    #[test]
    fn a_binding_style_record_says_the_file_has_a_label_index() {
        // The callers of the shared assembler (Python, R, Java, the browser) set
        // no `label_index` option, yet the file has the index; the record reads
        // it from the build's stats.
        let quads: Vec<crate::format::RawQuad> = vec![(
            "<http://ex/a>".into(),
            "<http://www.w3.org/2000/01/rdf-schema#label>".into(),
            "\"A\"".into(),
            None,
        )];
        let (image, stats) = crate::ingest::assemble_dataset_quad(quads, &[]);
        assert!(crate::Rete::open(&image).unwrap().has_label_index());
        let r = Record {
            front_end: front_end::PYTHON.into(),
            front_end_version: "9.9.9".into(),
            built_at: None,
            options: Options {
                command: "build".into(),
                ..Default::default()
            },
            inputs: vec![],
            statements_read: Some(1),
        };
        let v: Value = serde_json::from_slice(&to_json(&r, Some(&stats))).unwrap();
        assert_eq!(v["params"]["label_index"], true);
        // Without stats (nothing known) the field stays absent.
        let v: Value = serde_json::from_slice(&to_json(&r, None)).unwrap();
        assert!(v["params"].get("label_index").is_none());
    }
}

/// The key under which the writer's derived-IRI detection reports (SPEC.md
/// section 5.3): what it found in the data, and why it took or left a family.
pub const DERIVE_REPORT_KEY: &str = "derive_iris";

/// The byte range of `"derive_iris":<value>` (key and value) in a record's JSON
/// text, if it has the key at its top level.
fn report_span(text: &str) -> Option<(usize, usize)> {
    let needle = format!("\"{DERIVE_REPORT_KEY}\":");
    let at = text.find(&needle)?;
    // The key must be the record's own: parse the whole and look it up.
    let doc: serde_json::Value = serde_json::from_str(text).ok()?;
    doc.get(DERIVE_REPORT_KEY)?;
    let v0 = at + needle.len();
    let mut it =
        serde_json::Deserializer::from_str(&text[v0..]).into_iter::<serde::de::IgnoredAny>();
    it.next()?.ok()?;
    Some((at, v0 + it.byte_offset()))
}

/// The record `rec` (JSON text, possibly space-padded) with `report` under
/// [`DERIVE_REPORT_KEY`], placed before a trailing `conversions` list (which the
/// CLI appends to) and replacing an older report. The padding is kept, and the
/// result is as long as `rec` plus the report: a reservation made from it holds
/// the report next to a record that is later sealed to at most `rec`'s length.
/// `None` when `rec` is not a JSON object.
pub fn with_derive_report(rec: &[u8], report: &serde_json::Value) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(rec).ok()?;
    let body = text.trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    let pad = &text[body.len()..];
    let mut body = body.trim_start().to_string();
    if !body.starts_with('{') || !body.ends_with('}') {
        return None;
    }
    if let Some((a, b)) = report_span(&body) {
        // Drop the old key with one neighbouring comma.
        let (mut a, mut b) = (a, b);
        if body[..a].trim_end().ends_with(',') {
            a = body[..a].trim_end().len() - 1;
        } else if body[b..].starts_with(',') {
            b += 1;
        }
        body.replace_range(a..b, "");
    }
    let entry = format!("\"{DERIVE_REPORT_KEY}\":{report}");
    let at = body.rfind(",\"conversions\":[").unwrap_or(body.len() - 1);
    let head = body[..at].trim_end();
    let comma = if head.ends_with('{') { "" } else { "," };
    let out = format!("{head}{comma}{entry}{}{pad}", &body[at..]);
    Some(out.into_bytes())
}

/// The text of the report in `rec`, if it has one.
pub fn derive_report_text(rec: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(rec).ok()?;
    let body = text.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    let (a, b) = report_span(body)?;
    Some(body[a + DERIVE_REPORT_KEY.len() + 3..b].to_string())
}

#[cfg(test)]
mod derive_report_tests {
    use super::*;

    #[test]
    fn a_report_is_spliced_once_before_conversions() {
        let r = serde_json::json!({"taken": [], "n": 3});
        let rec = br#"{"schema":1,"builder":"x","conversions":[{"command":"c"}]}   "#;
        let out = with_derive_report(rec, &r).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains(
            r#""builder":"x","derive_iris":{"n":3,"taken":[]},"conversions":[{"command":"c"}]}"#
        ));
        assert!(text.ends_with("}   "));
        assert!(serde_json::from_str::<serde_json::Value>(&text).is_ok());
        // Again: replaced, not doubled.
        let r2 = serde_json::json!({"x": 1});
        let again = with_derive_report(&out, &r2).unwrap();
        let t2 = String::from_utf8(again.clone()).unwrap();
        assert_eq!(t2.matches("derive_iris").count(), 1);
        assert_eq!(derive_report_text(&again).as_deref(), Some(r#"{"x":1}"#));
        // A record without a list, and an empty one.
        let o = with_derive_report(br#"{"schema":1}"#, &r2).unwrap();
        assert_eq!(o, br#"{"schema":1,"derive_iris":{"x":1}}"#);
        let o = with_derive_report(b"{}", &r2).unwrap();
        assert_eq!(o, br#"{"derive_iris":{"x":1}}"#);
        assert!(with_derive_report(b"not json", &r2).is_none());
    }
}
