// Derive the curated IDENTITY fields for the .rete files THIS PROJECT publishes,
// and write one `--enrich` document per dataset.
//
// WHY THIS EXISTS. `recard.sh` carries a published card's curated half forward
// and adds nothing, on purpose: for someone else's file (nkod, the Czech
// national catalog) inventing a ROR or a DOI would plant false metadata in
// another party's record. But 21 of the 22 files in the 2026-08 batch are ours,
// and there the same caution only preserves blanks — every one of them shipped
// with no `keywords`, no `theme`, no `canonical_url`, no `publisher`, no
// `derived_from`. This file fills exactly the blanks that are DERIVABLE, from
// sources named per field, and leaves the rest empty.
//
//   node scripts/recard/enrich/generate.mjs [--check]
//
// `--check` re-derives and diffs instead of writing (CI / pre-commit).
//
// THE SOURCE IS THE CATALOG, LOADED AS CODE. web/playground-src/catalog.js is
// evaluated, never regex'd: `datasets[].url` is the published URL,
// `datasetMeta[key]` the licence/source/provenance prose, `datasetExtra[key]`
// the playground's tags.
//
// WHAT IS *NOT* DERIVED, AND WHY — the whole point is that this list stays short
// and explicit rather than being quietly filled with plausible-looking values:
//
//   creators   people, with ORCID IRIs. No ORCID for anyone is on record in this
//              repo, and deciding whose name goes on 21 published files is a
//              human's call, not a derivation. Empty everywhere.
//   doi        NONE of these .rete files has a DOI of its own. gni-bim's UPSTREAM
//              has two real Zenodo DOIs — they are recorded as `derived_from`
//              and `cite_as`, which is what they are. Putting an upstream's DOI
//              in `doi` would claim this file is that deposit.
//   version    only where the UPSTREAM names a version string (ontoneurolog is
//              "2.2"). A snapshot date is `source_date`; conflating the two
//              would make `version` mean two things across the catalog.
//
// TAGS ARE NOT KEYWORDS. `datasetExtra[key].tags` is playground UI copy, and it
// mixes three kinds of thing: real subject terms, licence codes that duplicate
// `license`, and rete-internal product labels ("option-C", "tiles-in-rete",
// "semantic-zoom", "federation"). Worse, on a SHARDED dataset the tags are the
// FAMILY's: deps-dev's list says "npm, PyPI, Maven", none of which is in the
// Cargo shard this card describes. So tags are filtered (DROP_TAGS), spelled out
// (REWRITE), and per-key corrected (ADD_KEYWORDS) — never copied wholesale.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..", "..", "..");

globalThis.window = {};
await import(path.join(ROOT, "web", "playground-src", "catalog.js"));
const CATALOG = globalThis.window.RETE_PLAYGROUND_CATALOG;
const BY_KEY = new Map(CATALOG.datasets.map((d) => [d.key, d]));

// The landing page every dataset already has: scripts/preview/build_pages.mjs
// writes docs/d/<key>.html and publishes it at this base. It is the human page
// that says where the data came from — which is precisely why it goes in the
// `extra` bag and NOT in `sparql_endpoint`.
const SITE = "https://caviri.github.io/rete/";

// -------------------------------------------------------------- keywords ----

// Dropped from every tag list, with the reason. A tag is dropped only if it
// belongs to one of these three classes — nothing is dropped for being ugly.
const DROP_TAGS = new Map([
  // 1. licence codes: `license` already says this, in the publisher's own words
  ["CC0", "licence"], ["CC-BY", "licence"], ["CC BY", "licence"],
  ["CC-BY-NC-SA", "licence"], ["ODC-By", "licence"], ["CC-BY-3.0", "licence"],
  // 2. rete-internal product / UI labels: true of the FILE, not of the subject
  ["federation", "rete-internal"], ["sharded", "rete-internal"],
  ["option-C", "rete-internal"], ["tiles-in-rete", "rete-internal"],
  ["semantic-zoom", "rete-internal"], ["remote-lazy", "rete-internal"],
  ["open data", "rete-internal"], ["complete archive", "rete-internal"],
  // 3. magnitudes and states: they are measured in the card already, or they age
  ["2.5B", "magnitude"], ["20 languages", "magnitude"], ["48 teams", "magnitude"],
  ["live", "ages"],
]);

