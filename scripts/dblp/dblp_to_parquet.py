"""Convert the DBLP XML dump into Parquet — streaming, no full extraction.

Input:  data/dblp/dblp-<date>.xml.gz  (one giant ISO-8859-1 XML: millions of
        <article>/<inproceedings>/<incollection>/<proceedings>/<book>/<www>/...
        entries under <dblp>) + the referenced DTD (defines the HTML entities).
        CC0 (Schloss Dagstuhl).

Output (data/dblp/parquet/<table>/part-*.parquet):
  record      one row per publication entry: key, type, year, title, venue,
              volume/number/pages, publisher, isbn, **doi** (from <ee>), all
              ee/url, crossref, n_authors, authors_json, editors_json
  authorship  one row per (record, author): key, pos, author, **orcid** — the
              author<->publication edge table (co-authorship graph). DBLP tags
              a growing share of authors with their ORCID iD, so this joins
              straight to the ORCID tables; `doi` joins to DataCite/OpenAIRE.

Parsed with a single lxml iterparse pass (fast C parser) that resolves the
DTD entities and frees each element after use, so memory stays flat. Rolling
zstd Parquet writers.

Usage:
  python scripts/dblp/dblp_to_parquet.py
  python scripts/dblp/dblp_to_parquet.py --max-records 50000 --out /tmp/t
"""

import argparse
import glob
import gzip
import os
import time

from lxml import etree
import orjson
import pyarrow as pa
import pyarrow.parquet as pq

DATA = r"D:\pro\rete\data\dblp"
OUT = r"D:\pro\rete\data\dblp\parquet"

ENTRY_TAGS = {"article", "inproceedings", "proceedings", "book", "incollection",
              "phdthesis", "mastersthesis", "www", "person", "data"}

RECORD_COLS = ["key", "type", "mdate", "publtype", "title", "year", "venue",
               "volume", "number", "pages", "publisher", "isbn", "series",
               "doi", "url", "crossref", "n_authors", "authors_json",
               "editors_json", "ee_json"]
AUTHORSHIP_COLS = ["key", "type", "year", "pos", "author", "orcid"]

RECORD_SCHEMA = pa.schema(
    [(c, pa.int32() if c in ("year", "n_authors") else pa.string()) for c in RECORD_COLS]
)
AUTHORSHIP_SCHEMA = pa.schema(
    [(c, pa.int32() if c in ("year", "pos") else pa.string()) for c in AUTHORSHIP_COLS]
)

# journal for article, booktitle for the rest; take first present
VENUE_TAGS = ("journal", "booktitle")


def to_int(s):
    try:
        return int(s)
    except (TypeError, ValueError):
        return None


def doi_from_ee(ees):
    for ee in ees:
        if not ee:
            continue
        low = ee.lower()
        if "doi.org/" in low:
            return low.split("doi.org/", 1)[1]
        if low.startswith("10.") and "/" in low:
            return low
    return None


def first_text(elem, tag):
    e = elem.find(tag)
    return e.text if (e is not None and e.text) else None


class RollingWriter:
    def __init__(self, out_dir, schema, rows_per_file, chunk_rows):
        self.out_dir = out_dir
        self.schema = schema
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.cols = {f.name: [] for f in schema}
        self.buffered = 0
        self.writer = None
        self.file_index = 0
        self.file_rows = 0
        self.total_rows = 0
        os.makedirs(out_dir, exist_ok=True)

    def add(self, row):
        for f in self.schema:
            self.cols[f.name].append(row.get(f.name))
        self.buffered += 1
        self.total_rows += 1
        if self.buffered >= self.chunk_rows:
            self._flush()

    def _flush(self):
        if not self.buffered:
            return
        try:
            batch = pa.RecordBatch.from_pydict(self.cols, schema=self.schema)
        except (pa.ArrowInvalid, pa.ArrowTypeError):
            arrays = []
            for f in self.schema:
                vals = self.cols[f.name]
                try:
                    arrays.append(pa.array(vals, type=f.type))
                except (pa.ArrowInvalid, pa.ArrowTypeError):
                    if pa.types.is_integer(f.type):
                        vals = [to_int(v) for v in vals]
                    else:
                        vals = [v if v is None else str(v) for v in vals]
                    arrays.append(pa.array(vals, type=f.type))
            batch = pa.RecordBatch.from_arrays(arrays, schema=self.schema)
        if self.writer is None:
            path = os.path.join(self.out_dir, f"part-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(path, self.schema, compression="zstd",
                                           compression_level=3)
        self.writer.write_table(pa.Table.from_batches([batch], schema=self.schema),
                                row_group_size=self.chunk_rows)
        self.file_rows += batch.num_rows
        self.cols = {f.name: [] for f in self.schema}
        self.buffered = 0
        if self.file_rows >= self.rows_per_file:
            self.writer.close()
            self.writer = None
            self.file_index += 1
            self.file_rows = 0

    def close(self):
        self._flush()
        if self.writer is not None:
            self.writer.close()
            self.writer = None


