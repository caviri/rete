---
name: rete-from-graph
description: Build a queryable `.rete` file from ANY graph-based data source — an RDF dump (.nt/.nq/.ttl/.rdf/.owl), a SPARQL endpoint, a Wikibase, an OWL ontology, GeoJSON, tabular/CSV, a GBIF Darwin-Core archive, a TEI corpus, or a JSON API. Use whenever the task is "turn this dataset/graph/ontology/endpoint into a .rete" (then hand off to the rete-publish skill to register it in the playground). Covers source→N-Triples conversion, the `rete build` flags, the Docker-only build, and verification.
---

# Build a `.rete` from any graph-based source

`.rete` is a cloud-native, range-queryable RDF file format. The job is always the
same shape: **get RDF out of the source, then `rete build` it.** Most graph sources
are already RDF (or trivially become RDF), so the work is mostly *getting clean
N-Triples* and *choosing the right build flags*.

```
source ──▶ N-Triples/N-Quads ──▶ rete validate ──▶ rete build ──▶ rete verify/stats
 (§1)            (§2)               (§3)              (§4)            (§5)
```

> The golden rule: **`rete build` ingests RDF directly** — `.nt`, `.nq`, `.ttl`,
> and `.rdf`/`.owl`/`.rdfxml` (RDF/XML — how most OWL ontologies ship), plus `-`
> for stdin. If the source is already one of those, **skip conversion** and build
> it as-is. You only write a converter for *non-RDF* sources (GeoJSON, CSV, JSON
> APIs, Darwin-Core, …).

## 1. Identify the source and get the RDF

Match the source to a recipe in **[reference/sources.md](reference/sources.md)** —
it has a concrete recipe + the existing repo script to model on for each of:

| Source kind | Path to N-Triples |
|---|---|
| RDF dump (`.nt/.nq/.ttl/.rdf/.owl`) | none — build it directly |
| OWL/XML (not RDF/XML) | `scripts/owl_to_nt.py` (owlready2) |
| SPARQL endpoint | `scripts/sparql_to_nt.py` (paginated CONSTRUCT/SELECT) |
| Wikibase / Wikidata slice | SPARQL or the truthy dump → NT |
| GeoJSON / shapefiles | hand-written WKT emitter → GeoSPARQL (see `scripts/geoboundaries_to_nt.py`) |
| Tabular / CSV / Parquet | a per-row triple emitter (see `scripts/*_to_nt.py`) |
| GBIF Darwin-Core archive | DwC-A → NT (see `scripts/bioexplora_to_nt.py`) |
| TEI / TEITOK corpus | XML → NT (see `scripts/postscriptum_to_nt.py`) |
| JSON API | per-entity harvest → NT |

Write converters as **streaming N-Triples emitters** (one `<s> <p> <o> .` line at a
time). Reuse the term/escape helpers from any `scripts/*_to_nt.py`. Model a clean,
small ontology (classes + a handful of object/datatype properties) so the schema
pyramid and the playground's schema view are meaningful.

## 2. Clean and validate the N-Triples

Real-world dumps have malformed lines (stray `>`, trailing spaces in IRIs,
bad escapes). Run them through the validator before building:

```bash
python skills/rete-from-graph/scripts/nt_clean.py data/foo/raw.nt > data/foo/foo.nt
# then a parse-only check with the engine (no build):
skills/rete-from-graph/scripts/rete validate /work/data/foo/foo.nt
```

`rete validate` reports counts or fails with the exact parse error — always green
it before building. (See **[reference/verify.md](reference/verify.md)**.)

## 3. Build the `.rete`

The build runs in the project's Docker image (`rete-dev`), against the compiled
binary. Use the wrapper so you don't memorize the incantation:

```bash
# the wrapper finds `rete` on PATH, else runs it in rete-dev against target/release/rete
# The .rete lives WITH its dataset, in data/<name>/ (NOT web/) — it's published
# to R2, and web/ is only a staging step for the few small EMBEDDED datasets.
skills/rete-from-graph/scripts/rete build /work/data/foo/foo.nt \
  -o /work/data/foo/foo.rete --pyramid-algo types --card
```