// Tags kept but spelled as a reader would search for them. Cross-dataset
// joinability is the point: bioexplora says "Darwin Core", plantatlas says
// "Darwin-Core", and `dcat:keyword` is a literal — they have to agree.
const REWRITE = new Map([
  ["natural-history", "natural history"],
  ["rare-books", "rare books"],
  ["citizen-science", "citizen science"],
  ["Darwin-Core", "Darwin Core"],
  ["reference-collection", "reference collection"],
  ["early-modern-print", "early modern print"],
  ["supply-chain", "supply chain"],
  ["St-Andrews", "St Andrews"],
]);

// Per-key corrections, each with a reason. Only two kinds appear:
//   - a SHARD whose family tags describe siblings it does not contain
//   - a dataset whose tags omit what its own title/description states
const ADD_KEYWORDS = {
  // The tag list belongs to the deps-dev FAMILY; this card describes the Cargo
  // shard alone. npm / PyPI / Maven are dropped below and the ecosystem this
  // file actually holds is named instead (from the card's own title).
  "deps-dev-cargo": ["Cargo", "crates.io", "Rust"],
  // geoadmin-tiles is the geoadmin graph plus a tile archive; its tags describe
  // only the tiling, so the graph's own subject is missing.
  "geoadmin-tiles": ["boundaries", "Natural Earth", "administrative areas"],
};
const DROP_KEYWORDS = {
  "deps-dev-cargo": ["npm", "PyPI", "Maven"], // siblings, not this shard
};

// ----------------------------------------------------------------- theme ----

// `theme` takes IRIs into a PUBLISHED concept scheme and nothing else. Every
// IRI below was resolved before being written here (2026-08-06):
//   - EU Data Themes: the 13 concepts fetched from the authority list itself,
//     labels confirmed (EDUC is "Education, culture and sport", TECH is
//     "Science and technology", REGI "Regions and cities", SOCI "Population and
//     society").
//   - Wikidata: every QID resolved through wbsearchentities and its label read.
//     This is not optional ceremony — Q199821 "looked like" the Spanish Civil
//     War and is Gran Colombia; Q7205 "looked like" archaeobotany and is
//     paleontology; Q204034 "looked like" subtitle and is hyperbolic function.
//     Three of the first eight guesses were wrong.
//   - OBO: CHEBI_24431 was verified by querying chebi-full ITSELF — its
//     rdfs:label there is "chemical entity", so that theme IRI is a node in the
//     graph the card describes.
// Datasets whose subject no scheme states cleanly get fewer themes, or one; none
// gets a minted IRI.
const EU = (t) => `http://publications.europa.eu/resource/authority/data-theme/${t}`;
const WD = (q) => `https://www.wikidata.org/entity/${q}`;
const OBO = (t) => `http://purl.obolibrary.org/obo/${t}`;

