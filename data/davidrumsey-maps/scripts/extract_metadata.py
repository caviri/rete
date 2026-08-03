#!/usr/bin/env python3
"""Flatten the harvested IIIF manifests into one JSONL record per item and
emit per-tier image URL manifests for the asset harvest.

    python extract_metadata.py [--manifests DIR] [--out DIR]

Reads  raw/manifests/**/<id>.json.gz   (from harvest_manifests.py)
Writes raw/derived/rumsey_items.jsonl.gz   one JSON object per item:
         id, title, fields{label:[values]}, width, height, iiif_image,
         thumbnail, detail_url, image_path (e.g. "229/18059000"), jp2_url
       raw/assets/size{0,1,2,3,4}.tsv        <relpath>\t<url>
       raw/assets/jp2_masters.tsv            <relpath>\t<url>

Image tier URL scheme (derived from the "Download 1" JP2 href in each record):
  Size0-2 (~96/~192/~768px): https://www.davidrumsey.com/rumsey/Size{N}/RUMSEY~8~1/<dir>/<stem>.jpg
  Size3-4 (~1.5k/~3k px):    https://media.davidrumsey.com/MediaManager/srvr?mediafile=/Size{N}/RUMSEY~8~1/<dir>/<stem>.jpg
  JP2 master (full res):     https://www.davidrumsey.com/static/jp2k/<dir>/<stem>.jp2

Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import gzip
import json
import re
import sys
from pathlib import Path

DL_RE = re.compile(r"download\.pl\?image=/([^ >'\"]+?)\.(jp2|sid)\b", re.I)


def parse_args(argv: list[str]):
    manifests = Path("data/davidrumsey-maps/raw/manifests")
    out = Path("data/davidrumsey-maps/raw")
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--manifests":
            manifests = Path(argv[i + 1]); i += 2; continue
        if a == "--out":
            out = Path(argv[i + 1]); i += 2; continue
        sys.exit(f"unknown arg: {a}")
    return manifests, out


def record_of(man: dict) -> dict | None:
    mid = man.get("@id", "")
    iid = mid.split("/iiif/m/", 1)[-1].rsplit("/manifest", 1)[0] if "/iiif/m/" in mid else ""
    seqs = man.get("sequences") or [{}]
    canvases = seqs[0].get("canvases") or []
    if not iid or not canvases:
        return None
    cv = canvases[0]
    fields: dict[str, list[str]] = {}
    for m in cv.get("metadata") or []:
        label, value = str(m.get("label", "")).strip(), m.get("value")
        if not label or value is None:
            continue
        fields.setdefault(label, []).append(str(value))
    img = (cv.get("images") or [{}])[0].get("resource", {})
    svc = img.get("service") or {}
    rec = {
        "id": iid,
        "title": " ".join(str(man.get("label", "")).split()),
        "fields": fields,
        "width": cv.get("width"),
        "height": cv.get("height"),
        "iiif_image": svc.get("@id"),
        "thumbnail": (cv.get("thumbnail") or {}).get("@id"),
        "detail_url": man.get("related"),
        "canvases": len(canvases),
        "image_path": None,
        "jp2_url": None,
    }
    for v in fields.get("Download 1", []):
        m = DL_RE.search(v)
        if m:
            rec["image_path"] = m.group(1)          # e.g. "229/18059000"
            rec["jp2_url"] = f"https://www.davidrumsey.com/static/jp2k/{m.group(1)}.{m.group(2).lower()}"
            break
    return rec


def main() -> None:
    manifests, out = parse_args(sys.argv[1:])
    derived, assets = out / "derived", out / "assets"
    derived.mkdir(parents=True, exist_ok=True)
    assets.mkdir(parents=True, exist_ok=True)

    files = sorted(manifests.glob("*/*.json.gz"))
    print(f"manifests={len(files)}", flush=True)

    tiers = {n: [] for n in range(5)}
    masters: list[tuple[str, str]] = []
    n_ok = n_bad = n_multi = n_nopath = 0
    with gzip.open(derived / "rumsey_items.jsonl.gz", "wt", encoding="utf-8") as jf:
        for i, f in enumerate(files, 1):
            try:
                man = json.loads(gzip.open(f, "rt", encoding="utf-8").read())
                rec = record_of(man)
            except Exception as e:  # noqa: BLE001 — count, don't die
                print(f"  BAD {f.name}: {type(e).__name__}: {e}", flush=True)
                n_bad += 1
                continue
            if rec is None:
                n_bad += 1
                continue
            jf.write(json.dumps(rec, ensure_ascii=False) + "\n")
            n_ok += 1
            if rec["canvases"] > 1:
                n_multi += 1
            p = rec["image_path"]
            if not p:
                n_nopath += 1
            else:
                rel = f"{p}.jpg"
                for n in (0, 1, 2):
                    tiers[n].append((rel, f"https://www.davidrumsey.com/rumsey/Size{n}/RUMSEY~8~1/{p}.jpg"))
                for n in (3, 4):
                    tiers[n].append((rel, f"https://media.davidrumsey.com/MediaManager/srvr?mediafile=/Size{n}/RUMSEY~8~1/{p}.jpg"))
                masters.append((f"{p}.jp2", rec["jp2_url"]))
            if i % 10000 == 0:
                print(f"  {i}/{len(files)}", flush=True)

    for n, rows in tiers.items():
        with (assets / f"size{n}.tsv").open("w", encoding="utf-8") as f:
            for rel, url in rows:
                f.write(f"{rel}\t{url}\n")
    with (assets / "jp2_masters.tsv").open("w", encoding="utf-8") as f:
        for rel, url in masters:
            f.write(f"{rel}\t{url}\n")

    print(f"DONE items={n_ok} bad={n_bad} multi_canvas={n_multi} no_image_path={n_nopath}", flush=True)
    print(f"  -> {derived / 'rumsey_items.jsonl.gz'}", flush=True)
    print(f"  -> {assets}/size0..4.tsv + jp2_masters.tsv", flush=True)


if __name__ == "__main__":
    main()
