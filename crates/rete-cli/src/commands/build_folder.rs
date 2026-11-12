//! Folders and per-file graphs for `rete build`.
//!
//! `rete build DIR` walks the directory and builds one file in which every
//! input file is a named graph. `--graph-per-file` does the same for a list of
//! files. `--merge-files` reads a directory's files as if they had been listed.
//!
//! The result of [`resolve`] is a flat list of input paths (what the build
//! functions already take) and one [`FilePlan`] per path: the graph its triples
//! go to, if any, and the tag that keeps its blank nodes apart from the other
//! files'. A blank node label means something only inside the document that
//! wrote it, so `_:b0` of two inputs are two nodes whenever there are two
//! inputs, unless `--shared-blank-nodes` says the files are one document cut
//! into pieces.

use std::path::Path;

use rete_core::ingest::RawQuad;

/// The graph IRI prefix when `--graph-base` is not given.
pub(crate) const DEFAULT_BASE: &str = "urn:rete:file:";

/// How many skipped names the note prints.
const NOTE_NAMES: usize = 5;

/// The command-line choices that decide how inputs are read.
pub(crate) struct Options<'a> {
    pub format: Option<&'a str>,
    pub merge_files: bool,
    pub graph_per_file: bool,
    pub collapse_graphs: bool,
    pub graph_base: Option<&'a str>,
    pub shared_blank_nodes: bool,
}

/// What to do with the quads of one input file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilePlan {
    /// The graph term (`<iri>`) its triples go to; `None` leaves them where
    /// they are.
    pub graph: Option<String>,
    /// What its blank node labels are prefixed with; `None` leaves them as
    /// written.
    pub tag: Option<String>,
}

/// Inputs ready for the build functions.
pub(crate) struct Resolved {
    /// File paths, in the order they are read (`-` is stdin).
    pub files: Vec<String>,
    /// One plan per file, same indexing; `None` when no quad needs a change
    /// (one input, or a list read with `--shared-blank-nodes`).
    pub plans: Option<Vec<FilePlan>>,
}

/// The plans of files that get no graph: a tag by position when there is more
/// than one file and blank nodes are not shared.
fn merged_plans(n: usize, shared_blank_nodes: bool) -> Option<Vec<FilePlan>> {
    if shared_blank_nodes || n < 2 {
        return None;
    }
    Some(
        (0..n)
            .map(|i| FilePlan {
                graph: None,
                tag: Some(format!("f{i}")),
            })
            .collect(),
    )
}

