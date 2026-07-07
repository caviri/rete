#!/usr/bin/env python3
"""Harvest BCUL medieval manuscripts from e-codices (IIIF Presentation v2).

Collection: https://www.e-codices.unifr.ch/metadata/iiif/collection/bcul.json
For each manifest -> raw JSON, normalized unified record, and a thumbnail.
Small (≈15 manuscripts) so it runs synchronously.
"""
from __future__ import annotations

import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from http_util import Fetcher  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
COLLECTION = "https://www.e-codices.unifr.ch/metadata/iiif/collection/bcul.json"
YEAR_RE = re.compile(r"(1[0-9]{3}|[89][0-9]{2})")


def iiif_str(v):
    """Flatten a IIIF v2 value (str | {@value} | list) to a plain string, preferring English."""
    if v is None:
        return None
    if isinstance(v, str):
        return v
    if isinstance(v, dict):
        return v.get("@value") or v.get("value")
    if isinstance(v, list):
        en = [x for x in v if isinstance(x, dict) and x.get("@language") in ("en", "eng")]
        pick = (en or v)[0]
        return iiif_str(pick)
    return str(v)


def meta_map(manifest):
    out = {}
    for pair in manifest.get("metadata", []) or []:
        label = iiif_str(pair.get("label"))
        value = iiif_str(pair.get("value"))
        if label:
            out.setdefault(label, value)
    return out


def thumb_url(manifest):
    # manifest-level thumbnail first
    th = manifest.get("thumbnail")
    if isinstance(th, dict):
        if th.get("@id"):
            return th["@id"]
        svc = th.get("service", {})
        if isinstance(svc, dict) and svc.get("@id"):
            return svc["@id"].rstrip("/") + "/full/!300,300/0/default.jpg"
    if isinstance(th, str):
        return th
    # else first canvas image service
    try:
        canvas = manifest["sequences"][0]["canvases"][0]
        img = canvas["images"][0]["resource"]
        svc = img.get("service")
        if isinstance(svc, dict) and svc.get("@id"):
            return svc["@id"].rstrip("/") + "/full/!300,300/0/default.jpg"
        return img.get("@id")
    except (KeyError, IndexError, TypeError):
        return None


def canvas_count(manifest):
    try:
        return len(manifest["sequences"][0]["canvases"])
    except (KeyError, IndexError, TypeError):
        return None


def parse_dates(meta):
    for key in ("Date of Origin (English)", "Date of Origin", "Date", "Century", "Datation"):
        s = meta.get(key)
        if not s:
            continue
        years = [int(y) for y in re.findall(r"\b(1[0-9]{3})\b", s)]
        if years:
            return min(years), max(years)
        cents = [int(c) for c in re.findall(r"(\d{1,2})(?:st|nd|rd|th)\s*cent", s.lower())]
        if cents:
            return (min(cents) - 1) * 100, max(cents) * 100 - 1
    return None, None


def strip_html(s):
    return re.sub(r"<[^>]+>", "", s).strip() if s else s


def parse_persons(s):
    out = []
    for part in (s or "").split(";"):
        part = part.strip()
        if not part:
            continue
        if ":" in part:
            role, name = part.split(":", 1)
            out.append({"name": name.strip(), "role": role.strip().lower(), "main": False})
        else:
            out.append({"name": part, "role": None, "main": False})
    return out


