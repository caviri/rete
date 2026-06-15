#!/usr/bin/env python3
"""OpenAlex (CC0) -> a connected ASTROCYTE research graph as atlas/playground N-Triples.

Fetches the top-cited works on the Astrocyte concept (C2777542381) and emits a graph of
papers <-> authors <-> institutions <-> sub-topics with an intra-set citation network:

  <work>   a ex:Work ; dct:title "..." ; ex:year Y ; ex:citationCount N ;
           cito:cites <work2> ; ex:author <author> ; ex:topic <concept> .
  <author> a ex:Person ; foaf:name "..." ; ex:affiliation <institution> .
  <institution> a ex:Institution ; rdfs:label "..." .
  <concept>     a ex:Concept ; rdfs:label "..." .

Citation edges are kept only between works in the fetched set, so the graph is densely
connected (a citation core), good for Path / Aggregate / Construct queries.

Usage:  python3 scripts/fetch_openalex_astrocytes.py [N_WORKS] > data/playground/openalex-astrocytes.nt
"""
import json
import sys
import urllib.parse
import urllib.request

EX = "http://ex/"
DCT = "http://purl.org/dc/terms/"
CITO = "http://purl.org/spar/cito/"
FOAF = "http://xmlns.com/foaf/0.1/"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"
CONCEPT = "C2777542381"  # Astrocyte
MAILTO = "carlosvivarrios@gmail.com"
MAX_AUTHORS, MAX_TOPICS = 12, 8

SELECT = "id,title,publication_year,cited_by_count,referenced_works,authorships,concepts"


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ").replace("\r", " ").replace("\t", " ")


def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "rete-atlas/0.1 (%s)" % MAILTO})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def fetch_works(n):
    works, cursor = [], "*"
    base = ("https://api.openalex.org/works?filter=concepts.id:%s"
            "&sort=cited_by_count:desc&per-page=200&select=%s&mailto=%s"
            % (CONCEPT, urllib.parse.quote(SELECT), MAILTO))
    while len(works) < n and cursor:
        d = get(base + "&cursor=" + urllib.parse.quote(cursor))
        works.extend(d["results"])
        cursor = d["meta"].get("next_cursor")
    return works[:n]


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 500
    works = fetch_works(n)
    ids = {w["id"] for w in works}
    out, seen = sys.stdout, set()

    def once(line):
        if line not in seen:
            seen.add(line); out.write(line + "\n")

    def label(iri, cls, name, pred=RDFS + "label"):
        once(f"<{iri}> <{RDF}type> <{cls}> .")
        if name:
            once(f'<{iri}> <{pred}> "{esc(name)}"@en .')

    nw = 0
    for w in works:
        wid = w["id"]
        out.write(f"<{wid}> <{RDF}type> <{EX}Work> .\n")
        if w.get("title"):
            out.write(f'<{wid}> <{DCT}title> "{esc(w["title"])}" .\n')
        if w.get("publication_year"):
            out.write(f'<{wid}> <{EX}year> "{int(w["publication_year"])}"^^<{XSD_INT}> .\n')
        out.write(f'<{wid}> <{EX}citationCount> "{int(w.get("cited_by_count") or 0)}"^^<{XSD_INT}> .\n')
        for ref in (w.get("referenced_works") or []):
            if ref in ids:
                out.write(f"<{wid}> <{CITO}cites> <{ref}> .\n")
        for a in (w.get("authorships") or [])[:MAX_AUTHORS]:
            au = a.get("author") or {}
            aid = au.get("id")
            if not aid:
                continue
            out.write(f"<{wid}> <{EX}author> <{aid}> .\n")
            label(aid, EX + "Person", au.get("display_name"), FOAF + "name")
            for inst in (a.get("institutions") or [])[:2]:
                iid = inst.get("id")
                if iid:
                    once(f"<{aid}> <{EX}affiliation> <{iid}> .")
                    label(iid, EX + "Institution", inst.get("display_name"))
        for c in (w.get("concepts") or []):
            if (c.get("score") or 0) < 0.3 or c.get("id", "").endswith(CONCEPT):
                continue
            cid = c.get("id")
            if cid:
                out.write(f"<{wid}> <{EX}topic> <{cid}> .\n")
                label(cid, EX + "Concept", c.get("display_name"))
        nw += 1
    print(f"openalex-astrocytes: {nw} works, {len(seen)} dedup + per-work triples", file=sys.stderr)


if __name__ == "__main__":
    main()