/// Does the extension of `path` name an RDF input syntax? Asks
/// [`super::build::input_format`], so a format added there is found here: it
/// answers `nt` for anything it does not know, so only `nt` needs its own test.
pub(crate) fn is_known_input(path: &str) -> bool {
    if super::build::input_format(path, None) != "nt" {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    let p = lower
        .strip_suffix(".gz")
        .or_else(|| lower.strip_suffix(".gzip"))
        .unwrap_or(lower.as_str());
    p.ends_with(".nt") || p.ends_with(".ntriples")
}

/// Turn the positional inputs into files and, when each file is a graph, their
/// graph terms.
pub(crate) fn resolve(inputs: &[String], opts: &Options<'_>) -> anyhow::Result<Resolved> {
    let any_dir = inputs.iter().any(|i| i != "-" && Path::new(i).is_dir());
    // A directory is graph-per-file unless told otherwise. Collapsing every
    // graph into the default one leaves nothing for a file graph to be.
    let per_file = opts.graph_per_file || (any_dir && !opts.merge_files && !opts.collapse_graphs);
    if !per_file {
        if opts.graph_base.is_some() {
            anyhow::bail!(
                "--graph-base names the graph of each file, but this build puts no file in a \
                 graph of its own (use a directory without --merge-files, or --graph-per-file)"
            );
        }
        if !any_dir {
            return Ok(Resolved {
                files: inputs.to_vec(),
                plans: merged_plans(inputs.len(), opts.shared_blank_nodes),
            });
        }
    }
    let base = opts.graph_base.unwrap_or(DEFAULT_BASE);
    check_base(base)?;

    // (path to open, name for the graph IRI)
    let mut found: Vec<(String, String)> = Vec::new();
    for input in inputs {
        if input == "-" {
            if per_file {
                anyhow::bail!("a graph per file needs file names: `-` (stdin) has none");
            }
            found.push((input.clone(), String::new()));
        } else if Path::new(input).is_dir() {
            let (files, skipped) = walk(Path::new(input), opts.format)?;
            if let Some(note) = skipped_note(input, &skipped) {
                eprintln!("{note}");
            }
            if files.is_empty() {
                anyhow::bail!("{input}: no RDF files found in this directory");
            }
            for rel in files {
                let full = Path::new(input).join(&rel);
                found.push((full.to_string_lossy().into_owned(), rel));
            }
        } else {
            found.push((input.clone(), given_name(input)?));
        }
    }
    let files: Vec<String> = found.iter().map(|(p, _)| p.clone()).collect();
    if !per_file {
        let plans = merged_plans(files.len(), opts.shared_blank_nodes);
        return Ok(Resolved { files, plans });
    }

    let mut plans: Vec<FilePlan> = Vec::with_capacity(found.len());
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, (path, name)) in found.iter().enumerate() {
        let iri = format!("{base}{}", graph_path(name));
        if let Some(&j) = seen.get(&iri) {
            anyhow::bail!(
                "{} and {path} both become the graph <{iri}>; rename one, or use --merge-files",
                found[j].0
            );
        }
        seen.insert(iri.clone(), i);
        // The tag comes from the graph name, so a file's labels do not change
        // when another file is added to the folder.
        let tag = (!opts.shared_blank_nodes).then(|| {
            let h = blake3::hash(iri.as_bytes());
            format!("f{}", &h.to_hex()[..8])
        });
        plans.push(FilePlan {
            graph: Some(format!("<{iri}>")),
            tag,
        });
    }
    Ok(Resolved {
        files,
        plans: Some(plans),
    })
}

fn check_base(base: &str) -> anyhow::Result<()> {
    if base.is_empty()
        || base
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"{}|\\^`".contains(c))
    {
        anyhow::bail!("--graph-base {base:?} is not usable as the start of an IRI");
    }
    Ok(())
}

/// The relative path of a file named on the command line: `/` separators, no
/// drive, root or `.` segments. A `..` segment is refused, it has no place in
/// a graph name.
fn given_name(path: &str) -> anyhow::Result<String> {
    let norm = path.replace('\\', "/");
    let mut segs = Vec::new();
    for s in norm.split('/') {
        match s {
            "" | "." => {}
            ".." => anyhow::bail!(
                "{path}: a graph per file cannot name a file by a path with `..`; \
                 run from a directory above it"
            ),
            s => segs.push(s),
        }
    }
    Ok(segs.join("/"))
}

/// The graph IRI path of a relative file name: the compression suffix and the
/// format extension removed, every segment percent-encoded.
///
/// Only an extension that names an RDF syntax is removed, so a file read under a
/// forced `--format` keeps whatever else it is called.
fn graph_path(name: &str) -> String {
    let mut n = name;
    for gz in [".gz", ".gzip"] {
        if let Some(s) = strip_suffix_ci(n, gz) {
            n = s;
            break;
        }
    }
    if is_known_input(n) {
        if let Some(dot) = n.rfind('.') {
            // Only the last segment can carry the extension.
            if n[dot..].find('/').is_none() {
                n = &n[..dot];
            }
        }
    }
    n.split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn strip_suffix_ci<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = s.len().checked_sub(suffix.len())?;
    (s.is_char_boundary(cut) && s[cut..].eq_ignore_ascii_case(suffix)).then(|| &s[..cut])
}

