#!/usr/bin/env python3
"""Harvest the Arxiu Municipal de Barcelona (catalegarxiumunicipal.bcn.cat).

Reverse-engineered the Vue/Spring app's JSON API: POST /api/search with the exact form
body (page is 1-indexed; body MUST be UTF-8 — a mis-encoded accent 400s). It enumerates
per archive (q=null, archive=<full label>), paginated (itemsPerPage=50). Each record
carries rich ISAD-ish metadata + a `digitized` flag + a `files[]` array giving, per digital
object, its mime, name, human-readable size, and a content URL (/api/v1/nodes/<id>/content).

So this captures records + PDF/image LINK + SIZE + media type directly — no file downloads.
Descriptions are public domain ("De domini públic" / "Lliure accés"). Output: data/bcn/records.jsonl.
Resumable per (archive,page). Polite: one session, sequential, browser UA.
"""
import json, sys, time, math, ssl, urllib.request, http.cookiejar
from pathlib import Path

B = "https://catalegarxiumunicipal.bcn.cat"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/125.0 Safari/537.36"
REPO = Path(__file__).resolve().parents[2]
OUT = REPO / "data" / "bcn"; OUT.mkdir(parents=True, exist_ok=True)
CTX = ssl.create_default_context(); CTX.check_hostname = False; CTX.verify_mode = ssl.CERT_NONE
SIZE = 50

ARCHIVES = [
    "AFB Arxiu Fotogràfic de Barcelona",
    "AHCB Arxiu Històric de la Ciutat de Barcelona",
    "AMCB Arxiu Municipal Contemporani de Barcelona",
    "AMDC Arxiu Municipal del Districte de Les Corts",
    "AMDCV Arxiu Municipal del Districte de Ciutat Vella",
    "AMDE Arxiu Municipal del Districte de l'Eixample",
    "AMDG Arxiu Municipal del Districte de Gràcia",
    "AMDHG Arxiu Municipal del Districte d'Horta-Guinardó",
    "AMDNB Arxiu Municipal del Districte de Nou Barris",
    "AMDS Arxiu Municipal del Districte de Sants-Montjuïc",
    "AMDSA Arxiu Municipal del Districte de Sant Andreu",
    "AMDSG Arxiu Municipal del Districte de Sarrià-Sant Gervasi",
    "AMDSM Arxiu Municipal del Districte de Sant Martí",
]
FORM = {"q": None, "operator": "ANYWORDS", "page": 1, "itemsPerPage": SIZE, "orderField": None,
        "folder": None, "facets": None, "classifications": None, "classification": None,
        "yearRange": None, "dateRange": None, "wordExact": None, "not": None, "wordAll": None,
        "wordAny": None, "mediaType": None, "doctype": None, "subjects": None, "person": None,
        "geographical": None, "author": None, "producers": None, "archive": None, "fond": None}

cj = http.cookiejar.CookieJar()
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj),
                                     urllib.request.HTTPSHandler(context=CTX))

def warm():
    req = urllib.request.Request(B + "/search?q=barcelona&page=1&itemsPerPage=10&operator=ANYWORDS",
                                 headers={"User-Agent": UA})
    opener.open(req, timeout=60).read()

def search(archive, page, size=SIZE):
    body = dict(FORM); body["archive"] = archive; body["page"] = page; body["itemsPerPage"] = size
    data = json.dumps(body, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(B + "/api/search", data=data, method="POST", headers={
        "User-Agent": UA, "Content-Type": "application/json;charset=UTF-8",
        "Accept": "application/json, text/plain, */*", "Origin": B, "Referer": B + "/search",
        "X-Requested-With": "XMLHttpRequest"})
    for a in range(4):
        try:
            d = json.loads(opener.open(req, timeout=90).read().decode("utf-8"))
            return d.get("results", d)
        except Exception as e:
            if a == 3: raise
            time.sleep(2 * (a + 1))

def slim(r):
    files = []
    for f in (r.get("files") or []):
        files.append({"id": f.get("id"), "mime": f.get("mime"), "name": f.get("name"),
                      "size": f.get("size"), "url": (f.get("url") or "").replace(B, ""),
                      "location": f.get("location")})
    ctr = r.get("center") or []
    return {"id": r.get("id"), "name": r.get("name"), "nodeType": r.get("nodeType"),
            "parentId": r.get("parentId"), "center": ctr[0] if ctr else None,
            "fond": r.get("fond"), "reference": r.get("fondoDocCode") or r.get("adnId"),
            "startDate": r.get("startDate"), "endDate": r.get("endDate"),
            "stringStartDate": r.get("stringStartDate"), "stringEndDate": r.get("stringEndDate"),
            "date": r.get("date"), "summary": r.get("summary"), "access": r.get("access"),
            "producers": r.get("producers"), "reuse": r.get("fondoReuseConditions"),
            "digitized": r.get("digitized"), "files": files}

def main():
    warm()
    out = OUT / "records.jsonl"
    done_file = OUT / "done_pages.txt"
    done = set(done_file.read_text().split()) if done_file.exists() else set()
    fout = out.open("a", encoding="utf-8")
    grand = sum(1 for _ in out.open(encoding="utf-8")) if out.exists() else 0
    for archive in ARCHIVES:
        code = archive.split(" ")[0]
        total = search(archive, 1, 1).get("totalElements", 0)
        pages = math.ceil(total / SIZE)
        print(f"=== {code}: {total:,} records, {pages:,} pages ===", flush=True)
        for p in range(1, pages + 1):
            tag = f"{code}:{p}"
            if tag in done:
                continue
            res = search(archive, p)
            for r in res.get("content", []):
                fout.write(json.dumps(slim(r), ensure_ascii=False) + "\n")
                grand += 1
            done.add(tag)
            if p % 25 == 0:
                fout.flush(); done_file.write_text(" ".join(sorted(done)))
                print(f"  {code} {p}/{pages}  (grand {grand:,})", flush=True)
        fout.flush(); done_file.write_text(" ".join(sorted(done)))
        print(f"  {code} DONE ({grand:,} total)", flush=True)
    fout.close()
    print(f"DONE: {grand:,} records -> {out}", flush=True)

if __name__ == "__main__":
    main()
