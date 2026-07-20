"""Harvest the full EPFL Infoscience repository via the DSpace-CRIS REST API,
one JSONL file per entity type.

Infoscience (https://infoscience.epfl.ch) runs DSpace-CRIS. Its REST discover
endpoint pages through every item of a given `entityType` with no
deep-pagination wall, returning rich JSON (the full metadata map incl. dc.*,
epfl.*, cris.*, oairecerif.* fields and the `authority` links to related
entities). All metadata is CC0.

Entity types & counts (2026-07):
  Publication 192,451 · Event 29,262 · Person 22,663 · Journal 14,735 ·
  OrgUnit 1,422 · Patent 1,896 · Product 812 · Funding 43

Output: data/epfl-infoscience/jsonl/<entitytype>.jsonl — one item per line,
the indexableObject (uuid/handle/name/entityType/lastModified + full metadata),
minus HATEOAS `_links`, plus derived join keys (doi, orcid, sciper).

- Resumable per type: page + count checkpointed after each page; a re-run
  truncates any partial tail and continues.
- WAF-friendly: browser User-Agent, small delay, exponential backoff.

Usage:
  python scripts/epfl-infoscience/harvest_rest.py                 # all types
  python scripts/epfl-infoscience/harvest_rest.py --types Person,OrgUnit,Journal,Event
  python scripts/epfl-infoscience/harvest_rest.py --types Journal --max-pages 2 --fresh
"""

import argparse
import json
import os
import re
import time
import urllib.error
import urllib.parse
import urllib.request

API = "https://infoscience.epfl.ch/server/api/discover/search/objects"
OUTDIR = r"D:\pro\rete\data\epfl-infoscience\jsonl"
UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/125.0 Safari/537.36")
DEFAULT_TYPES = ["Publication", "Person", "OrgUnit", "Journal", "Event",
                 "Patent", "Product", "Funding"]
MAX_ATTEMPTS = 8
DOI_RE = re.compile(r"10\.\d{4,9}/\S+")


def fetch_json(url):
    for attempt in range(1, MAX_ATTEMPTS + 1):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA,
                                                       "Accept": "application/json"})
            with urllib.request.urlopen(req, timeout=120) as resp:
                return json.loads(resp.read())
        except (urllib.error.HTTPError, urllib.error.URLError, ValueError,
                TimeoutError, OSError) as e:
            wait = min(2 ** attempt, 120)
            code = getattr(e, "code", "")
            print(f"      retry {attempt}/{MAX_ATTEMPTS} ({code} {e}) waiting {wait}s", flush=True)
            time.sleep(wait)
    raise RuntimeError(f"giving up on {url}")


def md_first(md, field):
    v = md.get(field)
    return v[0].get("value") if v else None


def extract_row(obj):
    md = obj.get("metadata", {})
    doi = md_first(md, "dc.identifier.doi")
    if not doi:
        for f in ("dc.identifier", "dc.relation.uri", "dc.identifier.uri"):
            for entry in md.get(f, []):
                m = DOI_RE.search(entry.get("value", ""))
                if m:
                    doi = m.group(0)
                    break
            if doi:
                break
    if doi:
        doi = doi.lower().rstrip(".")
    return {
        "uuid": obj.get("uuid"),
        "handle": obj.get("handle"),
        "name": obj.get("name"),
        "entityType": obj.get("entityType"),
        "lastModified": obj.get("lastModified"),
        "inArchive": obj.get("inArchive"),
        "discoverable": obj.get("discoverable"),
        "withdrawn": obj.get("withdrawn"),
        "doi": doi,
        "orcid": md_first(md, "person.identifier.orcid"),
        "sciper": md_first(md, "epfl.sciperId"),
        "metadata": md,
    }


def load_ckpt(path):
    cp = path + ".ckpt.json"
    if os.path.exists(cp):
        with open(cp) as f:
            return json.load(f)
    return {"page": 0, "count": 0, "done": False}


def save_ckpt(path, cp):
    tmp = path + ".ckpt.json.tmp"
    with open(tmp, "w") as f:
        json.dump(cp, f)
    os.replace(tmp, path + ".ckpt.json")


def truncate_to(path, n):
    if not os.path.exists(path):
        return
    tmp, kept = path + ".trunc", 0
    with open(path, encoding="utf-8") as fin, open(tmp, "w", encoding="utf-8") as fout:
        for line in fin:
            if kept >= n:
                break
            fout.write(line)
            kept += 1
    os.replace(tmp, path)


def harvest_type(etype, args):
    path = os.path.join(args.out, f"{etype.lower()}.jsonl")
    cp = {"page": 0, "count": 0, "done": False} if args.fresh else load_ckpt(path)
    if cp.get("done"):
        print(f"[{etype}] already complete: {cp['count']:,} items")
        return cp["count"]
    if args.fresh and os.path.exists(path):
        os.remove(path)
    if cp["count"]:
        print(f"[{etype}] resuming at page {cp['page']} ({cp['count']:,} items); trimming tail")
        truncate_to(path, cp["count"])

    total = None
    t0 = time.time()
    with open(path, "a", encoding="utf-8") as out:
        while True:
            q = urllib.parse.urlencode({"dsoType": "item",
                                        "f.entityType": f"{etype},equals",
                                        "size": args.size, "page": cp["page"]})
            sr = fetch_json(f"{API}?{q}").get("_embedded", {}).get("searchResult", {})
            if total is None:
                total = sr.get("page", {}).get("totalElements")
            objs = sr.get("_embedded", {}).get("objects", [])
            if not objs:
                cp["done"] = True
                save_ckpt(path, cp)
                break
            for wrap in objs:
                obj = wrap.get("_embedded", {}).get("indexableObject", {})
                out.write(json.dumps(extract_row(obj), ensure_ascii=False) + "\n")
            out.flush()
            cp["count"] += len(objs)
            cp["page"] += 1
            save_ckpt(path, cp)
            pct = f"{100*cp['count']/total:.1f}%" if total else "?"
            print(f"[{etype}] page {cp['page']:>5} | {cp['count']:>7,}/{total} ({pct})", flush=True)
            if len(objs) < args.size:
                cp["done"] = True
                save_ckpt(path, cp)
                break
            if args.max_pages and cp["page"] >= args.max_pages:
                break
            time.sleep(args.delay)
    print(f"[{etype}] DONE {cp['count']:,} in {(time.time()-t0)/60:.1f} min -> {path}")
    return cp["count"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--types", default=",".join(DEFAULT_TYPES))
    ap.add_argument("--out", default=OUTDIR)
    ap.add_argument("--size", type=int, default=100)
    ap.add_argument("--delay", type=float, default=0.3)
    ap.add_argument("--max-pages", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    grand = 0
    for etype in [t.strip() for t in args.types.split(",") if t.strip()]:
        grand += harvest_type(etype, args)
    print(f"ALL DONE: {grand:,} items total across types")


if __name__ == "__main__":
    main()
