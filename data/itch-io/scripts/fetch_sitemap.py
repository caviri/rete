"""Layer 1 — the COMPLETE itch.io sitemap (every sub-sitemap type).

itch.io's sitemap index (https://itch.io/sitemap.xml) groups sub-sitemaps by
type: games, users (creators), jams, jam_games, collections, community_topics,
blog_posts, browse — each `<type>*.xml` holding ~50,000 <loc> URLs. Robots-allowed
(the site publishes it). We download EVERY sub-sitemap and extract the URLs
grouped by type; games/users/jams are the ones that matter for the games graph.

Output:
  data/itch-io/raw/sitemap/*.xml       # every raw sub-sitemap, as-is
  data/itch-io/raw/urls/<type>.txt     # deduped URLs per type
  data/itch-io/raw/game_urls.txt       # = urls/games.txt (back-compat)

Run: python data/itch-io/scripts/fetch_sitemap.py
"""
import os
import re
import time
import urllib.request
from collections import defaultdict

RAW = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw")
SM = os.path.join(RAW, "sitemap")
URLS = os.path.join(RAW, "urls")
UA = "Mozilla/5.0 (rete dataset acquisition; +https://w3id.org/rete)"
INDEX = "https://itch.io/sitemap.xml"


def get(url, tries=5):
    for i in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.read()
        except Exception as e:
            print(f"    retry {i+1}: {str(e)[:60]}", flush=True)
            time.sleep(10 * (i + 1))
    return None


def main():
    os.makedirs(SM, exist_ok=True)
    os.makedirs(URLS, exist_ok=True)
    idx = get(INDEX)
    if idx is None:
        raise SystemExit("could not fetch sitemap index")
    subs = re.findall(rb"<loc>(https://itch\.io/sitemaps/[a-z0-9_]+\.xml)</loc>", idx)
    print(f"sub-sitemaps in index: {len(subs)}", flush=True)
    by_type = defaultdict(set)
    for j, sub in enumerate(subs, 1):
        sub = sub.decode()
        name = sub.rsplit("/", 1)[-1]                       # games_18.xml
        typ = re.sub(r"(_\d+)?\.xml$", "", name)            # games
        local = os.path.join(SM, name)
        if os.path.exists(local) and os.path.getsize(local) > 100:
            body = open(local, "rb").read()
        else:
            body = get(sub)
            if body is None:
                print(f"  {name}: FAILED", flush=True); continue
            open(local, "wb").write(body)
            time.sleep(1)
        locs = re.findall(rb"<loc>(https://[^<]+)</loc>", body)
        for u in locs:
            by_type[typ].add(u.decode())
        if j % 10 == 0 or j == len(subs):
            print(f"  [{j}/{len(subs)}] {name} ({typ})", flush=True)
    for typ, urls in sorted(by_type.items()):
        open(os.path.join(URLS, f"{typ}.txt"), "w", encoding="utf-8").write("\n".join(sorted(urls)))
        print(f"  {typ:18s} {len(urls):>9,} urls", flush=True)
    # back-compat game_urls.txt
    if "games" in by_type:
        open(os.path.join(RAW, "game_urls.txt"), "w", encoding="utf-8").write("\n".join(sorted(by_type["games"])))
    print(f"DONE: {sum(len(v) for v in by_type.values()):,} URLs across {len(by_type)} types", flush=True)


if __name__ == "__main__":
    main()