const THEME = {
  albala: [EU("EDUC"), WD("Q166118")],                  // archives
  arxiu: [EU("EDUC"), WD("Q166118")],                   // archives
  bioexplora: [EU("ENVI"), WD("Q47041")],               // biodiversity
  bph: [EU("EDUC")],
  "chebi-full": [EU("TECH"), OBO("CHEBI_24431")],       // chemical entity
  "deps-dev-cargo": [EU("TECH"), WD("Q7397")],          // software
  "factgrid-illuminati": [EU("EDUC"), WD("Q309")],      // history
  "gbif-birds": [EU("ENVI"), WD("Q47041"), WD("Q5113")],// biodiversity, bird
  geoadmin: [EU("REGI"), WD("Q56061")],                 // administrative territorial entity
  "geoadmin-tiles": [EU("REGI"), WD("Q56061")],
  "gni-bim": [EU("TECH"), WD("Q842017")],               // building information modeling
  memoria: [EU("SOCI"), WD("Q10859")],                  // Spanish Civil War
  // A mapping linkset's subject is the entities it maps — Irish manuscripts —
  // so the culture theme holds. No Wikidata concept is added: "mappings" is a
  // statement about the file's form, and forcing an IRI onto it is what the
  // format's own guidance says not to do.
  "mira-wikidata": [EU("EDUC")],
  nidm: [EU("HEAL"), WD("Q551875")],                    // neuroimaging
  ontoneurolog: [EU("HEAL"), WD("Q551875")],
  orkg: [EU("TECH")],
  plantatlas: [EU("TECH"), WD("Q636481")],              // archaeobotany
  proteinbase: [EU("TECH"), WD("Q410814")],             // protein design
  subtitles: [EU("EDUC"), WD("Q204028")],               // subtitle
  ustc: [EU("EDUC")],
  worldcup: [EU("EDUC"), WD("Q2736")],                  // association football
  worldcup2026: [EU("EDUC"), WD("Q2736")],
};

// --------------------------------------------------------- provenance -------

// `derived_from` (prov:wasDerivedFrom) is written ONLY where it says something
// `source` does not — the actual API, dump or deposit the file came out of.
// Every URL here returned 200 on 2026-08-06; a dead link in a provenance chain
// is worse than no link.
const DERIVED_FROM = {
  arxiu: ["https://backend.arxiusenlinia.cultura.gencat.cat/unitat/search/advanced"],
  bioexplora: ["https://ipt.gbif.es"],
  bph: ["https://sourcelibrary.org"],
  "chebi-full": ["https://ftp.ebi.ac.uk/pub/databases/chebi/ontology/chebi.owl"],
  "deps-dev-cargo": ["https://docs.deps.dev/bigquery/v1/"],
  "factgrid-illuminati": ["https://database.factgrid.de/sparql"],
  "gbif-birds": ["https://registry.opendata.aws/gbif/"],
  geoadmin: ["https://www.geoboundaries.org", "https://www.naturalearthdata.com"],
  // The tiled file is derived from the graph file next to it, and says so.
  "geoadmin-tiles": [
    "https://data.graphplaza.com/geoadmin/geoadmin.rete",
    "https://www.geoboundaries.org",
    "https://www.naturalearthdata.com",
  ],
  "gni-bim": [
    "https://doi.org/10.5281/zenodo.19722011",
    "https://github.com/ZijianWang-ZW/GNI-BIM-Dataset",
  ],
  // The five regional portals are named in memoria's OWN description; they did
  // not have to be guessed, only resolved.
  memoria: [
    "https://analisi.transparenciacatalunya.cat",
    "https://opendata.euskadi.eus",
    "https://www.juntadeandalucia.es",
    "https://www.jcyl.es",
    "https://dadesobertes.gva.es",
  ],
  "mira-wikidata": ["https://data.graphplaza.com/mira/mira.rete", "https://www.wikidata.org"],
  nidm: ["https://github.com/incf-nidash/nidm-specs", "https://openneuro.org/datasets/ds000030"],
  worldcup: ["https://github.com/openfootball/worldcup", "https://www.wikidata.org"],
  worldcup2026: ["https://github.com/openfootball/worldcup", "https://www.wikidata.org"],
};

// The SOURCE data's own snapshot date, only where the provenance states one.
// Four of twenty-two do. The rest record when the .rete was built (`created`),
// which is a different fact, and are left alone rather than back-filled from it.
const SOURCE_DATE = {
  arxiu: "2026-07-14",          // "Harvested 2026-07-13/14"
  "deps-dev-cargo": "2026-07-13", // "snapshot 2026-07-13"
  "gbif-birds": "2026-07-01",   // "Parquet snapshot 2026-07-01"
  proteinbase: "2026-01-28",    // "ProteinBase CSV export (2026-01-28)"
};

