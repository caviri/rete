#!/usr/bin/env python3
"""Ingest a Metrica Sports tracking match into a spatiotemporal .rete graph.

Metrica raw tracking: 25 fps, normalized [0,1] x/y for every player + the ball,
Home and Away in separate CSVs (3 header rows: team, jersey, column names).

We downsample to FPS frames/sec, scale to a 105x68 m pitch, and emit one
Position per (object, sampled-frame): object + time + x + y. Positions are keyed
by zero-padded frame index so they sort in TIME order in the index — a
time-window query reads a contiguous byte range.
"""
from __future__ import annotations
import csv, sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
RAW = REPO / "data" / "tracking" / "raw"
OUT = REPO / "data" / "tracking" / "tracking.nt"
FPS = 5                      # downsample target (source is 25 fps)
STEP = 25 // FPS            # keep every STEP-th frame
PITCH_L, PITCH_W = 105.0, 68.0

TR="https://w3id.org/rete/tracking#"; B="https://w3id.org/rete/tracking/"
SC="http://schema.org/"; RDF="http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS="http://www.w3.org/2000/01/rdf-schema#"; XSD="http://www.w3.org/2001/XMLSchema#"

_out=[]
def iri(s,p,o): _out.append(f"<{s}> <{p}> <{o}> .")
def lit(s,p,v,dt=None): _out.append(f'<{s}> <{p}> "{v}"'+(f"^^<{dt}>" if dt else "")+" .")

def read_team(path, team, want_ball):
    """Return (objects, frames) where objects=[(objid,label,jersey,is_ball)],
    frames=iterator of (period, frame, time, {objid:(x,y)})."""
    rows=list(csv.reader(open(path, encoding="utf-8")))
    names=rows[2]                       # Period,Frame,Time [s],Player11,,Player1,,...,Ball,
    cols=[]                             # (objid, label, jersey, is_ball, xcol)
    for i in range(3, len(names)):
        nm=names[i].strip()
        if not nm: continue
        if nm.lower()=="ball":
            if want_ball: cols.append(("ball","Ball",None,True,i))
        elif nm.lower().startswith("player"):
            jersey=nm[6:].strip()
            oid=f"{team.lower()}_{jersey}"
            cols.append((oid, f"{team} #{jersey}", jersey, False, i))
    objs=[(c[0],c[1],c[2],c[3]) for c in cols]
    def frames():
        for r in rows[3:]:
            if len(r)<3 or not r[1].strip().isdigit(): continue
            fr=int(r[1])
            if fr % STEP != 0: continue
            pos={}
            for oid,_,_,_,xc in cols:
                try:
                    x=r[xc].strip(); y=r[xc+1].strip()
                    if x=="" or y=="" or x.lower()=="nan" or y.lower()=="nan": continue
                    fx=float(x); fy=float(y)
                    if fx!=fx or fy!=fy: continue      # skip NaN
                    pos[oid]=(fx*PITCH_L, fy*PITCH_W)
                except (IndexError,ValueError): continue
            yield (int(r[0]), fr, float(r[2]), pos)
    return objs, frames

def main():
    home_objs, home_frames = read_team(RAW/"Home.csv", "Home", want_ball=True)
    away_objs, away_frames = read_team(RAW/"Away.csv", "Away", want_ball=False)
    # match + object metadata
    mi=B+"match/2"
    iri(mi, RDF, TR+"TrackedMatch"); lit(mi, RDFS+"label","Metrica Sports — Sample Game 2")
    lit(mi, TR+"fps", FPS, dt=XSD+"integer")
    lit(mi, TR+"pitchLength", PITCH_L, dt=XSD+"decimal"); lit(mi, TR+"pitchWidth", PITCH_W, dt=XSD+"decimal")
    lit(mi, TR+"source","Metrica Sports open sample data (anonymised)")
    for oid,label,jersey,is_ball in home_objs+away_objs:
        o=B+"obj/"+oid
        iri(o, RDF, TR+("Ball" if is_ball else "Player")); lit(o, RDFS+"label", label)
        if not is_ball:
            lit(o, TR+"team", label.split()[0]);
            if jersey: lit(o, TR+"jersey", jersey, dt=XSD+"integer")
    # positions — iterate both teams in lockstep by frame index
    def emit(frames):
        n=0
        for period, fr, t, pos in frames:
            idx=f"{fr:06d}"
            for oid,(x,y) in pos.items():
                p=B+f"pos/{idx}_{oid}"
                iri(p, TR+"object", B+"obj/"+oid)
                lit(p, TR+"t", round(t,2), dt=XSD+"decimal")
                lit(p, TR+"x", round(x,1), dt=XSD+"decimal")
                lit(p, TR+"y", round(y,1), dt=XSD+"decimal")
                n+=1
            yield
        emit.count=n
    for _ in emit(home_frames()): pass
    hn=emit.count
    for _ in emit(away_frames()): pass
    an=emit.count
    OUT.write_text("\n".join(_out)+"\n", encoding="utf-8")
    print(f"triples: {len(_out):,}  positions: home {hn:,} + away {an:,}  objects: {len(home_objs)+len(away_objs)}")

if __name__=="__main__": main()
