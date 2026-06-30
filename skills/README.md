# rete skills — graph source → `.rete` → playground

Reusable, repo-aware skills that codify the workflow this project uses again and
again: turn **any** graph-based data source into a queryable `.rete` file and (if
you want) publish it to the browser playground. Each skill is a `SKILL.md` (the
instructions Claude follows), reference docs, and **working utility scripts**.

## The two skills

| Skill | Use it when | Entry point |
|---|---|---|
| **[rete-from-graph](rete-from-graph/SKILL.md)** | "turn this dataset/graph/ontology/endpoint into a `.rete`" | source → N-Triples → `rete build` → verify |
| **[rete-publish](rete-publish/SKILL.md)** | "make this `.rete` explorable in the playground" | companions → bucket → catalog → rebuild → verify |

```
                    rete-from-graph                         rete-publish
 any graph    ─▶  NT  ─▶  rete build  ─▶  verify   ─▶   companions ─▶ bucket ─▶ catalog ─▶ playground
 (RDF/SPARQL/OWL/
  GeoJSON/CSV/DwC-A/
  TEI/JSON API)
```

## Utilities included (all runnable)

`rete-from-graph/scripts/`
- **`rete`** — run the `rete` CLI (PATH, else the `rete-dev` Docker binary).
- **`nt_clean.py`** — validate/clean an N-Triples stream before building.
- **`sparql_to_nt.py`** — harvest a SPARQL endpoint (paginated CONSTRUCT/SELECT) → NT.
- **`owl_to_nt.py`** — OWL/XML → NT fallback (owlready2) for the one syntax `rete build` can't read.
- **`verify_rete.sh`** — info/stats/verify/card/schema + a spot-check query.

`rete-publish/scripts/`
- **`make_companions.py`** — NT → a `triples` table in Parquet/DuckDB/SQLite.
- **`upload_bucket.sh`** — `hf buckets cp/sync` to the playground bucket.

The skills also point at the many **existing** converters in `scripts/` (e.g.
`geoboundaries_to_nt.py`, `bioexplora_to_nt.py`, `wikidata_parquet_to_nt.py`) as
worked examples per source type — see `rete-from-graph/reference/sources.md`.

## Using these as live Claude Code skills

Claude Code discovers skills under `.claude/skills/`. To make these invocable as
`/rete-from-graph` and `/rete-publish`, point that directory at them once:

```bash
mkdir -p .claude/skills
# symlink (POSIX):
ln -s ../../skills/rete-from-graph .claude/skills/rete-from-graph
ln -s ../../skills/rete-publish    .claude/skills/rete-publish
# …or just copy the two folders into .claude/skills/ on Windows.
```

They also work as plain documentation/playbooks if you'd rather read and run them
by hand.

## Conventions these skills bake in (this repo)

- **Docker-only build**: the `rete` binary lives at `/work/target/release/rete` in
  the `rete-dev` image, not on PATH (the `rete` wrapper handles it).
- `rete build` ingests RDF directly (`.nt/.nq/.ttl/.rdf/.owl/.rdfxml` + stdin) — only
  non-RDF sources need a converter.
- `data/` and `web/*.rete` are gitignored; the **converter script** is what gets
  committed (the `.rete` lives in the bucket).
- Commit **without** the Claude co-author trailer.
- Don't scrape robots-blocked sites — go to an official dump / endpoint / open API.
- The bucket read token is shareable (it's already in `catalog.js`); bucket *writes*
  use your `hf` CLI auth.
