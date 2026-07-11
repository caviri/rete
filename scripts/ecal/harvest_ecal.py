#!/usr/bin/env python3
"""Polite harvest of the ECAL library catalogue (BiblioMaker OPAC) -> unified JSONL.

The ECAL online catalogue (cloud7.bibliomaker.ch:33000) is a BiblioMaker OPAC with
robots.txt `Disallow: /` — the user authorized a polite, low-rate, metadata-only
harvest. There is no OAI/SRU/export and list pagination is session-bound, so we
enumerate stateless detail pages by document id:

    ListTitl.htm?BM_ZOOM=DOCUMENT&BM_GET_DOCUMENT=<id>&BM_QUERY=WORDS&BM_ENDUSER_LNG=French

Each detail page is a <TD class="FieldName">Label</TD><TD class="FieldValue…">Value</TD>
table (Titre, Auteurs, Editeur, Cote, Catégorie, Type, Format, Matières, Notes, …).
Records map to the SAME unified schema as the BCU twin (data/bcul/schema) so the
graph converter is reused. Single-thread, ~1 req/1.2 s, resumable.

Usage:
  python harvest_ecal.py                 # full (resume), ids 1..MAX
  python harvest_ecal.py --max-id 31767 --rate 0.83
  python harvest_ecal.py --limit 10      # smoke test
  python harvest_ecal.py --restart
"""
from __future__ import annotations

import argparse
import html
import json
import re
import ssl
import sys
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
BASE = "https://cloud7.bibliomaker.ch:33000/French/"
UA = "ECAL-twin-research/1.0 (polite low-rate metadata harvest; contact carlos.vivarrios@epfl.ch)"
_CTX = ssl.create_default_context()
_CTX.check_hostname = False
_CTX.verify_mode = ssl.CERT_NONE

YEAR_RE = re.compile(r"(1[0-9]{3}|20[0-9]{2})")
# Category code -> human section (ECAL classification). Extend as seen.
CATEGORY = {
    "DG": "Design graphique", "AV": "Art visuel / Arts plastiques", "CI": "Cinéma",
    "PH": "Photographie", "DI": "Design industriel / produit", "DM": "Design & média",
    "AR": "Architecture / espace", "TH": "Théorie / esthétique", "TY": "Typographie",
    "MO": "Mode / textile", "SC": "Sciences humaines", "GE": "Généralités",
}
# BiblioMaker Type -> unified resource type
TYPE_MAP = {
    "monographie": "text", "périodique": "serial", "periodique": "serial",
    "enregistrement vidéo": "moving-image", "enregistrement video": "moving-image",
    "dvd": "moving-image", "enregistrement sonore": "sound-music", "cd": "sound-music",
    "mémoire": "text", "memoire": "text", "diplôme": "text", "these": "text",
    "affiche": "still-image", "multimédia": "electronic", "objet": "object",
}


def now_iso():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def strip_tags(s):
    s = re.sub(r"(?is)<(script|style).*?</\1>", " ", s)
    s = re.sub(r"<[^>]+>", " ", s)
    return re.sub(r"\s+", " ", html.unescape(s)).strip()


class Fetcher:
    def __init__(self, rate=0.83, retries=5, timeout=45):
        self.min_interval = (1.0 / rate) if rate else 0.0
        self.retries = retries
        self.timeout = timeout
        self._last = 0.0
        self.n = 0
        self.op = urllib.request.build_opener(urllib.request.HTTPSHandler(context=_CTX))

    def get(self, url):
        if self.min_interval:
            dt = time.time() - self._last
            if dt < self.min_interval:
                time.sleep(self.min_interval - dt)
        self._last = time.time()
        last = None
        for a in range(self.retries):
            try:
                r = urllib.request.Request(url, headers={"User-Agent": UA})
                data = self.op.open(r, timeout=self.timeout).read()
                self.n += 1
                return data.decode("utf-8", "replace")
            except Exception as e:  # network / timeout / 5xx
                last = e
                time.sleep(min(60, 2 ** a * 2))
        raise RuntimeError(f"GET failed {url}: {last}")


def detail_url(i):
    return (BASE + f"ListTitl.htm?BM_ZOOM=DOCUMENT&BM_GET_DOCUMENT={i}"
            "&BM_QUERY=WORDS&BM_ENDUSER_LNG=French&BM_MAX_NB_REC=1")


