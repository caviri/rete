"""Layer 2 — per-game metadata from itch.io's browse feed.

`https://itch.io/games/<sort>?format=json&page=N` returns JSON
{page, num_items, content} where `content` is an HTML fragment of ~36
`game_cell` blocks carrying game_id, the game URL, title, author, price,
platform icons, genre and cover image. `/games` is robots-allowed (only /search
is disallowed). We save each page's JSON as-is; parsing into rows is a later
step. Resumable (existing page files skipped), polite ~1 req / 1.2 s.

Default sort `newest` is ordered by date so it eventually covers everything;
pass other sorts (top-sellers, top-rated, featured) for weighted coverage.

Output: data/itch-io/raw/browse/<sort>_page{NNNNN}.json

Run: python data/itch-io/scripts/fetch_browse.py               # newest
     python data/itch-io/scripts/fetch_browse.py top-sellers   # a specific sort
"""
import json
import os
import sys
import time
import urllib.request

RAW = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw")
UA = "Mozilla/5.0 (rete dataset acquisition; +https://w3id.org/rete)"
URL = "https://itch.io/games/{sort}?format=json&page={page}"
DELAY = 1.2
MAX_PAGES = 60000  # safety cap (~2.1M games at 36/page); it stops earlier when empty


def get(url, tries=5):
    for i in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=45) as r:
                return json.loads(r.read().decode("utf-8"))
        except Exception as e:
            print(f"    retry {i+1}: {str(e)[:60]}", flush=True)
            time.sleep(8 * (i + 1))
    return None


def main():
    sort = sys.argv[1] if len(sys.argv) > 1 else "newest"
    out = os.path.join(RAW, "browse")
    os.makedirs(out, exist_ok=True)
    page = 1
    total = 0
    while page <= MAX_PAGES:
        path = os.path.join(out, f"{sort}_page{page:05d}.json")
        if os.path.exists(path) and os.path.getsize(path) > 2:
            try:
                n = json.load(open(path, encoding="utf-8")).get("num_items", 0)
                total += n
                if n == 0:
                    break
                page += 1
                continue
            except Exception:
                pass
        d = get(URL.format(sort=sort, page=page))
        if d is None:
            print(f"  page {page}: FAILED — stop (resume by re-running)", flush=True)
            break
        n = d.get("num_items", 0)
        tmp = path + ".part"
        json.dump(d, open(tmp, "w", encoding="utf-8"), ensure_ascii=False)
        os.replace(tmp, path)
        total += n
        if page % 25 == 0 or n < 36:
            print(f"  {sort} page {page}: {n} items  (running total {total:,})", flush=True)
        if n == 0:
            break
        page += 1
        time.sleep(DELAY)
    print(f"DONE ({sort}): {page-1} pages, {total:,} cell rows", flush=True)


if __name__ == "__main__":
    main()
