#!/usr/bin/env python3
"""records.jsonl (Arxiu Municipal de Barcelona /api/search harvest) -> a schema.org
archival knowledge graph. Each record is a schema:ArchiveComponent held by its archive
(schema:ArchiveOrganization), part of its parent (hierarchy via parentId). Digitised
records link each digital object: images as schema:image, PDFs as schema:associatedMedia,
both → /api/v1/nodes/<id>/content, with encodingFormat + byte contentSize (parsed from the
API's human-readable size). Public-domain descriptions (Ajuntament de Barcelona)."""
import json, re
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SRC  = REPO / "data" / "bcn" / "records.jsonl"
OUT  = REPO / "data" / "bcn" / "bcn.nt"
SITE = "https://catalegarxiumunicipal.bcn.cat"

BCN    = "https://w3id.org/rete/bcn#"
B      = "https://w3id.org/rete/bcn/"
SCHEMA = "http://schema.org/"
DCT    = "http://purl.org/dc/terms/"
RDF    = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD    = "http://www.w3.org/2001/XMLSchema#"

def esc(s): return str(s).replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "").replace("\t", "\\t")
def safe(s): return re.sub(r"[^A-Za-z0-9._-]", "_", str(s))

UNITS = {"B": 1, "BYTES": 1, "KB": 1024, "MB": 1024**2, "GB": 1024**3, "TB": 1024**4}
def to_bytes(hs):
    if not hs: return None
    m = re.match(r"\s*([\d.,]+)\s*([A-Za-z]+)", str(hs))
    if not m: return None
    try: val = float(m.group(1).replace(",", ""))
    except ValueError: return None
    return int(val * UNITS.get(m.group(2).upper(), 1))

def main():
    out = OUT.open("w", encoding="utf-8")
    def iri(s, p, o): out.write(f"<{s}> <{p}> <{o}> .\n")
    def lit(s, p, v, dt=None): out.write(f'<{s}> <{p}> "{esc(v)}"' + (f"^^<{dt}>" if dt else "") + " .\n")

    archives, n, ndig, nfile, nimg = {}, 0, 0, 0, 0
    seen_media = set()
    seen = set()
    for line in SRC.open(encoding="utf-8"):
        r = json.loads(line)
        rid = r.get("id")
        if not rid or rid in seen:
            continue
        seen.add(rid)
        u = B + "node/" + safe(rid)
        iri(u, RDF, SCHEMA + "ArchiveComponent"); iri(u, RDF, BCN + "Unit")
        if r.get("name"): lit(u, SCHEMA + "name", r["name"])
        if r.get("nodeType"): lit(u, BCN + "nodeType", r["nodeType"])
        iri(u, SCHEMA + "url", SITE + "/detail/" + safe(rid))
        ref = r.get("reference")
        if ref: lit(u, DCT + "identifier", ref)
        ctr = r.get("center")
        if ctr:
            code = ctr.split(" ")[0]
            a = B + "archive/" + safe(code)
            iri(u, SCHEMA + "holdingArchive", a)
            if code not in archives: archives[code] = ctr
        if r.get("fond"): lit(u, BCN + "fond", r["fond"])
        pid = r.get("parentId")
        if pid and pid != rid:
            iri(u, SCHEMA + "isPartOf", B + "node/" + safe(pid))
        if r.get("startDate"): lit(u, SCHEMA + "startDate", r["startDate"])
        if r.get("endDate"): lit(u, SCHEMA + "endDate", r["endDate"])
        if r.get("date"): lit(u, SCHEMA + "temporalCoverage", r["date"])
        if r.get("summary"): lit(u, SCHEMA + "description", r["summary"])
        if r.get("access"): lit(u, BCN + "access", r["access"])
        if r.get("reuse"): lit(u, DCT + "rights", r["reuse"])
        for pr in (r.get("producers") or []):
            lit(u, DCT + "creator", pr)
        lit(u, BCN + "digitized", "true" if r.get("digitized") else "false", XSD + "boolean")
        if r.get("digitized"): ndig += 1
        for f in (r.get("files") or []):
            fid = f.get("id")
            if not fid: continue
            mime = (f.get("mime") or "").lower()
            nm = re.sub(r"\s*\([^)]*\)\s*$", "", f.get("name") or "").strip()
            # /file/download/<rec>/<file>/<name>.<ext> is PUBLIC + CORS-open (the /detail viewer's
            # own file endpoint) — unlike the session-gated /api/nodes content URL. The server only
            # validates the EXTENSION (must match the original), so <fid>.<ext> works and needs no
            # encoding. Ending in .jpg/.pdf, the playground renders it inline (image) / as a link.
            ext = (nm.rsplit(".", 1)[-1].lower() if "." in nm else {"image/jpeg": "jpg", "image/png": "png",
                    "image/tiff": "tif", "application/pdf": "pdf"}.get(mime, "bin"))
            furl = f"{SITE}/file/download/{safe(rid)}/{safe(fid)}/{safe(fid)}.{ext}"
            pred = SCHEMA + "image" if mime.startswith("image/") else SCHEMA + "associatedMedia"
            iri(u, pred, furl)
            nfile += 1
            if mime.startswith("image/"):
                nimg += 1
            if furl not in seen_media:
                seen_media.add(furl)
                iri(furl, RDF, SCHEMA + "MediaObject")
                if f.get("mime"): lit(furl, SCHEMA + "encodingFormat", f["mime"])
                if nm: lit(furl, SCHEMA + "name", nm)
                b = to_bytes(f.get("size"))
                if b: lit(furl, SCHEMA + "contentSize", str(b), XSD + "integer")
        n += 1

    for code, name in sorted(archives.items()):
        a = B + "archive/" + safe(code)
        iri(a, RDF, SCHEMA + "ArchiveOrganization")
        lit(a, SCHEMA + "name", name)
    out.close()
    print(f"units: {n:,} · digitised: {ndig:,} · file-links: {nfile:,} (images: {nimg:,}) · archives: {len(archives)} -> {OUT}")

if __name__ == "__main__":
    main()
