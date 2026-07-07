#!/usr/bin/env python3
"""Harvest the STRUCTURE + THUMBNAILS of Scriptorium (BCU Lausanne digitized press).

Platform: MediaINFO (AngularJS SPA) at https://www.scriptorium.ch/
  (scriptorium.bcu-lausanne.ch redirects here).

WHAT MACHINE INTERFACES EXIST (probed 2026-07-07):
  * /robots.txt            -> Crawl-delay: 10; DISALLOWS /api/app/, /api/browse, /api/search/
                              (also /views/ /scripts/ /styles/ /images/ and build.json).
  * /sitemap_index.xml     -> 14 gzipped sub-sitemaps (sitemap0..13.xml.gz).
                              Together = 683,080 URLs, ALL of the form
                              https://www.scriptorium.ch/zoom/<id>/view  (one per ISSUE object).
  * /api/  (MediaINFO API) -> /api/status works; /api/item/<id> returns rich JSON metadata;
                              /api/item/<id>/thumbnail returns a JPEG miniature.
                              These per-object endpoints are NOT covered by any robots Disallow.
  * NO OAI-PMH, NO usable public IIIF image API, NO sitemap of titles/issues:
    the browse/category tree lives only behind /api/browse + /api/app/ which robots DISALLOWS.

STRATEGY (robots-compliant):
  * We NEVER touch /api/app/, /api/browse or /api/search/.
  * The full issue universe (all 683,080 zoom ids) is taken from the ALLOWED sitemaps and
    saved as a complete index to  raw/scriptorium/issue_index.tsv.gz .
  * TITLES (serials) are not directly enumerable (that needs the disallowed browse API), so we
    RECONSTRUCT them by aggregating fields.category / fields.title from a stratified sample of
    /api/item/<id> responses. Because a newspaper's issues occupy a contiguous id-block, an even
    stride reliably discovers every title whose run is larger than the stride; very small runs
    may be missed (logged in the report + each serial's `notes`).
  * ISSUE-level records: 683k issues cannot be fetched politely in full, so we enrich a
    stratified SAMPLE and write those as issue records. The cap is logged, never silent; the
    complete id list is in raw/ so a future run can enrich more.

Rate-limited & resumable. stdlib only + sibling http_util.Fetcher.

Usage:
    python harvest_scriptorium.py [--sample N] [--rate R] [--no-thumbs]
"""
from __future__ import annotations

import argparse
import gzip
import json
import re
import sys
import unicodedata
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from http_util import Fetcher  # noqa: E402

BASE = "https://www.scriptorium.ch"
PROVIDER = "Scriptorium — BCU Lausanne"
SOURCE = "scriptorium"

# robots.txt Disallow prefixes we must NEVER request (checked defensively at call sites).
ROBOTS_DISALLOW = ("/api/app/", "/api/browse", "/api/search/",
                   "/views/", "/scripts/", "/styles/", "/images/", "/build.json")

ROOT = Path(__file__).resolve().parents[2] / "data" / "bcul"
RAW = ROOT / "raw" / "scriptorium"
THUMBS = ROOT / "thumbnails" / "scriptorium"
NORM = ROOT / "normalized" / "scriptorium.jsonl"
SITEMAP_INDEX = f"{BASE}/sitemap_index.xml"


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def robots_ok(path: str) -> bool:
    return not any(path.startswith(p) for p in ROBOTS_DISALLOW)


def slugify(s: str) -> str:
    s = unicodedata.normalize("NFKD", s).encode("ascii", "ignore").decode("ascii")
    s = re.sub(r"[^a-zA-Z0-9]+", "-", s).strip("-").lower()
    return s or "untitled"


# --------------------------------------------------------------------------- #
# Step 1: sitemaps -> complete issue-id index
# --------------------------------------------------------------------------- #
LOC_RE = re.compile(r"<loc>\s*([^<]+?)\s*</loc>")
MOD_RE = re.compile(r"<lastmod>\s*([^<]+?)\s*</lastmod>")
ZOOM_RE = re.compile(r"/zoom/(\d+)/view")


