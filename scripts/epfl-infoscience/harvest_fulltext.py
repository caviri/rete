"""Harvest the extracted FULL TEXT of open-access EPFL Infoscience publications.

DSpace-CRIS extracts the text of each open PDF into a `TEXT` bundle (one
`<file>.pdf.txt` bitstream per source file). ~81,268 of 192,451 publications
(~42%) have open-access content; the rest are metadata-only / restricted /
embargoed. Everything here is CC0 and downloadable anonymously.

For each publication that has original-bundle content, we fetch the item with
its bundles/bitstreams embedded, download every TEXT bitstream, concatenate,
and write one JSONL line: {uuid, handle, doi, name, n_text_bitstreams,
n_chars, text}.

- Resumable: uuids already written are skipped (a set is loaded from the output).
- WAF-friendly: browser UA, small delay, exponential backoff.

Usage:
  python scripts/epfl-infoscience/harvest_fulltext.py
  python scripts/epfl-infoscience/harvest_fulltext.py --max-items 50 --fresh   # quick test
"""

import argparse
import http.client
import http.cookiejar
import json
import os
import re
import time
import urllib.error
import urllib.parse
import urllib.request

# Maintain a session cookie (DSpace sets one) to avoid tripping the WAF/throttle
# on sustained anonymous access.
_OPENER = urllib.request.build_opener(
    urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

DISCOVER = "https://infoscience.epfl.ch/server/api/discover/search/objects"
ITEM = "https://infoscience.epfl.ch/server/api/core/items/{}?embed=bundles/bitstreams"
OUT = r"D:\pro\rete\data\epfl-infoscience\jsonl\fulltext.jsonl"
UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/125.0 Safari/537.36")
MAX_ATTEMPTS = 8
DOI_RE = re.compile(r"10\.\d{4,9}/\S+")


def fetch(url, raw=False):
    for attempt in range(1, MAX_ATTEMPTS + 1):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA,
                                         "Accept": "*/*" if raw else "application/json"})
            with _OPENER.open(req, timeout=120) as resp:
                data = resp.read()
            return data if raw else json.loads(data)
        except urllib.error.HTTPError as e:
            # restricted/embargoed bitstream (401/403/404 on a content download) → skip.
            # 401 on the listing/item endpoint means throttling → retry with backoff.
            if e.code in (403, 404) or (e.code == 401 and raw):
                return None
            wait = min(2 ** attempt, 120)
            print(f"      retry {attempt} ({e.code}) waiting {wait}s", flush=True)
            time.sleep(wait)
        except (urllib.error.URLError, ValueError, TimeoutError, OSError,
                http.client.HTTPException) as e:  # incl. IncompleteRead / chunked-read cutoffs
            wait = min(2 ** attempt, 120)
            print(f"      retry {attempt} ({type(e).__name__}: {e}) waiting {wait}s", flush=True)
            time.sleep(wait)
    return None


def md_first(md, field):
    v = md.get(field)
    return v[0].get("value") if v else None


def item_fulltext(uuid):
    """Return (name, doi, n_bitstreams, text) for the item's TEXT bundle, or None."""
    it = fetch(ITEM.format(uuid))
    if not it:
        return None
    md = it.get("metadata", {})
    doi = md_first(md, "dc.identifier.doi")
    if doi:
        doi = doi.lower().rstrip(".")
    bundles = it.get("_embedded", {}).get("bundles", {}).get("_embedded", {}).get("bundles", [])
    parts = []
    for b in bundles:
        if b.get("name") != "TEXT":
            continue
        bss = b.get("_embedded", {}).get("bitstreams", {}).get("_embedded", {}).get("bitstreams", [])
        for bs in bss:
            href = bs.get("_links", {}).get("content", {}).get("href")
            if not href:
                continue
            raw = fetch(href, raw=True)
            if raw:
                parts.append(raw.decode("utf-8", "replace"))
    if not parts:
        return None
    return it.get("name"), doi, len(parts), "\n\n".join(parts)


def load_done(path):
    done = set()
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    done.add(json.loads(line)["uuid"])
                except Exception:  # noqa: BLE001
                    pass
    return done


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--size", type=int, default=100)
    ap.add_argument("--delay", type=float, default=0.25)
    ap.add_argument("--max-items", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    if args.fresh and os.path.exists(args.out):
        os.remove(args.out)
    done = load_done(args.out)
    print(f"resuming: {len(done):,} already harvested" if done else "starting fresh", flush=True)

    n_new = n_txt = 0
    total = None
    t0 = time.time()
    with open(args.out, "a", encoding="utf-8") as out:
        page = 0
        while True:
            q = urllib.parse.urlencode({"dsoType": "item",
                                        "f.entityType": "Publication,equals",
                                        "f.has_content_in_original_bundle": "true,equals",
                                        "size": args.size, "page": page})
            sr = (fetch(f"{DISCOVER}?{q}") or {}).get("_embedded", {}).get("searchResult", {})
            if total is None:
                total = sr.get("page", {}).get("totalElements")
            objs = sr.get("_embedded", {}).get("objects", [])
            if not objs:
                break
            for wrap in objs:
                obj = wrap.get("_embedded", {}).get("indexableObject", {})
                uuid = obj.get("uuid")
                if not uuid or uuid in done:
                    continue
                res = item_fulltext(uuid)
                done.add(uuid)
                n_new += 1
                if res:
                    name, doi, nb, text = res
                    out.write(json.dumps({"uuid": uuid, "handle": obj.get("handle"),
                                          "doi": doi, "name": name,
                                          "n_text_bitstreams": nb, "n_chars": len(text),
                                          "text": text}, ensure_ascii=False) + "\n")
                    n_txt += 1
                if n_new % 100 == 0:
                    out.flush()
                    rate = n_new / max(time.time() - t0, 1)
                    print(f"seen {len(done):,}/{total} | new {n_new:,} | with-text {n_txt:,} | "
                          f"{rate:.1f} item/s", flush=True)
                if args.max_items and n_new >= args.max_items:
                    out.flush()
                    print(f"reached --max-items; {n_txt} with text")
                    return
                time.sleep(args.delay)
            page += 1
    print(f"DONE: {n_txt:,} publications with full text (of {n_new:,} seen) "
          f"in {(time.time()-t0)/60:.1f} min -> {args.out}")


if __name__ == "__main__":
    main()