Flag che-sheet (full detail + memory/large-build guidance in
**[reference/build.md](reference/build.md)**):

- `--pyramid-algo types` — **default choice** for typed graphs: deterministic,
  parallelizable, self-naming communities. Falls back to louvain when untyped.
- `--card` — embed the Dataset Card (counts, top predicates/classes, vocabularies,
  + curated `--title/--license/--source/--description`). Always pass it for a
  publishable dataset.
- `--text-index` — full-text word index (`rete search --contains`). Add for
  text-heavy graphs where content search matters; it grows the file.
- `--type-predicate <FULL-IRI>` — when the typing predicate isn't `rdf:type`
  (e.g. Wikidata `http://www.wikidata.org/prop/direct/P31`). **Pass the full IRI,
  not a prefix** — a prefixed form silently yields "0 typed classes".
- `--no-pyramid` — drop the pyramid (smaller, still fully queryable) for shards or
  huge graphs where the single-threaded pyramid is the bottleneck.
- Multiple inputs are merged into one file: `rete build a.nt b.nt ontology.ttl -o out.rete`.

## 4. Large graphs (memory)

A monolithic build holds the dictionary + index in RAM. If it OOMs:
1. `--pyramid-algo types` + `--no-pyramid` first (pyramid is the biggest section).
2. **`--memory-budget-mb <N> --tmp-dir <spill>`** — the external build: chunks
   the input to disk and merges in bounded RAM, producing ONE byte-identical
   .rete of any size (proven: ORCID 1.3B triples → one 17.5 GB file @ 16 GiB).
   For billion-scale runs follow the **external-build playbook** in
   [reference/build.md](reference/build.md): emit NT to disk then build in a
   DETACHED container, spill is resumable after a crash, and verify/query the
   output with `sparql-url` (lazy), never plain `sparql`.
3. **Shard** when the remaining external-build limits bite (pyramid, text index
   — named graphs build fine) or you need parallel per-part builds: split by
   subject into ~1–2 GB
   N-Triples shards, build each with `--no-pyramid`, and ship a folder +
   manifest (federation). See `scripts/build_databnf_shards.sh` and
   **[reference/build.md](reference/build.md)**.

## 5. Verify

```bash
skills/rete-from-graph/scripts/verify_rete.sh /work/data/foo/foo.rete
```
Runs `rete info` / `rete stats` / `rete verify` (content-hash) and a sample
`rete sparql`. For engine-level correctness on the codebase itself, the project's
differential oracle vs Oxigraph is the gold standard —
`cargo test -p bench --test differential` (see **[reference/verify.md](reference/verify.md)**).

## 6. Publish (next skill)

Once built + verified, hand off to the **rete-publish** skill: generate
Parquet/DuckDB/SQLite companions, upload to the bucket, and register the dataset
(catalog entry, metadata, examples) so it shows up in the playground.

## Gotchas (hard-won)

- **Windows CRLF**: Python `print()` on Windows writes `\r\n`. Strip `\r` or open
  output in binary; a stray `\r` in an IRI breaks the NT parser.
- **The binary isn't on PATH** in `rete-dev` — it's at `/work/target/release/rete`
  (the wrapper handles this). The image must have the repo built (`cargo build --release`).
- **Don't scrape robots-blocked sites.** Prefer an official dump, a SPARQL endpoint,
  or an open-data API. Several datasets in this repo went to primary sources for
  exactly this reason.
- Keep the built `.rete` in its dataset folder, `data/<name>/<name>.rete` — NOT
  in `web/`. All of `data/` is gitignored, so the file stays out of git; the
  *converter script* lives in the repo and R2 holds the published bytes. `web/` is
  only a staging area for the handful of small EMBEDDED datasets that get base64
  inlined into `docs/playground.html` (see rete-publish); everything remote-lazy
  is served straight from R2 and never touches `web/`.
