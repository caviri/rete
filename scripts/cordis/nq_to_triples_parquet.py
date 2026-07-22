"""Stream the CORDIS EURIO Knowledge Graph (N-Quads, inside the .zip) into a flat
triples Parquet — the basis for both the ontology extraction and the per-class
property tables.

Output: data/cordis/triples/<entity>.parquet
  columns: subject, predicate, object, otype ('iri'|'bnode'|'lit'),
           lang, datatype, graph

Explicit N-Quads char-parser (no regex) — robust to literals containing spaces,
quotes, and IRIs. One worker per named-graph member (they parse independently
from their own zip handle).
"""

import os
import re
import zipfile
from concurrent.futures import ProcessPoolExecutor

import pyarrow as pa
import pyarrow.parquet as pq

ZIP = r"D:\pro\rete\data\cordis\cordis-EURIOKnowledgeGraph-nq.zip"
BASE = "extraction_2026_04_02_12_51_13_316/"
OUT = r"D:\pro\rete\data\cordis\triples"
MEMBERS = ["Project.nq", "Grant.nq", "Result.nq", "Organisation.nq",
           "OrganisationRole.nq", "FundingScheme.nq"]

SCHEMA = pa.schema([
    ("subject", pa.string()), ("predicate", pa.string()), ("object", pa.string()),
    ("otype", pa.string()), ("lang", pa.string()), ("datatype", pa.string()),
    ("graph", pa.string()),
])

_ESC = {"t": "\t", "b": "\b", "n": "\n", "r": "\r", "f": "\f",
        '"': '"', "\\": "\\", "'": "'", "/": "/"}
_U = re.compile(r"\\u([0-9A-Fa-f]{4})|\\U([0-9A-Fa-f]{8})")


def _unescape(s):
    if "\\" not in s:
        return s
    s = _U.sub(lambda m: chr(int(m.group(1) or m.group(2), 16)), s)
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            out.append(_ESC.get(s[i + 1], s[i + 1]))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _skip_ws(s, i):
    n = len(s)
    while i < n and s[i] in " \t":
        i += 1
    return i


def _read_node(s, i):
    """IRI or blank node -> (value, otype, next_i)."""
    i = _skip_ws(s, i)
    if s[i] == "<":
        j = s.index(">", i + 1)
        return s[i + 1:j], "iri", j + 1
    j = i
    n = len(s)
    while j < n and s[j] not in " \t":
        j += 1
    return s[i:j], "bnode", j


def parse_line(line):
    """N-Quads line -> (s, p, o, otype, lang, datatype, graph) or None."""
    line = line.rstrip("\r\n")
    if not line or line[0] == "#":
        return None
    # drop the trailing ' .'
    e = len(line) - 1
    while e >= 0 and line[e] in " \t":
        e -= 1
    if e < 0 or line[e] != ".":
        return None
    line = line[:e]
    subj, _, i = _read_node(line, 0)
    pred, _, i = _read_node(line, i)
    i = _skip_ws(line, i)
    lang = dt = None
    if line[i] == '"':
        j = i + 1
        while True:
            c = line[j]
            if c == "\\":
                j += 2
                continue
            if c == '"':
                break
            j += 1
        lit = _unescape(line[i + 1:j])
        j += 1
        if j < len(line) and line[j] == "@":
            k = j + 1
            while k < len(line) and line[k] not in " \t":
                k += 1
            lang = line[j + 1:k]
            j = k
        elif line[j:j + 2] == "^^":
            k = line.index(">", j + 3)
            dt = line[j + 3:k]
            j = k + 1
        obj, otype, i = lit, "lit", j
    else:
        obj, otype, i = _read_node(line, i)
    graph, _, i = _read_node(line, i)
    return subj, pred, obj, otype, lang, dt, graph


def convert_member(member):
    os.makedirs(OUT, exist_ok=True)
    entity = member[:-3]
    out_path = os.path.join(OUT, f"{entity}.parquet")
    z = zipfile.ZipFile(ZIP)
    cols = {f.name: [] for f in SCHEMA}
    writer = pq.ParquetWriter(out_path, SCHEMA, compression="zstd", compression_level=3)
    n = bad = 0
    CHUNK = 500_000

    def flush():
        if not cols["subject"]:
            return
        writer.write_table(pa.table(cols, schema=SCHEMA))
        for k in cols:
            cols[k].clear()

    import io
    with z.open(BASE + member) as f:
        for line in io.TextIOWrapper(f, encoding="utf-8", errors="replace"):
            r = parse_line(line)
            if r is None:
                if line.strip():
                    bad += 1
                continue
            s, p, o, ot, lang, dt, g = r
            cols["subject"].append(s); cols["predicate"].append(p)
            cols["object"].append(o); cols["otype"].append(ot)
            cols["lang"].append(lang); cols["datatype"].append(dt); cols["graph"].append(g)
            n += 1
            if len(cols["subject"]) >= CHUNK:
                flush()
    flush()
    writer.close()
    return entity, n, bad


def main():
    os.makedirs(OUT, exist_ok=True)
    with ProcessPoolExecutor(max_workers=6) as pool:
        for entity, n, bad in pool.map(convert_member, MEMBERS):
            print(f"{entity:18s} {n:>12,} triples  ({bad} bad lines)", flush=True)
    total = sum(pq.read_metadata(os.path.join(OUT, f)).num_rows
                for f in os.listdir(OUT) if f.endswith(".parquet"))
    print(f"TOTAL {total:,} triples -> {OUT}", flush=True)


if __name__ == "__main__":
    main()
