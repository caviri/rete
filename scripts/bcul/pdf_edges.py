#!/usr/bin/env python3
"""Emit `<item> schema:associatedMedia <pdf-url>` for every Patrinum record that has a
digitised PDF (the ?v=pdf full document). URLs come straight from the harvest's files[]
(the canonical patrinum.ch/record/<id>/files/<name>.pdf — 302s to a CORS-open, range-
readable nanna PDF that the browser renders natively). ~374k edges over ~284k records.

Rebuild bcul with:  rete build bcul.nt bcul_images.nt bcul_pdf.nt bcul_iiif.nt -o bcul.rete
Pure stdlib; runs locally."""
import json, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from jsonl_to_nt import subject_iri  # noqa

REPO   = Path(__file__).resolve().parents[2]
SRC    = REPO / "data" / "bcul" / "normalized" / "bcul.jsonl"
OUT    = REPO / "data" / "bcul" / "bcul_pdf.nt"
ASSOC  = "http://schema.org/associatedMedia"

def main():
    recs = edges = 0
    with OUT.open("w", encoding="utf-8") as f:
        for line in SRC.open(encoding="utf-8"):
            r = json.loads(line)
            if r.get("source") != "patrinum":
                continue
            pdfs = [x.get("url") for x in (r.get("files") or [])
                    if str(x.get("url", "")).lower().endswith(".pdf")]
            if not pdfs:
                continue
            s = subject_iri(r)
            if not s:
                continue
            recs += 1
            for u in pdfs:
                f.write(f"<{s}> <{ASSOC}> <{u}> .\n")
                edges += 1
    print(f"{recs:,} records, {edges:,} schema:associatedMedia PDF edges -> {OUT}")

if __name__ == "__main__":
    main()
