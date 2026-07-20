#!/usr/bin/env python3
"""Build the dance-kg Parquet companions from the CoMPAS3D raw takes.

Produces three tables under <out_dir>/ :
  segments.parquet - one row per annotated segment (denormalised: performance, pair,
                     skill, section, primary move + all move types, hold, turns, timing,
                     derived motion metrics). Mirrors the graph's Segment entities.
  frames.parquet   - per-frame motion for every usable take (30 fps): leader/follower
                     pelvis xyz, follower-in-leader-frame offset, couple separation,
                     min hand distance, contact flag. The robot / virtual-env export
                     surface; NOT in the graph (too fat for triples).
and a combined dance.duckdb with both + the flat `triples` table (if present).

Usage:  python build_parquet.py <raw_compas3d_dir> <out_dir>
"""
import sys, re
from pathlib import Path
import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_nt as B  # reuse PAIR_SKILL, SONGS, derived_series, parse_desc, SECTION_CLASS


def main():
    root = Path(sys.argv[1])
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)

    seg_rows = []
    frame_cols = {k: [] for k in
                  ("performance", "pair", "skill", "song", "take", "frame", "t",
                   "leader_x", "leader_y", "leader_z", "follower_x", "follower_y", "follower_z",
                   "follower_forward", "follower_left", "separation", "hand_distance", "hand_contact")}

    for td in sorted(root.glob("Pair*/*")):
        m = re.match(r"Pair(\d+)_song(\d+)_take(\d+)", td.name)
        if not m:
            continue
        pn, sn, tk = map(int, m.groups())
        perf = f"pair{pn}-song{sn}-take{tk}"
        skill = B.PAIR_SKILL[pn]
        stem = td.name

        lead = td / f"{stem}_leader.npz"
        if not lead.exists():
            alt = td / f"{stem}_leaderi.npz"
            lead = alt if alt.exists() else lead
        fol = td / f"{stem}_follower.npz"
        ser = None
        if lead.exists() and fol.exists():
            try:
                ser = B.derived_series(lead, fol)
            except Exception as e:
                print(f"  ! motion load failed {stem}: {e}", file=sys.stderr)

        # per-frame rows (all usable takes, annotated or not)
        if ser is not None:
            n = ser["n"]; fps = ser["fps"]
            t = np.arange(n) / fps
            contact = (ser["hand"] < B.CONTACT_THRESH)
            frame_cols["performance"] += [perf] * n
            frame_cols["pair"] += [pn] * n
            frame_cols["skill"] += [skill] * n
            frame_cols["song"] += [sn] * n
            frame_cols["take"] += [tk] * n
            frame_cols["frame"] += list(range(n))
            frame_cols["t"] += [round(float(x), 4) for x in t]
            frame_cols["leader_x"] += [round(float(x), 4) for x in ser["lt"][:, 0]]
            frame_cols["leader_y"] += [round(float(x), 4) for x in ser["lt"][:, 1]]
            frame_cols["leader_z"] += [round(float(x), 4) for x in ser["lt"][:, 2]]
            frame_cols["follower_x"] += [round(float(x), 4) for x in ser["ft"][:, 0]]
            frame_cols["follower_y"] += [round(float(x), 4) for x in ser["ft"][:, 1]]
            frame_cols["follower_z"] += [round(float(x), 4) for x in ser["ft"][:, 2]]
            frame_cols["follower_forward"] += [round(float(x), 4) for x in ser["fwd"]]
            frame_cols["follower_left"] += [round(float(x), 4) for x in ser["left"]]
            frame_cols["separation"] += [round(float(x), 4) for x in ser["sep"]]
            frame_cols["hand_distance"] += [round(float(x), 4) for x in ser["hand"]]
            frame_cols["hand_contact"] += [bool(x) for x in contact]

        # segment rows (annotated takes only)
        txt = td / f"{stem}.txt"
        if not txt.exists():
            continue
        for i, line in enumerate(txt.read_text(encoding="utf-8", errors="replace").splitlines()):
            p = line.split("\t")
            if len(p) < 9:
                continue
            try:
                t0, t1 = float(p[3]), float(p[5])
            except ValueError:
                continue
            section = p[0].strip()
            desc = p[8].strip()
            if not desc:
                continue
            is_error = section == "Errors"
            moves, holds, tl, tf = B.parse_desc(desc, is_error)
            row = {
                "segment": f"{perf}-seg{i:03d}", "performance": perf,
                "pair": pn, "skill": skill, "song": sn, "take": tk,
                "section": section,
                "primary_move": moves[0] if moves else None,
                "moves": moves, "hold": holds[0] if holds else None,
                "turn_by_leader": tl, "turn_by_follower": tf,
                "start_s": round(t0, 3), "end_s": round(t1, 3), "duration_s": round(t1 - t0, 3),
                "description": desc,
            }
            if ser is not None:
                i0, i1 = int(t0 * ser["fps"]), min(int(t1 * ser["fps"]), ser["n"])
                if i1 > i0:
                    row["mean_separation"] = round(float(ser["sep"][i0:i1].mean()), 3)
                    row["follower_mean_forward"] = round(float(ser["fwd"][i0:i1].mean()), 3)
                    row["follower_mean_left"] = round(float(ser["left"][i0:i1].mean()), 3)
                    hm = ser["hand"][i0:i1]
                    row["mean_hand_distance"] = round(float(hm.mean()), 3)
                    row["hand_contact_fraction"] = round(float((hm < B.CONTACT_THRESH).mean()), 3)
            seg_rows.append(row)

    # --- segments.parquet ---
    seg_cols = ["segment", "performance", "pair", "skill", "song", "take", "section",
                "primary_move", "moves", "hold", "turn_by_leader", "turn_by_follower",
                "start_s", "end_s", "duration_s", "mean_separation", "follower_mean_forward",
                "follower_mean_left", "mean_hand_distance", "hand_contact_fraction", "description"]
    seg_table = pa.table({c: [r.get(c) for r in seg_rows] for c in seg_cols})
    pq.write_table(seg_table, out / "segments.parquet", compression="zstd")

    # --- frames.parquet ---
    frame_table = pa.table(frame_cols)
    pq.write_table(frame_table, out / "frames.parquet", compression="zstd")

    print(f"segments.parquet: {seg_table.num_rows} rows, {len(seg_cols)} cols")
    print(f"frames.parquet:   {frame_table.num_rows} rows, {len(frame_cols)} cols")


if __name__ == "__main__":
    main()
