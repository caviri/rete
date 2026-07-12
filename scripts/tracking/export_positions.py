#!/usr/bin/env python3
"""Export the tracking graph's positions to a compact JSON the replay viewer
embeds. Reads tracking.nt (the exact data in tracking.rete). Coords stored as
integers in decimetres (x*10, y*10) to shrink the payload; absent objects = null."""
import json, re
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
NT = REPO / "data" / "tracking" / "tracking.nt"
OUT = REPO / "data" / "tracking" / "positions.json"
TR="https://w3id.org/rete/tracking#"

def main():
    objmeta={}                       # objid -> {label, team, ball}
    frames={}                        # frameIdx -> {t, {objid:(x,y)}}
    posbuf={}                        # posIRI -> {obj,t,x,y}
    for line in NT.open(encoding="utf-8"):
        # object metadata
        m=re.match(r'<[^>]*obj/([^>]+)> <([^>]+)> (.+) \.$', line)
        if m and "/obj/" in line and "/pos/" not in line:
            oid,p,o=m.group(1),m.group(2),m.group(3)
            d=objmeta.setdefault(oid,{"label":oid,"team":None,"ball":False})
            if p.endswith("#label"): d["label"]=o.strip('"').split('"')[0]
            elif p==TR+"team": d["team"]=o.strip('"').split('"')[0]
            elif p.endswith("#type") and o.endswith("Ball>"): d["ball"]=True
            continue
        # positions
        m=re.match(r'<[^>]*pos/(\d+)_([^>]+)> <([^>]+)> (.+) \.$', line)
        if not m: continue
        idx,oid,p,o=int(m.group(1)),m.group(2),m.group(3),m.group(4)
        fr=frames.setdefault(idx,{"t":None,"pos":{}})
        if p==TR+"t": fr["t"]=float(o.split('"')[1])
        elif p==TR+"x": fr["pos"].setdefault(oid,[None,None])[0]=round(float(o.split('"')[1])*10)
        elif p==TR+"y": fr["pos"].setdefault(oid,[None,None])[1]=round(float(o.split('"')[1])*10)

    objs=sorted(objmeta.keys())
    oidx={o:i for i,o in enumerate(objs)}
    out_objs=[{"id":o,"label":objmeta[o]["label"],"team":objmeta[o]["team"],"ball":objmeta[o]["ball"]} for o in objs]
    out_frames=[]
    for idx in sorted(frames):
        fr=frames[idx]; row=[round(fr["t"] or 0,1)]
        flat=[None]*(len(objs)*2)
        for oid,(x,y) in fr["pos"].items():
            if x is not None and y is not None:
                flat[oidx[oid]*2]=x; flat[oidx[oid]*2+1]=y
        row.append(flat); out_frames.append(row)
    doc={"pitch":[105,68],"fps":5,"objects":out_objs,"frames":out_frames}
    OUT.write_text(json.dumps(doc, separators=(",",":")), encoding="utf-8")
    print(f"objects: {len(objs)}, frames: {len(out_frames)}, json: {OUT.stat().st_size/1e6:.1f} MB")

if __name__=="__main__": main()