def process_entry(elem, rec_w, auth_w):
    key = elem.get("key")
    etype = etree.QName(elem).localname
    authors = []
    editors = []
    ees = []
    fields = {}
    for c in elem:
        tag = etree.QName(c).localname if isinstance(c.tag, str) else None
        if tag == "author":
            authors.append({"name": (c.text or "").strip(), "orcid": c.get("orcid")})
        elif tag == "editor":
            editors.append({"name": (c.text or "").strip(), "orcid": c.get("orcid")})
        elif tag == "ee":
            if c.text:
                ees.append(c.text.strip())
        elif tag in ("title", "year", "volume", "number", "pages", "publisher",
                     "isbn", "series", "url", "crossref", "journal", "booktitle"):
            if tag not in fields and c.text:
                # title may contain markup (i/sub/sup) -> use itertext
                fields[tag] = "".join(c.itertext()).strip() if tag == "title" else c.text.strip()
    venue = fields.get("journal") or fields.get("booktitle")
    rec_w.add({
        "key": key, "type": etype, "mdate": elem.get("mdate"),
        "publtype": elem.get("publtype"), "title": fields.get("title"),
        "year": to_int(fields.get("year")), "venue": venue,
        "volume": fields.get("volume"), "number": fields.get("number"),
        "pages": fields.get("pages"), "publisher": fields.get("publisher"),
        "isbn": fields.get("isbn"), "series": fields.get("series"),
        "doi": doi_from_ee(ees), "url": fields.get("url"),
        "crossref": fields.get("crossref"), "n_authors": len(authors),
        "authors_json": orjson.dumps(authors).decode() if authors else None,
        "editors_json": orjson.dumps(editors).decode() if editors else None,
        "ee_json": orjson.dumps(ees).decode() if ees else None,
    })
    yr = to_int(fields.get("year"))
    for pos, a in enumerate(authors):
        auth_w.add({"key": key, "type": etype, "year": yr, "pos": pos,
                    "author": a["name"], "orcid": a["orcid"]})


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--data", default=DATA)
    ap.add_argument("--xml", default=None, help="explicit .xml.gz path")
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--rows-per-file", type=int, default=4_000_000)
    ap.add_argument("--chunk-rows", type=int, default=200_000)
    ap.add_argument("--max-records", type=int, default=None)
    args = ap.parse_args()

    xml_gz = args.xml or sorted(glob.glob(os.path.join(args.data, "dblp-*.xml.gz")))[-1]
    os.makedirs(args.out, exist_ok=True)
    print(f"parsing {os.path.basename(xml_gz)}", flush=True)

    rec_w = RollingWriter(os.path.join(args.out, "record"), RECORD_SCHEMA,
                          args.rows_per_file, args.chunk_rows)
    auth_w = RollingWriter(os.path.join(args.out, "authorship"), AUTHORSHIP_SCHEMA,
                           args.rows_per_file, args.chunk_rows)

    t0 = time.time()
    n = 0
    types = {}
    with gzip.open(xml_gz, "rb") as fh:
        # GzipFile.name carries the .gz path; lxml resolves the relative SYSTEM
        # DTD (which defines the HTML entities like &uuml;) in the same folder.
        context = etree.iterparse(
            fh, events=("end",), tag=list(ENTRY_TAGS),
            load_dtd=True, resolve_entities=True, no_network=True,
            huge_tree=True, recover=True,
        )
        for _, elem in context:
            process_entry(elem, rec_w, auth_w)
            types[etree.QName(elem).localname] = types.get(etree.QName(elem).localname, 0) + 1
            n += 1
            # free memory: clear this element and its already-processed siblings
            elem.clear()
            while elem.getprevious() is not None:
                del elem.getparent()[0]
            if n % 500_000 == 0:
                print(f"[{(time.time()-t0)/60:5.1f}m] records {n:>10,}  "
                      f"authorships {auth_w.total_rows:>11,}", flush=True)
            if args.max_records is not None and n >= args.max_records:
                break

    rec_w.close()
    auth_w.close()
    print(f"DONE in {(time.time()-t0)/60:.1f} min: {rec_w.total_rows:,} records, "
          f"{auth_w.total_rows:,} authorships", flush=True)
    print("by type:", ", ".join(f"{k} {v:,}" for k, v in
                                 sorted(types.items(), key=lambda kv: -kv[1])), flush=True)


if __name__ == "__main__":
    main()