/// Percent-encode a path segment: everything but ASCII letters, digits and
/// `-` `.` `_` `~`, byte by byte over the UTF-8, so the IRI is plain ASCII and
/// does not depend on how a file system normalizes names.
fn encode_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Every readable file under `dir`, as relative paths with `/` separators in
/// byte order, plus the non-hidden files that were left out for want of a known
/// extension. Hidden names (leading `.`) are skipped, directory symlinks are
/// not followed.
fn walk(dir: &Path, format: Option<&str>) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    walk_into(dir, "", format.is_some(), &mut files, &mut skipped)?;
    files.sort();
    skipped.sort();
    Ok((files, skipped))
}

fn walk_into(
    dir: &Path,
    prefix: &str,
    any_extension: bool,
    files: &mut Vec<String>,
    skipped: &mut Vec<String>,
) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "{}: a file name that is not valid UTF-8 cannot be a graph name",
                dir.display()
            )
        })?;
        if name.starts_with('.') {
            continue;
        }
        let rel = format!("{prefix}{name}");
        let ty = entry.file_type()?;
        if ty.is_dir() {
            walk_into(
                &entry.path(),
                &format!("{rel}/"),
                any_extension,
                files,
                skipped,
            )?;
        } else if ty.is_file() || (ty.is_symlink() && entry.path().is_file()) {
            if any_extension || is_known_input(&rel) {
                files.push(rel);
            } else {
                skipped.push(rel);
            }
        }
    }
    Ok(())
}

fn skipped_note(dir: &str, skipped: &[String]) -> Option<String> {
    if skipped.is_empty() {
        return None;
    }
    let shown: Vec<&str> = skipped
        .iter()
        .take(NOTE_NAMES)
        .map(String::as_str)
        .collect();
    let more = if skipped.len() > NOTE_NAMES {
        ", ..."
    } else {
        ""
    };
    Some(format!(
        "{dir}: skipped {} file{} with no known RDF extension ({}{more}); \
         --format reads them all",
        skipped.len(),
        if skipped.len() == 1 { "" } else { "s" },
        shown.join(", ")
    ))
}

/// Apply a file's plan to one of its quads.
///
/// With a graph, a triple (no graph) goes to it and a quad that names a graph
/// keeps its own. With a tag, every blank node label gets it as a prefix, in
/// subject, object and graph position and inside a quoted triple or triple
/// term: `_:b0` of the file tagged `f1` is `_:f1_b0`.
pub(crate) fn place(q: RawQuad, plan: &FilePlan) -> RawQuad {
    let (s, p, o, g) = q;
    let tag = plan.tag.as_deref();
    let g = match g {
        Some(g) => Some(scope(g, tag)),
        None => plan.graph.clone(),
    };
    (scope(s, tag), p, scope(o, tag), g)
}

fn scope(term: String, tag: Option<&str>) -> String {
    let Some(tag) = tag else { return term };
    if let Some(label) = term.strip_prefix("_:") {
        return format!("_:{tag}_{label}");
    }
    if term.starts_with("<<") {
        return scope_inside(&term, tag);
    }
    term
}

