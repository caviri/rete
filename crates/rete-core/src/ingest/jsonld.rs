//! JSON-LD and YAML-LD readers.
//!
//! JSON-LD (`"jsonld"`) goes through `oxjsonld`. YAML-LD (`"yamlld"`, the basic
//! profile) is the same document written in YAML: it is turned into JSON values
//! first and then read as JSON-LD, so the two give the same quads.
//!
//! A JSON-LD document is read whole, not line by line: the context at the top
//! decides what every later key means, and a document can only be expanded once
//! it has been seen. Nothing here streams from a reader.
//!
//! **No I/O.** A document that names a remote `@context` gets it from a
//! [`ContextLoader`] the caller supplies; with none, the parse fails with
//! [`IngestError::JsonLdRemoteContext`] naming the IRI. That keeps the core free
//! of sockets and files (the wasm engine compiles it) and a build offline and
//! repeatable unless the caller opts in.
//!
//! Named graphs (`@graph` beside an `@id`) come out as named graphs. Relative
//! IRIs are resolved against no base, as in the Turtle reader (it has none
//! either); JSON-LD drops a relative `@id` it cannot resolve. A blank-node label
//! written in a document (`_:b0`) is renamed per document, as `oxttl` does for
//! Turtle, so two files that both use `_:b0` do not merge their nodes.

#[cfg(not(feature = "jsonld"))]
use super::{IngestError, IriAudit, RawQuad};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Arc;

/// Supplies the text of a remote `@context` (or any remote document the
/// processor asks for) by IRI. The core never opens a socket or a file; the
/// caller decides where contexts come from.
///
/// The returned bytes are a JSON document (`{"@context": …}`); a YAML one is
/// accepted too and converted.
pub trait ContextLoader: Send + Sync + UnwindSafe + RefUnwindSafe {
    /// The document at `iri`; `Ok(None)` if this loader has nothing for it
    /// (the parse then fails with [`IngestError::JsonLdRemoteContext`]), `Err`
    /// if it should have and could not.
    fn load(&self, iri: &str) -> Result<Option<Vec<u8>>, String>;
}

impl<F> ContextLoader for F
where
    F: Fn(&str) -> Result<Option<Vec<u8>>, String> + Send + Sync + UnwindSafe + RefUnwindSafe,
{
    fn load(&self, iri: &str) -> Result<Option<Vec<u8>>, String> {
        self(iri)
    }
}

/// How a JSON-LD / YAML-LD document is read. The default reads documents with
/// inline contexts only.
#[derive(Clone, Default)]
pub struct JsonLdOptions {
    /// Where remote contexts come from. `None`: nowhere, so a document that
    /// needs one fails with [`IngestError::JsonLdRemoteContext`].
    pub context_loader: Option<Arc<dyn ContextLoader>>,
}

impl JsonLdOptions {
    /// Options that resolve remote contexts through `loader`.
    pub fn with_loader(loader: impl ContextLoader + 'static) -> Self {
        JsonLdOptions {
            context_loader: Some(Arc::new(loader)),
        }
    }
}

#[cfg(feature = "jsonld")]
pub(super) use imp::parse;

#[cfg(not(feature = "jsonld"))]
pub(super) fn parse(
    _text: &str,
    _yaml: bool,
    _options: &JsonLdOptions,
    _audit: Option<&mut IriAudit>,
    _f: &mut dyn FnMut(RawQuad),
) -> Result<(), IngestError> {
    Err(IngestError::JsonLd(
        "this build of rete-core was compiled without the `jsonld` feature".into(),
    ))
}