// The UPSTREAM's version string, where the upstream has one.
const VERSION = { ontoneurolog: "2.2" };

// `cite_as` only where the SOURCE asks to be cited in a particular way. These
// are transcriptions, not compositions — gni-bim's is the repository's own
// BibTeX, ontoneurolog's was resolved through Crossref (DOI 10.1016/j.jbi.2008
// .03.002, Journal of Biomedical Informatics 41:766-778, 2008), USTC's and
// GBIF's are the attribution their own terms state.
const CITE_AS = {
  "gni-bim":
    "Wang, Z., Fuchs, S., Wu, J., Esser, S., Wrabel, T. & Borrmann, A. (2026). " +
    "GNI BIM Dataset. Technical University of Munich, Georg Nemetschek Institute. " +
    "https://doi.org/10.5281/zenodo.19722012",
  ontoneurolog:
    "Temal, L., Dojat, M., Kassel, G. & Gibaud, B. (2008). Towards an ontology " +
    "for sharing medical images and regions of interest in neuroimaging. " +
    "Journal of Biomedical Informatics 41:766-778. " +
    "https://doi.org/10.1016/j.jbi.2008.03.002",
  ustc: "Universal Short Title Catalogue (USTC), University of St Andrews. https://www.ustc.ac.uk/",
  "gbif-birds":
    "GBIF.org (2026). GBIF Occurrence Snapshot 2026-07-01, AWS Open Data " +
    "(s3://gbif-open-data-eu-central-1). https://www.gbif.org",
};

// `sparql_endpoint` is `void:sparqlEndpoint`: an endpoint a client can actually
// send this dataset's query to. A project home page is NOT one, and putting one
// there is a lie a client can act on. The bar used here is all three of:
//   (1) it answers the SPARQL protocol,
//   (2) it answers about THIS dataset's own IRIs, and
//   (3) it is the upstream publisher's endpoint, not a third party's copy.
//
// Probed 2026-08-06, one dataset passed:
//   database.factgrid.de/sparql   200, and `ASK { <…/entity/Q1000> ?p ?o }` —
//                                 an IRI taken out of the file — is TRUE. This
//                                 file was CONSTRUCTed from that endpoint and
//                                 kept its IRIs. PASSES.
//   orkg.org/triplestore          200, but `ASK` on this file's own
//                                 `orkg.org/resource/…` subject is FALSE and the
//                                 whole store holds 5,589 triples of Virtuoso
//                                 internals. It answers; it does not answer
//                                 about this. FAILS (2).
//   ebi.ac.uk/rdf/services/sparql 404 — EBI's RDF platform endpoint is gone. The
//                                 only endpoint found serving ChEBI IRIs is
//                                 Ontobee (sparql.hegroup.org, U. Michigan), a
//                                 third party's copy of an unstated release.
//                                 FAILS (1) then (3).
// Everything else has no upstream endpoint at all.
const SPARQL_ENDPOINT = {
  "factgrid-illuminati": "https://database.factgrid.de/sparql",
};

// Fields that are simply MISSING from a published card and whose value the
// catalog states. Not identity fields — but re-writing the file is the moment
// they cost nothing, and a card with no licence is a worse card.
//
// These are written UNCONDITIONALLY so this generator's output depends only on
// the repo (catalog.js + docs/d/), never on scratch state — otherwise re-running
// it after a re-card would see the already-filled fields and quietly emit a
// different document. The "was it really absent?" question is not dropped, it is
// ASSERTED: when the published-card snapshot is on disk (dev/recard/before/),
// filling a field the publisher had already set aborts the run.
const FILL = {
  arxiu: {
    license: "Descriptions: Llicència oberta / CC0 (Llei 37/2007); images © holding archives",
  },
  "gbif-birds": { source: "https://www.gbif.org" },
  // geoadmin-tiles shipped with NO CARD AT ALL, so everything is new here.
  "geoadmin-tiles": {
    title: "geoBoundaries + PMTiles — world administrative boundaries, graph and tiles in one file",
    description:
      "The `geoadmin` GeoSPARQL graph (213 countries, 3,133 regions, 48,362 districts, " +
      "1,251 places) with a 117.6 MB PMTiles vector-tile archive embedded in the same " +
      "`.rete` — one HTTP-range-queryable file that answers SPARQL and serves map tiles. " +
      "The tiles are built from the full-detail geoBoundaries GeoJSON with tippecanoe " +
      "(z0-9, four layers), so the map has true per-zoom detail while the graph keeps the " +
      "~1 km simplified geometry.",
    license: "CC BY 4.0 (geoBoundaries) + CC0 (Natural Earth places)",
    source: "https://www.geoboundaries.org",
  },
};

