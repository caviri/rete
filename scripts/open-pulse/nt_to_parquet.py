#!/usr/bin/env python3
"""Flatten canonical N-Triples into a single triples.parquet.

Streams an N-Triples file (as produced by `rete export`) and emits a flat
subject/predicate/object table with object-term metadata, so the full graph is
queryable in DuckDB alongside the per-class tables.

Columns: subject, predicate, object, obj_kind (iri|literal|bnode),
         obj_datatype, obj_lang
"""
import argparse
import gzip
import os

import pyarrow as pa
import pyarrow.parquet as pq

BATCH = 500_000


def opener(path):
    return gzip.open(path, "rt", encoding="utf-8") if path.endswith(".gz") \
        else open(path, "r", encoding="utf-8")


def unescape(s):
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            nxt = s[i + 1]
            mp = {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\", "/": "/"}
            if nxt in mp:
                out.append(mp[nxt]); i += 2; continue
            if nxt == "u":
                out.append(chr(int(s[i + 2:i + 6], 16))); i += 6; continue
            if nxt == "U":
                out.append(chr(int(s[i + 2:i + 10], 16))); i += 10; continue
        out.append(c); i += 1
    return "".join(out)


def parse_object(tok):
    """tok is the object term (already stripped of trailing ' .')."""
    if tok.startswith("<") and tok.endswith(">"):
        return tok[1:-1], "iri", None, None
    if tok.startswith("_:"):
        return tok, "bnode", None, None
    if tok.startswith('"'):
        # find closing unescaped quote
        i = 1
        while i < len(tok):
            if tok[i] == "\\":
                i += 2; continue
            if tok[i] == '"':
                break
            i += 1
        lit = unescape(tok[1:i])
        rest = tok[i + 1:]
        if rest.startswith("^^<"):
            return lit, "literal", rest[3:-1], None
        if rest.startswith("@"):
            return lit, "literal", None, rest[1:]
        return lit, "literal", None, None
    return tok, "literal", None, None


def split_triple(line):
    line = line.rstrip("\n")
    if line.endswith(" ."):
        line = line[:-2]
    elif line.endswith("."):
        line = line[:-1].rstrip()
    # subject
    if line[0] == "<":
        e = line.index(">"); s = line[1:e]; rest = line[e + 1:].lstrip()
    else:  # bnode subject
        e = line.index(" "); s = line[:e]; rest = line[e + 1:].lstrip()
    # predicate (always an IRI in NT)
    e = rest.index(">"); p = rest[1:e]; obj = rest[e + 1:].lstrip()
    return s, p, obj


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--nt", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    # Flat-triples schema matching the playground's multi-backend Explore tab
    # convention (cf. the bcul companion): object = the raw N-Triples token,
    # value = the decoded literal / IRI, plus otype / datatype / lang.
    schema = pa.schema([
        ("subject", pa.string()), ("predicate", pa.string()),
        ("object", pa.string()), ("otype", pa.string()),
        ("value", pa.string()), ("datatype", pa.string()), ("lang", pa.string()),
    ])
    writer = pq.ParquetWriter(args.out, schema, compression="zstd")
    cols = [[], [], [], [], [], [], []]
    n = 0

    def flush():
        arrs = [pa.array(c, type=schema.field(i).type) for i, c in enumerate(cols)]
        writer.write_table(pa.Table.from_arrays(arrs, schema=schema))
        for c in cols:
            c.clear()

    with opener(args.nt) as fh:
        for line in fh:
            if not line or line[0] == "#" or not line.strip():
                continue
            try:
                s, p, obj = split_triple(line)
                o, kind, dt, lang = parse_object(obj)
            except Exception:
                continue
            cols[0].append(s); cols[1].append(p)
            cols[2].append(obj); cols[3].append(kind)
            cols[4].append(o); cols[5].append(dt); cols[6].append(lang)
            n += 1
            if len(cols[0]) >= BATCH:
                flush()
    if cols[0]:
        flush()
    writer.close()
    print(f"wrote {args.out}: {n} triples")


if __name__ == "__main__":
    main()
