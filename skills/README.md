# rete skills — use, build, and publish `.rete` graphs

Reusable, repo-aware skills that codify this project's recurring workflows.
Each skill is a `SKILL.md` (the instructions Claude follows), reference docs,
and — where useful — **working utility scripts**. They ship with the repo's
[Claude Code plugin](../.claude-plugin/plugin.json) (`/plugin marketplace add
caviri/rete`).

## The skills

| Skill | Use it when | Entry point |
|---|---|---|
| **[rete-sparql](rete-sparql/SKILL.md)** | "write or debug a SPARQL query against a `.rete`" | supported surface → extensions (reasoning, RDF-star, SERVICE) → 0-rows gotchas → result formats |
| **[rete-catalog](rete-catalog/SKILL.md)** | "use an existing published dataset" | discover → card/schema/examples → open/download/federate |
| **[rete-clients](rete-clients/SKILL.md)** | "wire rete into a new project" | Python / Pyodide / JS / script-tag / wasm / Rust setup + first query |
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
for s in rete-catalog rete-clients rete-from-graph rete-publish; do
  ln -s "../../skills/$s" ".claude/skills/$s"
done
# …or just copy the folders into .claude/skills/ on Windows.
```

Installed via the plugin they need no activation — they load namespaced as
`/rete-graph:<skill>` in any project.

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
