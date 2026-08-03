#!/usr/bin/env python3
"""Flatten the harvested catalog into one JSONL record per item and emit
per-tier image URL manifests for the asset harvest.

    python extract_metadata.py [--raw DIR]

Primary input:  raw/catalog/os_*.json.gz      (harvest_catalog.py — LUNA API
                batches; id, displayName, ~38 metadata labels, urlSize0-4)
Optional overlay: raw/manifests/**/<id>.json.gz (harvest_manifests.py — adds
                master pixel width/height when present)

Writes raw/derived/rumsey_items.jsonl.gz   one JSON object per item:
         id, title, description, fields{label:[values]}, width, height,
         iiif_image, iiif_manifest, detail_url, image_path, jp2_url,
         url_size0..url_size4
       raw/assets/size{0,1,2,3,4}.tsv        <relpath>\t<url>
       raw/assets/jp2_masters.tsv            <relpath>\t<url>

<relpath> mirrors the collection's own path scheme (e.g. 229/18059000.jpg),
so every tier and the masters share one collision-free layout.
Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import gzip
import json
import re
import sys
from pathlib import Path

DL_RE = re.compile(r"download\.pl\?image=/([^ >'\"]+?)\.(jp2|sid)\b", re.I)
SIZE_RE = re.compile(r"/Size\d/RUMSEY~8~1/(.+?)\.jpg", re.I)


def path_of(rec: dict, fields: dict) -> tuple[str | None, str | None]:
    """(image_path like '229/18059000', jp2_url) from tier URLs / Download 1."""
    for k in ("urlSize2", "urlSize4", "urlSize0"):
        m = SIZE_RE.search(rec.get(k) or "")
        if m:
            path = m.group(1)
            break
    else:
        path = None
    jp2 = None
    for v in fields.get("Download 1", []):
        m = DL_RE.search(v)
        if m:
            path = path or m.group(1)
            jp2 = f"https://www.davidrumsey.com/static/jp2k/{m.group(1)}.{m.group(2).lower()}"
            break
    return path, jp2


def record_of(r: dict) -> dict | None:
    iid = r.get("id", "")
    if not iid:
        return None
    fields: dict[str, list[str]] = {}
    for fv in r.get("fieldValues", []):
        for label, vals in fv.items():
            if vals is None:
                continue
            vs = vals if isinstance(vals, list) else [vals]
            fields.setdefault(label, []).extend(str(v) for v in vs)
    path, jp2 = path_of(r, fields)
    return {
        "id": iid,
        "title": " ".join(str(r.get("displayName", "")).split()),
        "description": r.get("description") or None,
        "fields": fields,
        "width": None,   # overlay from IIIF manifest when harvested
        "height": None,
        "iiif_image": f"https://www.davidrumsey.com/luna/servlet/iiif/{iid}",
        "iiif_manifest": r.get("iiifManifest"),
        "detail_url": f"https://www.davidrumsey.com/luna/servlet/detail/{iid}",
        "image_path": path,
        "jp2_url": jp2,
        **{f"url_size{n}": r.get(f"urlSize{n}") for n in range(5)},
    }


def overlay_dims(recs: dict[str, dict], manifests: Path) -> int:
    n = 0
    for f in manifests.glob("*/*.json.gz"):
        iid = f.name[:-len(".json.gz")]
        rec = recs.get(iid)
        if rec is None or rec["width"] is not None:
            continue
        try:
            man = json.loads(gzip.open(f, "rt", encoding="utf-8").read())
            cv = man["sequences"][0]["canvases"][0]
            rec["width"], rec["height"] = cv.get("width"), cv.get("height")
            n += 1
        except Exception:  # noqa: BLE001 — dims are best-effort enrichment
            pass
    return n


def main() -> None:
    raw = Path("data/davidrumsey-maps/raw")
    if len(sys.argv) == 3 and sys.argv[1] == "--raw":
        raw = Path(sys.argv[2])
    cat, derived, assets = raw / "catalog", raw / "derived", raw / "assets"
    derived.mkdir(parents=True, exist_ok=True)
    assets.mkdir(parents=True, exist_ok=True)

    recs: dict[str, dict] = {}
    files = sorted(cat.glob("os_*.json.gz"))
    print(f"catalog batches: {len(files)}", flush=True)
    for i, f in enumerate(files, 1):
        d = json.loads(gzip.open(f, "rt", encoding="utf-8").read())
        for r in d.get("results", []):
            rec = record_of(r)
            if rec and rec["id"] not in recs:
                recs[rec["id"]] = rec
        if i % 50 == 0:
            print(f"  {i}/{len(files)} batches, {len(recs)} items", flush=True)

    if (raw / "manifests").is_dir():
        n = overlay_dims(recs, raw / "manifests")
        print(f"overlaid width/height from {n} IIIF manifests", flush=True)

    tiers: dict[int, list[tuple[str, str]]] = {n: [] for n in range(5)}
    masters: list[tuple[str, str]] = []
    n_nopath = 0
    with gzip.open(derived / "rumsey_items.jsonl.gz", "wt", encoding="utf-8") as jf:
        for rec in recs.values():
            jf.write(json.dumps(rec, ensure_ascii=False) + "\n")
            p = rec["image_path"]
            if not p:
                n_nopath += 1
                continue
            rel = f"{p}.jpg"
            for n in range(5):
                u = rec.get(f"url_size{n}")
                if u:
                    tiers[n].append((rel, u))
            if rec["jp2_url"]:
                masters.append((f"{p}.jp2", rec["jp2_url"]))

    for n, rows in tiers.items():
        with (assets / f"size{n}.tsv").open("w", encoding="utf-8") as f:
            for rel, url in rows:
                f.write(f"{rel}\t{url}\n")
    with (assets / "jp2_masters.tsv").open("w", encoding="utf-8") as f:
        for rel, url in masters:
            f.write(f"{rel}\t{url}\n")

    print(f"DONE items={len(recs)} no_image_path={n_nopath} "
          f"masters={len(masters)}", flush=True)
    print(f"  -> {derived / 'rumsey_items.jsonl.gz'}", flush=True)
    print(f"  -> {assets}/size0..4.tsv + jp2_masters.tsv", flush=True)


if __name__ == "__main__":
    main()
