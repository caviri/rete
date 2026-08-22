//! RDF projections of the Dataset Card — JSON-LD (VoID + schema.org + PROV-O)
//! and a Croissant subset.
//!
//! The card is **plain JSON at rest** and stays that way: the metadata section
//! is fetched by every reader on every open, and a measurement on the two
//! reference cards showed the faithful JSON-LD form costs materially more (the
//! VoID partitions need one object per row where the card stores a two-element
//! array — see `docs/dataset-cards.md` for the numbers). So the RDF view is a
//! **pure projection** — `rete card --format jsonld` reshapes bytes already
//! fetched, no extra network, no index read — and drift is impossible because
//! the projection has no second artefact: it is derived from the stored card on
//! every call.
//!
//! Vocabulary policy (issue #153): map onto **VoID** before inventing terms
//! (`void:triples`, `void:vocabulary`, `void:propertyPartition`,
//! `void:classPartition`, `void:sparqlEndpoint`, `void:dataDump`), **PROV-O**
//! for origin (`prov:wasDerivedFrom`, `prov:wasGeneratedBy`), **schema.org**
//! for the descriptive header Croissant shares, and a small `rete:` namespace
//! only for what no standard covers (counts of terms/named graphs, the content
//! hash, profile caps).
//!
//! Croissant is projected **honestly**: the descriptive header, licence,
//! creators and distribution map faithfully; `recordSet` does not — Croissant
//! models tables (`recordSet` → `field` → `dataType`) and an RDF graph has no
//! records — so the Croissant projection carries **no recordSet** rather than a
//! fabricated one, and no `sha256` (the format's integrity hash is blake3-16,
//! which Croissant has no slot for; it is published as `rete:contentHash`).

use serde_json::{json, Map, Value};

use super::buildinfo::BuildInfo;
use super::card::DatasetCard;

/// The `rete:` vocabulary IRI for card terms no standard vocabulary covers.
pub(crate) const RETE_NS: &str = "https://w3id.org/rete/card#";

