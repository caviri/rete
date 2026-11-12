//! Where the remote `@context`s of a JSON-LD / YAML-LD input come from.
//!
//! A build is offline and repeatable by default: a document that names a remote
//! context fails, and the error names the IRI. Two ways out, both given on the
//! command line and read here:
//!
//! * `--context IRI=FILE` (repeatable) answers one context IRI from a local file;
//! * `--fetch-contexts` fetches any other context over HTTP(S).
//!
//! rete-core owns the parsing and does no I/O; this is the loader it calls.
//! The settings are set once, before any input is read (`build` and `validate`
//! share them), so they are kept here rather than threaded through every
//! signature between `main` and the parser.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::OnceLock;

use rete_core::ingest::JsonLdOptions;

/// Cap on one fetched context: a context is a few KiB, so this only stops a
/// server that never stops.
const MAX_CONTEXT_BYTES: u64 = 16 << 20;

struct Sources {
    files: HashMap<String, PathBuf>,
    fetch: bool,
}

static SOURCES: OnceLock<Sources> = OnceLock::new();

/// Record the `--context IRI=FILE` pairs and `--fetch-contexts` for this run.
pub(crate) fn configure(contexts: &[String], fetch: bool) -> anyhow::Result<()> {
    let mut files = HashMap::new();
    for c in contexts {
        let (iri, file) = c.split_once('=').filter(|(i, f)| !i.is_empty() && !f.is_empty()).ok_or_else(
            || anyhow::anyhow!("--context {c}: expected IRI=FILE, e.g. --context https://schema.org/=schema.jsonld"),
        )?;
        files.insert(iri.to_string(), PathBuf::from(file));
    }
    // A second call (the CLI runs one command per process) keeps the first.
    let _ = SOURCES.set(Sources { files, fetch });
    Ok(())
}

/// The options the JSON-LD reader gets: the loader over what [`configure`] set,
/// or none at all, so a document needing a remote context fails with the
/// context IRI named.
pub(crate) fn options() -> JsonLdOptions {
    match SOURCES.get() {
        Some(s) if !s.files.is_empty() || s.fetch => {
            JsonLdOptions::with_loader(|iri: &str| load(iri))
        }
        _ => JsonLdOptions::default(),
    }
}

fn load(iri: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(s) = SOURCES.get() else {
        return Ok(None);
    };
    if let Some(file) = s.files.get(iri) {
        return std::fs::read(file)
            .map(Some)
            .map_err(|e| format!("--context file {}: {e}", file.display()));
    }
    if !s.fetch {
        return Ok(None);
    }
    fetch(iri).map(Some)
}

fn fetch(iri: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build();
    let resp = agent
        .get(iri)
        .set("Accept", "application/ld+json, application/json;q=0.9")
        .set("User-Agent", "rete-cli (JSON-LD context fetch)")
        .call()
        .map_err(|e| format!("fetching: {e}"))?;
    let mut body = Vec::new();
    resp.into_reader()
        .take(MAX_CONTEXT_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("reading: {e}"))?;
    if body.len() as u64 > MAX_CONTEXT_BYTES {
        return Err(format!("larger than {MAX_CONTEXT_BYTES} bytes"));
    }
    Ok(body)
}

/// What to add under an error that names a context IRI nobody supplied.
pub(crate) fn remote_context_hint(iri: &str) -> String {
    format!(
        "hint: builds are offline, so the remote context is not fetched. Either save it and \
         pass `--context {iri}=FILE`,\n      or allow the fetch with `--fetch-contexts`."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_option_needs_an_iri_and_a_file() {
        assert!(configure(&["no-equals".into()], false).is_err());
        assert!(configure(&["=file".into()], false).is_err());
        assert!(configure(&["http://x/=".into()], false).is_err());
    }
}
