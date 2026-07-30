"""Harvest the Fedlex RDF metadata KG from its Virtuoso SPARQL endpoint -> N-Quads.

Switzerland's federal law portal (fedlex.admin.ch) publishes its legal metadata as
Linked Data (JOLux + ELI ontologies) but offers NO static RDF dump: the graph lives
only behind the SPARQL endpoint https://fedlex.data.admin.ch/sparqlendpoint. This is
~56.3M triples across ~497,896 named graphs (one graph per act/version/language +
the controlled vocabularies). This script snapshots the whole thing as gzipped
N-Quads, faithfully preserving named graphs, term types, language tags and datatypes.

The endpoint is OpenLink Virtuoso, which imposes two hard limits we design around:
  * ORDER BY is capped at 10,000 rows to sort (error SR353) -> no global keyset sort.
  * ResultSetMaxRows = 100,000 -> every response is silently truncated at 100k rows.

Strategy
--------
Phase 1 - enumerate every named graph.
  `SELECT ?g WHERE { GRAPH ?g {?s ?p ?o} } GROUP BY ?g LIMIT 10000 OFFSET N`
  is index-backed (fast, ~3s/page) and stable across calls, so we page it with a
  growing OFFSET, dedupe into a set, and write raw/graphs.txt (~498k lines). No
  ORDER (would trip SR353); no system/default-graph noise (named graphs are clean).

Phase 2 - harvest quads, batch by graph, guarded against silent truncation.
  For a batch of graphs we first COUNT them, then pull the data as SPARQL-JSON
  (`SELECT ?g ?s ?p ?o WHERE { VALUES ?g {..} GRAPH ?g {?s ?p ?o} }`). JSON keeps
  each term's type (uri/bnode/literal), xml:lang and datatype so we can round-trip
  to N-Quads. We assert rows == COUNT; a mismatch means Virtuoso truncated, so we
  recurse-split the batch. A single graph that alone exceeds the cap is paginated
  with per-graph LIMIT/OFFSET (stable, no sort needed).

Resumable (raw/quads/_progress.json records completed shards), polite (small delay),
stdlib-only so it runs unchanged in python:3.12-slim under Docker.

Run: python data/switzerland-fedlex/scripts/fetch_sparql.py
     python data/switzerland-fedlex/scripts/fetch_sparql.py --enumerate-only
"""
import argparse
import gzip
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

EP = "https://fedlex.data.admin.ch/sparqlendpoint"
UA = "Mozilla/5.0 (rete dataset acquisition; +https://w3id.org/rete)"
RAW = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw")
QUADS = os.path.join(RAW, "quads")
GRAPHS_TXT = os.path.join(RAW, "graphs.txt")
PROGRESS = os.path.join(QUADS, "_progress.json")
SKIP = os.path.join(QUADS, "_skipped.txt")

CAP = 100000            # Virtuoso ResultSetMaxRows
SAFE = 90000            # stay comfortably under the cap
ENUM_PAGE = 10000       # graphs per enumeration page
BATCH = 100             # graphs per data request (top level)
SHARD_BATCHES = 50      # top-level batches per output shard (~5000 graphs/shard)
DELAY = 0.35            # politeness between requests
TIMEOUT = 90


# ----------------------------------------------------------------------------- http
class BadQuery(Exception):
    """A request the endpoint rejected (HTTP 400/404) or that failed all retries.
    Caught by fetch_batch so one bad graph splits/skips instead of killing the run."""


def sparql(query, accept="application/sparql-results+json", tries=6):
    data = urllib.parse.urlencode({"query": query}).encode()
    for i in range(tries):
        try:
            req = urllib.request.Request(
                EP, data=data,
                headers={"User-Agent": UA, "Accept": accept,
                         "Content-Type": "application/x-www-form-urlencoded"},
            )
            with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
                body = r.read()
            if accept.endswith("json"):
                return json.loads(body.decode("utf-8"))
            return body.decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            if e.code in (400, 404):            # permanent — retrying won't help
                raise BadQuery(f"HTTP {e.code}")
            wait = min(60, 5 * (i + 1))
            print(f"    retry {i+1}/{tries} in {wait}s: HTTP {e.code}", flush=True)
            time.sleep(wait)
        except Exception as e:
            wait = min(60, 5 * (i + 1))
            print(f"    retry {i+1}/{tries} in {wait}s: {str(e)[:80]}", flush=True)
            time.sleep(wait)
    raise BadQuery("failed after retries")