/// Strip the N-Triples brackets from a stored IRI term (`<http://x>` → `http://x`).
fn iri(term: &str) -> &str {
    term.strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(term)
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Percent-encode a custom-field key so `rete:extra/<key>` always expands to a
/// **valid IRI**. `RETE_NS` ends in `#`, so the key lands in the fragment;
/// RFC 3987 fragment characters (alphanumerics, `-._~!$&'()*+,;=:@/?`) pass
/// through, everything else — spaces, quotes, `#`, `%`, non-ASCII — is
/// percent-encoded byte-wise. Injective, so distinct keys keep distinct IRIs.
fn fragment_safe(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for b in key.bytes() {
        let ok = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
                    | b'?'
            );
        if ok {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Insert `key: value` unless the value is `Null` / empty array / empty string.
fn put(obj: &mut Map<String, Value>, key: &str, value: Value) {
    let empty = match &value {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::String(s) => s.is_empty(),
        _ => false,
    };
    if !empty {
        obj.insert(key.to_string(), value);
    }
}

fn opt(v: &Option<String>) -> Value {
    v.as_ref().map(|s| json!(s)).unwrap_or(Value::Null)
}

/// A value that the context types as `@id` when it is a URL, else a literal.
fn id_or_text(s: &str) -> Value {
    if is_url(s) {
        json!({ "@id": s })
    } else {
        json!(s)
    }
}

/// The dataset's IRI: the curated canonical URL wins; else the source the
/// command was pointed at, when that is itself a URL (`card-url`); else none
/// (the dataset is a blank node — still valid JSON-LD).
fn dataset_id(card: &DatasetCard, source: &str) -> Option<String> {
    card.canonical_url
        .clone()
        .or_else(|| is_url(source).then(|| source.to_string()))
}

/// Project a card (+ optional build info) to one JSON-LD document typed both
/// `schema:Dataset` and `void:Dataset` — one dataset described in two
/// vocabularies, not two objects.
pub(crate) fn to_jsonld(
    card: &DatasetCard,
    build: Option<&BuildInfo>,
    checksum: &str,
    source: &str,
) -> Value {
    let mut d = Map::new();
    d.insert(
        "@context".into(),
        json!({
            "@vocab": "https://schema.org/",
            "void": "http://rdfs.org/ns/void#",
            "dcat": "http://www.w3.org/ns/dcat#",
            "dct": "http://purl.org/dc/terms/",
            "prov": "http://www.w3.org/ns/prov#",
            "xsd": "http://www.w3.org/2001/XMLSchema#",
            "rete": RETE_NS,
            "void:vocabulary": { "@type": "@id" },
            "dcat:theme": { "@type": "@id" },
            "void:sparqlEndpoint": { "@type": "@id" },
            "void:dataDump": { "@type": "@id" },
            "void:property": { "@type": "@id" },
            "void:class": { "@type": "@id" },
            "prov:wasDerivedFrom": { "@type": "@id" },
            "prov:endedAtTime": { "@type": "xsd:dateTime" }
        }),
    );
    if let Some(id) = dataset_id(card, source) {
        d.insert("@id".into(), json!(id));
    }
    d.insert("@type".into(), json!(["Dataset", "void:Dataset"]));

    // --- Descriptive header (schema.org — the part Croissant shares). ---
    put(&mut d, "name", opt(&card.title));
    put(&mut d, "description", opt(&card.description));
    if let Some(l) = &card.license {
        d.insert("license".into(), id_or_text(l));
    }
    put(&mut d, "version", opt(&card.version));
    put(&mut d, "dateCreated", opt(&card.created));
    put(&mut d, "citation", opt(&card.cite_as));
    // Keywords under BOTH standard terms — the same dual-vocabulary stance as
    // the schema:Dataset + void:Dataset typing: `schema:keywords` for
    // dataset-search harvesters, `dcat:keyword` for DCAT catalogs. One list,
    // two agreed names; a keyword in the `extra` bag would have had neither.
    let kw: Vec<Value> = card.keywords.iter().map(|k| json!(k)).collect();
    put(&mut d, "keywords", Value::Array(kw.clone()));
    put(&mut d, "dcat:keyword", Value::Array(kw));
    // Themes are IRIs into a controlled vocabulary, typed @id in the context —
    // exactly one standard term (`dcat:theme`), so no schema.org double.
    put(
        &mut d,
        "dcat:theme",
        Value::Array(card.theme.iter().map(|t| json!(t)).collect()),
    );
    // No curated language projects here: in RDF the language rides on each
    // literal, so the card DERIVES it — `signals.default_lang` fills
    // `schema:inLanguage` below, measured, never declared.
    if let Some(doi) = &card.doi {
        d.insert("identifier".into(), json!({ "@id": doi }));
    }
    let creators: Vec<Value> = card
        .creators
        .iter()
        .map(|c| {
            let mut p = Map::new();
            p.insert("@type".into(), json!("Person"));
            p.insert("name".into(), json!(c.name));
            if let Some(orcid) = &c.orcid {
                p.insert("@id".into(), json!(orcid));
            }
            Value::Object(p)
        })
        .collect();
    put(&mut d, "creator", Value::Array(creators));
    if let Some(p) = &card.publisher {
        let mut o = Map::new();
        o.insert("@type".into(), json!("Organization"));
        o.insert("name".into(), json!(p.name));
        if let Some(ror) = &p.ror {
            o.insert("@id".into(), json!(ror));
        }
        d.insert("publisher".into(), Value::Object(o));
    }
    if let Some(lang) = &card.signals.default_lang {
        d.insert("inLanguage".into(), json!(lang));
    }
    if let Some((from, to)) = &card.signals.temporal_extent {
        d.insert("temporalCoverage".into(), json!(format!("{from}/{to}")));
    }
    if let Some([min_lon, min_lat, max_lon, max_lat]) = card.signals.spatial_bbox {
        d.insert(
            "spatialCoverage".into(),
            json!({
                "@type": "Place",
                "geo": {
                    "@type": "GeoShape",
                    // schema:box is "minLat,minLon maxLat,maxLon".
                    "box": format!("{min_lat},{min_lon} {max_lat},{max_lon}")
                }
            }),
        );
    }
    if let Some(url) = &card.canonical_url {
        d.insert(
            "distribution".into(),
            json!([{
                "@type": "DataDownload",
                "contentUrl": url,
                "encodingFormat": "application/x-rete",
                "rete:contentHash": format!("blake3-16:{checksum}")
            }]),
        );
    } else {
        d.insert(
            "rete:contentHash".into(),
            json!(format!("blake3-16:{checksum}")),
        );
    }

    // --- The graph, in VoID: the native fit. ---
    // void:triples is the WHOLE dataset (default graph + named graphs).
    d.insert("void:triples".into(), json!(card.quad_count));
    put(&mut d, "void:sparqlEndpoint", opt(&card.sparql_endpoint));
    put(&mut d, "void:dataDump", opt(&card.canonical_url));
    put(
        &mut d,
        "void:vocabulary",
        Value::Array(card.vocabularies.iter().map(|v| json!(v)).collect()),
    );
    put(
        &mut d,
        "void:propertyPartition",
        Value::Array(
            card.predicates
                .iter()
                .map(|(p, n)| json!({ "void:property": iri(p), "void:triples": n }))
                .collect(),
        ),
    );
    put(
        &mut d,
        "void:classPartition",
        Value::Array(
            card.classes
                .iter()
                .map(|(c, n)| json!({ "void:class": iri(c), "void:entities": n }))
                .collect(),
        ),
    );

    // --- What no standard vocabulary covers (kept in rete:, not bent terms). ---
    d.insert("rete:defaultGraphTriples".into(), json!(card.triple_count));
    if card.named_graph_count > 0 {
        d.insert("rete:namedGraphs".into(), json!(card.named_graph_count));
    }
    d.insert("rete:terms".into(), json!(card.term_count));
    if card.truncated {
        d.insert("rete:truncated".into(), json!(true));
        if card.top_n > 0 {
            d.insert("rete:partitionTopN".into(), json!(card.top_n));
        }
    }
    // Publisher-defined custom fields, each under `rete:extra/<key>` — kept in
    // the projection (omitting them would make it unfaithful: a consumer
    // diffing card against projection would find fields missing with no
    // explanation), but as **opaque values, not vocabulary**: the IRI means
    // "the publisher-defined field named <key>", whose semantics are private
    // to this card's publisher. Scalars project as plain literals; container
    // values (objects/arrays) are typed `@json` in the context so they become
    // JSON-LD 1.1 JSON literals (`rdf:JSON`) instead of blank-node structures
    // pretending to be modelled data. The consumer gets the values — never
    // their meaning.
    for (key, value) in &card.extra {
        let term = format!("rete:extra/{}", fragment_safe(key));
        if value.is_object() || value.is_array() {
            d.get_mut("@context")
                .expect("context inserted above")
                .as_object_mut()
                .expect("context is an object")
                .insert(term.clone(), json!({ "@type": "@json" }));
        }
        d.insert(term, value.clone());
    }

    // --- Provenance (PROV-O). ---
    let mut derived: Vec<Value> = Vec::new();
    if let Some(src) = &card.source {
        if is_url(src) {
            derived.push(json!(src));
        } else {
            d.insert("dct:source".into(), json!(src));
        }
    }
    for s in &card.derived_from {
        if is_url(s) {
            derived.push(json!(s));
        } else {
            d.entry("dct:source".to_string())
                .and_modify(|v| {
                    let prev = v.take();
                    *v = match prev {
                        Value::Array(mut a) => {
                            a.push(json!(s));
                            Value::Array(a)
                        }
                        other => json!([other, s]),
                    };
                })
                .or_insert_with(|| json!(s));
        }
    }
    put(&mut d, "prov:wasDerivedFrom", Value::Array(derived));
    put(&mut d, "rete:sourceDate", opt(&card.source_date));
    if let Some(b) = build {
        let mut act = Map::new();
        act.insert("@type".into(), json!("prov:Activity"));
        if let Some(t) = &b.built_at {
            act.insert("prov:endedAtTime".into(), json!(t));
        }
        if let Some(builder) = &b.builder {
            let (name, version) = builder.split_once(' ').unwrap_or((builder.as_str(), ""));
            let mut agent = Map::new();
            agent.insert("@type".into(), json!("SoftwareApplication"));
            agent.insert("name".into(), json!(name));
            if !version.is_empty() {
                agent.insert("softwareVersion".into(), json!(version));
            }
            act.insert("prov:wasAssociatedWith".into(), Value::Object(agent));
        }
        d.insert("prov:wasGeneratedBy".into(), Value::Object(act));
    }

    Value::Object(d)
}

/// Project the **honestly-mappable Croissant subset**: descriptive header,
/// licence, creators/publisher, provenance, and the `.rete` file as a
/// `cr:FileObject` distribution. Deliberately absent: `recordSet` (Croissant
/// models tables; an RDF graph has no records — an empty or fabricated record
/// set would validate and mislead). The graph-shaped facts (partitions,
/// counts) live in the VoID projection, where they have standard terms.
///
/// `sha256`: Croissant requires an md5/sha256 on every `FileObject`, and a
/// self-contained file **cannot carry its own whole-file sha256** (the hash
/// would change the bytes being hashed). The format's integrity hash
/// (blake3-16 over the payload sections) is published as `rete:contentHash`;
/// pass the file's sha256 — known only *outside* the file — via
/// `--sha256` to emit a fully validator-clean document. Without it,
/// `mlcroissant` reports exactly that one missing property.
pub(crate) fn to_croissant(
    card: &DatasetCard,
    checksum: &str,
    source: &str,
    sha256: Option<&str>,
) -> Value {
    let mut d = Map::new();
    // The standard Croissant 1.0 context, verbatim from the spec.
    d.insert(
        "@context".into(),
        json!({
            "@language": "en",
            "@vocab": "https://schema.org/",
            "citeAs": "cr:citeAs",
            "column": "cr:column",
            "conformsTo": "dct:conformsTo",
            "cr": "http://mlcommons.org/croissant/",
            "rai": "http://mlcommons.org/croissant/RAI/",
            "data": { "@id": "cr:data", "@type": "@json" },
            "dataType": { "@id": "cr:dataType", "@type": "@vocab" },
            "dct": "http://purl.org/dc/terms/",
            "equivalentProperty": "cr:equivalentProperty",
            "examples": { "@id": "cr:examples", "@type": "@json" },
            "extract": "cr:extract",
            "field": "cr:field",
            "fileProperty": "cr:fileProperty",
            "fileObject": "cr:fileObject",
            "fileSet": "cr:fileSet",
            "format": "cr:format",
            "includes": "cr:includes",
            "isLiveDataset": "cr:isLiveDataset",
            "jsonPath": "cr:jsonPath",
            "key": "cr:key",
            "md5": "cr:md5",
            "parentField": "cr:parentField",
            "path": "cr:path",
            "recordSet": "cr:recordSet",
            "references": "cr:references",
            "regex": "cr:regex",
            "repeated": "cr:repeated",
            "replace": "cr:replace",
            "samplingRate": "cr:samplingRate",
            "sc": "https://schema.org/",
            "separator": "cr:separator",
            "source": "cr:source",
            "subField": "cr:subField",
            "transform": "cr:transform",
            "rete": RETE_NS
        }),
    );
    d.insert("@type".into(), json!("sc:Dataset"));
    d.insert(
        "conformsTo".into(),
        json!("http://mlcommons.org/croissant/1.0"),
    );
    // Croissant requires a name; fall back to the file/URL basename.
    let fallback_name = source
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("dataset")
        .to_string();
    d.insert(
        "name".into(),
        json!(card.title.clone().unwrap_or(fallback_name)),
    );
    put(&mut d, "description", opt(&card.description));
    if let Some(l) = &card.license {
        // mlcroissant wants a plain Text/URL value here, not an @id node.
        d.insert("license".into(), json!(l));
    }
    put(&mut d, "version", opt(&card.version));
    // Keywords have a home here: Croissant's descriptive header is
    // schema.org, and `keywords` is part of it (unlike the `extra` bag,
    // which is omitted — and unlike `theme`, whose only term is `dcat:theme`:
    // it stays in the JSON-LD projection rather than being bent into a
    // schema.org shape here).
    put(
        &mut d,
        "keywords",
        Value::Array(card.keywords.iter().map(|k| json!(k)).collect()),
    );
    // datePublished is recommended by the validator; the curated creation date
    // is the closest truthful value the card holds.
    put(&mut d, "datePublished", opt(&card.created));
    put(&mut d, "citeAs", opt(&card.cite_as));
    if let Some(u) = dataset_id(card, source) {
        d.insert("url".into(), json!(u));
    }
    let creators: Vec<Value> = card
        .creators
        .iter()
        .map(|c| {
            let mut p = Map::new();
            p.insert("@type".into(), json!("sc:Person"));
            p.insert("name".into(), json!(c.name));
            if let Some(orcid) = &c.orcid {
                p.insert("@id".into(), json!(orcid));
            }
            Value::Object(p)
        })
        .collect();
    put(&mut d, "creator", Value::Array(creators));
    if let Some(p) = &card.publisher {
        let mut o = Map::new();
        o.insert("@type".into(), json!("sc:Organization"));
        o.insert("name".into(), json!(p.name));
        if let Some(ror) = &p.ror {
            o.insert("@id".into(), json!(ror));
        }
        d.insert("publisher".into(), Value::Object(o));
    }
    if let Some(url) = &card.canonical_url {
        let mut file = Map::new();
        file.insert("@type".into(), json!("cr:FileObject"));
        file.insert("@id".into(), json!("rete-file"));
        file.insert("name".into(), json!("rete-file"));
        file.insert(
            "description".into(),
            json!("The single-file range-queryable RDF graph (.rete)."),
        );
        file.insert("contentUrl".into(), json!(url));
        file.insert("encodingFormat".into(), json!("application/x-rete"));
        if let Some(h) = sha256 {
            file.insert("sha256".into(), json!(h));
        }
        file.insert(
            "rete:contentHash".into(),
            json!(format!("blake3-16:{checksum}")),
        );
        d.insert("distribution".into(), json!([Value::Object(file)]));
    }
    // NO recordSet: an RDF graph is not a table. Tabular companions (Parquet),
    // where they exist, are the honest recordSet material — they are published
    // beside the file, not inside it, and carry their own Croissant documents.
    // And NO custom fields (`card.extra`): Croissant is the honestly-mappable
    // subset for ML loaders, and publisher-defined keys have no Croissant
    // terms. They stay in `--json` (verbatim) and `--format jsonld`
    // (`rete:extra/<key>` opaque values).
    Value::Object(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rete_core::card::{Creator, Publisher};

    fn sample_card() -> DatasetCard {
        DatasetCard {
            title: Some("Demo".into()),
            description: Some("A demo graph".into()),
            license: Some("https://creativecommons.org/publicdomain/zero/1.0/".into()),
            source: Some("https://example.org/dump.nt".into()),
            version: Some("2026-08-04".into()),
            creators: vec![Creator {
                name: "Ada".into(),
                orcid: Some("https://orcid.org/0000-0002-1825-0097".into()),
            }],
            publisher: Some(Publisher {
                name: "EPFL".into(),
                ror: Some("https://ror.org/02s376052".into()),
            }),
            canonical_url: Some("https://data.example.org/demo.rete".into()),
            sparql_endpoint: Some("https://example.org/sparql".into()),
            derived_from: vec!["https://example.org/raw.csv".into()],
            triple_count: 10,
            quad_count: 12,
            named_graph_count: 1,
            term_count: 9,
            predicates: vec![("<http://ex/p>".into(), 7)],
            classes: vec![("<http://ex/C>".into(), 3)],
            vocabularies: vec!["http://ex/".into()],
            format_version: 5,
            ..Default::default()
        }
    }

    #[test]
    fn jsonld_maps_onto_void_schema_and_prov() {
        let card = sample_card();
        let build = crate::commands::buildinfo::BuildInfo {
            schema: 1,
            built_at: Some("2026-08-04T00:00:00Z".into()),
            builder: Some("rete-cli 0.3.2".into()),
            ..Default::default()
        };
        let v = to_jsonld(&card, Some(&build), "aa".repeat(16).as_str(), "demo.rete");

        assert_eq!(v["@id"], "https://data.example.org/demo.rete");
        assert_eq!(v["@type"], json!(["Dataset", "void:Dataset"]));
        assert_eq!(v["void:triples"], 12, "void:triples is the WHOLE dataset");
        assert_eq!(v["rete:defaultGraphTriples"], 10);
        assert_eq!(v["void:sparqlEndpoint"], "https://example.org/sparql");
        assert_eq!(v["void:dataDump"], "https://data.example.org/demo.rete");
        // Partitions carry unbracketed IRIs.
        assert_eq!(
            v["void:propertyPartition"][0]["void:property"],
            "http://ex/p"
        );
        assert_eq!(v["void:classPartition"][0]["void:entities"], 3);
        // Provenance: source + derived_from as prov:wasDerivedFrom @ids.
        let derived = v["prov:wasDerivedFrom"].as_array().unwrap();
        assert!(derived.contains(&json!("https://example.org/dump.nt")));
        assert!(derived.contains(&json!("https://example.org/raw.csv")));
        // Creator ORCID / publisher ROR are the node @ids — joinable IRIs.
        assert_eq!(
            v["creator"][0]["@id"],
            "https://orcid.org/0000-0002-1825-0097"
        );
        assert_eq!(v["publisher"]["@id"], "https://ror.org/02s376052");
        // The build activity, from the unhashed build-info section.
        assert_eq!(
            v["prov:wasGeneratedBy"]["prov:endedAtTime"],
            "2026-08-04T00:00:00Z"
        );
        assert_eq!(
            v["prov:wasGeneratedBy"]["prov:wasAssociatedWith"]["softwareVersion"],
            "0.3.2"
        );
        // Deterministic output for identical input.
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&to_jsonld(
                &card,
                Some(&build),
                "aa".repeat(16).as_str(),
                "demo.rete"
            ))
            .unwrap()
        );
    }

    #[test]
    fn croissant_carries_no_recordset_and_no_fabricated_sha256() {
        let card = sample_card();
        let v = to_croissant(&card, "bb", "demo.rete", None);
        assert_eq!(v["conformsTo"], "http://mlcommons.org/croissant/1.0");
        assert_eq!(v["distribution"][0]["@type"], "cr:FileObject");
        assert_eq!(v["distribution"][0]["encodingFormat"], "application/x-rete");
        // The honesty line: no fabricated tables, no fake checksum slot — a
        // file cannot carry its own whole-file sha256.
        assert!(v.get("recordSet").is_none());
        assert!(v["distribution"][0].get("sha256").is_none());
        assert_eq!(v["distribution"][0]["rete:contentHash"], "blake3-16:bb");
        // The validator wants a plain license value, not an @id node.
        assert!(v["license"].is_string());

        // With the publisher-supplied hash (known only outside the file), the
        // FileObject is complete and the document validator-clean.
        let with = to_croissant(&card, "bb", "demo.rete", Some("ab".repeat(32).as_str()));
        assert_eq!(with["distribution"][0]["sha256"], "ab".repeat(32));
    }

    /// The curated discovery fields — fields WITH agreed meaning, unlike the
    /// bag — project under their standard terms: keywords under BOTH
    /// `schema:keywords` and `dcat:keyword`, themes as `@id`-typed
    /// `dcat:theme` IRIs. Keywords carry into Croissant's schema.org header;
    /// `theme` (whose only term is DCAT's) does not. Absent fields emit
    /// nothing anywhere, and `schema:inLanguage` stays what it always was:
    /// the MEASURED dominant literal tag (there is deliberately no curated
    /// language — the data itself carries it, per-literal).
    #[test]
    fn discovery_fields_project_under_standard_terms_and_into_croissant() {
        let mut card = sample_card();
        card.keywords = vec!["catalog".into(), "open data".into()];
        card.theme =
            vec!["http://publications.europa.eu/resource/authority/data-theme/GOVE".into()];
        card.signals.default_lang = Some("cs".into());

        let v = to_jsonld(&card, None, "aa", "demo.rete");
        assert_eq!(v["keywords"], json!(["catalog", "open data"]));
        assert_eq!(v["dcat:keyword"], json!(["catalog", "open data"]));
        assert_eq!(v["@context"]["dcat"], "http://www.w3.org/ns/dcat#");
        assert_eq!(
            v["dcat:theme"],
            json!(["http://publications.europa.eu/resource/authority/data-theme/GOVE"])
        );
        assert_eq!(v["@context"]["dcat:theme"]["@type"], "@id");
        assert_eq!(v["inLanguage"], "cs", "measured, never curated");

        let c = to_croissant(&card, "aa", "demo.rete", None);
        assert_eq!(c["keywords"], json!(["catalog", "open data"]));
        assert!(c.get("dcat:theme").is_none(), "no DCAT term in Croissant");

        // No fields, no keys — in either projection.
        let plain = to_jsonld(&sample_card(), None, "aa", "demo.rete");
        assert!(plain.get("keywords").is_none());
        assert!(plain.get("dcat:keyword").is_none());
        assert!(plain.get("dcat:theme").is_none());
        let plain_cr = to_croissant(&sample_card(), "aa", "demo.rete", None);
        assert!(plain_cr.get("keywords").is_none());
    }

    /// Custom fields project per key under `rete:extra/<key>` — values, not
    /// vocabulary: scalars as plain literals, containers as `@json`-typed
    /// JSON literals, keys percent-encoded so the IRI is always valid.
    /// Croissant omits them entirely; a card without a bag emits nothing.
    #[test]
    fn extra_projects_per_key_as_opaque_values_and_croissant_omits_it() {
        let mut card = sample_card();
        card.extra = [
            ("atlas:layer".to_string(), json!(84)),
            ("review".to_string(), json!({"by": "dg", "ok": true})),
            ("my field".to_string(), json!("spaced")),
        ]
        .into_iter()
        .collect();

        let v = to_jsonld(&card, None, "aa", "demo.rete");
        // Scalar: a plain literal under its own rete:extra/ term (`:` is
        // IRI-fragment-legal, so the key passes through unencoded).
        assert_eq!(v["rete:extra/atlas:layer"], 84);
        // Container: emitted verbatim AND typed @json in the context, so it
        // expands to one rdf:JSON literal, not a blank-node structure.
        assert_eq!(v["rete:extra/review"]["by"], "dg");
        assert_eq!(v["@context"]["rete:extra/review"]["@type"], "@json");
        assert!(
            v["@context"].get("rete:extra/atlas:layer").is_none(),
            "scalars need no @json typing"
        );
        // An IRI-hostile key is percent-encoded (space → %20), keeping the
        // expanded IRI valid.
        assert_eq!(v["rete:extra/my%20field"], "spaced");
        // No bare top-level property was minted.
        assert!(v.get("atlas:layer").is_none());
        assert!(v.get("review").is_none());

        let c = to_croissant(&card, "aa", "demo.rete", None);
        assert!(c.get("extra").is_none(), "Croissant omits the bag");
        assert!(c.get("rete:extra/review").is_none());

        let plain = to_jsonld(&sample_card(), None, "aa", "demo.rete");
        assert!(
            !plain
                .as_object()
                .unwrap()
                .keys()
                .any(|k| k.starts_with("rete:extra/")),
            "no bag, no keys"
        );
    }

    #[test]
    fn cardless_iri_falls_back_to_url_source_only() {
        let mut card = sample_card();
        card.canonical_url = None;
        let remote = to_jsonld(&card, None, "cc", "https://host/x.rete");
        assert_eq!(remote["@id"], "https://host/x.rete");
        let local = to_jsonld(&card, None, "cc", "D:/data/x.rete");
        assert!(local.get("@id").is_none(), "a local path is not an IRI");
        // Without a distribution the hash still surfaces on the dataset node.
        assert_eq!(local["rete:contentHash"], "blake3-16:cc");
    }
}
