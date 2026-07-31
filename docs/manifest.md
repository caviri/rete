# Writable graphs — the manifest & WAL

A `.rete` file is **immutable by design**: sorted, delta-encoded tiles behind a
content hash are what make it range-cacheable forever. So how do you *edit* a
graph — grow it across sessions, delete and update quads, keep querying while
you write — without giving that up?

The answer is the same one LevelDB, RocksDB, and the lakehouse formats reached:
**never mutate data files; mutate a tiny pointer to them.** A **manifest** is a
small JSON document naming an *ordered log* of immutable `.rete` **segments**;
the graph you see is the fold of that log. The only file that ever changes is
the manifest itself.

```json
{
  "rete_manifest": 1,
  "name": "mygraph",
  "generation": 3,
  "log": [
    { "adds": { "url": "base-1a2b3c4d.rete", "size": 84210, "blake3_16": "…" } },
    { "adds": { "url": "mygraph-g2-5e6f7a8b.rete", "size": 1204, "blake3_16": "…" } },
    { "adds": { "url": "mygraph-g3-9c0d1e2f.rete", "size": 890, "blake3_16": "…" },
      "dels": { "url": "mygraph-g3-9c0d1e2f.tomb.rete", "size": 412, "blake3_16": "…" } }
  ]
}
```

Each log entry contributes quads to **add** and, optionally, a **tombstone**
segment — a plain `.rete` whose quads are the *deleted* quads. The visible
graph is the ordered fold:

```text
visible = ∅
for entry in log:   visible = (visible ∖ entry.dels) ∪ entry.adds
```

Deletion, update, and re-add all follow from entry order:

- **delete** — a later entry's tombstone hides an earlier quad;
- **update** — one entry that deletes the old quad and adds the new one
  (RDF has no in-place update: an update *is* delete + insert);
- **re-add** — an even later `adds` wins over the older tombstone.

Every segment is pinned by `{size, blake3_16}` — the 16-byte blake3 content
hash each `.rete` already carries in its header (the same contract the
playground's `datasets.lock.json` uses). A replaced or truncated segment fails
loudly at query time, never silently returns fewer rows.

## Quickstart

```sh
# Session 1: start the log from a built base file.
rete manifest init mygraph.rete-manifest.json base.rete

# Session 2 (independent, later, elsewhere): built its own segment — append it.
rete manifest add mygraph.rete-manifest.json --adds session2.rete

# Query it all as ONE graph — joins across segments resolve.
rete manifest query mygraph.rete-manifest.json \
  "SELECT ?n WHERE { <http://ex/a> <http://ex/knows> ?x . ?x <http://ex/name> ?n }"

# Delete/update by appending a tombstone segment.
rete manifest add mygraph.rete-manifest.json --adds new.rete --dels obsolete.rete

# The log, verified against its pins (add --count for the visible quad count).
rete manifest status mygraph.rete-manifest.json

# Fold everything back into ONE fresh .rete (single entry, generation++).
rete manifest compact mygraph.rete-manifest.json
```

`manifest query` is **not** `rete federate`: federation runs the whole query
per source and unions rows (each file keeps its own dictionary — see
[Federated queries](federation.md)), so a join whose patterns live in
*different* files cannot resolve. The manifest fold re-assembles all segments
under **one dictionary**, so cross-segment joins behave exactly as if the
graph had been built as one file — which is the point: segments are *one*
logical graph's history, not independent datasets.

## Live writes: serve, journal, seal

`rete serve` accepts a manifest, which closes the write loop:

```sh
rete serve mygraph.rete-manifest.json          # SPARQL 1.1 Protocol + Update
```

Updates append to a plain-text journal (`<manifest>.changes`, one `+`/`-`
N-Quads line per change — the write-ahead log) while queries keep answering
from memory; the segments are never touched. **Sealing** is the checkpoint:

```sh
rete manifest seal mygraph.rete-manifest.json  # stop the server first
```

`seal` nets the journal per quad (the last op wins), builds the net additions
as a fresh segment and the net deletions as a tombstone segment, appends one
log entry, and truncates the journal. Restarting the server on the sealed
manifest reproduces the exact same state — now durable in content-addressed
segments instead of a replay log. Repeat forever; `compact` when the log gets
long.

## Semantics in short

| operation | how |
|---|---|
| add quads | append an entry with `adds` |
| delete quads | append an entry with `dels` (tombstone `.rete`) |
| update a quad | one entry: old quad in `dels`, new quad in `adds` |
| re-add after delete | later `adds` beats the older tombstone |
| named graphs | preserved — segments hold quads, not just triples |
| detect change | `generation` counter (one small-file poll, never the data) |
| verify | every segment checked against `{size, blake3_16}` before use |
| consolidate | `compact` → one `.rete`, deterministic (same fold ⇒ same hash) |

## Limitations (prototype honesty)

- **The fold is materialized per open.** `manifest query` and `serve` load
  every segment and re-assemble one in-memory image — O(total quads), like
  `rete serve` has always worked. Right for living datasets up to a few
  million quads; wrong for multi-GB catalogs. A lazy multi-segment view that
  keeps the base file range-read is future work; until then, `compact` is the
  bridge back to the serverless read path.
- **The per-query image skips the pyramid and text index** (fast open). Use
  `compact` to get a full artifact with semantic zoom back.
- **Single writer.** The journal and the manifest assume one writer at a
  time (`seal` and `serve` must not run concurrently). Multi-session use is
  append-only coordination: build segments independently, `add` them one at
  a time. Compare-and-swap manifest updates over HTTP (R2 conditional PUTs)
  are designed but not implemented.
- **Local manifests only for writing.** Segments may be `http(s)://` URLs
  (they are fetched and verified), but the manifest itself is read/written as
  a local file.