# ---- query builders (Fedlex has malformed graph IRIs with literal spaces) --------
# A literal space (and <>"{}|^`\ / control) is illegal inside a SPARQL <IRIREF>, so
# such graphs can't go in `VALUES ?g { <..> }`. They ARE addressable via the URI()
# function fed a string literal (space legal inside quotes), which yields the exact
# stored IRI term. Good graphs keep the fast VALUES path; bad ones use BIND(URI()).
_BADCHAR = re.compile(r'[\x00-\x20<>"{}|\\^`]')


def _strlit(g):
    return g.replace("\\", "\\\\").replace('"', '\\"')


def graph_block(graphs):
    good = [g for g in graphs if not _BADCHAR.search(g)]
    bad = [g for g in graphs if _BADCHAR.search(g)]
    parts = []
    if good:
        vals = " ".join(f"<{g}>" for g in good)
        parts.append(f"{{ VALUES ?g {{ {vals} }} GRAPH ?g {{ ?s ?p ?o }} }}")
    for g in bad:
        parts.append(f'{{ BIND(URI("{_strlit(g)}") AS ?g) GRAPH ?g {{ ?s ?p ?o }} }}')
    return " UNION ".join(parts)


def graph_one(g):
    if _BADCHAR.search(g):
        return f'BIND(URI("{_strlit(g)}") AS ?g) GRAPH ?g {{ ?s ?p ?o }}'
    return f"GRAPH <{g}> {{ ?s ?p ?o }}"


def count_of(graphs):
    q = f"SELECT (COUNT(*) AS ?n) WHERE {{ {graph_block(graphs)} }}"
    d = sparql(q)
    return int(d["results"]["bindings"][0]["n"]["value"])


# ------------------------------------------------------------------ term serialization
_IRI_BAD = re.compile(r'[\x00-\x20<>"{}|\\^`]')
_ESC = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_ESC_RE = re.compile(r'[\\"\n\r\t]')
_CTRL_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")
_BNODE_RE = re.compile(r"[^A-Za-z0-9]")


def iri(v):
    return "<" + _IRI_BAD.sub(lambda m: "%%%02X" % ord(m.group()), v) + ">"


def bnode(v):
    return "_:B" + _BNODE_RE.sub("", v or "x") or "_:Bx"


def literal(b):
    s = _ESC_RE.sub(lambda m: _ESC[m.group()], b["value"])
    s = _CTRL_RE.sub(lambda m: "\\u%04X" % ord(m.group()), s)
    out = '"' + s + '"'
    if b.get("xml:lang"):
        return out + "@" + b["xml:lang"]
    dt = b.get("datatype")
    if dt and dt != "http://www.w3.org/2001/XMLSchema#string":
        return out + "^^" + iri(dt)
    return out


def term(b):
    t = b["type"]
    if t == "uri":
        return iri(b["value"])
    if t == "bnode":
        return bnode(b["value"])
    return literal(b)              # literal / typed-literal


def quad_line(g, b_s, b_p, b_o):
    return f"{term(b_s)} {term(b_p)} {term(b_o)} {iri(g)} .\n"


# ------------------------------------------------------------------------- phase 1
def enumerate_graphs():
    if os.path.exists(GRAPHS_TXT) and os.path.getsize(GRAPHS_TXT) > 0:
        n = sum(1 for _ in open(GRAPHS_TXT, encoding="utf-8"))
        print(f"graphs.txt exists: {n:,} graphs (skip enumeration)", flush=True)
        return [l.strip() for l in open(GRAPHS_TXT, encoding="utf-8") if l.strip()]
    print("enumerating named graphs (GROUP BY ?g, paged by OFFSET)...", flush=True)
    seen = set()
    off = 0
    while True:
        q = ("SELECT ?g WHERE { GRAPH ?g {?s ?p ?o} } GROUP BY ?g "
             f"LIMIT {ENUM_PAGE} OFFSET {off}")
        d = sparql(q, accept="text/csv")
        rows = [r for r in d.splitlines()[1:] if r]
        got = 0
        for r in rows:
            g = r.strip().strip('"')
            if g.startswith("http"):
                seen.add(g)
                got += 1
        print(f"  offset {off:>7}: +{got} graphs (total {len(seen):,})", flush=True)
        if got == 0:
            break
        off += ENUM_PAGE
        time.sleep(DELAY)
    graphs = sorted(seen)
    tmp = GRAPHS_TXT + ".part"
    open(tmp, "w", encoding="utf-8").write("\n".join(graphs) + "\n")
    os.replace(tmp, GRAPHS_TXT)
    print(f"DONE enumeration: {len(graphs):,} graphs -> {GRAPHS_TXT}", flush=True)
    return graphs


