#!/usr/bin/env python3
"""Enrich the MIrA Wikidata-aligned RDF for the rete playground.

MIrA (Manuscripts with Irish Associations, mira.ie, Pádraic Moran, Univ. of Galway,
CC BY-NC-SA 4.0) ships a Wikidata-aligned Turtle dump (data/rdf/mira_wikidata_aligned.ttl):
manuscripts, libraries, people and texts described with standard Wikidata properties
(P31 instance-of, P195 collection, P217 shelfmark, P571 inception, P1071 origin,
P2048/9 dimensions, P1574 exemplar-of) and wd:Q* values; people carry owl:sameAs to
Wikidata. But the opaque P/Q codes don't read, and the IIIF manifests live only in the
per-manuscript XML. This produces two side files to merge with the TTL at build time:

  mira_extra.nt   - IIIF manifest URLs (wdt:P6108, as IRIs so the playground renders the
                    image viewer) + a few literal extras from the XML (name, script,
                    folios, contents) under a small mira: namespace.
  mira_labels.nt  - rdfs:label for every wd:Q* / wdt:P* used (fetched from the Wikidata
                    API), plus the mira: predicate labels, so queries/schema read in
                    plain English (P1071 -> "location of creation", Q142 -> "France").
"""
import glob
import json
import os
import re
import urllib.request
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = os.path.join(HERE, "data", "mira", "repo")
TTL = os.path.join(REPO, "data", "rdf", "mira_wikidata_aligned.ttl")
MSS = os.path.join(REPO, "data", "mss_mira")
OUT_EXTRA = os.path.join(HERE, "data", "mira", "mira_extra.nt")
OUT_LABELS = os.path.join(HERE, "data", "mira", "mira_labels.nt")

ENT = "https://mira.ie/entity/manuscript/"
CATBASE = "https://mira.ie/entity/category/"   # MIrA inclusion-criterion categories
CATXML = os.path.join(REPO, "data", "other", "categories.xml")
PROP = "https://mira.ie/prop/"          # mira: literal extras
WDT = "http://www.wikidata.org/prop/direct/"
WD = "http://www.wikidata.org/entity/"
P6108 = WDT + "P6108"                   # IIIF manifest
RDFS = "http://www.w3.org/2000/01/rdf-schema#label"


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", " ").replace("\r", " ").replace("\t", " ")).strip()


def lit(s):
    return '"' + esc(re.sub(r"\s+", " ", s)) + '"'


def main():
    # The inclusion-criterion categories (the /about "Criteria for inclusion"):
    # id -> label, from data/other/categories.xml.
    cat_labels = {}
    try:
        for c in ET.parse(CATXML).getroot().iter("category"):
            cid = c.get("id")
            if cid and (c.text or "").strip():
                cat_labels[cid] = c.text.strip()
    except (ET.ParseError, FileNotFoundError):
        pass

    extra = []
    cats_used = set()
    n_iiif = n_ms = 0
    for path in sorted(glob.glob(os.path.join(MSS, "*.xml"))):
        try:
            root = ET.parse(path).getroot()
        except ET.ParseError:
            continue
        mid = root.get("id")
        if not mid:
            continue
        s = f"<{ENT}{mid}>"
        n_ms += 1
        # IIIF manifests (one or more) -> P6108 as IRIs (the viewer auto-detects them)
        for link in root.iter("link"):
            if (link.get("type") or "").lower() == "iiif" and (link.text or "").strip():
                extra.append(f"{s} <{P6108}> <{link.text.strip()}> .")
                n_iiif += 1
        name = root.findtext(".//identifier/ms_name")
        if name and name.strip():
            extra.append(f"{s} <{PROP}name> {lit(name)} .")
        scr = root.findtext(".//description/script")
        if scr and scr.strip():
            extra.append(f"{s} <{PROP}script> {lit(scr)} .")
        fol = root.findtext(".//description/folios")
        if fol and fol.strip():
            extra.append(f"{s} <{PROP}folios> {lit(fol)} .")
        cont = root.find(".//description/contents")
        if cont is not None:
            txt = " ".join(cont.itertext()).strip()
            if txt:
                extra.append(f"{s} <{PROP}contents> {lit(txt[:1000])} .")
        # inclusion-criterion categories (notes/@categories = "#sc-ire #vern ...")
        for nt in root.iter("notes"):
            for code in (nt.get("categories") or "").split():
                cid = code.lstrip("#")
                if cid in cat_labels:
                    extra.append(f"{s} <{PROP}category> <{CATBASE}{cid}> .")
                    cats_used.add(cid)
    with open(OUT_EXTRA, "w", encoding="utf-8") as f:
        f.write("\n".join(extra) + "\n")
    print(f"extra: {n_ms} mss, {n_iiif} IIIF manifests, {len(cats_used)} categories -> {OUT_EXTRA}")

    # --- labels: every wd:Q* and wdt:P* used, from the Wikidata API ---------------
    ttl = open(TTL, encoding="utf-8").read()
    qids = sorted(set(re.findall(r"wd:(Q\d+)", ttl)) | set(re.findall(r"entity/(Q\d+)", ttl)))
    pids = sorted(set(re.findall(r"wdt:(P\d+)", ttl)) | {"P6108"})
    labels = {}

    def fetch(ids):
        url = ("https://www.wikidata.org/w/api.php?action=wbgetentities&props=labels"
               "&languages=en&format=json&ids=" + "|".join(ids))
        req = urllib.request.Request(url, headers={"User-Agent": "rete-mira/1.0 (research)"})
        data = json.load(urllib.request.urlopen(req, timeout=60))
        for k, v in data.get("entities", {}).items():
            lbl = v.get("labels", {}).get("en", {}).get("value")
            if lbl:
                labels[k] = lbl

    allids = qids + pids
    for i in range(0, len(allids), 50):
        fetch(allids[i:i + 50])

    lines = []
    for q in qids:
        if q in labels:
            lines.append(f"<{WD}{q}> <{RDFS}> {lit(labels[q])}@en .")
    for p in pids:
        if p in labels:
            lines.append(f"<{WDT}{p}> <{RDFS}> {lit(labels[p])}@en .")
    # mira: predicate labels
    for pn, pl in [("name", "manuscript name"), ("script", "script"),
                   ("folios", "folios"), ("contents", "contents"),
                   ("category", "inclusion criterion")]:
        lines.append(f"<{PROP}{pn}> <{RDFS}> {lit(pl)}@en .")
    # category node labels (the /about criteria)
    for cid in sorted(cats_used):
        lines.append(f"<{CATBASE}{cid}> <{RDFS}> {lit(cat_labels[cid])}@en .")
    with open(OUT_LABELS, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print(f"labels: {len(qids)} Q + {len(pids)} P codes resolved -> {OUT_LABELS}")


if __name__ == "__main__":
    main()
