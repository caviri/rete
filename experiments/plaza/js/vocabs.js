// vocabs.js — the ontologies / vocabularies a dataset is *built with* (its
// predicate/class namespaces), resolved to friendly names. Distinct from
// providers.js ("connected to" = external ID/DB targets). Two passes:
//   1. full-namespace registry (OWL, RDFS, SKOS, GeoSPARQL, PROV, ChemROF, …)
//   2. OBO sub-ontologies pulled from class/predicate IRIs (obo/CHMO_…, RXNO_…,
//      BFO_…, CHEBI_…) — these all share the obo/ namespace, so the card's
//      `vocabularies` list can't separate them; the IRIs can.

const localName = (iri) => {
  const s = String(iri).replace(/^[<(]|[>)]$/g, "");
  const m = s.match(/[#/]([^#/]+)\/?$/);
  return (m ? m[1] : s).slice(0, 28);
};

const NS = [
  { match: "2002/07/owl#", name: "OWL", url: "https://www.w3.org/TR/owl2-overview/" },
  { match: "2000/01/rdf-schema#", name: "RDFS", url: "https://www.w3.org/TR/rdf-schema/" },
  { match: "22-rdf-syntax-ns#", name: "RDF", url: "https://www.w3.org/TR/rdf11-concepts/" },
  { match: "2004/02/skos/core#", name: "SKOS", url: "https://www.w3.org/TR/skos-reference/" },
  { match: "xmlns.com/foaf/", name: "FOAF", url: "http://xmlns.com/foaf/spec/" },
  { match: "purl.org/dc/terms/", name: "Dublin Core Terms", url: "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/" },
  { match: "purl.org/dc/elements/", name: "Dublin Core", url: "https://www.dublincore.org/specifications/dublin-core/dces/" },
  { match: "w3.org/ns/prov#", name: "PROV-O", url: "https://www.w3.org/TR/prov-o/" },
  { match: "opengis.net/ont/geosparql#", name: "GeoSPARQL", url: "https://www.ogc.org/standard/geosparql/" },
  { match: "2003/01/geo/wgs84_pos#", name: "WGS84 Geo", url: "https://www.w3.org/2003/01/geo/" },
  { match: "geneontology.org/formats/oboinowl#", name: "oboInOwl", url: "https://owlcollab.github.io/oboformat/doc/obo-syntax.html" },
  { match: "w3id.org/chemrof", name: "ChemROF", url: "https://w3id.org/chemrof/" },
  { match: "nfdi.fiz-karlsruhe.de/ontology/", name: "NFDICore", url: "https://nfdi.fiz-karlsruhe.de/ontology/" },
  { match: "purl.org/spar/cito", name: "CiTO (SPAR)", url: "http://www.sparontologies.net/ontologies/cito" },
  { match: "purl.org/spar/", name: "SPAR Ontologies", url: "http://www.sparontologies.net/" },
  { match: "cidoc-crm.org", name: "CIDOC-CRM", url: "https://www.cidoc-crm.org/" },
  { match: "erlangen-crm.org", name: "CIDOC-CRM (Erlangen)", url: "https://www.cidoc-crm.org/" },
  { match: "w3id.org/biolink", name: "Biolink Model", url: "https://biolink.github.io/biolink-model/" },
  { match: "schema.org", name: "schema.org", url: "https://schema.org/" },
  { match: "vocab.getty.edu/ontology", name: "Getty GVP Ontology", url: "https://vocab.getty.edu/" },
  { match: "purl.org/vocab/frbr", name: "FRBR", url: "https://www.ifla.org/publications/functional-requirements-for-bibliographic-records/" },
  { match: "purl.org/ontology/mo/", name: "Music Ontology", url: "http://musicontology.com/" },
  { match: "purl.org/vocab/relationship", name: "REL (Relationship)", url: "https://vocab.org/relationship/" },
  { match: "followthemoney", name: "FollowTheMoney", url: "https://followthemoney.tech/" },
  { match: "w3id.org/ftm", name: "FollowTheMoney", url: "https://followthemoney.tech/" },
  { match: "opensanctions.org", name: "FollowTheMoney (OpenSanctions)", url: "https://www.opensanctions.org/reference/" },
  { match: "2006/time#", name: "OWL-Time", url: "https://www.w3.org/TR/owl-time/" },
  { match: "w3.org/ns/dcat#", name: "DCAT", url: "https://www.w3.org/TR/vocab-dcat-3/" },
  { match: "rdfs.org/ns/void#", name: "VoID", url: "https://www.w3.org/TR/void/" },
  { match: "w3.org/ns/org#", name: "Organization Ontology", url: "https://www.w3.org/TR/vocab-org/" },
  { match: "2006/vcard/ns#", name: "vCard", url: "https://www.w3.org/TR/vcard-rdf/" },
  { match: "w3.org/ns/sosa/", name: "SOSA", url: "https://www.w3.org/TR/vocab-ssn/" },
  { match: "w3.org/ns/ssn/", name: "SSN", url: "https://www.w3.org/TR/vocab-ssn/" },
  { match: "qudt.org", name: "QUDT", url: "https://qudt.org/" },
  { match: "wikiba.se/ontology#", name: "Wikibase", url: "https://www.mediawiki.org/wiki/Wikibase/Indexing/RDF_Dump_Format" },
  { match: "purl.org/ontology/bibo/", name: "BIBO", url: "https://www.dublincore.org/specifications/bibo/" },
  { match: "purl.org/net/c4dm/event.owl#", name: "Event Ontology", url: "http://motools.sourceforge.net/event/event.html" },
  { match: "purl.org/goodrelations/", name: "GoodRelations", url: "http://www.heppnetz.de/projects/goodrelations/" },
  { match: "purl.org/linked-data/cube#", name: "RDF Data Cube", url: "https://www.w3.org/TR/vocab-data-cube/" },
];

// name → homepage, so curated `ontologies: [{name}]` inherit the registry URL.
const NAME_URL = {};
NS.forEach((r) => { if (!NAME_URL[r.name]) NAME_URL[r.name] = r.url; });

// OBO sub-ontology codes (obo/XXX_NNN) → friendly name.
const OBO = {
  CHEBI: "ChEBI", CHMO: "CHMO (Chemical Methods)", RXNO: "RXNO (Name Reactions)",
  BFO: "BFO (Basic Formal Ontology)", IAO: "IAO (Information Artifact)", RO: "RO (Relations)",
  NCBITAXON: "NCBI Taxon", PATO: "PATO", UO: "Units (UO)", OBI: "OBI", ENVO: "ENVO",
  GO: "Gene Ontology", PR: "Protein Ontology", SO: "Sequence Ontology", MOP: "MOP",
};

// Short, ontology-focused descriptions (keyed by the friendly name above).
const DESC = {
  OWL: "Web Ontology Language — classes, properties, restrictions and formal axioms.",
  RDFS: "RDF Schema — class and property hierarchies (subClassOf, domain, range).",
  RDF: "The base RDF vocabulary — type, statements, lists.",
  SKOS: "Simple Knowledge Organization System — concept schemes, labels, broader/narrower.",
  FOAF: "Friend of a Friend — describing people, accounts and the links between them.",
  "Dublin Core Terms": "DCMI Terms — generic descriptive metadata (title, creator, date, …).",
  "Dublin Core": "The 15 legacy Dublin Core elements for resource description.",
  "PROV-O": "W3C provenance ontology — entities, activities, agents and derivation.",
  GeoSPARQL: "OGC standard for geometries (WKT) and topological spatial relations.",
  oboInOwl: "OBO-in-OWL annotation vocabulary — synonyms, xrefs, definitions for OBO terms.",
  ChemROF: "Chemical Resource Object Framework — structure, formula, mass, InChI/SMILES.",
  NFDICore: "NFDI core ontology — research-data entities aligned to BFO.",
  "FollowTheMoney": "OpenSanctions' entity model — people, companies, assets, sanctions.",
  "FollowTheMoney (OpenSanctions)": "OpenSanctions' entity model — people, companies, assets, sanctions.",
  "CIDOC-CRM": "Cultural-heritage event-centric model (objects, actors, places, periods).",
  "Music Ontology": "Artists, releases, tracks and performances.",
  "REL (Relationship)": "Interpersonal relationships (knows, friendOf, collaboratesWith).",
  "schema.org": "The schema.org vocabulary for web/structured-data markup.",
  Wikibase: "The Wikibase RDF model behind Wikidata (items, statements, ranks).",
  "Getty GVP Ontology": "Getty Vocabulary Program ontology (ULAN/AAT/TGN relations).",
  "OWL-Time": "Temporal entities — instants, intervals and their relations.",
  "Organization Ontology": "Organizations, memberships, roles and reporting structure.",
  ChEBI: "Chemical Entities of Biological Interest — the chemistry classification.",
  "CHMO (Chemical Methods)": "Chemical Methods Ontology — measurement, preparation and analysis methods.",
  "RXNO (Name Reactions)": "Name Reaction Ontology — classes of organic reactions.",
  "BFO (Basic Formal Ontology)": "Upper ontology of continuants and occurrents.",
  "IAO (Information Artifact)": "Information Artifact Ontology — documents, identifiers, definitions.",
  "RO (Relations)": "OBO Relations Ontology — part-of, participates-in, etc.",
  OBI: "Ontology for Biomedical Investigations — assays, protocols, materials.",
  "Units (UO)": "Units of measurement ontology.",
  "Gene Ontology": "Gene functions: molecular function, biological process, cellular component.",
};

/** Named ontologies/vocabularies a dataset is built with, deduped.
 *  Auto-detected from the card's IRIs + curated `entry.ontologies` (for ones the
 *  card can't surface, e.g. a subClassOf taxonomy like CHMO/RXNO). */
export function usedOntologies(card, entry) {
  const out = [];
  const seen = new Set();
  const add = (name, url, desc) => { if (name && !seen.has(name)) { out.push({ name, url: url || NAME_URL[name] || null, desc: desc || DESC[name] || null }); seen.add(name); } };

  for (const ns of (card.vocabularies || [])) {
    const low = ns.toLowerCase();
    for (const r of NS) if (low.includes(r.match)) add(r.name, r.url);
  }
  const iris = [];
  for (const [i] of (card.classes || [])) iris.push(i);
  for (const [i] of (card.predicates || [])) iris.push(i);
  for (const [i] of (card.top_hubs || [])) iris.push(i);
  for (const [i] of (card.in_hubs || [])) iris.push(i);
  for (const l of (card.class_links || [])) { iris.push(l.s_class); iris.push(l.o_class); iris.push(l.predicate); }
  for (const iri of iris)
    for (const m of String(iri).matchAll(/obo\/([A-Za-z][A-Za-z0-9]*)_\d/g)) {
      const code = m[1].toUpperCase();
      add(OBO[code] || code, "http://purl.obolibrary.org/obo/" + code.toLowerCase() + ".owl");
    }
  for (const o of (entry && entry.ontologies) || []) add(o.name, o.url, o.desc);
  return out;
}

// Substrings that identify a named ontology's terms (for "which terms are used").
function matchersFor(name) {
  const out = [];
  for (const r of NS) if (r.name === name) out.push(r.match);
  for (const code in OBO) if (OBO[code] === name) out.push("obo/" + code.toLowerCase() + "_");
  return out;
}

/** Homepage + description for an ontology by name (for its dedicated page). */
export function ontologyMeta(name) {
  let url = NAME_URL[name] || null;
  if (!url) for (const code in OBO) if (OBO[code] === name) url = "http://purl.obolibrary.org/obo/" + code.toLowerCase() + ".owl";
  return { url, desc: DESC[name] || null };
}

/** The specific classes/properties of ontology `name` that appear in `card`. */
export function ontologyTerms(name, card) {
  if (!card) return [];
  const ms = matchersFor(name);
  if (!ms.length) return [];
  const seen = new Set(), terms = [];
  const scan = (iri) => {
    const low = String(iri).toLowerCase();
    if (ms.some((m) => low.includes(m))) { const ln = localName(iri); if (ln && !seen.has(ln)) { seen.add(ln); terms.push(ln); } }
  };
  for (const [i] of (card.classes || [])) scan(i);
  for (const [i] of (card.predicates || [])) scan(i);
  for (const l of (card.class_links || [])) scan(l.predicate);
  return terms.slice(0, 14);
}