// Text that is WRONG in a published card and provably so. memoria's title and
// description were written through a latin-1/UTF-8 double encoding: the stored
// bytes decode to "MemÃ²ria" and "memoria histÃ³rica", and re-decoding them as
// UTF-8 gives back exactly "Memòria" and "memoria histórica". This is the only
// such repair in the batch — every other carried string was scanned for the same
// signature and is clean.
//
// The REPAIRED text is stored here rather than recomputed from whatever card is
// on disk, for the same reason as FILL: running this twice must not double-decode
// anything. The published bytes are what justifies it, so when the snapshot is
// available the run asserts they still decode to exactly this.
const REPAIR = {
  memoria: {
    title: "Memòria - Spanish Civil War victims & mass graves (open data)",
    description:
      "An aggregation of openly-licensed Spanish Civil War 'memoria histórica' " +
      "datasets into one knowledge graph: 99,542 named victims/repressed persons " +
      "and 2,872 mass graves (fosas), linked through provinces. Persons " +
      "(mc:Victim) from Catalonia's reparació jurídica (69,834), Catalonia's " +
      "desapareguts (8,339) and Euskadi's víctimas mortales (21,369) - name, sex, " +
      "age, birth/residence municipality & province, cause of death, sentence, " +
      "profession, military unit, executed flag. Mass graves (mc:MassGrave) from " +
      "the Catalan (1,027, with WGS84 coordinates), Andalusian (615, with " +
      "narratives), Castilla y León (701) and Valencian (529) fosas registries - " +
      "municipality, province, victim count, status, repressor side. Provinces " +
      "(mc:Province) tie victims and graves together. Aggregated from regional " +
      "open-data portals (transparenciacatalunya.cat, opendata.euskadi.eus, " +
      "juntadeandalucia.es, jcyl.es, gva.es); all CC-BY-style open data. " +
      "Sensitive personal/historical data - attribute the source administrations.",
  },
};

const fixMojibake = (s) => Buffer.from(s, "latin1").toString("utf8");

// ----------------------------------------------------------------- build ----

function keywordsFor(key) {
  const tags = ((CATALOG.datasetExtra || {})[key] || {}).tags || [];
  const drop = new Set(DROP_KEYWORDS[key] || []);
  const out = new Set();
  for (const t of tags) {
    if (DROP_TAGS.has(t) || drop.has(t)) continue;
    out.add(REWRITE.get(t) || t);
  }
  for (const t of ADD_KEYWORDS[key] || []) out.add(t);
  return [...out].sort();
}

function canonicalUrl(key) {
  const ds = BY_KEY.get(key);
  if (ds && ds.url) return ds.url;
  // A shard is not its own catalog entry — it is one url in its family's
  // `shards` list. Match on the file name so the shard's card points at the
  // shard, not at the family's first file.
  for (const d of CATALOG.datasets) {
    for (const u of d.shards || []) if (u.endsWith(`/${key}.rete`)) return u;
  }
  return null;
}