#[cfg(feature = "jsonld")]
mod imp {
    use super::{ContextLoader, JsonLdOptions};
    use crate::ingest::{audit_quad, IngestError, IriAudit, RawQuad};
    use oxjsonld::{JsonLdParser, JsonLdRemoteDocument};
    use oxrdf12::{BlankNode, GraphName, NamedOrBlankNode, Term};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// YAML text to the JSON bytes of the same document.
    fn yaml_to_json(text: &str) -> Result<Vec<u8>, String> {
        let value: serde_json::Value = yaml_serde::from_str(text).map_err(|e| e.to_string())?;
        serde_json::to_vec(&value).map_err(|e| e.to_string())
    }

    /// A loaded context as JSON bytes: JSON passes through, anything else is
    /// tried as YAML.
    fn as_json(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
        if serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).is_ok() {
            return Ok(bytes);
        }
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        yaml_to_json(&text)
    }

    pub(in crate::ingest) fn parse(
        text: &str,
        yaml: bool,
        options: &JsonLdOptions,
        mut audit: Option<&mut IriAudit>,
        f: &mut dyn FnMut(RawQuad),
    ) -> Result<(), IngestError> {
        let name = if yaml { "yaml-ld" } else { "json-ld" };
        let converted;
        let bytes: &[u8] = if yaml {
            converted =
                yaml_to_json(text).map_err(|e| IngestError::JsonLd(format!("{name}: {e}")))?;
            &converted
        } else {
            text.as_bytes()
        };

        // The first remote document the processor asked for and could not get:
        // the parse error alone ("loading remote context failed") would not say
        // which one, or that there is a way out.
        let missing: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let loader: Option<Arc<dyn ContextLoader>> = options.context_loader.clone();
        let seen = missing.clone();
        let parser = JsonLdParser::new()
            .for_slice(bytes)
            .with_load_document_callback(move |iri, _opts| {
                let found = match &loader {
                    Some(loader) => loader.load(iri).and_then(|d| d.map(as_json).transpose()),
                    None => Ok(None),
                };
                match found {
                    Ok(Some(document)) => Ok(JsonLdRemoteDocument {
                        document,
                        document_url: iri.to_string(),
                    }),
                    Ok(None) => {
                        *seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(iri.to_string());
                        Err("no context loaded".into())
                    }
                    Err(e) => Err(format!("{iri}: {e}").into()),
                }
            });

        // Labels written in the document get a fresh identity per document.
        let mut labels: HashMap<String, BlankNode> = HashMap::new();
        let mut fresh = |b: &BlankNode| -> BlankNode {
            labels.entry(b.as_str().to_string()).or_default().clone()
        };
        for r in parser {
            let q = match r {
                Ok(q) => q,
                Err(e) => {
                    let remote = missing.lock().unwrap_or_else(|e| e.into_inner()).take();
                    return Err(match remote {
                        Some(iri) => IngestError::JsonLdRemoteContext(iri),
                        None => IngestError::JsonLd(format!("{name}: {e}")),
                    });
                }
            };
            let subject = match &q.subject {
                NamedOrBlankNode::BlankNode(b) => NamedOrBlankNode::BlankNode(fresh(b)),
                other => other.clone(),
            }
            .to_string();
            let object = match &q.object {
                Term::BlankNode(b) => Term::BlankNode(fresh(b)),
                other => other.clone(),
            }
            .to_string();
            let graph = match &q.graph_name {
                GraphName::DefaultGraph => None,
                GraphName::BlankNode(b) => Some(fresh(b).to_string()),
                other => Some(other.to_string()),
            };
            let q = (subject, q.predicate.to_string(), object, graph);
            audit_quad(
                audit.as_deref_mut(),
                &q.0,
                &q.1,
                &q.2,
                q.3.as_deref(),
                || name.into(),
            )?;
            f(q);
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "jsonld"))]
mod tests {
    use super::*;
    use crate::ingest::{parse_jsonld_audited, parse_statements, IngestError, RawQuad};

