#!/usr/bin/env python3
"""Fetch a life-sciences slice of Wikidata as one N-Triples file — a real, big,
richly-typed graph for demonstrating rete (build it, query it, watch the
community pyramid find organism/disease clusters, range-query it over HTTP).

It runs several **bounded** CONSTRUCT queries against the Wikidata Query
Service (each well under the 60 s limit) and concatenates the results into one
`.nt`. The sub-graphs connect through shared entity IRIs — genes, the
proteins they encode, the diseases they associate with, drugs that treat those
diseases, and the taxa they belong to — so the merged graph is one connected
biology network, not disjoint tables. Every entity carries an English
`rdfs:label`. Output is N-Triples (one triple per line) so concatenating and
deduping the sub-queries is exact — no fragile multi-line Turtle merging.

Wikidata terms used (so the output is self-describing):
  wd:   <http://www.wikidata.org/entity/>        (Q… entities)
  wdt:  <http://www.wikidata.org/prop/direct/>   (P… properties)
  gene Q7187 · protein Q8054 · disease Q12136 · medication Q12140
  human Q15978631 · P703 found-in-taxon · P688 encodes · P2293 genetic-assoc
  P2175 medical-condition-treated · P279 subclass-of · P680 molecular-function

Usage:
  uv run python scripts/fetch_wikidata_bio.py                 # -> data/wikidata-bio.nt
  uv run python scripts/fetch_wikidata_bio.py --limit 5000 -o bio.nt
  uv run python scripts/fetch_wikidata_bio.py --taxon Q83310  # mouse instead of human
Then: rete build data/wikidata-bio.nt -o bio.rete

Network notes: WDQS requires a descriptive User-Agent (set below) and rate-limits
aggressively — this fetches a handful of queries, not a firehose. A query that
times out server-side is reported and skipped; rerun with a smaller --limit.
"""

from __future__ import annotations

import argparse
import sys
import time
import urllib.parse
import urllib.request

ENDPOINT = "https://query.wikidata.org/sparql"
# WDQS blocks generic/empty agents; identify the tool and a contact URL.
USER_AGENT = "rete-demo/0.1 (https://github.com/caviri/rete; biology slice fetcher)"

PREFIXES = """\
PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
"""


def query(taxon: str, limit: int) -> list[tuple[str, str]]:
    """(name, CONSTRUCT query) pairs. Each is self-contained (carries the
    labels for the entities it introduces) and LIMIT-bounded."""
    return [
        (
            "gene-disease",
            f"""CONSTRUCT {{
  ?gene a wd:Q7187 ; rdfs:label ?gl ; wdt:P2293 ?disease ; wdt:P703 wd:{taxon} .
  ?disease a wd:Q12136 ; rdfs:label ?dl .
}} WHERE {{
  ?gene wdt:P31 wd:Q7187 ; wdt:P703 wd:{taxon} ; wdt:P2293 ?disease .
  ?gene rdfs:label ?gl . FILTER(LANG(?gl) = "en")
  ?disease rdfs:label ?dl . FILTER(LANG(?dl) = "en")
}} LIMIT {limit}""",
        ),
        (
            "gene-protein",
            f"""CONSTRUCT {{
  ?gene wdt:P688 ?protein .
  ?protein a wd:Q8054 ; rdfs:label ?pl .
}} WHERE {{
  ?gene wdt:P31 wd:Q7187 ; wdt:P703 wd:{taxon} ; wdt:P688 ?protein .
  ?protein rdfs:label ?pl . FILTER(LANG(?pl) = "en")
}} LIMIT {limit}""",
        ),
        (
            "protein-function",
            f"""CONSTRUCT {{
  ?protein wdt:P680 ?fn .
  ?fn rdfs:label ?fl .
}} WHERE {{
  ?protein wdt:P31 wd:Q8054 ; wdt:P703 wd:{taxon} ; wdt:P680 ?fn .
  ?fn rdfs:label ?fl . FILTER(LANG(?fl) = "en")
}} LIMIT {limit}""",
        ),
        (
            "drug-disease",
            f"""CONSTRUCT {{
  ?drug a wd:Q12140 ; rdfs:label ?ml ; wdt:P2175 ?disease .
}} WHERE {{
  ?drug wdt:P31 wd:Q12140 ; wdt:P2175 ?disease .
  ?drug rdfs:label ?ml . FILTER(LANG(?ml) = "en")
}} LIMIT {limit}""",
        ),
        (
            "disease-hierarchy",
            f"""CONSTRUCT {{
  ?disease wdt:P279 ?super .
  ?super a wd:Q12136 ; rdfs:label ?sl .
}} WHERE {{
  ?disease wdt:P31 wd:Q12136 ; wdt:P279 ?super .
  ?super rdfs:label ?sl . FILTER(LANG(?sl) = "en")
}} LIMIT {limit}""",
        ),
    ]


def fetch(construct: str) -> str | None:
    """Run one CONSTRUCT, returning N-Triples text (or None on a server error /
    timeout — reported, not fatal). N-Triples = one full triple per line, so the
    caller can dedup and concatenate sub-queries line by line."""
    data = urllib.parse.urlencode({"query": PREFIXES + construct}).encode()
    req = urllib.request.Request(
        ENDPOINT,
        data=data,
        headers={
            "User-Agent": USER_AGENT,
            "Accept": "application/n-triples",
            "Content-Type": "application/x-www-form-urlencoded",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=90) as r:
            return r.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")
        hint = " (server-side timeout — try a smaller --limit)" if "Timeout" in body else ""
        print(f"  HTTP {e.code}{hint}", file=sys.stderr)
        return None
    except Exception as e:  # noqa: BLE001 — network is best-effort here
        print(f"  failed: {e}", file=sys.stderr)
        return None


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--taxon", default="Q15978631", help="found-in-taxon QID (default human)")
    ap.add_argument("--limit", type=int, default=2500, help="max rows per sub-query")
    ap.add_argument("-o", "--output", default="data/wikidata-bio.nt")
    args = ap.parse_args()

    import os

    os.makedirs(os.path.dirname(args.output) or ".", exist_ok=True)

    # N-Triples: one full triple per line, so deduping across the sub-queries
    # (which share genes/diseases/proteins) is exact line-level set union.
    triples: dict[str, None] = {}
    for name, construct in query(args.taxon, args.limit):
        print(f"fetching {name} (limit {args.limit})…", file=sys.stderr)
        nt = fetch(construct)
        if nt is None:
            continue
        n = 0
        for raw in nt.splitlines():
            line = raw.strip()
            if line and not line.startswith("#"):
                triples[line] = None
                n += 1
        print(f"  +{n} statements", file=sys.stderr)
        time.sleep(1.0)  # be polite to WDQS between queries

    if not triples:
        print("no data fetched (network/timeout) — nothing written", file=sys.stderr)
        sys.exit(1)

    with open(args.output, "w", encoding="utf-8", newline="\n") as f:
        for t in triples:
            f.write(t + "\n")
    print(
        f"wrote {len(triples)} statements to {args.output}\n"
        f"next: rete build {args.output} -o "
        f"{os.path.splitext(args.output)[0]}.rete",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