function landingPage(key) {
  if (fs.existsSync(path.join(ROOT, "docs", "d", `${key}.html`))) {
    return `${SITE}d/${key}.html`;
  }
  // A shard has no page of its own; its family's page is the one that describes
  // it, and saying so beats saying nothing.
  for (const d of CATALOG.datasets) {
    for (const u of d.shards || []) {
      if (u.endsWith(`/${key}.rete`) &&
          fs.existsSync(path.join(ROOT, "docs", "d", `${d.key}.html`))) {
        return `${SITE}d/${d.key}.html`;
      }
    }
  }
  return null;
}

const KEYS = Object.keys(THEME).sort();

function enrichmentFor(key, published) {
  const doc = {};
  const url = canonicalUrl(key);
  if (!url) throw new Error(`${key}: no published URL in the catalog`);

  for (const [field, value] of Object.entries(FILL[key] || {})) {
    if (published && published[field] !== undefined && published[field] !== value) {
      throw new Error(
        `${key}: FILL would overwrite the publisher's own ${field} ` +
          `(${JSON.stringify(published[field]).slice(0, 80)}…) — remove it from FILL`
      );
    }
    doc[field] = value;
  }
  for (const [field, fixed] of Object.entries(REPAIR[key] || {})) {
    if (published && typeof published[field] === "string" &&
        fixMojibake(published[field]) !== fixed) {
      throw new Error(
        `${key}: the published ${field} no longer decodes to the recorded repair ` +
          `— re-derive it before trusting this entry`
      );
    }
    doc[field] = fixed;
  }
  if (VERSION[key]) doc.version = VERSION[key];
  // The organisation that makes THIS FILE available. Not the upstream — the
  // upstream is in `source` and `derived_from`, and crediting it with publishing
  // a re-modelled derivative it never released would be the misattribution the
  // carry-only rule exists to prevent. `rete` is the name this project already
  // publishes under: scripts/preview/build_pages.mjs stamps
  // `creator: {"@type":"Organization","name":"rete"}` into the schema.org
  // JSON-LD of every one of these datasets' landing pages. No `ror` — the
  // project has none, and a ROR IRI is exactly the kind of authority identifier
  // that must never be guessed.
  doc.publisher = { name: "rete" };
  doc.canonical_url = url;
  if (SPARQL_ENDPOINT[key]) doc.sparql_endpoint = SPARQL_ENDPOINT[key];
  if (SOURCE_DATE[key]) doc.source_date = SOURCE_DATE[key];
  if (DERIVED_FROM[key]) doc.derived_from = DERIVED_FROM[key];
  if (CITE_AS[key]) doc.cite_as = CITE_AS[key];
  doc.keywords = keywordsFor(key);
  doc.theme = THEME[key];
  const page = landingPage(key);
  if (page) doc.extra = { landing_page: page };
  return doc;
}

// The card of the PUBLISHED file, snapshotted before anything was rebuilt
// (dev/recard/before/<key>.card.json, written by dev/recard/before.sh). It is
// the evidence FILL and REPAIR are asserted against. It is scratch, so it may be
// absent — a fresh clone can still generate and `--check` these documents; it
// just cannot re-prove the two claims that were proven when they were written.
function publishedCard(key) {
  const p = path.join(ROOT, "dev", "recard", "before", `${key}.card.json`);
  if (!fs.existsSync(p)) return null;
  const text = fs.readFileSync(p, "utf8");
  if (!text.trimStart().startsWith("{")) return null; // "(no dataset card)"
  return JSON.parse(text);
}

const check = process.argv.includes("--check");
let bad = 0;
for (const key of KEYS) {
  const doc = enrichmentFor(key, publishedCard(key));
  const text = JSON.stringify(doc, null, 2) + "\n";
  const out = path.join(HERE, `${key}.json`);
  if (check) {
    const have = fs.existsSync(out) ? fs.readFileSync(out, "utf8") : "";
    if (have !== text) { console.error(`STALE ${key}.json`); bad++; }
  } else {
    fs.writeFileSync(out, text);
    console.log(`${key}: ${Object.keys(doc).sort().join(", ")}`);
  }
}
if (check && bad) process.exit(1);
if (check) console.log(`${KEYS.length} enrichment documents up to date`);