    const JSON: &str = r#"{
      "@context": {
        "ex": "http://ex/",
        "name": "ex:name",
        "knows": {"@id": "ex:knows", "@type": "@id"},
        "age": {"@id": "ex:age", "@type": "http://www.w3.org/2001/XMLSchema#integer"},
        "label": {"@id": "ex:label", "@language": "en"},
        "items": {"@id": "ex:items", "@container": "@list"}
      },
      "@graph": [
        {"@id": "ex:alice", "name": "Alice", "age": "42", "label": "Alice",
         "knows": {"@id": "ex:bob", "name": "Bob"},
         "items": ["a", "b"],
         "ex:friend": {"name": "anon"}},
        {"@id": "ex:g1", "@graph": [{"@id": "ex:carol", "name": "Carol"}]}
      ]
    }"#;

    const YAML: &str = r#"
"@context":
  ex: http://ex/
  name: ex:name
  knows: {"@id": "ex:knows", "@type": "@id"}
  age: {"@id": "ex:age", "@type": "http://www.w3.org/2001/XMLSchema#integer"}
  label: {"@id": "ex:label", "@language": en}
  items: {"@id": "ex:items", "@container": "@list"}
"@graph":
  - "@id": ex:alice
    name: Alice
    age: "42"
    label: Alice
    knows:
      "@id": ex:bob
      name: Bob
    items: [a, b]
    "ex:friend":
      name: anon
  - "@id": ex:g1
    "@graph":
      - "@id": ex:carol
        name: Carol
