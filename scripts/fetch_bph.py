#!/usr/bin/env python3
"""Harvest the Embassy of the Free Mind / Bibliotheca Philosophica Hermetica (bph)
digital collection from Source Library (sourcelibrary.org) into local JSON, for
building a .rete knowledge graph.

Data sources (all public, no key):
  * enumerate:  GET /api/books/browse?library=bph&limit=100&skip=N  -> {books,total}
  * per book:   GET /api/books/{id}   -> full record (metadata, dublin_core, AI
                summary/significance, subject index, chapters, locations,
                related_books, per-page image URLs). ~480 KB each.
  * cover:      each book's `thumbnail` (images.sourcelibrary.org) -> one image/book.

Full per-page OCR/translation TEXT is NOT harvested here: /api/books/{id}/text is
rate-limited (100 pages/day anon) and the bulk /api/dataset/v1/pages needs an API
key (sourcelibrary.org/dataset). Text (CC-BY-SA-4.0) can be added later with a key.

Phases (idempotent / resumable):
  python scripts/fetch_bph.py enumerate
  python scripts/fetch_bph.py books      # skips already-downloaded ids
  python scripts/fetch_bph.py covers     # skips already-downloaded covers
  python scripts/fetch_bph.py all
"""
import concurrent.futures as cf
import json
import os
import sys
import time
import urllib.request
import urllib.error

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "data", "bph")
BOOKS_DIR = os.path.join(OUT, "books")
COVERS_DIR = os.path.join(OUT, "covers")
INDEX = os.path.join(OUT, "index.json")
BASE = "https://sourcelibrary.org"
LIBRARY = "bph"
UA = "Mozilla/5.0 (compatible; rete-dataset-harvester/1.0; +https://github.com/caviri/rete)"
WORKERS = 8
RETRIES = 4

for d in (OUT, BOOKS_DIR, COVERS_DIR):
    os.makedirs(d, exist_ok=True)


def fetch(url, binary=False, tries=RETRIES):
    last = None
    for i in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=90) as r:
                data = r.read()
            return data if binary else data.decode("utf-8")
        except urllib.error.HTTPError as e:
            last = e
            if e.code in (404, 400, 410):
                return None            # permanent — don't retry
            time.sleep(1.5 * (i + 1))  # 429/5xx — back off
        except Exception as e:
            last = e
            time.sleep(1.0 * (i + 1))
    print(f"  ! give up {url}: {last}", flush=True)
    return None


def enumerate_books():
    all_books, skip, total = [], 0, None
    while True:
        url = f"{BASE}/api/books/browse?library={LIBRARY}&limit=100&skip={skip}"
        txt = fetch(url)
        if not txt:
            break
        d = json.loads(txt)
        books = d.get("books", [])
        total = d.get("total", total)
        all_books.extend(books)
        print(f"  enumerate skip={skip} got={len(books)} total={total} accum={len(all_books)}", flush=True)
        skip += 100
        if not books or (total is not None and len(all_books) >= total):
            break
        time.sleep(0.1)
    # de-dup by id, keep order
    seen, uniq = set(), []
    for b in all_books:
        i = b.get("id")
        if i and i not in seen:
            seen.add(i); uniq.append(b)
    json.dump(uniq, open(INDEX, "w", encoding="utf-8"), ensure_ascii=False)
    print(f"enumerate: wrote {len(uniq)} books to {INDEX} (reported total={total})", flush=True)
    return uniq


def load_index():
    if not os.path.exists(INDEX):
        return enumerate_books()
    return json.load(open(INDEX, encoding="utf-8"))


def fetch_book(b):
    bid = b["id"]
    dst = os.path.join(BOOKS_DIR, bid + ".json")
    if os.path.exists(dst) and os.path.getsize(dst) > 100:
        return "skip"
    txt = fetch(f"{BASE}/api/books/{bid}")
    if not txt:
        return "fail"
    open(dst, "w", encoding="utf-8").write(txt)
    return "ok"


def fetch_cover(b):
    bid = b["id"]
    url = b.get("thumbnail") or b.get("image_thumb") or b.get("thumbnail_blob")
    if not url:
        return "nourl"
    dst = os.path.join(COVERS_DIR, bid + ".jpg")
    if os.path.exists(dst) and os.path.getsize(dst) > 100:
        return "skip"
    data = fetch(url, binary=True)
    if not data:
        return "fail"
    open(dst, "wb").write(data)
    return "ok"


def run_phase(name, fn, items):
    from collections import Counter
    c = Counter()
    t0 = time.time()
    with cf.ThreadPoolExecutor(max_workers=WORKERS) as ex:
        futs = {ex.submit(fn, b): b for b in items}
        for n, f in enumerate(cf.as_completed(futs), 1):
            c[f.result()] += 1
            if n % 100 == 0 or n == len(items):
                el = time.time() - t0
                print(f"  {name}: {n}/{len(items)} {dict(c)} {el:.0f}s", flush=True)
    print(f"{name} done: {dict(c)}", flush=True)


def main():
    phase = sys.argv[1] if len(sys.argv) > 1 else "all"
    if phase in ("enumerate", "all"):
        enumerate_books()
    idx = load_index()
    if phase in ("books", "all"):
        run_phase("books", fetch_book, idx)
    if phase in ("covers", "all"):
        run_phase("covers", fetch_cover, idx)


if __name__ == "__main__":
    main()
