# ethz-research-collection

Complete metadata harvest of the **ETH Research Collection** — the institutional
repository of ETH Zürich (DSpace 7.6), covering publications, theses, research
data, patents, presentations and more.

- Source page: https://www.research-collection.ethz.ch/
- Platform: DSpace 7.6 — REST API + OAI-PMH + sitemaps
- OAI-PMH base: `https://www.research-collection.ethz.ch/server/oai/<context>`
- OAI docs: https://unlimited.ethz.ch/spaces/RC/pages/194119646/OAI-PMH+interface
- Snapshot: **2026-07-23** — `completeListSize = 306,835` records (context `all_items`)
- Downloaded: 2026-07-23

## What we harvested

The **`all_items`** OAI context (every reviewed item — 306,835 — not just the
~114k with open full text), in **two** metadata formats:

**`xoai`** — the raw DSpace registry and, on this instance, the **complete dump**.
Beyond every stored metadata field it embeds two things no other OAI format gives
and that otherwise require the REST API:

- the **entity relationship graph**: `<relation>` → `isAuthorOfPublication`,
  `isJournalOfPublication`, `isPub{New,Previous}VersionOfPub`, `isPubCitedByPub`,
  `isPubReferencesPub`, `isPubHasPartPub`, `isSupervisorOfPublication`, … each
  carrying the **linked entity UUID** + `virtual::` authority + confidence
- the **file manifest** (no bytes): `<bundles>/<bundle>/<bitstreams>/<bitstream>`
  with `name`, `format` (MIME), `size` (bytes), `url` (download link),
  `checksum` + `checksumAlgorithm` (MD5) — verified to match the REST API exactly

i.e. it is equivalent to `/items/{uuid}?embed=bundles,relationships` **without
downloading any dataset/PDF bytes** — the "full enrichment" in one cheap,
resumable OAI stream (no 300k-request REST pass needed).

**`oai_ethz`** — a curated, friendlier crosswalk of the same items (nested
author→name+ORCID, structured grant objects). A convenience view; a strict subset
of `xoai` at the field level (drops `relation.*`, `dspace.entity.type`, rosetta
admin). Kept because its shape is convenient for modelling. It carries:

- authors + ORCID, editors, supervisors, contact
- the ETH affiliation tree (`ethz:leitzahl`, `::`-delimited department→group path)
- identifiers: DOI, arXiv, WoS, Scopus, ISSN, ISBN, NEBIS, Handle URI, e-cit PIDs
- bibliographic: title/subtitle, type, journal (title/vol/issue/pages), publisher,
  place, event (name/date/location), book title, dates (issued/available/deposited)
- subjects/keywords, DDC + JEL codes, abstract, language, pages/size, format
- grants (funder + DOI + programme + agreement no.), rights license + URI, notes

Pages are stored gzipped exactly as returned (100 records/page), so the raw bytes
round-trip losslessly to a future `.rete` build.

### Licensing

The **metadata** is harvested through ETH's public OAI-PMH interface, which is
provided for open metadata harvesting. Per-*item* rights (`dc:rights-license`,
`dc:rights-uri`) describe the attached full-text objects, not the metadata record,
and vary per item — e.g. `In Copyright - Non-Commercial Use Permitted`,
`CC BY 4.0`, `CC BY-NC-ND 4.0`, `CC BY-NC-SA 4.0`. Attribution: *ETH Zürich
Research Collection* (research-collection@library.ethz.ch). Preserve each item's
own rights statement when redistributing objects.

## Layout

```
data/ethz-research-collection/
  README.md
  SHA256SUMS.txt
  raw/
    xoai/              # page_NNNNN.xml.gz — COMPLETE dump: metadata + relationships + file manifest
    oai_ethz/          # page_NNNNN.xml.gz — curated crosswalk (convenience view)
    sets/              # page_00000.xml.gz — ListSets (community/collection vocab)
  scripts/
    harvest_oai.py     # resumable, stdlib-only OAI-PMH harvester
    download.sh        # Docker wrapper — reconstructs raw/ from scratch
    inspect.py         # schema/statistics profiler
    harvest.log        # run log
```

## OAI contexts & formats (for reference)

