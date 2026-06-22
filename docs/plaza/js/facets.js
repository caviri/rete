// facets.js — derive informative chip labels from a dataset's card + manifest
// entry. "Facets" are computed facts (delivery, geometry, time, multilinguality,
// vocab count, license, coherence) that complement the manifest's topical tags.
// Shared by the catalog grid and the detail page so both stay in sync.

// The subset that makes sense as catalog *filters* (small, categorical).
export const FILTERABLE = new Set([
  "remote",
  "bundled",
  "GeoSPARQL",
  "temporal",
  "multilingual",
  "incoherent",
  "header-only",
]);

/** All derived facet labels for display (ordered: categorical first, then info). */
export function derivedFacets(card, entry) {
  const t = [];
  const sig = (card && card.signals) || {};
  t.push(entry.kind === "remote-lazy" ? "remote" : "bundled");
  if (sig.geo_wkt || sig.geo_latlong) t.push("GeoSPARQL");
  if (sig.temporal_extent || (sig.time_predicates && sig.time_predicates.length)) t.push("temporal");
  const langs = ((card && card.languages) || []).filter((l) => l[0] && l[0] !== "");
  if (langs.length > 1) t.push("multilingual");
  if (card && card.coherence && card.coherence.coherent === false) t.push("incoherent");
  if (card && card._lite) t.push("header-only");
  const vocabs = (card && card.vocabularies) || [];
  if (vocabs.length > 1) t.push(`${vocabs.length} vocabularies`);
  const lic = (card && card.license) || entry.license;
  if (lic) t.push(lic);
  return t;
}
