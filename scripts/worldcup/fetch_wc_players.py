#!/usr/bin/env python3
"""Fetch career club-stints for marquee WC2022 players from Wikidata and append
them to worldcup.nt as n-ary CareerStint nodes (the temporal 'player evolution').

Links to the player nodes already minted from OpenFootball scorers (same name
slug). Uses Wikidata's mwapi search to resolve each name -> QID, then P54
(member of sports team) statements with P580/P582 start/end qualifiers.
"""
from __future__ import annotations
import json, urllib.parse, urllib.request, ssl, re
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
NT = REPO / "data" / "worldcup" / "worldcup.nt"
WC="https://w3id.org/rete/worldcup#"; B="https://w3id.org/rete/worldcup/"
SC="http://schema.org/"; RDF="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS="http://www.w3.org/2000/01/rdf-schema#"; XSD="http://www.w3.org/2001/XMLSchema#"
OWL="http://www.w3.org/2002/07/owl#"
UA="rete-worldcup-research/1.0 (KG build; contact carlosvivarrios@gmail.com)"
_CTX=ssl.create_default_context()

# OpenFootball scorer name (must match the player/<slug> already in the graph)  ->  Wikidata full name
STARS={
 "Lionel Messi":"Lionel Messi","Mbappé":"Kylian Mbappé","Julián Álvarez":"Julián Álvarez",
 "Olivier Giroud":"Olivier Giroud","Neymar":"Neymar","Bukayo Saka":"Bukayo Saka",
 "Harry Kane":"Harry Kane","Cody Gakpo":"Cody Gakpo","Marcus Rashford":"Marcus Rashford",
 "Gonçalo Ramos":"Gonçalo Ramos","Richarlison":"Richarlison","Enner Valencia":"Enner Valencia",
}

def slug(s): return urllib.parse.quote(s.strip().replace(" ","_"), safe="")
def esc(s): return str(s).replace("\\","\\\\").replace('"','\\"').replace("\n"," ")

def sparql(q):
    url="https://query.wikidata.org/sparql?format=json&query="+urllib.parse.quote(q)
    req=urllib.request.Request(url, headers={"User-Agent":UA,"Accept":"application/sparql-results+json"})
    with urllib.request.urlopen(req, timeout=60, context=_CTX) as r:
        return json.load(r)

VALUES=" ".join(f'"{n}"@en' for n in STARS.values())
Q=f"""
SELECT ?name ?player ?playerLabel ?club ?clubLabel ?start ?end WHERE {{
  VALUES ?name {{ {VALUES} }}
  ?player rdfs:label ?name ; wdt:P106 wd:Q937857 .
  ?player p:P54 ?st . ?st ps:P54 ?club .
  OPTIONAL {{ ?st pq:P580 ?start }} OPTIONAL {{ ?st pq:P582 ?end }}
  SERVICE wikibase:label {{ bd:serviceParam wikibase:language "en". }}
}}"""

def main():
    out=[]
    data=sparql(Q)
    # invert map: wikidata name -> openfootball name (for the graph slug)
    wd2of={v:k for k,v in STARS.items()}
    seen_player=set(); n_stint=0
    rows=data["results"]["bindings"]
    for r in rows:
        wdname=r["name"]["value"]
        ofname=wd2of.get(wdname)
        if not ofname: continue
        pi=B+"player/"+slug(ofname)
        qid=r["player"]["value"]
        if pi not in seen_player:
            out.append(f'<{pi}> <{OWL}sameAs> <{qid}> .')
            out.append(f'<{pi}> <{RDF}type> <{SC}Person> .')
            out.append(f'<{pi}> <{SC}name> "{esc(ofname)}" .')
            seen_player.add(pi)
        club=r["club"]["value"]; clubL=r.get("clubLabel",{}).get("value","")
        start=r.get("start",{}).get("value","")[:4]; end=r.get("end",{}).get("value","")[:4]
        sid=B+f"stint/{slug(ofname)}_{slug(clubL)}_{start or 'x'}"
        out.append(f'<{sid}> <{RDF}type> <{WC}CareerStint> .')
        out.append(f'<{sid}> <{WC}player> <{pi}> .')
        out.append(f'<{sid}> <{WC}club> <{club}> .')
        if clubL:
            out.append(f'<{club}> <{RDFS}label> "{esc(clubL)}" .')
            out.append(f'<{sid}> <{RDFS}label> "{esc(ofname)} @ {esc(clubL)}" .')
        if start.isdigit(): out.append(f'<{sid}> <{SC}startDate> "{start}"^^<{XSD}gYear> .')
        if end.isdigit(): out.append(f'<{sid}> <{SC}endDate> "{end}"^^<{XSD}gYear> .')
        n_stint+=1
    with open(NT,"a",encoding="utf-8") as fh:
        fh.write("\n".join(out)+"\n")
    print(f"players resolved: {len(seen_player)}, career stints: {n_stint}")

if __name__=="__main__":
    main()