def download_sitemaps(fx: Fetcher) -> list[str]:
    """Return list of local sub-sitemap .gz paths, downloading any missing ones."""
    idx_path = RAW / "sitemap_index.xml"
    if not idx_path.exists():
        data, _ct, st = fx.get(SITEMAP_INDEX)
        if st != 200 or not data:
            raise RuntimeError(f"sitemap_index.xml -> HTTP {st}")
        idx_path.write_bytes(data)
    idx_xml = idx_path.read_text("utf-8", "replace")
    subs = [m for m in LOC_RE.findall(idx_xml) if m.endswith(".xml.gz")]
    local = []
    for url in subs:
        name = url.rsplit("/", 1)[-1]
        p = RAW / name
        if not p.exists():
            data, _ct, st = fx.get(url)
            if st != 200 or not data:
                print(f"  WARN sitemap {name} -> HTTP {st}", file=sys.stderr)
                continue
            p.write_bytes(data)
        local.append(p)
    return [str(p) for p in local]


def build_issue_index(sitemap_paths: list[str]) -> list[tuple[int, str]]:
    """Parse every sub-sitemap -> sorted [(id, lastmod)], cache to issue_index.tsv.gz."""
    out_path = RAW / "issue_index.tsv.gz"
    if out_path.exists():
        rows = []
        with gzip.open(out_path, "rt", encoding="utf-8") as fh:
            for line in fh:
                iid, lastmod = line.rstrip("\n").split("\t")
                rows.append((int(iid), lastmod))
        return rows
    rows: dict[int, str] = {}
    for p in sitemap_paths:
        with gzip.open(p, "rt", encoding="utf-8") as fh:
            xml = fh.read()
        # <url><loc>...</loc><lastmod>...</lastmod></url> — parse per <url> block
        for block in xml.split("<url>")[1:]:
            loc = LOC_RE.search(block)
            if not loc:
                continue
            zm = ZOOM_RE.search(loc.group(1))
            if not zm:
                continue
            mod = MOD_RE.search(block)
            rows[int(zm.group(1))] = mod.group(1) if mod else ""
    ordered = sorted(rows.items())
    with gzip.open(out_path, "wt", encoding="utf-8") as fh:
        for iid, lastmod in ordered:
            fh.write(f"{iid}\t{lastmod}\n")
    return ordered


