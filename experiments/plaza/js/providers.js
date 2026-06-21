// providers.js — "which other databases is this dataset connected to?"
//
// We answer it mostly from the data: scan the IRIs the card actually uses
// (vocabularies, classes, predicates, top/in hubs, base IRI) and match them
// against a registry of known Linked-Data / identifier providers. The manifest
// can add curated `connections` for things the card can't surface (a bundled
// file with no card, or CURIE-style cross-refs like ChEBI's KEGG/CAS/PubChem).

// match: domain/path substrings (lower-case) that signal the provider is used.
export const PROVIDERS = [
  { name: "Wikidata", url: "https://www.wikidata.org", match: ["wikidata.org"] },
  { name: "DBpedia", url: "https://www.dbpedia.org", match: ["dbpedia.org"] },
  { name: "Getty Vocabularies", url: "https://vocab.getty.edu", match: ["vocab.getty.edu", "getty.edu"] },
  { name: "ChEBI", url: "https://www.ebi.ac.uk/chebi/", match: ["obo/chebi", "ebi.ac.uk/chebi"] },
  { name: "OBO Foundry", url: "https://obofoundry.org", match: ["purl.obolibrary.org/obo"] },
  { name: "ChemROF", url: "https://w3id.org/chemrof/", match: ["w3id.org/chemrof"] },
  { name: "NFDICore", url: "https://nfdi.fiz-karlsruhe.de/ontology/", match: ["nfdi.fiz-karlsruhe.de"] },
  { name: "Nomisma", url: "http://nomisma.org", match: ["nomisma.org", "numismatics.org"] },
  { name: "OpenHistoricalMap", url: "https://www.openhistoricalmap.org", match: ["openhistoricalmap.org"] },
  { name: "OpenAlex", url: "https://openalex.org", match: ["openalex.org"] },
  { name: "OpenCitations / SPAR", url: "https://opencitations.net", match: ["opencitations", "purl.org/spar"] },
  { name: "FactGrid", url: "https://database.factgrid.de", match: ["factgrid"] },
  { name: "Monarch / Biolink", url: "https://monarchinitiative.org", match: ["monarchinitiative", "biolink", "w3id.org/biolink"] },
  { name: "ORKG", url: "https://orkg.org", match: ["orkg.org"] },
  { name: "ORCID", url: "https://orcid.org", match: ["orcid.org"] },
  { name: "DOI / Crossref", url: "https://www.doi.org", match: ["doi.org"] },
  { name: "VIAF", url: "https://viaf.org", match: ["viaf.org"] },
  { name: "GeoNames", url: "https://www.geonames.org", match: ["geonames.org", "sws.geonames.org"] },
  { name: "Pleiades", url: "https://pleiades.stoa.org", match: ["pleiades.stoa.org"] },
  { name: "Library of Congress", url: "https://id.loc.gov", match: ["id.loc.gov"] },
  { name: "OpenSanctions", url: "https://www.opensanctions.org", match: ["opensanctions.org"] },
];

/**
 * Providers a dataset is connected to: auto-detected from the card's IRIs, then
 * merged with the manifest's curated `connections` (deduped by name).
 */
export function detectProviders(card, entry) {
  const hay = [];
  const push = (s) => { if (s) hay.push(String(s)); };
  for (const ns of (card.vocabularies || [])) push(ns);
  for (const [iri] of (card.classes || [])) push(iri);
  for (const [iri] of (card.predicates || [])) push(iri);
  for (const [iri] of (card.top_hubs || [])) push(iri);
  for (const [iri] of (card.in_hubs || [])) push(iri);
  if (card.signals && card.signals.base_iri) push(card.signals.base_iri);
  const text = hay.join(" ").toLowerCase();

  const found = [];
  const seen = new Set();
  for (const p of PROVIDERS) {
    if (p.match.some((m) => text.includes(m))) { found.push({ name: p.name, url: p.url }); seen.add(p.name); }
  }
  // curated additions (manifest), e.g. CURIE xrefs the card can't surface
  for (const c of (entry.connections || [])) {
    if (!seen.has(c.name)) { found.push(c); seen.add(c.name); }
  }
  return found;
}