| context     | formats                              | scope                                  |
|-------------|--------------------------------------|----------------------------------------|
| `all_items` | `oai_dc`, `xoai`, **`oai_ethz`**     | every reviewed item (306,835) — used   |
| `request`   | `oai_dc`, `qdc`, `xoai`, `oai_datacite` | items with open full text (~114k)   |
| `doi`       | `oai_dc`, `qdc`, `oai_dc_doi`        | items with a DOI                       |
| `openaire`  | `oai_dc`, `mets`, `oai_datacite`     | OpenAIRE feed (excl. research data)    |

DataCite kernel-4 XML is available only on `request`/`openaire`; not needed —
`xoai` already carries the citation/version edges as `<relation>` entries.

### Why no REST enrichment pass

The item *web page* renders from the DSpace REST API (`/server/api`). We checked
whether harvesting via OAI misses "sections" the page shows — files and the entity
graph. It does not: **`xoai` embeds both**. The alternative (paging
`/discover/search/objects?embed=bundles/bitstreams,relationships`) was measured at
26–39 s per 100-item page → 25–33 h of heavy load for the full corpus, and
`/core/items` requires auth. `xoai` delivers the same content in ~1 h of polite
OAI paging, so the REST route was dropped. (For the handful of high-degree *entity*
items — Person/Journal with >… relationships — xoai lists all of them inline, with
no embed-pagination cap, so nothing is truncated.)

## Dataset shape

Full profile in `scripts/inspect.txt` (regenerate with `scripts/inspect.py`).
Snapshot 2026-07-23 headline numbers:

- **306,835** OAI records = **293,270** live + **13,565** deleted tombstones.
- **Types**: 46.9% Journal Article, 12.7% Conference Paper, 10.4% Doctoral Thesis,
  6.9% Other Conference Item, 3.2% Book Chapter, … (Dataset/Software/etc. in tail).
- **Entity types** (xoai): 289,592 Publication + 3,678 ResearchData.
- **Identifiers**: DOI 40.0%, WoS 42.3%, Scopus 26.6%, ISBN 23.2%, ISSN 147%
  (multi-valued), Handle 95.7%, arXiv 0.8%. (No PMIDs.)
- **Availability**: 55.5% metadata-only, 36.3% open access, 3.5% closed, 0.2% embargoed.
- **Authors**: 94.9% of records have ≥1 author; mean 4.7, max 633. ORCID captured per author.
- **Languages**: 83.5% en, 10.8% de, 0.7% fr.
- **Affiliation tree** (`ethz:leitzahl`) present on 75.3% of records.

**Relationship graph** (xoai `<relation>`): 79.4% of records carry ≥1 edge —
196,355 `isJournalOfPublication`, 136,501 `isAuthorOfPublication`, plus a long tail
(supervisor, editor, version chains, citation/references, supplement, ResearchData
links) across ~90 relationship types, each with the linked entity UUID.

**File manifest** (xoai `<bundles>`, no bytes downloaded): 868,237 bitstreams
totalling **10.30 TB** across ORIGINAL/TEXT/THUMBNAIL/METS/LICENSE bundles — each
with MIME, size, MD5 checksum, and download URL. Top MIME types: application/xml,
text/plain, application/octet-stream, application/pdf, application/zip, netcdf, etc.

## Reproduce

```bash
# 1. harvest (Docker, ~1h; resumable — just re-run to continue after an interruption)
bash data/ethz-research-collection/scripts/download.sh

# 2. profile
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
    python data/ethz-research-collection/scripts/inspect.py

# 3. checksums
( cd data/ethz-research-collection/raw && find . -name '*.xml.gz' | sort | \
    xargs sha256sum > ../SHA256SUMS.txt )
```

## Next step

Groundwork for a future `ethz-research-collection.rete`. The `xoai` records map to
a rich scholarly graph — works ↔ authors ↔ ORCID ↔ affiliations ↔ grants ↔ DOIs,
plus works ↔ files (with checksums/sizes/URLs) and the DSpace entity relationship
graph (works ↔ journals, version chains, citation edges). They align with the
existing rete scholarly hub (DBLP / OpenAIRE / OpenCitations / ORCID / ROR /
Crossref) on DOI, ORCID and ROR/affiliation keys — hand off to the
**rete-from-graph** skill. Model from `xoai` (complete); `oai_ethz` is a
convenience cross-check.