def parse_fields(html_text):
    """Detail HTML -> {label: raw_value_html}. Split on the FieldName cells so the
    nested value tables in Titre/Auteurs/Matières stay intact for link extraction."""
    fields = {}
    segs = re.split(r'<TD[^>]*class="FieldName"[^>]*>', html_text, flags=re.I)
    for seg in segs[1:]:
        parts = seg.split("</TD>", 1)
        label = strip_tags(parts[0]).rstrip(" :").strip()
        rest = parts[1] if len(parts) > 1 else ""
        vm = re.search(r'<TD[^>]*class="FieldValue[^"]*"[^>]*>(.*)', rest, re.I | re.S)
        raw = (vm.group(1) if vm else rest)
        if label and strip_tags(raw):
            fields.setdefault(label, raw)  # raw HTML; decode via get_text / get_links
    return fields


def _norm_label(l):
    return (l.lower().replace("é", "e").replace("è", "e").replace("à", "a")
            .replace("î", "i").replace("ô", "o").replace("û", "u").strip())


LINK_RE = re.compile(r"<A\b[^>]*>(.*?)</A>", re.I | re.S)


def get_text(fields, *names):
    idx = {_norm_label(k): v for k, v in fields.items()}
    for n in names:
        v = idx.get(_norm_label(n))
        if v:
            t = strip_tags(v)
            if t:
                return t
    return None


def get_links(fields, *names):
    """Author/subject fields are one <A>…</A> per value; return them as a list.
    Falls back to splitting the plain text when there are no links."""
    idx = {_norm_label(k): v for k, v in fields.items()}
    for n in names:
        v = idx.get(_norm_label(n))
        if not v:
            continue
        links = [strip_tags(x) for x in LINK_RE.findall(v)]
        links = [x.strip(" ,.") for x in links if x and len(x) > 1]
        if links:
            return links
        t = strip_tags(v)
        return [p.strip(" ,.") for p in re.split(r"\s*;\s*|\s{2,}", t) if len(p.strip()) > 1]
    return []


get_field = get_text  # back-compat alias for the plain-text fields below