# ------------------------------------------------------------------------- phase 2
def _split_or_skip(graphs, out, reason, oversize=False):
    """Recover from a failed/oversize batch: split in half, or for a lone graph
    paginate it (oversize) else log+skip so the harvest never dies on one graph."""
    if len(graphs) == 1:
        if oversize:
            try:
                return paginate_single(graphs[0], count_of(graphs), out)
            except Exception as e:
                reason = f"paginate: {str(e)[:50]}"
        with open(SKIP, "a", encoding="utf-8") as f:
            f.write(f"{graphs[0]}\t{reason}\n")
        print(f"    SKIP {graphs[0][:66]} ({reason})", flush=True)
        return 0
    mid = len(graphs) // 2
    return fetch_batch(graphs[:mid], out) + fetch_batch(graphs[mid:], out)


def fetch_batch(graphs, out):
    """Emit all quads of `graphs` to `out`, guarded against Virtuoso truncation
    and malformed-IRI graphs; resilient (splits/skips on failure)."""
    try:
        n = count_of(graphs)
    except Exception as e:
        return _split_or_skip(graphs, out, f"count: {str(e)[:50]}")
    if n == 0:
        return 0
    if n <= SAFE:
        try:
            q = f"SELECT ?g ?s ?p ?o WHERE {{ {graph_block(graphs)} }}"
            d = sparql(q)
            rows = d["results"]["bindings"]
        except Exception as e:
            return _split_or_skip(graphs, out, f"data: {str(e)[:50]}")
        if len(rows) != n:                      # truncated -> split down
            return _split_or_skip(graphs, out, "truncated")
        for b in rows:
            out.write(quad_line(b["g"]["value"], b["s"], b["p"], b["o"]))
        return len(rows)
    return _split_or_skip(graphs, out, "oversize", oversize=True)   # too big -> split/paginate


def paginate_single(g, total, out):
    """A single graph larger than the cap: page it with per-graph OFFSET."""
    emitted = 0
    off = 0
    while emitted < total:
        q = (f"SELECT ?s ?p ?o WHERE {{ {graph_one(g)} }} "
             f"LIMIT {SAFE} OFFSET {off}")
        d = sparql(q)
        rows = d["results"]["bindings"]
        if not rows:
            break
        for b in rows:
            out.write(quad_line(g, b["s"], b["p"], b["o"]))
        emitted += len(rows)
        off += SAFE
        if len(rows) < SAFE:
            break
        time.sleep(DELAY)
    if emitted != total:
        print(f"    WARN {g}: emitted {emitted} != count {total}", flush=True)
    return emitted


def load_progress():
    if os.path.exists(PROGRESS):
        return json.load(open(PROGRESS))
    return {"shards_done": 0, "quads": 0}


def harvest(graphs):
    os.makedirs(QUADS, exist_ok=True)
    prog = load_progress()
    batches = [graphs[i:i + BATCH] for i in range(0, len(graphs), BATCH)]
    n_shards = (len(batches) + SHARD_BATCHES - 1) // SHARD_BATCHES
    print(f"{len(graphs):,} graphs -> {len(batches):,} batches -> {n_shards} shards "
          f"(resuming at shard {prog['shards_done']})", flush=True)
    total = prog["quads"]
    t0 = time.time()
    for shard in range(prog["shards_done"], n_shards):
        lo = shard * SHARD_BATCHES
        hi = min(lo + SHARD_BATCHES, len(batches))
        path = os.path.join(QUADS, f"part-{shard:04d}.nq.gz")
        tmp = path + ".part"
        emitted = 0
        with gzip.open(tmp, "wt", encoding="utf-8", compresslevel=6) as out:
            for bi in range(lo, hi):
                emitted += fetch_batch(batches[bi], out)
                time.sleep(DELAY)
        os.replace(tmp, path)
        total += emitted
        prog = {"shards_done": shard + 1, "quads": total}
        json.dump(prog, open(PROGRESS + ".part", "w"))
        os.replace(PROGRESS + ".part", PROGRESS)
        rate = total / max(1e-9, time.time() - t0)
        pct = 100.0 * (shard + 1) / n_shards
        print(f"  shard {shard:04d}/{n_shards-1}  +{emitted:,}  total {total:,} quads "
              f"({pct:.1f}%, ~{rate:,.0f} q/s)", flush=True)
    print(f"DONE harvest: {total:,} quads across {n_shards} shards -> {QUADS}", flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--enumerate-only", action="store_true")
    args = ap.parse_args()
    graphs = enumerate_graphs()
    if args.enumerate_only:
        return
    harvest(graphs)


if __name__ == "__main__":
    main()
