"""Harvest the SteamSpy `all` endpoint — the app index + core stats for every
game SteamSpy tracks (owners, reviews, playtime, price, ccu). Paginated 1000
games/page, rate-limited to 1 request / 60 s. Resumable: pages already on disk
are skipped; it stops when a page comes back empty.

Output: data/steamdb/raw/steamspy/all_page{NNNN}.json  (one page per file, as-is)

Run: python data/steamdb/scripts/fetch_steamspy.py
"""
import json
import os
import time
import urllib.request

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw", "steamspy")
UA = "Mozilla/5.0 (rete dataset acquisition; +https://w3id.org/rete)"
BASE = "https://steamspy.com/api.php?request=all&page="
DELAY = 61  # SteamSpy `all` rate limit is 1 req / 60 s


def get(url, tries=5):
    for i in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.loads(r.read().decode("utf-8"))
        except Exception as e:
            wait = 30 * (i + 1)
            print(f"    retry {i+1}/{tries} after {wait}s: {str(e)[:60]}", flush=True)
            time.sleep(wait)
    return None


def main():
    os.makedirs(OUT, exist_ok=True)
    page = 0
    total = 0
    while True:
        path = os.path.join(OUT, f"all_page{page:04d}.json")
        if os.path.exists(path) and os.path.getsize(path) > 2:
            try:
                n = len(json.load(open(path, encoding="utf-8")))
                total += n
                print(f"  page {page}: cached ({n} games)", flush=True)
                if n == 0:
                    break
                page += 1
                continue
            except Exception:
                pass  # corrupt -> refetch
        d = get(BASE + str(page))
        if d is None:
            print(f"  page {page}: FAILED after retries — stopping (resume by re-running)", flush=True)
            break
        n = len(d)
        # atomic write
        tmp = path + ".part"
        json.dump(d, open(tmp, "w", encoding="utf-8"))
        os.replace(tmp, path)
        total += n
        print(f"  page {page}: {n} games  (running total {total:,})", flush=True)
        if n == 0:
            break
        page += 1
        time.sleep(DELAY)
    print(f"DONE: {page} pages, {total:,} game rows", flush=True)


if __name__ == "__main__":
    main()