def normalize(fields, rid, has_cover=False):
    title = get_field(fields, "Titre", "Title")
    editeur = get_field(fields, "Editeur", "Éditeur", "Publisher") or ""
    # "Place: Publisher, Year"
    place = pub = None
    date_disp = editeur or None
    m = re.match(r"\s*([^:]+):\s*(.*?)(?:,\s*)?([0-9]{4})?\s*$", editeur)
    if m:
        place = (m.group(1) or "").strip(" ,.") or None
        pub = (m.group(2) or "").strip(" ,.") or None
    ymatch = YEAR_RE.search(editeur)
    date_start = int(ymatch.group(1)) if ymatch else None

    creators = [{"name": a, "role": None, "main": i == 0}
                for i, a in enumerate(get_links(fields, "Auteurs", "Auteur", "Author"))]
    subjects = get_links(fields, "Matières", "Matieres", "Sujets", "Descripteurs")

    cote = get_field(fields, "Cote")
    cat_code = (get_field(fields, "Catégorie", "Categorie") or "").strip()
    indice = get_field(fields, "Indice")
    collections = ["ECAL Library"]
    if cat_code:
        collections.append(CATEGORY.get(cat_code.upper(), cat_code))
    typ_raw = (get_field(fields, "Type") or "").strip()
    rtype = TYPE_MAP.get(typ_raw.lower(), "text")

    isbn = get_field(fields, "ISBN")
    ids = {"marc001": str(rid), "bm_num": str(rid)}
    if isbn:
        ids["isbn"] = [x for x in re.split(r"[\s;]+", isbn) if re.search(r"\d", x)][:4]

    # covers exist only when the detail page references them (else the endpoint 404s)
    thumb = (BASE + f"BM_DOCUMENT_COVER_PAGE_THUMBNAIL/{rid}") if has_cover else None
    cover = (BASE + f"BM_DOCUMENT_COVER_PAGE/{rid}") if has_cover else None
    return {
        "id": f"ecal:{rid}",
        "source": "ecal",
        "local_id": str(rid),
        "record_url": detail_url(rid),
        "type": rtype,
        "title": title,
        "title_full": None,
        "creators": creators,
        "publication": {"place": place, "publisher": pub, "date": date_disp},
        "date_start": date_start,
        "date_end": None,
        "languages": [get_field(fields, "Langue", "Language")] if get_field(fields, "Langue", "Language") else [],
        "subjects": subjects,
        "genres": [indice] if indice else [],
        "places": [place] if place else [],
        "shelfmark": cote,
        "holdings": [{"library": "ECAL - Bibliothèque", "location": cote, "call_number": cote,
                      "availability": get_field(fields, "Disponibilité", "Disponibilite"), "kind": "physical"}] if cote else [],
        "libraries": ["ECAL - Bibliothèque"],
        "collections": collections,
        "extent": get_field(fields, "Format"),
        "description": None,
        "notes": [n for n in [get_field(fields, "Notes", "Note")] if n],
        "identifiers": ids,
        "files": ([{"url": cover, "label": "cover", "format": "image/jpeg"}] if cover else []),
        "iiif_manifest": None,
        "thumbnail_url": thumb,
        "thumbnail_local": None,
        "has_digital": bool(has_cover),
        "rights": None,
        "provider": "ECAL — École cantonale d'art de Lausanne, Bibliothèque",
        "category_code": cat_code or None,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-dir", default=str(REPO / "data" / "ecal"))
    ap.add_argument("--max-id", type=int, default=31767)
    ap.add_argument("--rate", type=float, default=0.83)  # ~1 request / 1.2 s
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--restart", action="store_true")
    args = ap.parse_args()

    base = Path(args.base_dir)
    (base / "state").mkdir(parents=True, exist_ok=True)
    (base / "logs").mkdir(parents=True, exist_ok=True)
    (base / "normalized").mkdir(parents=True, exist_ok=True)
    norm_path = base / "normalized" / "ecal.jsonl"
    state_path = base / "state" / "ecal.json"
    log_path = base / "logs" / "ecal.log"

    def log(m):
        line = f"[{now_iso()}] {m}"
        print(line, flush=True)
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")

    state = {"next_id": 1, "records": 0, "empty": 0, "started": now_iso(), "done": False}
    if state_path.exists() and not args.restart:
        state = json.loads(state_path.read_text(encoding="utf-8"))
        if state.get("done"):
            log(f"ECAL harvest already complete ({state['records']} records).")
            return 0
        log(f"Resuming ECAL from id {state['next_id']} ({state['records']} records).")
    else:
        norm_path.write_text("", encoding="utf-8")

    f = Fetcher(rate=args.rate)
    fh = open(norm_path, "a", encoding="utf-8")
    done_this_run = 0
    try:
        i = state["next_id"]
        while i <= args.max_id:
            try:
                h = f.get(detail_url(i))
            except Exception as e:
                # a persistent per-id failure must not kill the whole (10 h) run
                log(f"  fetch failed id {i}: {e}; skipping")
                state.setdefault("failed", []).append(i)
                state["next_id"] = i + 1
                state_path.write_text(json.dumps(state), encoding="utf-8")
                time.sleep(10)  # back off in case the server is briefly unwell
                i += 1
                continue
            if "Numéro" in strip_tags(h) or 'class="FieldName"' in h and "Titre" in h:
                fields = parse_fields(h)
                if fields.get("Titre") or fields.get("Numéro"):
                    has_cover = f"BM_DOCUMENT_COVER_PAGE_THUMBNAIL/{i}" in h
                    rec = normalize(fields, i, has_cover)
                    rec["harvested_at"] = now_iso()
                    rec["_fields"] = {k: strip_tags(v) for k, v in fields.items()}  # source snapshot
                    fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
                    state["records"] += 1
                else:
                    state["empty"] += 1
            else:
                state["empty"] += 1
            state["next_id"] = i + 1
            if i % 10 == 0:
                fh.flush()
                state_path.write_text(json.dumps(state), encoding="utf-8")
                if i % 50 == 0:
                    log(f"id {i}/{args.max_id} | records {state['records']} empty {state['empty']} req#{f.n}")
            done_this_run += 1
            if args.limit and done_this_run >= args.limit:
                fh.flush(); state_path.write_text(json.dumps(state), encoding="utf-8")
                log(f"limit {args.limit} reached at id {i}; pausing.")
                return 0
            i += 1
        state["done"] = True
        fh.flush()
        state_path.write_text(json.dumps(state), encoding="utf-8")
        log(f"DONE. ECAL: {state['records']} records ({state['empty']} empty ids), req#{f.n}.")
    finally:
        fh.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
