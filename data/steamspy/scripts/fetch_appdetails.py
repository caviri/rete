"""Enrich each SteamSpy-tracked app with the Steam Store `appdetails` — the rich
per-game metadata (type, name, description, price, genres, categories,
developers, publishers, release date, platforms, metacritic, screenshots,
achievements, …). Rate-limited (~1 req / 1.5 s; Steam allows ~200 / 5 min).
Resumable: apps already saved (or already known-empty) are skipped; misses go to
download_failures.txt for a targeted re-run.

Reads the appid universe from data/steamdb/raw/steamspy/*.json (fallback:
raw/applist.json). Output: data/steamdb/raw/appdetails/{appid}.json  (the
appdetails `data` object, or {"success": false} for delisted/region-locked apps).

Run: python data/steamdb/scripts/fetch_appdetails.py
"""
import glob
import json
import os
import time
import urllib.request

RAW = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw")
OUT = os.path.join(RAW, "appdetails")
UA = "Mozilla/5.0 (rete dataset acquisition; +https://w3id.org/rete)"
URL = "https://store.steampowered.com/api/appdetails?appids={}&cc=us&l=english"
DELAY = 1.6
FAILLOG = os.path.join(RAW, "..", "download_failures.txt")


def appid_universe():
    ids = {}
    for f in sorted(glob.glob(os.path.join(RAW, "steamspy", "*.json"))):
        try:
            for k, v in json.load(open(f, encoding="utf-8")).items():
                ids[int(v.get("appid", k))] = v.get("name")
        except Exception:
            pass
    if not ids:  # fallback to the full app list if present
        try:
            for a in json.load(open(os.path.join(RAW, "applist.json"), encoding="utf-8"))["applist"]["apps"]:
                ids[int(a["appid"])] = a["name"]
        except Exception:
            pass
    return ids


def get(appid, tries=4):
    for i in range(tries):
        try:
            req = urllib.request.Request(URL.format(appid), headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=45) as r:
                if r.status == 429:
                    raise RuntimeError("429 rate limited")
                return json.loads(r.read().decode("utf-8"))
        except Exception as e:
            wait = 20 * (i + 1) if "429" in str(e) else 5 * (i + 1)
            time.sleep(wait)
    return None


def main():
    os.makedirs(OUT, exist_ok=True)
    ids = appid_universe()
    print(f"appid universe: {len(ids):,}", flush=True)
    done = fetched = failed = 0
    fails = []
    for appid in sorted(ids):
        path = os.path.join(OUT, f"{appid}.json")
        if os.path.exists(path) and os.path.getsize(path) > 2:
            done += 1
            continue
        d = get(appid)
        node = (d or {}).get(str(appid))
        if node is None:
            failed += 1
            fails.append(appid)
        else:
            payload = node.get("data") if node.get("success") else {"success": False}
            tmp = path + ".part"
            json.dump(payload, open(tmp, "w", encoding="utf-8"), ensure_ascii=False)
            os.replace(tmp, path)
            fetched += 1
        n = done + fetched + failed
        if n % 500 == 0:
            print(f"  {n:,}/{len(ids):,}  (fetched {fetched:,}, cached {done:,}, failed {failed:,})", flush=True)
        time.sleep(DELAY)
    if fails:
        open(FAILLOG, "w").write("\n".join(map(str, fails)))
    print(f"DONE: fetched {fetched:,}, cached {done:,}, failed {failed:,}", flush=True)


if __name__ == "__main__":
    main()