"#;

    fn read(text: &str, yaml: bool) -> Vec<RawQuad> {
        parse_jsonld_audited(text, yaml, &JsonLdOptions::default(), None).unwrap()
    }

    /// The quads as sorted lines with every blank node reduced to `_:b`: blank
    /// node labels are fresh on each read, so only their shape can be compared.
    fn shape(quads: &[RawQuad]) -> Vec<String> {
        let anon = |t: &str| {
            if t.starts_with("_:") {
                "_:b".to_string()
            } else {
                t.to_string()
            }
        };
        let mut v: Vec<String> = quads
            .iter()
            .map(|(s, p, o, g)| {
                format!(
                    "{} {} {} {}",
                    anon(s),
                    p,
                    anon(o),
                    g.as_deref().map_or(String::new(), anon)
                )
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn json_ld_reads_nodes_literals_lists_and_named_graphs() {
        let q = read(JSON, false);
        assert_eq!(q.len(), 13, "{q:#?}");
        let has = |s: &str, p: &str, o: &str, g: Option<&str>| {
            q.iter()
                .any(|(qs, qp, qo, qg)| qs == s && qp == p && qo == o && qg.as_deref() == g)
        };
        assert!(has(
            "<http://ex/alice>",
            "<http://ex/name>",
            "\"Alice\"",
            None
        ));
        assert!(has(
            "<http://ex/alice>",
            "<http://ex/age>",
            "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            None
        ));
        assert!(has(
            "<http://ex/alice>",
            "<http://ex/label>",
            "\"Alice\"@en",
            None
        ));
        assert!(has(
            "<http://ex/alice>",
            "<http://ex/knows>",
            "<http://ex/bob>",
            None
        ));
        assert!(has(
            "<http://ex/carol>",
            "<http://ex/name>",
            "\"Carol\"",
            Some("<http://ex/g1>")
        ));
        // The list is a chain of blank nodes ending in rdf:nil.
        let rdf = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
        let firsts = q.iter().filter(|x| x.1 == format!("<{rdf}first>")).count();
        let nil = q.iter().filter(|x| x.2 == format!("<{rdf}nil>")).count();
        assert_eq!((firsts, nil), (2, 1));
        // The anonymous node has its own name triple.
        assert!(q
            .iter()
            .any(|x| x.0.starts_with("_:") && x.2 == "\"anon\"" && x.3.is_none()));
    }

    #[test]
    fn yaml_ld_gives_the_same_quads_as_json_ld() {
        assert_eq!(shape(&read(YAML, true)), shape(&read(JSON, false)));
    }

    #[test]
    fn the_text_entry_point_reads_both_formats() {
        assert_eq!(parse_statements(JSON, "jsonld").unwrap().len(), 13);
        assert_eq!(parse_statements(YAML, "yamlld").unwrap().len(), 13);
    }

    #[test]
    fn malformed_input_is_a_named_error() {
        let e = parse_statements("{\"@context\": ", "jsonld").unwrap_err();
        assert!(matches!(e, IngestError::JsonLd(_)), "{e}");
        let e = parse_statements("a: [1, 2", "yamlld").unwrap_err();
        assert!(e.to_string().starts_with("yaml-ld:"), "{e}");
    }

    #[test]
    fn a_blank_node_label_does_not_survive_into_another_document() {
        let doc = r#"{"@context": {"ex": "http://ex/"}, "@id": "_:b0", "ex:p": "x"}"#;
        let (a, b) = (read(doc, false), read(doc, false));
        assert_ne!(a[0].0, b[0].0);
        // Within one document the label still names one node.
        let two = r#"{"@context": {"ex": "http://ex/"}, "@graph": [
            {"@id": "_:n", "ex:p": "x"}, {"@id": "ex:s", "ex:q": {"@id": "_:n"}}]}"#;
        let q = read(two, false);
        let n = q.iter().find(|x| x.2 == "\"x\"").unwrap().0.clone();
        assert!(q.iter().any(|x| x.2 == n));
    }

    const REMOTE: &str =
        r#"{"@context": "http://remote.example/ctx", "@id": "http://ex/a", "name": "x"}"#;

    #[test]
    fn a_remote_context_without_a_loader_names_the_iri() {
        let e = parse_jsonld_audited(REMOTE, false, &JsonLdOptions::default(), None).unwrap_err();
        match &e {
            IngestError::JsonLdRemoteContext(iri) => assert_eq!(iri, "http://remote.example/ctx"),
            other => panic!("wrong error: {other}"),
        }
        assert!(e.to_string().contains("http://remote.example/ctx"));
    }

    #[test]
    fn a_remote_context_resolves_through_the_loader() {
        let json = JsonLdOptions::with_loader(|iri: &str| {
            assert_eq!(iri, "http://remote.example/ctx");
            Ok(Some(
                br#"{"@context": {"name": "http://ex/name"}}"#.to_vec(),
            ))
        });
        let q = parse_jsonld_audited(REMOTE, false, &json, None).unwrap();
        assert_eq!(
            q,
            vec![(
                "<http://ex/a>".to_string(),
                "<http://ex/name>".to_string(),
                "\"x\"".to_string(),
                None
            )]
        );
        // A context written in YAML is accepted too.
        let yaml = JsonLdOptions::with_loader(|_: &str| {
            Ok(Some(b"\"@context\":\n  name: http://ex/name\n".to_vec()))
        });
        assert_eq!(parse_jsonld_audited(REMOTE, false, &yaml, None).unwrap(), q);
        // A loader that fails: the error says which context.
        let bad = JsonLdOptions::with_loader(|_: &str| Err("no such file".to_string()));
        let e = parse_jsonld_audited(REMOTE, false, &bad, None).unwrap_err();
        assert!(matches!(e, IngestError::JsonLd(_)), "{e}");
        // A loader that has nothing for the IRI is the same as no loader.
        let none = JsonLdOptions::with_loader(|_: &str| Ok(None));
        let e = parse_jsonld_audited(REMOTE, false, &none, None).unwrap_err();
        assert!(matches!(e, IngestError::JsonLdRemoteContext(_)), "{e}");
    }

    #[test]
    fn a_stream_reader_refuses_json_ld_by_name() {
        let e = crate::ingest::stream_reader("{}".as_bytes(), "jsonld", &mut |_| {}).unwrap_err();
        assert!(e.to_string().contains("cannot be streamed"), "{e}");
    }
}
