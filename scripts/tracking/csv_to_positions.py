#!/usr/bin/env python3
"""Metrica Home+Away tracking CSVs -> compact positions.json for the replay app.

Same shape export_positions.py produces from the .rete, but straight from the raw
CSVs so we can add matches without building a 350 MB .nt each. Downsamples 25->5 fps,
scales normalized [0,1] coords to a 105x68 m pitch, stores decimetre ints (absent=null).

Usage: csv_to_positions.py <home.csv> <away.csv> <out.json>
"""
import csv, json, sys
from pathlib import Path

FPS = 5
STEP = 25 // FPS
PITCH_L, PITCH_W = 105.0, 68.0

def read_team(path, team, want_ball):
    rows = list(csv.reader(open(path, encoding="utf-8")))
    names = rows[2]                      # Period,Frame,Time [s],Player11,,Player1,,...,Ball,
    cols = []                            # (objid, label, jersey, is_ball, xcol)
    for i in range(3, len(names)):
        nm = names[i].strip()
        if not nm:
            continue
        if nm.lower() == "ball":
            if want_ball:
                cols.append(("ball", "Ball", None, True, i))
        elif nm.lower().startswith("player"):
            jersey = nm[6:].strip()
            cols.append((f"{team.lower()}_{jersey}", f"{team} #{jersey}", jersey, False, i))
    objs = [(c[0], c[1], c[2], c[3]) for c in cols]
    def frames():
        for r in rows[3:]:
            if len(r) < 3 or not r[1].strip().isdigit():
                continue
            fr = int(r[1])
            if fr % STEP != 0:
                continue
            pos = {}
            for oid, _, _, _, xc in cols:
                try:
                    x = r[xc].strip(); y = r[xc + 1].strip()
                    if x == "" or y == "" or x.lower() == "nan" or y.lower() == "nan":
                        continue
                    fx = float(x); fy = float(y)
                    if fx != fx or fy != fy:
                        continue
                    pos[oid] = (fx * PITCH_L, fy * PITCH_W)
                except (IndexError, ValueError):
                    continue
            yield (fr, float(r[2]), pos)
    return objs, frames

def main():
    home_csv, away_csv, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    home_objs, home_frames = read_team(home_csv, "Home", want_ball=True)
    away_objs, away_frames = read_team(away_csv, "Away", want_ball=False)
    objs = home_objs + away_objs
    oidx = {o[0]: i for i, o in enumerate(objs)}
    out_objs = [{"id": oid, "label": label,
                 "team": (None if is_ball else label.split()[0]), "ball": is_ball}
                for oid, label, jersey, is_ball in objs]

    byframe = {}                         # fr -> {"t":sec, "pos":{oid:(x,y)}}
    for fr, t, pos in home_frames():
        byframe.setdefault(fr, {"t": t, "pos": {}})["pos"].update(pos)
    for fr, t, pos in away_frames():
        byframe.setdefault(fr, {"t": t, "pos": {}})["pos"].update(pos)

    frames = []
    for fr in sorted(byframe):
        rec = byframe[fr]
        flat = [None] * (len(objs) * 2)
        for oid, (x, y) in rec["pos"].items():
            flat[oidx[oid] * 2] = round(x * 10)
            flat[oidx[oid] * 2 + 1] = round(y * 10)
        frames.append([round(rec["t"], 1), flat])

    doc = {"pitch": [105, 68], "fps": FPS, "objects": out_objs, "frames": frames}
    Path(out_path).write_text(json.dumps(doc, separators=(",", ":")), encoding="utf-8")
    dur = frames[-1][0] - frames[0][0] if frames else 0
    print(f"{out_path}: {len(out_objs)} objects, {len(frames)} frames, "
          f"{dur/60:.1f} min, {Path(out_path).stat().st_size/1e6:.1f} MB")

if __name__ == "__main__":
    main()
