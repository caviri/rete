#!/usr/bin/env python3
"""Harvest a SPARQL endpoint into N-Triples — the input to `rete build`.

Two modes:
  --construct "<CONSTRUCT ...>"   ask the endpoint for N-Triples directly
                                  (Accept: application/n-triples). Optionally
                                  paginate by appending LIMIT/OFFSET.
  --select    "<SELECT ?s ?p ?o>" page a SELECT that projects exactly ?s ?p ?o
                                  and serialize each binding to a triple. Robust
                                  pagination; include ORDER BY for stable pages.

Standard library only (urllib). Be a good citizen: page in chunks, add a delay,
resume-friendly. For big harvests prefer an official bulk dump over the endpoint.

Examples:
  python sparql_to_nt.py --endpoint https://vocab.getty.edu/sparql \\
    --construct "CONSTRUCT { ?s ?p ?o } WHERE { ?s a skos:Concept ; ?p ?o }" \\
    --page 50000 > getty.nt

  python sparql_to_nt.py --endpoint https://query.wikidata.org/sparql \\
    --select "SELECT ?s ?p ?o WHERE { ?s wdt:P31 wd:Q5 ; ?p ?o } ORDER BY ?s ?p ?o" \\
    --page 10000 > people.nt
"""
import argparse
import sys
import time
import urllib.parse
import urllib.request


def post(endpoint, query, accept, ua):
    data = urllib.parse.urlencode({"query": query}).encode()
    req = urllib.request.Request(endpoint, data=data, headers={
        "Accept": accept,
        "Content-Type": "application/x-www-form-urlencoded",
        "User-Agent": ua,
    })
    with urllib.request.urlopen(req, timeout=300) as r:
        return r.read()


def esc(v):
    return (v.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def term(b):
    t, v = b.get("type"), b.get("value", "")
    if t == "uri":
        return "<" + v + ">"
    if t == "bnode":
        return "_:" + v
    s = '"' + esc(v) + '"'
    if b.get("xml:lang"):
        return s + "@" + b["xml:lang"]
    if b.get("datatype"):
        return s + "^^<" + b["datatype"] + ">"
    return s


def run_select(args, out):
    import json
    base = args.select.rstrip()
    offset, total = 0, 0
    while True:
        q = f"{base} LIMIT {args.page} OFFSET {offset}"
        res = json.loads(post(args.endpoint, q, "application/sparql-results+json", args.ua))
        rows = res.get("results", {}).get("bindings", [])
        if not rows:
            break
        for r in rows:
            if "s" in r and "p" in r and "o" in r:
                out.write(f"{term(r['s'])} {term(r['p'])} {term(r['o'])} .\n")
                total += 1
        sys.stderr.write(f"  +{len(rows)} (offset {offset}) → {total} triples\n")
        offset += args.page
        if len(rows) < args.page:
            break
        time.sleep(args.delay)
    sys.stderr.write(f"sparql_to_nt: {total} triples\n")


def run_construct(args, out):
    base = args.construct.rstrip()
    if not args.page:
        out.buffer.write(post(args.endpoint, base, "application/n-triples", args.ua))
        return
    offset, total = 0, 0
    while True:
        q = f"{base} LIMIT {args.page} OFFSET {offset}"
        body = post(args.endpoint, q, "application/n-triples", args.ua).decode("utf-8", "replace")
        lines = [ln for ln in body.splitlines() if ln.strip() and not ln.startswith("#")]
        if not lines:
            break
        out.write("\n".join(lines) + "\n")
        total += len(lines)
        sys.stderr.write(f"  +{len(lines)} (offset {offset}) → {total} lines\n")
        offset += args.page
        if len(lines) < args.page:
            break
        time.sleep(args.delay)
    sys.stderr.write(f"sparql_to_nt: ~{total} triple lines\n")


def main():
    ap = argparse.ArgumentParser(description="SPARQL endpoint → N-Triples")
    ap.add_argument("--endpoint", required=True)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--construct", help="a CONSTRUCT query")
    g.add_argument("--select", help="a SELECT projecting exactly ?s ?p ?o")
    ap.add_argument("--page", type=int, default=0,
                    help="page size (LIMIT/OFFSET). 0 = one shot (CONSTRUCT only)")
    ap.add_argument("--delay", type=float, default=0.5, help="seconds between pages")
    ap.add_argument("--ua", default="rete-from-graph/sparql_to_nt (contact: you@example.org)")
    args = ap.parse_args()
    out = sys.stdout
    if args.select:
        if not args.page:
            args.page = 10000
        run_select(args, out)
    else:
        run_construct(args, out)


if __name__ == "__main__":
    main()
