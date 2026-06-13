#!/usr/bin/env python3
"""Linked Open Samian Ware (RGZM/samian-lod, DPPL) TTL dumps -> atlas N-Triples.

Joins three inputs (passed as a directory of .ttl files):
  - actor files (ae_*.ttl): `samian:ae_N rdfs:label '...'@en .` + `samian:ae_N lado:timeinterval '...' .`
  - ct_ae_pc_1.ttl:          `samian:ae_N lado:worksAtPlace samian:loc_pc_M .`
  - loc_productioncentre_1.ttl: `samian:loc_pc_M_geom geosparql:asWKT "<CRS> POINT(lon lat)"^^... .`
Emits one INTERVAL Roman potter per (potter with a parseable date AND a located centre):

  <http://ex/samian/ae_N> a <http://ex/Potter> ; rdfs:label "..."@en ;
      <http://ex/startYear> S ; <http://ex/endYear> E ; geo:hasGeometry <.../geom> .
  <.../geom> geo:asWKT "Point(lon lat)"^^wktLiteral .

Free-text lado:timeinterval -> years: 'AD 65-75'->65..75 ; '10 BC - AD 15'->-10..15 ;
'15 - 5 BC'->-15..-5 ; 'AD 30+'->30..55 ; '10 BC+'->-10..15 ; 'AD 40'->40..40. Prose-only
periods ('Tiberian', 'First century', 'Unknown') are dropped.

Usage:  python3 scripts/samian_to_nt.py data/atlas-extra/samian > samian.nt
"""
import glob
import os
import re
import sys

GEO = "http://www.opengis.net/ont/geosparql#"
EX = "http://ex/"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"
OPEN = 25  # 'AD 30+' open-ended span length

_NUM = r"\d{1,4}"


def parse_interval(t):
    """Free-text Samian date -> (start, end) signed years, or None."""
    s = t.strip()
    low = s.lower()
    if not s or "unknown" in low:
        return None
    # 'AD 65-75' / 'AD 65 - 75'
    m = re.fullmatch(rf"ad\s*({_NUM})\s*[-–]\s*({_NUM})", low)
    if m:
        return int(m.group(1)), int(m.group(2))
    # '10 BC - AD 15'
    m = re.fullmatch(rf"({_NUM})\s*bc\s*[-–]\s*ad\s*({_NUM})", low)
    if m:
        return -int(m.group(1)), int(m.group(2))
    # '15 - 5 BC'  (both BC)
    m = re.fullmatch(rf"({_NUM})\s*[-–]\s*({_NUM})\s*bc", low)
    if m:
        return -int(m.group(1)), -int(m.group(2))
    # 'AD 30+' open-ended
    m = re.fullmatch(rf"ad\s*({_NUM})\s*\+", low)
    if m:
        y = int(m.group(1)); return y, y + OPEN
    # '10 BC+' open-ended
    m = re.fullmatch(rf"({_NUM})\s*bc\s*\+", low)
    if m:
        y = -int(m.group(1)); return y, y + OPEN
    # bare 'AD 40'
    m = re.fullmatch(rf"ad\s*({_NUM})", low)
    if m:
        y = int(m.group(1)); return y, y
    return None  # prose-only ('Tiberian', 'First century', ...)


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")


def main():
    d = sys.argv[1]
    label, interval, works_at, centre = {}, {}, {}, {}
    for path in glob.glob(os.path.join(d, "*.ttl")):
        text = open(path, encoding="utf-8").read()
        for ae, lbl in re.findall(r"samian:(ae_\d+)\s+rdfs:label\s+'(.*?)'@en", text):
            label[ae] = lbl
        for ae, iv in re.findall(r"samian:(ae_\d+)\s+lado:timeinterval\s+'(.*?)'", text):
            interval[ae] = iv
        for ae, pc in re.findall(r"samian:(ae_\d+)\s+lado:worksAtPlace\s+samian:(loc_pc_\d+)", text):
            works_at[ae] = pc
        for pc, lon, lat in re.findall(r"samian:(loc_pc_\d+)_geom\s+geosparql:asWKT\s+\".*?POINT\(([-\d.]+)\s+([-\d.]+)\)", text):
            centre[pc] = (lon, lat)

    out = sys.stdout
    kept = 0
    for ae, lbl in label.items():
        iv = interval.get(ae)
        pc = works_at.get(ae)
        if not iv or not pc or pc not in centre:
            continue
        yrs = parse_interval(iv)
        if not yrs:
            continue
        sy, ey = yrs
        lon, lat = centre[pc]
        x = f"{EX}samian/{ae}"
        g = f"{x}/geom"
        out.write(f"<{x}> <{RDF_TYPE}> <{EX}Potter> .\n")
        out.write(f'<{x}> <{RDFS_LABEL}> "{esc(lbl)}"@en .\n')
        out.write(f'<{x}> <{EX}startYear> "{sy}"^^<{XSD_INT}> .\n')
        out.write(f'<{x}> <{EX}endYear> "{ey}"^^<{XSD_INT}> .\n')
        out.write(f"<{x}> <{GEO}hasGeometry> <{g}> .\n")
        out.write(f'<{g}> <{GEO}asWKT> "Point({lon} {lat})"^^<{GEO}wktLiteral> .\n')
        kept += 1
    print(f"samian: {kept} potters emitted", file=sys.stderr)


if __name__ == "__main__":
    main()