def normalize(manifest, slug, harvested_at):
    meta = meta_map(manifest)
    label = iiif_str(manifest.get("label")) or ""
    title = meta.get("Title (English)") or meta.get("Title") or \
        (label.split(",")[-1].strip() if "," in label else label)
    start, end = parse_dates(meta)
    creators = parse_persons(meta.get("Persons") or meta.get("Author") or "")
    related = manifest.get("related")
    record_url = related.get("@id") if isinstance(related, dict) else (related if isinstance(related, str) else None)
    tei = None
    for sa in (manifest.get("seeAlso") or []):
        if isinstance(sa, dict) and "tei" in (sa.get("@format", "")):
            tei = sa.get("@id")
    lang = meta.get("Text Language") or meta.get("Language")
    place = meta.get("Place of Origin (English)") or meta.get("Place of Origin")
    extent = " ; ".join(b for b in (meta.get("Number of Pages"), meta.get("Dimensions"), meta.get("Material")) if b) or None
    subjects = [v for k, v in meta.items() if k in ("Liturgica christiana", "Document Type", "Genre") and v]
    ids = {"marc001": slug}
    if meta.get("DOI"):
        ids["doi"] = [meta["DOI"]]
    if tei:
        ids["tei_xml"] = tei
    return {
        "id": f"ecodices:{slug}",
        "source": "ecodices",
        "local_id": slug,
        "record_url": record_url,
        "type": "manuscript-text",
        "title": title,
        "title_full": label or None,
        "creators": creators,
        "publication": {"place": place, "publisher": None,
                        "date": meta.get("Date of Origin (English)") or meta.get("Century")},
        "date_start": start,
        "date_end": end,
        "languages": [lang] if lang else [],
        "subjects": subjects,
        "genres": ["manuscript"],
        "places": [place] if place else [],
        "shelfmark": meta.get("Shelfmark") or slug.replace("bcul-", ""),
        "collections": ["e-codices BCUL"],
        "extent": extent,
        "description": strip_html(meta.get("Summary (English)") or iiif_str(manifest.get("description"))),
        "notes": [],
        "identifiers": ids,
        "files": [],
        "iiif_manifest": manifest.get("@id"),
        "thumbnail_url": thumb_url(manifest),
        "thumbnail_local": None,
        "has_digital": True,
        "rights": iiif_str(manifest.get("license")) or iiif_str(manifest.get("attribution")),
        "provider": "e-codices — Virtual Manuscript Library of Switzerland (BCU Lausanne)",
        "iiif_canvases": canvas_count(manifest),
        "metadata_raw": meta,
        "harvested_at": harvested_at,
    }


def main():
    base = REPO / "data" / "bcul"
    raw_dir = base / "raw" / "ecodices" / "manifests"
    thumb_dir = base / "thumbnails" / "ecodices"
    raw_dir.mkdir(parents=True, exist_ok=True)
    thumb_dir.mkdir(parents=True, exist_ok=True)
    out_path = base / "normalized" / "ecodices.jsonl"

    f = Fetcher(rate=2)
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")

    data, _, _ = f.get(COLLECTION)
    (base / "raw" / "ecodices" / "collection.json").write_bytes(data)
    coll = json.loads(data)
    manifests = coll.get("manifests", [])
    print(f"BCUL collection: {len(manifests)} manifests")

    records = []
    for entry in manifests:
        murl = entry["@id"]
        slug = murl.rstrip("/").split("/")[-2] if murl.endswith("manifest.json") else murl.rstrip("/").split("/")[-1]
        mdata, _, status = f.get(murl)
        if mdata is None:
            print(f"  !! {status} {murl}")
            continue
        (raw_dir / f"{slug}.json").write_bytes(mdata)
        manifest = json.loads(mdata)
        rec = normalize(manifest, slug, now)
        # thumbnail
        if rec["thumbnail_url"]:
            timg, ctype, st = f.get(rec["thumbnail_url"])
            if timg and ctype and ctype.startswith("image/"):
                ext = ".jpg" if "jpeg" in ctype or "jpg" in ctype else "." + ctype.split("/")[-1]
                tpath = thumb_dir / f"{slug}{ext}"
                tpath.write_bytes(timg)
                rec["thumbnail_local"] = f"thumbnails/ecodices/{slug}{ext}"
        records.append(rec)
        print(f"  {slug}: {(rec['title'] or '')[:60]} | {rec['iiif_canvases']} folia | thumb={'Y' if rec['thumbnail_local'] else 'n'}")

    with open(out_path, "w", encoding="utf-8") as fh:
        for r in records:
            fh.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"wrote {len(records)} records -> {out_path}")


if __name__ == "__main__":
    main()
