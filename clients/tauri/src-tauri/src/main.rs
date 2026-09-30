// Rete File Explorer — the desktop build.
//
// The window renders exactly the same page as `experiments/rete-file-explorer`;
// only the engine underneath changes. In the browser the UI talks to a Web
// Worker holding a wasm `RemoteGraph`; here it talks to these commands, which
// drive `rete-core` natively. That is the whole point of keeping `rete-fs.js`
// engine-agnostic: no wasm, no 4 GB heap ceiling, real threads, and positional
// reads straight against a local file.
//
// The command surface deliberately mirrors the worker's message protocol
// (`open` / `query` / `stats`), so the front end swaps one transport for the
// other and nothing else moves.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod source;

use std::sync::Mutex;

use rete_core::{eval_query, QueryOutput, RangeReader};
use serde::Serialize;
use tauri::Manager;

/// Matches the wasm build's envelope version so the shared front end can treat
/// both engines identically.
const JSON_SCHEMA_VERSION: u32 = 1;

#[derive(Default)]
struct AppState {
    opened: Mutex<Option<source::Opened>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenInfo {
    /// Total file length in bytes.
    size: u64,
    /// The fixed 1 KB header, handed back raw so the front end parses it with
    /// the same `parseHeader` it uses in the browser — one implementation of the
    /// section directory, not two.
    head: Vec<u8>,
    /// The Dataset Card as stored, or null when the file carries none.
    card: Option<String>,
    /// The baked schema pyramid as JSON, or null with `schemaError` set.
    schema: Option<String>,
    schema_error: Option<String>,
    content_hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Stats {
    file_length: u64,
    bytes: u64,
    requests: u64,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn open_graph(state: tauri::State<'_, AppState>, source: String) -> Result<OpenInfo, String> {
    let opened = source::open(&source).map_err(err)?;

    let size = opened.reader.len();
    let head_len = 1024u64.min(size);
    let head = opened.reader.read_at(0, head_len).map_err(err)?;

    let card = rete_core::read_metadata_ranged(&*opened.reader)
        .map_err(err)?
        .map(|b| String::from_utf8_lossy(&b).into_owned());

    // A file built without a pyramid has no class list. That is a fact about the
    // file, not a failure to open it, so report it and let the views degrade —
    // the same contract the browser build has.
    let (schema, schema_error) = match rete_core::read_schema_summary_ranged(&*opened.reader) {
        Ok(Some((classes, relations))) => {
            let classes: Vec<serde_json::Value> = classes
                .iter()
                .map(|(c, n)| serde_json::json!([c, n]))
                .collect();
            let relations: Vec<serde_json::Value> = relations
                .iter()
                .map(|(s, p, o, n)| serde_json::json!([s, p, o, n]))
                .collect();
            (
                Some(
                    serde_json::json!({
                        "schemaVersion": JSON_SCHEMA_VERSION,
                        "kind": "schema",
                        "classes": classes,
                        "relations": relations,
                    })
                    .to_string(),
                ),
                None,
            )
        }
        Ok(None) => (None, Some("file has no schema pyramid".to_string())),
        Err(e) => (None, Some(e.to_string())),
    };

    let content_hash = opened
        .rete
        .header()
        .content_hash
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    *state.opened.lock().unwrap() = Some(opened);

    Ok(OpenInfo {
        size,
        head,
        card,
        schema,
        schema_error,
        content_hash,
    })
}

#[tauri::command]
fn run_query(
    state: tauri::State<'_, AppState>,
    sparql: String,
    format: String,
) -> Result<String, String> {
    let guard = state.opened.lock().unwrap();
    let opened = guard.as_ref().ok_or("no archive open")?;
    let out = eval_query(&opened.rete, &sparql).map_err(err)?;

    // CONSTRUCT/DESCHRIBE asked for as text: emit N-Triples, which is a valid
    // subset of Turtle. The browser build has a prefix-folding writer in the
    // wasm crate; duplicating it here would be a second implementation to keep
    // honest for a cosmetic gain, so the desktop Turtle tab shows fully
    // expanded triples.
    if let QueryOutput::Construct(triples) = &out {
        if format == "ttl" || format == "turtle" {
            let text = construct_turtle(triples)?;
            let mut json = String::from(r#"{"kind":"construct","format":"ttl","text":"#);
            rete_core::push_json_string(&mut json, &text);
            json.push_str(&format!(r#","schemaVersion":{JSON_SCHEMA_VERSION}}}"#));
            return Ok(json);
        }
    }

    Ok(rete_core::results_envelope_json(
        &out,
        &format!(r#","schemaVersion":{JSON_SCHEMA_VERSION}"#),
    ))
}

/// CONSTRUCT triples as Turtle text — one expanded N-Triples line each, which is
/// valid Turtle — with every quoted triple written as the **RDF 1.2 triple
/// term** `<<( s p o )>>`, as `rete export` does by default.
///
/// The stored token is the RDF-star `<<s p o>>`, and in Turtle that spelling
/// means something else to an RDF 1.2 parser: a *reifier*, so pasting the
/// verbatim text into one yields a different graph (one statement becomes two,
/// with a blank node where the triple term was). RDF 1.2 allows a triple term in
/// object position only, so a quoted triple in a subject/predicate slot (or in
/// another's subject) has no Turtle 1.2 spelling and is refused by name rather
/// than written as a reifier.
fn construct_turtle(triples: &[(String, String, String)]) -> Result<String, String> {
    use rete_core::terms::{is_quoted_triple, rdf12_triple_term};
    let mut text = String::new();
    for (s, p, o) in triples {
        if is_quoted_triple(s) || is_quoted_triple(p) {
            return Err(format!(
                "this result has a quoted triple in the subject or predicate of a statement, \
                 which RDF 1.2 Turtle cannot write (a triple term stands in object position \
                 only): {s} {p} {o} . — read it from the table view instead"
            ));
        }
        let o = rdf12_triple_term(o).ok_or_else(|| {
            format!(
                "this result has a quoted triple nested in the subject of another, which RDF \
                 1.2 Turtle cannot write: {s} {p} {o} . — read it from the table view instead"
            )
        })?;
        text.push_str(s);
        text.push(' ');
        text.push_str(p);
        text.push(' ');
        text.push_str(&o);
        text.push_str(" .\n");
    }
    Ok(text)
}

/// Label prefix search, served from the pyramid's label index. Empty when the
/// file carries no pyramid.
#[tauri::command]
fn prefix_search(
    state: tauri::State<'_, AppState>,
    prefix: String,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let guard = state.opened.lock().unwrap();
    let opened = guard.as_ref().ok_or("no archive open")?;
    Ok(opened
        .rete
        .prefix_search(&prefix, limit)
        .into_iter()
        .map(|(label, subject)| serde_json::json!({ "label": label, "subject": subject }))
        .collect())
}

/// Full-text (word / CONTAINS) search over the TEXT_INDEX section. Empty when
/// the file was built without `--text-index`; the caller falls back to prefix.
#[tauri::command]
fn text_search(
    state: tauri::State<'_, AppState>,
    words: Vec<String>,
    contains_prefix: Option<String>,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let guard = state.opened.lock().unwrap();
    let opened = guard.as_ref().ok_or("no archive open")?;
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    Ok(opened
        .rete
        .text_search(&refs, contains_prefix.as_deref(), limit)
        .into_iter()
        .map(|subject| serde_json::json!({ "subject": subject }))
        .collect())
}

#[tauri::command]
fn graph_stats(state: tauri::State<'_, AppState>) -> Result<Stats, String> {
    let guard = state.opened.lock().unwrap();
    let opened = guard.as_ref().ok_or("no archive open")?;
    Ok(Stats {
        file_length: opened.reader.len(),
        bytes: opened.reader.bytes_read(),
        requests: opened.reader.requests(),
    })
}

/// Fit the window to the screen it opens on.
///
/// The configured default (1280×840) is bigger than plenty of real laptops once
/// scaling is taken into account, and a window larger than the display opens
/// with its edges off-screen — you cannot reach the corner to resize it, so the
/// app looks like it needs scrolling to be usable. Clamp to 92% of the monitor's
/// work area and centre.
fn fit_to_monitor(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()? else {
        return Ok(()); // headless or an unknown display: keep the configured size
    };
    let scale = monitor.scale_factor();
    let screen = monitor.size().to_logical::<f64>(scale);
    let current = window.outer_size()?.to_logical::<f64>(scale);

    let w = current.width.min(screen.width * 0.92);
    let h = current.height.min(screen.height * 0.92);
    if w < current.width || h < current.height {
        window.set_size(tauri::LogicalSize::new(w, h))?;
    }
    window.center()?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(AppState::default());
            if let Some(window) = app.get_webview_window("main") {
                // Best-effort: a sizing failure must not stop the app opening.
                if let Err(e) = fit_to_monitor(&window) {
                    eprintln!("could not fit the window to the monitor: {e}");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_graph,
            run_query,
            prefix_search,
            text_search,
            graph_stats
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Rete File Explorer");
}

#[cfg(test)]
mod tests {
    use super::construct_turtle;

    fn t(s: &str, p: &str, o: &str) -> (String, String, String) {
        (s.into(), p.into(), o.into())
    }

    #[test]
    fn plain_triples_are_written_as_they_are() {
        let out = construct_turtle(&[t("<http://ex/a>", "<http://ex/p>", "\"x\"@en")]).unwrap();
        assert_eq!(out, "<http://ex/a> <http://ex/p> \"x\"@en .\n");
    }

    #[test]
    fn a_quoted_object_becomes_an_rdf12_triple_term() {
        let nested =
            "<<<http://ex/c> <http://ex/s> <<<http://ex/a> <http://ex/p> <http://ex/b>>>>>";
        let out = construct_turtle(&[t("<http://ex/m>", "<http://ex/about>", nested)]).unwrap();
        assert_eq!(
            out,
            "<http://ex/m> <http://ex/about> <<( <http://ex/c> <http://ex/s> \
             <<( <http://ex/a> <http://ex/p> <http://ex/b> )>> )>> .\n"
        );
    }

    #[test]
    fn a_quoted_subject_is_refused_rather_than_written_as_a_reifier() {
        let quoted = "<<<http://ex/a> <http://ex/p> <http://ex/b>>>";
        let e = construct_turtle(&[t(quoted, "<http://ex/src>", "<http://ex/wiki>")]).unwrap_err();
        assert!(e.contains("object position only"), "{e}");
    }

    #[test]
    fn the_output_parses_as_rdf12_turtle_and_is_the_same_graph() {
        // Round trip through rete's own RDF 1.2 Turtle reader: the triple term
        // comes back as the one stored token it started as.
        let token = "<<<http://ex/a> <http://ex/p> <http://ex/b>>>";
        let out = construct_turtle(&[t("<http://ex/c>", "<http://ex/states>", token)]).unwrap();
        let quads = rete_core::ingest::parse_statements_audited_surface(
            &out,
            "ttl",
            None,
            rete_core::ingest::QuotedTripleSurface::Rdf12,
        )
        .unwrap();
        assert_eq!(quads.len(), 1, "a triple term, not a reifier: {quads:?}");
        assert_eq!(quads[0].2, token);
    }
}
