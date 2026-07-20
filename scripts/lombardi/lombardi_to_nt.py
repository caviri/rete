#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Turn the Mark Lombardi Networks corpus into one RDF graph.

Input   data/lombardi/raw  (see fetch_lombardi.sh)
Output  data/lombardi/lombardi.nt        the ABox
        data/lombardi/lombardi-rete.ttl  a small extension ontology (TBox)

Mark Lombardi (1951-2000) drew the financial and political scandals of the late
20th century as hand-drawn network diagrams. Robert Tolksdorf digitized 51 of
them by hand, typing every node and every arc against an OWL ontology in which
the *visual* convention carries the meaning -- a dashed arc is a financial
connection, a solid arrow is influence or control, an arc ending in a double
line is a blocked or failed transaction.

Node ids are GLOBAL across the corpus: the same actor keeps one id wherever
Lombardi drew them, which is what makes the whole corpus a single graph rather
than 51 unrelated ones. This script preserves that and adds three derived
layers that only structured data can give you:

  * work <-> work overlap (shared actors + Jaccard), computed over real actors
    only -- Year markers share ids too and would otherwise fake a similarity;
  * narration span per drawing, read off the Year nodes ("84/3" -> 1984);
  * same-name links between actor ids the digitization never merged.

Edges are emitted twice, on purpose: as a direct predicate (lor:influenceControl)
so SPARQL stays short, and as a reified Edge resource so the drawing it belongs
to and any dollar amount have somewhere to live.
"""
import collections
import glob
import html
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RAW = os.path.join(ROOT, "data", "lombardi", "raw")
OUT_NT = os.path.join(ROOT, "data", "lombardi", "lombardi.nt")
OUT_TTL = os.path.join(ROOT, "data", "lombardi", "lombardi-rete.ttl")

LOM = "http://www.lombardinetworks.net/lombardi.owl#"   # Tolksdorf's ontology
LO = "https://w3id.org/rete/lombardi/"                  # this dataset's terms
LOR = LO + "rel/"                                       # one predicate per arc type
ACTOR = LO + "actor/"
EDGE = LO + "edge/"
OVERLAP = LO + "overlap/"
WORK = "http://lombardinetworks.net/network/"           # the site's own work URIs

RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
DCT = "http://purl.org/dc/terms/"
SCHEMA = "https://schema.org/"
FOAF = "http://xmlns.com/foaf/0.1/"

LOMBARDI_WD = "http://www.wikidata.org/entity/Q314444"
TOLKSDORF = LO + "agent/tolksdorf"
MOMA_WD = "http://www.wikidata.org/entity/Q188740"

# Which of the 51 digitized drawings are the works MoMA holds. Curated by hand
# against MoMA's own titles rather than fuzzy-matched, because the near-misses
# are exactly the ones that matter: MoMA has the Gerry Bull *5th* version and the
# World Finance Corporation *4th*, while lombardinetworks also carries a 4th and a
# 6th of those — a similarity score happily maps them to the wrong sheet.
# Two MoMA accessions are single two-part works (".a-b") that lombardinetworks
# digitized as two separate drawings, so they appear twice on purpose.
# MoMA's four "Untitled" Lombardis are deliberately left unmatched: nothing in
# either record distinguishes them.
MOMA_MAP = {
    "1000": "2249.2005",      # Astra - Bmarc - Unwin, London
    "1017": "2251.2005",      # Trafalgar House Cementation - Armscor
    "1018": "2250.2005",      # Industries Carlos Cardoen (2nd Version)
    "1019": "2246.2005",      # Phil Schwab, CB Financial and Eureka Federal
    "1020": "2240.2005.a-b",  # Lincoln, Silverado — part I of the two-part work
    "1030": "2240.2005.a-b",  # Lincoln, Silverado — part II
    "1021": "2238.2005",      # Hernandez Cartaya
    "1022": "2247.2005",      # World Finance Corporation, Miami (4th Version)
    "1023": "2638.2001",      # Banco Nazionale del Lavoro … Arming of Iraq
    "1025": "2248.2005",      # Gerry Bull, Space Research … (5th Version)
    "1026": "2235.2005.a-b",  # First United, CB Fin
    "1029": "2239.2005",      # IOS to mid 1970
    "1031": "2236.2005.a-b",  # Flushing Fed, San Marino-FCA … — one part
    "1032": "2236.2005.a-b",  # … and the other
    "1043": "2241.2005.a-b",  # Lockheed
    "1044": "2234.2005.a-b",  # Butchers, ESM
    "1048": "2237.2005",      # Freeport
}

# The images stay at moma.org and are NOT part of this dataset's licence. Every
# work that carries one also carries this statement, so nobody downstream mistakes
# a CC BY-NC-SA graph for a licence over the artwork.
IMAGE_RIGHTS = ("Artwork © The Estate of Mark Lombardi. Image hosted by and "
                "courtesy of The Museum of Modern Art, New York; linked, not "
                "redistributed. To license a reproduction contact Art Resource "
                "(North America) or Scala Archives (elsewhere). NOT covered by "
                "this dataset's CC BY-NC-SA licence.")

# Node types that are actual actors in the story -- the rest (Year markers,
# outcome annotations, the (*) terminals) are notation, not participants.
ACTOR_TYPES = {"Person", "Institution", "MergedInstitution"}


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "").replace("\t", "\\t"))


def lit(s, dt=None, lang=None):
    v = '"%s"' % esc(s)
    if dt:
        return v + "^^<%s>" % dt
    if lang:
        return v + "@" + lang
    return v


class Sink:
    def __init__(self, path):
        self.f = open(path, "wb")   # binary: never let Windows inject \r
        self.n = 0

    def __call__(self, s, p, o):
        if not o.startswith('"'):
            o = "<%s>" % o
        self.f.write(("<%s> <%s> %s .\n" % (s, p, o)).encode("utf-8"))
        self.n += 1

    def close(self):
        self.f.close()


def camel(name):
    """InfluenceControl -> influenceControl (class name -> predicate name)."""
    return name[0].lower() + name[1:] if name else name


def norm_name(s):
    """Loose key for spotting the same actor drawn under two ids."""
    s = re.sub(r"\s+", " ", s.strip().lower())
    s = re.sub(r"[.,'\"]", "", s)
    s = re.sub(r"\s*-\s*$", "", s)
    return s


def parse_page(path):
    """Title, narration span and the external image link off a network page."""
    meta = {}
    if not os.path.exists(path):
        return meta
    h = open(path, encoding="utf-8", errors="ignore").read()
    m = re.search(r"<title>\s*Mark Lombardi Networks\s*-\s*(.*?)</title>", h, re.S)
    if m:
        meta["title"] = html.unescape(re.sub(r"\s+", " ", m.group(1))).strip()
    flat = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", re.sub(r"<script.*?</script>", "", h, flags=re.S)))
    m = re.search(r"narration takes place in the years (\d{4})\s*-\s*(\d{4})", flat)
    if m:
        meta["from"], meta["to"] = m.group(1), m.group(2)
    m = re.search(r'href="([^"]+)"[^>]*>\s*image on the Web', h)
    if m:
        # un-wrap the Wayback prefix: we want the live third-party page
        meta["image"] = re.sub(r"^https?://web\.archive\.org/web/\d+/", "", m.group(1))
    return meta


def main():
    ids = sorted(os.path.basename(os.path.dirname(p))
                 for p in glob.glob(os.path.join(RAW, "network", "*", "*.json")))
    if not ids:
        sys.exit("no networks found in %s -- run fetch_lombardi.sh first" % RAW)

    # MoMA's open collection data (CC0) for the 20 Lombardis they hold, if it has
    # been fetched. Optional: without it the graph simply carries no object records.
    moma = {}
    mp = os.path.join(RAW, "..", "moma", "lombardi_moma.json")
    if os.path.exists(mp):
        for rec in json.load(open(mp, encoding="utf-8")):
            moma[rec["AccessionNumber"]] = rec

    os.makedirs(os.path.dirname(OUT_NT), exist_ok=True)
    t = Sink(OUT_NT)

    # ---- the corpus itself -------------------------------------------------
    corpus = LO + "corpus"
    t(corpus, RDF + "type", SCHEMA + "Collection")
    t(corpus, RDFS + "label", lit("Mark Lombardi Networks", lang="en"))
    t(corpus, DCT + "title", lit("Mark Lombardi Networks", lang="en"))
    t(corpus, DCT + "description", lit(
        "51 network drawings by Mark Lombardi, digitized node by node and arc by "
        "arc by Robert Tolksdorf (Freie Universitat Berlin) and published as "
        "GraphML/JSON/XGMML at lombardinetworks.net.", lang="en"))
    t(corpus, DCT + "creator", LOMBARDI_WD)
    t(corpus, DCT + "contributor", TOLKSDORF)
    t(corpus, DCT + "license", "http://creativecommons.org/licenses/by-nc-sa/4.0/")
    t(corpus, DCT + "source", "https://lombardinetworks.net/")

    t(LOMBARDI_WD, RDF + "type", FOAF + "Person")
    t(LOMBARDI_WD, RDFS + "label", lit("Mark Lombardi", lang="en"))
    t(LOMBARDI_WD, SCHEMA + "sameAs", "https://en.wikipedia.org/wiki/Mark_Lombardi")
    t(TOLKSDORF, RDF + "type", FOAF + "Person")
    t(TOLKSDORF, RDFS + "label", lit("Robert Tolksdorf", lang="en"))
    t(TOLKSDORF, SCHEMA + "affiliation", lit("Freie Universitat Berlin"))

    # ---- pass 1: read every network ---------------------------------------
    works = {}          # id -> meta
    node_type = {}      # global node id -> type name
    node_name = {}      # global node id -> label
    appears = collections.defaultdict(set)   # node id -> {work id}
    all_edges = []      # (work, i, src, tgt, type, amount)

    for wid in ids:
        d = json.load(open(os.path.join(RAW, "network", wid, "%s.json" % wid), encoding="utf-8"))
        meta = parse_page(os.path.join(RAW, "network", wid, "page.html"))
        meta["nodes"], meta["edges"] = len(d["nodes"]), len(d["links"])
        works[wid] = meta
        for n in d["nodes"]:
            nid = n["id"]
            ty = (n.get("type") or "").split("#")[-1] or "Node"
            node_type[nid] = ty
            if n.get("name"):
                node_name.setdefault(nid, n["name"].strip())
            appears[nid].add(wid)
        for i, e in enumerate(d["links"]):
            ety = (e.get("type") or "").split("#")[-1] or "Connection"
            all_edges.append((wid, i, e["source"], e["target"], ety, (e.get("amount") or "").strip()))

    # ---- works -------------------------------------------------------------
    year_of = collections.defaultdict(set)   # work -> {years drawn as Year nodes}
    for nid, ty in node_type.items():
        if ty in ("Year", "YearFinal"):
            m = re.match(r"^(\d{2})\b", node_name.get(nid, ""))
            if m:
                y = 1900 + int(m.group(1))
                for w in appears[nid]:
                    year_of[w].add(y)

    for wid, meta in works.items():
        w = WORK + wid + "/"
        t(w, RDF + "type", LO + "Drawing")
        t(w, RDF + "type", SCHEMA + "VisualArtwork")
        title = meta.get("title", "Network %s" % wid)
        for p in (RDFS + "label", DCT + "title", SCHEMA + "name"):
            t(w, p, lit(title, lang="en"))
        t(w, SCHEMA + "creator", LOMBARDI_WD)
        t(w, DCT + "creator", LOMBARDI_WD)
        t(w, LO + "digitizedBy", TOLKSDORF)
        t(w, SCHEMA + "isPartOf", corpus)
        t(w, LO + "networkId", lit(wid, XSD + "string"))
        t(w, LO + "nodeCount", lit(str(meta["nodes"]), XSD + "integer"))
        t(w, LO + "edgeCount", lit(str(meta["edges"]), XSD + "integer"))
        t(w, DCT + "license", "http://creativecommons.org/licenses/by-nc-sa/4.0/")
        for ext in ("graphml", "json", "xgmml"):
            t(w, LO + "dataFile", "https://lombardinetworks.net/network/%s/%s.%s" % (wid, wid, ext))
        if meta.get("image"):
            t(w, RDFS + "seeAlso", meta["image"])
            t(w, LO + "imagePage", meta["image"])
        # narration span: the site's computed value, else the Year nodes drawn
        yfrom, yto = meta.get("from"), meta.get("to")
        if not yfrom and year_of.get(wid):
            yfrom, yto = str(min(year_of[wid])), str(max(year_of[wid]))
        if yfrom:
            t(w, LO + "narrationStart", lit(yfrom, XSD + "gYear"))
            t(w, LO + "narrationEnd", lit(yto, XSD + "gYear"))
            t(w, SCHEMA + "temporalCoverage", lit("%s/%s" % (yfrom, yto)))
        # the title usually carries Lombardi's own date range, e.g. "c. 1979-1990"
        m = re.search(r"c\.\s*(\d{4})\s*[-–]\s*(\d{2,4})", title)
        if m:
            t(w, LO + "titleDateRange", lit(m.group(0)))

        # ---- the physical object, when MoMA holds it (their open data is CC0)
        rec = moma.get(MOMA_MAP.get(wid, ""))
        if rec:
            # MoMA's export carries stray tabs/newlines inside Medium and friends
            rec = {k: re.sub(r"\s+", " ", v).strip() if isinstance(v, str) else v
                   for k, v in rec.items()}
            t(w, LO + "heldBy", MOMA_WD)
            t(w, LO + "accession", lit(rec["AccessionNumber"]))
            t(w, LO + "momaPage", rec["URL"])
            t(w, SCHEMA + "sameAs", rec["URL"])
            if rec.get("Date"):
                t(w, SCHEMA + "dateCreated", lit(rec["Date"]))
            if rec.get("Medium"):
                t(w, SCHEMA + "artMedium", lit(rec["Medium"]))
            if rec.get("Dimensions"):
                # the real thing: BNL is 50 x 120 inches of paper
                t(w, LO + "dimensions", lit(rec["Dimensions"]))
                t(w, SCHEMA + "size", lit(rec["Dimensions"]))
            if rec.get("CreditLine"):
                t(w, LO + "creditLine", lit(rec["CreditLine"]))
            if rec.get("ImageURL"):
                t(w, SCHEMA + "image", rec["ImageURL"])
                t(w, LO + "momaImage", rec["ImageURL"])
                t(w, DCT + "rights", lit(IMAGE_RIGHTS))

    if moma:
        t(MOMA_WD, RDF + "type", SCHEMA + "Museum")
        t(MOMA_WD, RDFS + "label", lit("The Museum of Modern Art, New York", lang="en"))
        t(MOMA_WD, SCHEMA + "url", "https://www.moma.org/")
        t(MOMA_WD, DCT + "source", "https://github.com/MuseumofModernArt/collection")

    # ---- actors ------------------------------------------------------------
    for nid, ty in sorted(node_type.items()):
        a = ACTOR + nid
        t(a, RDF + "type", LOM + ty)
        name = node_name.get(nid, "")
        if name:
            t(a, RDFS + "label", lit(name))
            t(a, SKOS + "prefLabel", lit(name))
            t(a, LO + "name", lit(name))
        t(a, LO + "actorId", lit(nid, XSD + "string"))
        t(a, LO + "workCount", lit(str(len(appears[nid])), XSD + "integer"))
        for wid in sorted(appears[nid]):
            t(a, LO + "appearsIn", WORK + wid + "/")
            t(WORK + wid + "/", LO + "depicts", a)
        if ty in ("Year", "YearFinal"):
            m = re.match(r"^(\d{2})\b", name)
            if m:
                t(a, LO + "year", lit(str(1900 + int(m.group(1))), XSD + "gYear"))

    # ---- arcs --------------------------------------------------------------
    arc_types = set()
    for wid, i, src, tgt, ety, amount in all_edges:
        arc_types.add(ety)
        s, o = ACTOR + src, ACTOR + tgt
        e = "%s%s-%d" % (EDGE, wid, i)
        t(s, LOR + camel(ety), o)          # short form for querying
        t(s, LO + "connectedTo", o)        # "any arc at all"
        t(e, RDF + "type", LOM + ety)      # reified form, for provenance
        t(e, RDF + "type", LO + "Arc")
        t(e, LO + "source", s)
        t(e, LO + "target", o)
        t(e, LO + "inDrawing", WORK + wid + "/")
        t(WORK + wid + "/", LO + "hasArc", e)
        t(e, LO + "arcType", LOM + ety)
        if amount:
            t(e, LO + "amount", lit(amount))
        if node_name.get(src) and node_name.get(tgt):
            t(e, RDFS + "label", lit("%s -> %s (%s)" % (node_name[src], node_name[tgt], ety)))

    # ---- derived: work-to-work overlap over real actors only ---------------
    actors_of = collections.defaultdict(set)
    for nid, ty in node_type.items():
        if ty in ACTOR_TYPES:
            for wid in appears[nid]:
                actors_of[wid].add(nid)
    pairs = 0
    for a_, b_ in ((a, b) for a in ids for b in ids if a < b):
        shared = actors_of[a_] & actors_of[b_]
        if not shared:
            continue
        pairs += 1
        union = len(actors_of[a_] | actors_of[b_])
        ov = OVERLAP + "%s-%s" % (a_, b_)
        wa, wb = WORK + a_ + "/", WORK + b_ + "/"
        t(ov, RDF + "type", LO + "Overlap")
        t(ov, LO + "betweenDrawing", wa)
        t(ov, LO + "betweenDrawing", wb)
        t(ov, LO + "sharedActorCount", lit(str(len(shared)), XSD + "integer"))
        t(ov, LO + "jaccard", lit("%.4f" % (len(shared) / union), XSD + "decimal"))
        t(ov, RDFS + "label", lit("%d shared actors" % len(shared)))
        for nid in sorted(shared):
            t(ov, LO + "sharedActor", ACTOR + nid)
        t(wa, LO + "sharesActorsWith", wb)
        t(wb, LO + "sharesActorsWith", wa)

    # ---- derived: ids the digitization left unmerged ------------------------
    by_name = collections.defaultdict(set)
    for nid, ty in node_type.items():
        if ty in ACTOR_TYPES and node_name.get(nid):
            by_name[norm_name(node_name[nid])].add(nid)
    same = 0
    for _, nids in by_name.items():
        if len(nids) < 2:
            continue
        for x in nids:
            for y in nids:
                if x != y:
                    t(ACTOR + x, LO + "sameNameAs", ACTOR + y)
                    same += 1

    t.close()
    open(OUT_TTL, "wb").write(ontology_ttl(sorted(arc_types)).encode("utf-8"))

    n_actors = sum(1 for ty in node_type.values() if ty in ACTOR_TYPES)
    print("networks        %d" % len(ids))
    print("distinct nodes  %d  (of which real actors %d)" % (len(node_type), n_actors))
    print("arcs            %d" % len(all_edges))
    print("cross-drawing   %d actors in >1 drawing" % sum(
        1 for nid, ty in node_type.items() if ty in ACTOR_TYPES and len(appears[nid]) > 1))
    print("overlap pairs   %d" % pairs)
    print("same-name links %d" % same)
    print("moma records    %d matched to %d drawings" % (
        len({MOMA_MAP[k] for k in MOMA_MAP if MOMA_MAP[k] in moma}),
        sum(1 for k in MOMA_MAP if MOMA_MAP[k] in moma)))
    print("arc types       %s" % ", ".join(sorted(arc_types)))
    print("triples         %d -> %s" % (t.n, OUT_NT))


def ontology_ttl(arc_types):
    """The extension TBox: our own terms + the arc classes the v0.3 OWL omits."""
    # arc classes present in the data; those missing from lombardi.owl v0.3 get
    # declared here so nothing in the graph is untyped.
    KNOWN = {"Association", "BlockedFailed", "BlockedFailedTransaction", "Connection",
             "Final", "FinancialAssociation", "FinancialConnection", "FinancialTransaction",
             "InfluenceControl", "SaleProperty", "SaleTransfer", "YearArrow"}
    ttl = ['''@prefix lo:     <https://w3id.org/rete/lombardi/> .
@prefix lor:    <https://w3id.org/rete/lombardi/rel/> .
@prefix lom:    <http://www.lombardinetworks.net/lombardi.owl#> .
@prefix rdf:    <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs:   <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl:    <http://www.w3.org/2002/07/owl#> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .
@prefix skos:   <http://www.w3.org/2004/02/skos/core#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix schema: <https://schema.org/> .
@prefix foaf:   <http://xmlns.com/foaf/0.1/> .

# Extension ontology for the Mark Lombardi Networks dataset. It sits on top of
# Tolksdorf's lombardi.owl (which types the nodes and arcs of the drawings) and
# adds what a queryable corpus needs: the drawings themselves as works, the arcs
# as first-class resources, and the derived overlap between drawings.
# OWL 2 QL only -- subclass, domain, range and inverse -- so it stays usable with
# rete's query-rewriting reasoner.

lo: a owl:Ontology ;
    rdfs:label "Mark Lombardi Networks (rete extension)"@en ;
    dcterms:description "Works, arcs and derived corpus structure over lombardi.owl."@en ;
    owl:imports <http://www.lombardinetworks.net/lombardi.owl> ;
    owl:versionInfo "1.0" .

lo:Drawing a owl:Class ; rdfs:subClassOf schema:VisualArtwork ;
    rdfs:label "Network drawing"@en ;
    rdfs:comment "One of Mark Lombardi's hand-drawn network diagrams."@en .
lo:Arc a owl:Class ;
    rdfs:label "Arc"@en ;
    rdfs:comment "One drawn arc, reified so it can carry the drawing it belongs to and any dollar amount written along it."@en .
lo:Overlap a owl:Class ;
    rdfs:label "Drawing overlap"@en ;
    rdfs:comment "Derived: the actors two drawings have in common. Computed over Person, Institution and MergedInstitution only -- Year markers share ids across drawings and would otherwise inflate the similarity."@en .

lo:appearsIn a owl:ObjectProperty ; rdfs:label "appears in"@en ;
    rdfs:domain lom:Node ; rdfs:range lo:Drawing ; owl:inverseOf lo:depicts ;
    rdfs:comment "An actor is drawn in this work. Node ids are global across the corpus, so this is the join that turns 51 drawings into one graph."@en .
lo:depicts a owl:ObjectProperty ; rdfs:label "depicts"@en ;
    rdfs:domain lo:Drawing ; rdfs:range lom:Node .
lo:connectedTo a owl:ObjectProperty ; rdfs:label "connected to"@en ;
    rdfs:comment "Any arc at all, whatever its type -- the cheap way to traverse the network."@en .
lo:source a owl:ObjectProperty ; rdfs:label "source"@en ; rdfs:domain lo:Arc .
lo:target a owl:ObjectProperty ; rdfs:label "target"@en ; rdfs:domain lo:Arc .
lo:inDrawing a owl:ObjectProperty ; rdfs:label "in drawing"@en ;
    rdfs:domain lo:Arc ; rdfs:range lo:Drawing ; owl:inverseOf lo:hasArc .
lo:hasArc a owl:ObjectProperty ; rdfs:label "has arc"@en ;
    rdfs:domain lo:Drawing ; rdfs:range lo:Arc .
lo:arcType a owl:ObjectProperty ; rdfs:label "arc type"@en ; rdfs:domain lo:Arc .
lo:digitizedBy a owl:ObjectProperty ; rdfs:label "digitized by"@en ; rdfs:domain lo:Drawing .
lo:sharesActorsWith a owl:ObjectProperty ; rdfs:label "shares actors with"@en ;
    rdfs:domain lo:Drawing ; rdfs:range lo:Drawing .
lo:betweenDrawing a owl:ObjectProperty ; rdfs:label "between drawing"@en ; rdfs:domain lo:Overlap .
lo:sharedActor a owl:ObjectProperty ; rdfs:label "shared actor"@en ; rdfs:domain lo:Overlap .
lo:sameNameAs a owl:ObjectProperty ; rdfs:label "same name as"@en ;
    rdfs:comment "Derived: two actor ids carrying the same name. The digitization never merged them, so this is a candidate identity, not an assertion of one."@en .

lo:name a owl:DatatypeProperty ; rdfs:label "name"@en ;
    rdfs:comment "The label as Lombardi hand-lettered it, abbreviations and all."@en .
lo:actorId a owl:DatatypeProperty ; rdfs:label "actor id"@en ; rdfs:range xsd:string .
lo:networkId a owl:DatatypeProperty ; rdfs:label "network id"@en ; rdfs:range xsd:string .
lo:nodeCount a owl:DatatypeProperty ; rdfs:label "node count"@en ; rdfs:range xsd:integer .
lo:edgeCount a owl:DatatypeProperty ; rdfs:label "arc count"@en ; rdfs:range xsd:integer .
lo:workCount a owl:DatatypeProperty ; rdfs:label "drawing count"@en ; rdfs:range xsd:integer ;
    rdfs:comment "In how many drawings this actor appears."@en .
lo:amount a owl:DatatypeProperty ; rdfs:label "amount"@en ;
    rdfs:comment "A sum of money Lombardi wrote along the arc."@en .
lo:year a owl:DatatypeProperty ; rdfs:label "year"@en ; rdfs:range xsd:gYear .
lo:narrationStart a owl:DatatypeProperty ; rdfs:label "narration start"@en ; rdfs:range xsd:gYear ;
    rdfs:comment "First year the drawing narrates, read off its Year markers."@en .
lo:narrationEnd a owl:DatatypeProperty ; rdfs:label "narration end"@en ; rdfs:range xsd:gYear .
lo:titleDateRange a owl:DatatypeProperty ; rdfs:label "date range in title"@en .
lo:sharedActorCount a owl:DatatypeProperty ; rdfs:label "shared actor count"@en ; rdfs:range xsd:integer .
lo:jaccard a owl:DatatypeProperty ; rdfs:label "Jaccard similarity"@en ; rdfs:range xsd:decimal .
lo:heldBy a owl:ObjectProperty ; rdfs:label "held by"@en ; rdfs:domain lo:Drawing ;
    rdfs:comment "The museum holding the physical drawing."@en .
lo:momaPage a owl:DatatypeProperty ; rdfs:label "MoMA page"@en ;
    rdfs:comment "The work's page in MoMA's online collection."@en .
lo:momaImage a owl:DatatypeProperty ; rdfs:label "MoMA image"@en ;
    rdfs:comment "A photograph of the original sheet, hosted by MoMA. Artwork (c) The Estate of Mark Lombardi; linked, never redistributed, and NOT covered by this dataset's licence."@en .
lo:accession a owl:DatatypeProperty ; rdfs:label "accession number"@en .
lo:dimensions a owl:DatatypeProperty ; rdfs:label "dimensions"@en ;
    rdfs:comment "The size of the physical sheet, as catalogued."@en .
lo:creditLine a owl:DatatypeProperty ; rdfs:label "credit line"@en .
lo:imagePage a owl:DatatypeProperty ; rdfs:label "image page"@en ;
    rdfs:comment "A museum or gallery page showing the original drawing."@en .
lo:dataFile a owl:DatatypeProperty ; rdfs:label "data file"@en ;
    rdfs:comment "The GraphML / JSON / XGMML this graph was built from."@en .
''']

    # one predicate per arc type, carrying the visual convention as its comment
    LOOK = {
        "Association": "bidirectional solid arc",
        "BlockedFailed": "solid arc ending in a double line",
        "BlockedFailedTransaction": "dashed arc ending in a double line",
        "Connection": "plain arc, no decoration",
        "Final": "red arc",
        "FinancialAssociation": "bidirectional dashed arc",
        "FinancialConnection": "dashed arc",
        "FinancialTransaction": "directed dashed arc",
        "InfluenceControl": "directed solid arc",
        "SaleProperty": "curled arc",
        "SaleTransfer": "directional broken arc",
        "YearArrow": "solid arrow with a year marker at one end",
        "YearLine": "line along the timeline of year markers",
        "SingleNearby": "a lone node set beside another",
    }
    ttl.append("\n# One predicate per arc type. In Lombardi's notation the LINE STYLE is the\n"
               "# semantics, so each comment records the mark on the paper.\n")
    for a in arc_types:
        if not a:
            continue
        p = a[0].lower() + a[1:]
        ttl.append('lor:%s a owl:ObjectProperty ; rdfs:label "%s"@en ; rdfs:subPropertyOf lo:connectedTo ;\n'
                   '    rdfs:comment "Drawn as: %s."@en .\n' % (p, a, LOOK.get(a, "an arc")))
    # lombardi.owl gives its classes a comment but no label, which leaves them
    # unnamed in any UI. Label them here (the ontology itself is shipped verbatim).
    NODE_LOOK = {
        "Person": "a person",
        "Institution": "a company, ministry, bank or agency",
        "MergedInstitution": "an institution composed of two institutions",
        "Year": "a two-digit year in a small circle",
        "YearFinal": "a solid circle closing a timeline",
        "FinalInfo": "the outcome written at the end of a red arc",
        # "Final" is deliberately absent: the source uses it for BOTH a node and an
        # arc type, and the arc loop below labels it ("red arc"). Labelling it twice
        # would leave the class with two conflicting labels.
        "Node": "an untyped node",
    }
    ttl.append("\n# lombardi.owl types its classes but never labels them; label them here so\n"
               "# every class reads as words in a result table.\n")
    for c, look in sorted(NODE_LOOK.items()):
        ttl.append('lom:%s rdfs:label "%s"@en ; lo:drawnAs "%s"@en .\n' % (c, c, look))
    for a in arc_types:
        if a:
            ttl.append('lom:%s rdfs:label "%s"@en ; lo:drawnAs "%s"@en .\n'
                       % (a, a, LOOK.get(a, "an arc")))
    ttl.append('lo:drawnAs a owl:DatatypeProperty ; rdfs:label "drawn as"@en ;\n'
               '    rdfs:comment "The mark on the paper. In Lombardi\'s notation the line style IS the semantics."@en .\n')

    missing = [a for a in arc_types if a and a not in KNOWN]
    if missing:
        ttl.append("\n# Arc types present in the digitized data but absent from lombardi.owl v0.3,\n"
                   "# declared here so every arc in the graph has a class.\n")
        for a in missing:
            ttl.append('lom:%s a owl:Class ; rdfs:subClassOf lom:Edge ; rdfs:label "%s"@en ;\n'
                       '    rdfs:comment "Drawn as: %s. Not declared in lombardi.owl v0.3."@en .\n'
                       % (a, a, LOOK.get(a, "an arc")))
    return "".join(ttl)


if __name__ == "__main__":
    main()