# --------------------------------------------------------------------------- #
# Step 2: /api/item fetching (cached / resumable)
# --------------------------------------------------------------------------- #
def load_item_cache() -> dict[int, dict]:
    cache: dict[int, dict] = {}
    p = RAW / "items_raw.jsonl.gz"
    if p.exists():
        with gzip.open(p, "rt", encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                    if isinstance(obj, dict) and "id" in obj:
                        cache[int(obj["id"])] = obj
                except Exception:
                    pass
    return cache


def fetch_item(fx: Fetcher, iid: int, cache: dict[int, dict], sink) -> dict | None:
    if iid in cache:
        return cache[iid]
    path = f"/api/item/{iid}"
    if not robots_ok(path):
        return None
    data, ctype, st = fx.get(f"{BASE}{path}")
    if st != 200 or not data or "json" not in (ctype or ""):
        return None
    try:
        obj = json.loads(data.decode("utf-8", "replace"))
    except Exception:
        return None
    if not isinstance(obj, dict) or "id" not in obj:
        return None
    cache[iid] = obj
    sink.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sink.flush()
    return obj


# --------------------------------------------------------------------------- #
# Step 3: metadata extraction from an /api/item object
# --------------------------------------------------------------------------- #
def _field_terms(fields: dict, key: str) -> list:
    """Flatten fields[key] across locales into a flat list of term-dicts/strings."""
    out = []
    for _loc, arr in (fields.get(key) or {}).items():
        for a in arr:
            if isinstance(a, list):
                out.extend(a)
            else:
                out.append(a)
    return out


def _names(fields: dict, key: str) -> list[str]:
    out = []
    for t in _field_terms(fields, key):
        if isinstance(t, dict):
            n = t.get("name")
            if n:
                out.append(n)
        elif isinstance(t, str):
            out.append(t)
    return out


def _first_str(fields: dict, key: str) -> str | None:
    for _loc, arr in (fields.get(key) or {}).items():
        for a in arr:
            if isinstance(a, str) and a.strip():
                return a.strip()
    return None


def _lang_codes(fields: dict) -> list[str]:
    codes = []
    for t in _field_terms(fields, "language"):
        if isinstance(t, dict) and t.get("value"):
            codes.append(t["value"])
    return sorted(set(codes))


def _cf14_date(fields: dict):
    """Return (from_ymd, to_ymd) where each is (y,m,d) or None, from the cf14 date field."""
    cf = fields.get("cf14") or {}
    frm = to = None
    for _loc, v in cf.items():
        if not isinstance(v, dict):
            continue
        if isinstance(v.get("from"), dict):
            d = v["from"]
            frm = (d.get("year"), d.get("month"), d.get("day"))
        if isinstance(v.get("to"), dict):
            d = v["to"]
            to = (d.get("year"), d.get("month"), d.get("day"))
    return frm, to


def _ymd_str(ymd) -> str | None:
    if not ymd or not ymd[0]:
        return None
    y, m, d = ymd
    if m and d:
        return f"{y:04d}-{m:02d}-{d:02d}"
    if m:
        return f"{y:04d}-{m:02d}"
    return f"{y:04d}"


def extract(obj: dict) -> dict:
    """Pull the fields we care about out of one /api/item object."""
    fields = obj.get("fields") or {}
    frm, to = _cf14_date(fields)
    year_from = frm[0] if frm and frm[0] else None
    year_to = to[0] if to and to[0] else None
    date_iso = _ymd_str(to) or _ymd_str(frm)
    return {
        "id": int(obj["id"]),
        "path": obj.get("path") or f"/zoom/{obj['id']}/view",
        "title": _first_str(fields, "title"),
        "classification": _names(fields, "classification"),
        "categories": _names(fields, "category"),
        "publishers": _names(fields, "publisher"),
        "languages": _lang_codes(fields),
        "shelfmark": _first_str(fields, "description"),
        "rights": _first_str(fields, "cf13"),
        "periodicity": _names(fields, "periodicity"),
        "format": _names(fields, "format"),
        "year_from": year_from,
        "year_to": year_to,
        "date_iso": date_iso,
    }


# --------------------------------------------------------------------------- #
# Step 4: thumbnails
# --------------------------------------------------------------------------- #
def thumb_url(iid: int) -> str:
    return f"{BASE}/api/item/{iid}/thumbnail"


def download_thumb(fx: Fetcher, iid: int) -> str | None:
    """Download /api/item/<id>/thumbnail -> thumbnails/scriptorium/<id>.jpg. Return rel path."""
    rel = f"thumbnails/scriptorium/{iid}.jpg"
    dst = THUMBS / f"{iid}.jpg"
    if dst.exists() and dst.stat().st_size > 0:
        return rel
    path = f"/api/item/{iid}/thumbnail"
    if not robots_ok(path):
        return None
    try:
        data, ctype, st = fx.get(f"{BASE}{path}")
    except Exception:
        return None
    if st != 200 or not data or not data[:2] == b"\xff\xd8":
        return None
    dst.write_bytes(data)
    return rel


# --------------------------------------------------------------------------- #
# main
# --------------------------------------------------------------------------- #
def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=1600,
                    help="number of issue ids to enrich via /api/item (stratified)")
    ap.add_argument("--rate", type=float, default=1.3,
                    help="max requests/second (robots asks crawl-delay:10; task allows <=2)")
    ap.add_argument("--no-thumbs", action="store_true")
    ap.add_argument("--issue-thumb-cap", type=int, default=1600,
                    help="max issue thumbnails to download")
    args = ap.parse_args()

    for d in (RAW, THUMBS, NORM.parent):
        d.mkdir(parents=True, exist_ok=True)

    fx = Fetcher(rate=args.rate)
    harvested = now_iso()

    print("[1/5] sitemaps ...", file=sys.stderr)
    sm = download_sitemaps(fx)
    index = build_issue_index(sm)
    total_issues = len(index)
    print(f"      {len(sm)} sub-sitemaps, {total_issues} issue ids", file=sys.stderr)

    # stratified sample (even stride across the sorted id list)
    n = min(args.sample, total_issues)
    stride = max(1, total_issues // n)
    sample = index[::stride][:n]
    print(f"[2/5] stratified sample: {len(sample)} issues (stride {stride})", file=sys.stderr)

    print("[3/5] fetching /api/item for sample (resumable cache) ...", file=sys.stderr)
    cache = load_item_cache()
    sink = gzip.open(RAW / "items_raw.jsonl.gz", "at", encoding="utf-8")
    items: list[tuple[dict, str]] = []  # (extracted, lastmod)
    got = miss = 0
    try:
        for k, (iid, lastmod) in enumerate(sample, 1):
            obj = fetch_item(fx, iid, cache, sink)
            if obj is None:
                miss += 1
                continue
            items.append((extract(obj), lastmod))
            got += 1
            if k % 100 == 0:
                print(f"      {k}/{len(sample)}  ok={got} miss={miss} req={fx.n_requests}",
                      file=sys.stderr)
    finally:
        sink.close()
    print(f"      fetched ok={got} miss={miss}", file=sys.stderr)

    # ---- aggregate serials (group issues by title string) ----
    serials: dict[str, dict] = {}
    for ex, _lastmod in items:
        title = ex["title"] or "(untitled)"
        s = serials.get(title)
        if s is None:
            s = serials[title] = {
                "title": title,
                "classification": set(),
                "collections": set(),
                "publishers": set(),
                "languages": set(),
                "periodicity": set(),
                "format": set(),
                "shelfmark": None,
                "rights": None,
                "year_min": None,
                "year_max": None,
                "sample_issue_count": 0,
                "repr_id": ex["id"],
                "repr_year": ex["year_to"] or ex["year_from"],
            }
        s["classification"].update(ex["classification"])
        s["collections"].update(ex["categories"])
        s["publishers"].update(ex["publishers"])
        s["languages"].update(ex["languages"])
        s["periodicity"].update(ex["periodicity"])
        s["format"].update(ex["format"])
        s["shelfmark"] = s["shelfmark"] or ex["shelfmark"]
        s["rights"] = s["rights"] or ex["rights"]
        for y in (ex["year_from"], ex["year_to"]):
            if y:
                s["year_min"] = y if s["year_min"] is None else min(s["year_min"], y)
                s["year_max"] = y if s["year_max"] is None else max(s["year_max"], y)
        # representative = earliest issue for a stable, meaningful thumbnail
        ry = ex["year_to"] or ex["year_from"]
        if ry and (s["repr_year"] is None or ry < s["repr_year"]):
            s["repr_year"], s["repr_id"] = ry, ex["id"]
        s["sample_issue_count"] += 1

    sample_frac = (got / total_issues) if total_issues else 0.0

    print(f"[4/5] discovered {len(serials)} distinct titles (serials)", file=sys.stderr)

    # ---- thumbnails ----
    thumbs_done = 0
    repr_ids = {s["repr_id"] for s in serials.values()}
    if not args.no_thumbs:
        print("      downloading thumbnails ...", file=sys.stderr)
        # serial representatives first (always), then sampled issues up to the cap
        order = list(repr_ids) + [ex["id"] for ex, _ in items if ex["id"] not in repr_ids]
        for iid in order:
            if thumbs_done >= args.issue_thumb_cap and iid not in repr_ids:
                break
            if download_thumb(fx, iid):
                thumbs_done += 1
        print(f"      thumbnails downloaded: {thumbs_done}", file=sys.stderr)

    def have_thumb(iid: int) -> str | None:
        p = THUMBS / f"{iid}.jpg"
        return f"thumbnails/scriptorium/{iid}.jpg" if p.exists() else None

    # ---- write normalized records: serials first, then issues ----
    print("[5/5] writing normalized records ...", file=sys.stderr)
    n_serial = n_issue = 0
    with open(NORM, "w", encoding="utf-8") as out:
        for title, s in sorted(serials.items()):
            slug = slugify(title)
            local_id = f"serial-{slug}"
            notes = [
                "Serial reconstructed by aggregating fields.category/fields.title from a "
                f"stratified {got}-issue sample (~1 per {stride}) of the platform's "
                f"{total_issues} issue objects; the MediaINFO browse/category API is "
                "robots-disallowed, so titles are not directly enumerable.",
                f"date_start/date_end and sample_issue_count are SAMPLE-DERIVED "
                f"(sampling fraction ~{sample_frac:.4f}); estimated total issues for this "
                f"title ~= {round(s['sample_issue_count'] / sample_frac) if sample_frac else 'n/a'}.",
            ]
            if s["periodicity"]:
                notes.append("periodicity: " + "; ".join(sorted(s["periodicity"])))
            if s["format"]:
                notes.append("format: " + "; ".join(sorted(s["format"])))
            rec = {
                "id": f"{SOURCE}:{local_id}",
                "source": SOURCE,
                "local_id": local_id,
                "record_url": None,  # browse/title landing is a robots-disallowed SPA route
                "type": "serial",
                "title": title,
                "title_full": None,
                "publication": {
                    "place": "Lausanne",
                    "publisher": "; ".join(sorted(s["publishers"])) or None,
                    "date": (f"{s['year_min']}-{s['year_max']}"
                             if s["year_min"] and s["year_max"] else None),
                },
                "date_start": s["year_min"],
                "date_end": s["year_max"],
                "languages": sorted(s["languages"]),
                "shelfmark": s["shelfmark"],
                "collections": sorted(s["collections"] | s["classification"]),
                "notes": notes,
                "identifiers": {"mediainfo_repr_item": str(s["repr_id"])},
                "iiif_manifest": None,
                "thumbnail_url": thumb_url(s["repr_id"]),
                "thumbnail_local": have_thumb(s["repr_id"]),
                "has_digital": True,
                "rights": s["rights"],
                "provider": PROVIDER,
                "harvested_at": harvested,
            }
            out.write(json.dumps(rec, ensure_ascii=False) + "\n")
            n_serial += 1

        for ex, lastmod in items:
            iid = ex["id"]
            title = ex["title"] or "(untitled)"
            disp = f"{title} — {ex['date_iso']}" if ex["date_iso"] else title
            rec = {
                "id": f"{SOURCE}:{iid}",
                "source": SOURCE,
                "local_id": str(iid),
                "record_url": f"{BASE}{ex['path']}",
                "type": "issue",
                "title": disp,
                "title_full": None,
                "publication": {
                    "place": "Lausanne",
                    "publisher": "; ".join(ex["publishers"]) or None,
                    "date": ex["date_iso"],
                },
                "date_start": ex["year_from"] or ex["year_to"],
                "date_end": ex["year_to"] or ex["year_from"],
                "languages": ex["languages"],
                "shelfmark": ex["shelfmark"],
                "collections": sorted(set(ex["categories"]) | set(ex["classification"])),
                "notes": [f"sitemap lastmod: {lastmod}"] if lastmod else [],
                "identifiers": {"mediainfo_item": str(iid), "serial_title": title},
                "iiif_manifest": None,
                "thumbnail_url": thumb_url(iid),
                "thumbnail_local": have_thumb(iid),
                "has_digital": True,
                "rights": ex["rights"],
                "provider": PROVIDER,
                "harvested_at": harvested,
            }
            out.write(json.dumps(rec, ensure_ascii=False) + "\n")
            n_issue += 1

    # ---- run report ----
    report = {
        "harvested_at": harvested,
        "platform": "MediaINFO (AngularJS SPA)",
        "interfaces": {
            "sitemap_index": SITEMAP_INDEX,
            "item_api": f"{BASE}/api/item/<id>",
            "thumbnail_api": f"{BASE}/api/item/<id>/thumbnail",
            "robots_disallowed": ["/api/app/", "/api/browse", "/api/search/"],
        },
        "total_issue_universe": total_issues,
        "sample_size_requested": args.sample,
        "issues_enriched": n_issue,
        "sampling_stride": stride,
        "sampling_fraction": round(sample_frac, 6),
        "serials_discovered": n_serial,
        "thumbnails_downloaded": thumbs_done,
        "requests_made": fx.n_requests,
    }
    (RAW / "harvest_report.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print("\n=== SCRIPTORIUM HARVEST SUMMARY ===", file=sys.stderr)
    print(json.dumps(report, ensure_ascii=False, indent=2), file=sys.stderr)


if __name__ == "__main__":
    main()
