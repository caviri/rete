#!/usr/bin/env python3
"""Fase 1 - harvest the CCBAE "Material cartografico AGS" listing (3,128 records,
20/page, paginated by ?posicion=1,21,...,3121). Saves each listing page's raw HTML
(robust: parse offline for title/author/date/place + the grupo.do image-group path
id). Honors robots Crawl-delay: 30. Resumable (skips pages already saved).

Run: uv run --no-project --with requests python scripts/harvest_ccbae.py
Images are NOT here (CCBAE gates them behind login); this is metadata only.
"""
import os
import sys
import time

import requests

BASE = "https://www.mcu.es/ccbae/es/consulta/resultados_busqueda.cmd"
PARAMS = {
    "busq_codsecc": "MCAGS",
    "tipo_busqueda": "mapas_planos_dibujos",
    "descrip_codsecc": "Material cartografico AGS",
}
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "data", "ccbae", "pages")
TOTAL = 3128
STEP = 20
DELAY = 30  # robots Crawl-delay
UA = "rete-research/1.0 (carlosvivarrios@gmail.com) heritage cartography metadata harvest"

os.makedirs(OUT, exist_ok=True)
s = requests.Session()
s.headers["User-Agent"] = UA

positions = list(range(1, TOTAL + 1, STEP))
print(f"listing pages to fetch: {len(positions)} (1..{positions[-1]}), {DELAY}s delay")
sys.stdout.flush()

for idx, pos in enumerate(positions, 1):
    out = os.path.join(OUT, f"page_{pos:05d}.html")
    if os.path.exists(out) and os.path.getsize(out) > 5000:
        continue
    p = dict(PARAMS)
    p["posicion"] = pos
    for attempt in range(3):
        try:
            r = s.get(BASE, params=p, timeout=90)
            if r.status_code == 200 and len(r.text) > 5000:
                with open(out, "w", encoding="utf-8") as f:
                    f.write(r.text)
                print(f"[{idx}/{len(positions)}] pos={pos}: {len(r.text)} bytes")
                break
            time.sleep(DELAY)
        except Exception as e:
            print(f"  retry pos={pos}: {e}")
            time.sleep(DELAY)
    sys.stdout.flush()
    time.sleep(DELAY)

print("CCBAE LISTING HARVEST DONE")