/// Tag the blank node labels inside a quoted triple or triple term. IRIs and
/// literals are copied as they are: a `_:` inside `<…>` or `"…"` is text.
fn scope_inside(term: &str, tag: &str) -> String {
    let b = term.as_bytes();
    let mut out = String::with_capacity(term.len() + 16);
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        match b[i] {
            // `<<` and `<<(` open a triple; a single `<` opens an IRI.
            b'<' if b.get(i + 1) == Some(&b'<') => i += 2,
            b'<' => {
                while i < b.len() && b[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'_' if b.get(i + 1) == Some(&b':') => {
                out.push_str(&term[copied..i + 2]);
                out.push_str(tag);
                out.push('_');
                i += 2;
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&term[copied.min(term.len())..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_paths() {
        assert_eq!(graph_path("a/b.ttl"), "a/b");
        assert_eq!(graph_path("a b/c d.nt.gz"), "a%20b/c%20d");
        assert_eq!(graph_path("é.TTL"), "%C3%A9");
        assert_eq!(graph_path("x.dat"), "x.dat");
        assert_eq!(graph_path("v1.2/x.ttl"), "v1.2/x");
        assert_eq!(graph_path("v1.2/x"), "v1.2/x");
    }

    #[test]
    fn known_inputs() {
        for f in ["a.nt", "a.NQ", "a.ttl.gz", "a.trig", "a.rdf", "a.jelly"] {
            assert!(is_known_input(f), "{f}");
        }
        for f in ["a.txt", "README", "a.json", "a.xml"] {
            assert!(!is_known_input(f), "{f}");
        }
    }

    fn plan(graph: Option<&str>, tag: Option<&str>) -> FilePlan {
        FilePlan {
            graph: graph.map(str::to_string),
            tag: tag.map(str::to_string),
        }
    }

    #[test]
    fn blank_nodes_are_scoped() {
        let q = ("_:b0".to_string(), "<p>".into(), "_:b1".into(), None);
        let a = place(q.clone(), &plan(Some("<urn:a>"), Some("f0")));
        assert_eq!(a.0, "_:f0_b0");
        assert_eq!(a.2, "_:f0_b1");
        assert_eq!(a.3.as_deref(), Some("<urn:a>"));
        // No graph: the triple stays in the default graph.
        let b = place(q.clone(), &plan(None, Some("f1")));
        assert_eq!((b.0.as_str(), b.3.as_deref()), ("_:f1_b0", None));
        // No tag: labels as written.
        let c = place(q, &plan(Some("<urn:c>"), None));
        assert_eq!(c.0, "_:b0");
        // A blank graph name is a blank node too; a named one is kept.
        let g = (
            "<s>".to_string(),
            "<p>".into(),
            "<o>".into(),
            Some("_:g".to_string()),
        );
        assert_eq!(
            place(g, &plan(Some("<urn:a>"), Some("f0"))).3.as_deref(),
            Some("_:f0_g")
        );
    }

    #[test]
    fn blank_nodes_inside_a_quoted_triple_are_scoped_and_text_is_not() {
        for (term, want) in [
            (
                "<< _:b0 <http://e/p> _:b1 >>",
                "<< _:f0_b0 <http://e/p> _:f0_b1 >>",
            ),
            (
                "<<( _:b0 <http://e/_:p> \"_:x \\\" _:y\" )>>",
                "<<( _:f0_b0 <http://e/_:p> \"_:x \\\" _:y\" )>>",
            ),
            (
                "<< << _:a <p> <o> >> <q> \"v\"@en >>",
                "<< << _:f0_a <p> <o> >> <q> \"v\"@en >>",
            ),
            ("<http://e/_:b0>", "<http://e/_:b0>"),
            ("\"_:b0\"", "\"_:b0\""),
        ] {
            assert_eq!(scope(term.to_string(), Some("f0")), want, "{term}");
        }
    }

    #[test]
    fn plans_of_a_list() {
        let o = |shared| Options {
            format: None,
            merge_files: false,
            graph_per_file: false,
            collapse_graphs: false,
            graph_base: None,
            shared_blank_nodes: shared,
        };
        let two = ["a.nt".to_string(), "b.nt".to_string()];
        let r = resolve(&two, &o(false)).unwrap();
        assert_eq!(
            r.plans,
            Some(vec![plan(None, Some("f0")), plan(None, Some("f1"))])
        );
        assert_eq!(resolve(&two, &o(true)).unwrap().plans, None);
        // One input: its labels are kept as written.
        assert_eq!(resolve(&two[..1], &o(false)).unwrap().plans, None);
    }
}
