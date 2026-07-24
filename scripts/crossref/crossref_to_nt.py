"""Emit the Crossref works+refs Parquet as N-Triples on stdout, modeled on the
rete Crossref ontology (https://w3id.org/rete/crossref#) and the scholar hub's
canonical-IRI policy — the feed for
`rete build - --format nt --memory-budget-mb N` (external single-file build).

Canonical IRIs (so a union graph joins the sibling datasets for free):
  work / cited work   https://doi.org/<doi>              (lowercased, bare)
  author (with ORCID) https://orcid.org/<id>             (joins orcid.rete)
  funder              https://doi.org/10.13039/<id>      (Crossref Funder ID)

The heavy lifting is DuckDB SQL: each SELECT builds the COMPLETE newline-joined
N-Triples for one row (escaped literals, IRI-safe DOIs, NULL parts dropped by
concat_ws), and Python just streams the single column out in record batches.

Model (one .rete of the full graph, ~4B triples):
  works    rdf:type (per Crossref type), cx:doi, rdfs:label(title),
           cx:containerTitle, cx:issuedYear, cx:publisherName, cx:issn,
           cx:isReferencedByCount
  authors  dcterms:creator "Family, Given" (name literal) +
           cx:authorORCID <orcid IRI> when present
  funders  cx:fundedBy <funder IRI> + rdfs:label on the funder
  cites    <work> cx:cites <cited work>   (DOI-matched references only)

Usage:
  python scripts/crossref/crossref_to_nt.py                 # everything (~4B)
  python scripts/crossref/crossref_to_nt.py --limit 50000   # smoke-test slice
"""

import argparse
import glob
import os
import sys

import duckdb

BASE = "D:/pro/rete/data/crossref/parquet-2026"
ONTOLOGY_TTL = r"D:\pro\rete\data\crossref\crossref.ttl"
CX = "https://w3id.org/rete/crossref#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
DCT_CREATOR = "http://purl.org/dc/terms/creator"
XSD = "http://www.w3.org/2001/XMLSchema#"

# --- literal escaping (backslash first, then quote, then control chars) -------
ESC = (
    "replace(replace(replace(replace(replace({c}, '\\', '\\\\'), '\"', '\\\"'), "
    "chr(10), '\\n'), chr(13), '\\r'), chr(9), '\\t')"
)

# --- percent-encode the characters illegal in an N-Triples IRIREF -------------
#     (% first so we never double-encode; then space and < > " \ ^ ` { | } + ctl)
_IRI_REPL = [
    ("%", "%25"), (" ", "%20"), ('"', "%22"), ("<", "%3C"), (">", "%3E"),
    ("\\", "%5C"), ("^", "%5E"), ("`", "%60"), ("{", "%7B"), ("|", "%7C"),
    ("}", "%7D"),
]


def _sqlstr(ch):
    return "chr(" + str(ord(ch)) + ")" if ord(ch) < 32 else "'" + ch.replace("'", "''") + "'"


def iri_enc(col):
    e = col
    for ch, pc in _IRI_REPL:
        e = "replace(" + e + ", " + _sqlstr(ch) + ", '" + pc + "')"
    for c in ("\t", "\n", "\r"):
        e = "replace(" + e + ", " + _sqlstr(c) + ", '%0" + format(ord(c), "X") + "')"
    return e


def doi_iri(col):
    return "'<https://doi.org/' || " + iri_enc(col) + " || '>'"


def esc(col):
    return ESC.format(c=col)


# Crossref titles/containers sometimes carry HTML entities (&amp;, &lt;, …).
# Unescape them BEFORE NT literal-escaping (&amp; last so &amp;lt; -> &lt; -> <).
_HTML = [("&lt;", "'<'"), ("&gt;", "'>'"), ("&quot;", "'\"'"),
         ("&apos;", "''''"), ("&#39;", "''''"), ("&#x27;", "''''"), ("&amp;", "'&'")]


def htext(col):
    e = col
    for ent, rep in _HTML:
        e = "replace(" + e + ", '" + ent + "', " + rep + ")"
    return e


def _lit(subj, pred, col):
    return ("CASE WHEN " + col + " IS NOT NULL AND len(trim(CAST(" + col + " AS VARCHAR))) > 0 "
            "THEN " + subj + " || ' <" + pred + '> "\' || ' + esc(col) + " || '\" .' END")


def gyear(subj, pred, col):
    return ("CASE WHEN " + col + " IS NOT NULL THEN " + subj + " || ' <" + pred + '> "\' || '
            + col + " || '\"^^<" + XSD + "gYear> .' END")


def intlit(subj, pred, col):
    return ("CASE WHEN " + col + " IS NOT NULL AND " + col + " > 0 THEN " + subj + " || ' <" + pred
            + '> "\' || CAST(' + col + " AS VARCHAR) || '\"^^<" + XSD + "integer> .' END")


def groups(base, sub, n):
    """The part-*.parquet under base/sub split into ~n contiguous file groups —
    chunking a json_each/huge scan by group keeps each pass's RAM bounded."""
    files = sorted(glob.glob(os.path.join(base, sub, "part-*.parquet")))
    if not files:
        raise SystemExit("no parquet under " + os.path.join(base, sub))
    k = max(1, (len(files) + n - 1) // n)
    return [files[i:i + k] for i in range(0, len(files), k)]


def rp(files):
    """read_parquet() over an explicit file list (a chunk of parts)."""
    return "read_parquet([" + ",".join("'" + f.replace("\\", "/") + "'" for f in files) + "])"


def emit(con, sql, out):
    reader = con.execute(sql).fetch_record_batch(200_000)
    n = 0
    while True:
        try:
            batch = reader.read_next_batch()
        except StopIteration:
            break
        lines = batch.column(0).to_pylist()
        lines = [ln for ln in lines if ln]
        if lines:
            out.write(("\n".join(lines) + "\n").encode("utf-8"))
            n += len(lines)
    return n


WORK_CLASS = (
    "CASE type "
    "WHEN 'journal-article' THEN 'JournalArticle' "
    "WHEN 'book-chapter' THEN 'BookChapter' "
    "WHEN 'proceedings-article' THEN 'ProceedingsArticle' "
    "WHEN 'dataset' THEN 'Dataset' "
    "WHEN 'posted-content' THEN 'PostedContent' "
    "WHEN 'book' THEN 'Book' "
    "WHEN 'monograph' THEN 'Monograph' "
    "WHEN 'report' THEN 'Report' "
    "WHEN 'dissertation' THEN 'Dissertation' "
    "WHEN 'standard' THEN 'Standard' "
    "WHEN 'peer-review' THEN 'PeerReview' "
    "WHEN 'reference-entry' THEN 'ReferenceEntry' "
    "WHEN 'component' THEN 'Component' "
    "WHEN 'journal-issue' THEN 'JournalIssue' "
    "WHEN 'grant' THEN 'Grant' "
    "ELSE 'OtherWork' END"
)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--base", default=BASE)
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--memory-limit", default="10GB",
                    help="DuckDB memory_limit — bounded so it survives a shared box.")
    ap.add_argument("--temp-dir", default=None,
                    help="DuckDB spill dir (needs to be writable + roomy; e.g. /spill/ddb-tmp).")
    ap.add_argument("--out", default=None,
                    help="single NT file (binary); default stdout. For smoke/small runs.")
    ap.add_argument("--shard-dir", default=None,
                    help="RESUMABLE per-group NT shards here (skips completed shards on rerun). "
                         "Use for the big build; the .rete build reads the whole dir.")
    ap.add_argument("--ontology", default=ONTOLOGY_TTL,
                    help="crossref.ttl to embed as NT (use the container path when in Docker).")
    # small groups keep each json_each pass well under the memory_limit — a
    # 5.6M-work author group peaked at 9.3 GiB, so ~1.4M works/group (128) is safe.
    ap.add_argument("--author-groups", type=int, default=128)
    ap.add_argument("--works-groups", type=int, default=16)
    ap.add_argument("--funder-groups", type=int, default=32)
    ap.add_argument("--cites-groups", type=int, default=64)
    args = ap.parse_args()
    lim = ("LIMIT " + str(args.limit)) if args.limit else ""

    con = duckdb.connect()
    con.execute("SET threads=" + str(args.threads))
    # NT order is irrelevant; preserving it buffers the whole result in RAM and
    # OOMs the billion-row json_each/cites queries. Cap memory so it stays bounded.
    con.execute("SET preserve_insertion_order=false")
    con.execute("SET memory_limit='" + args.memory_limit + "'")
    if args.temp_dir:
        os.makedirs(args.temp_dir, exist_ok=True)
        con.execute("SET temp_directory='" + args.temp_dir + "'")

    subj = doi_iri("doi")
    aname = ("coalesce("
             "nullif(trim(coalesce(json_extract_string(a.value,'$.family'),'') || ', ' || "
             "coalesce(json_extract_string(a.value,'$.given'),'')), ', '), "
             "json_extract_string(a.value,'$.name'))")
    aorcid = ("regexp_extract(coalesce(json_extract_string(a.value,'$.ORCID'),''), "
              "'(\\d{4}-\\d{4}-\\d{4}-[0-9xX]{4})', 1)")
    fdoi = "json_extract_string(f.value,'$.DOI')"
    fname = "json_extract_string(f.value,'$.name')"
    firi = "'<https://doi.org/' || " + iri_enc(fdoi) + " || '>'"

    # ---- per-file-group SQL builders --------------------------------------
    def works_sql(W):
        return """SELECT concat_ws(chr(10),
          {s} || ' <{rdf}> <{cx}' || {cls} || '> .',
          {s} || ' <{cx}doi> "' || {edoi} || '" .',
          {label}, {container}, {year}, {publisher}, {issn}, {refby}
        ) AS line FROM {W} WHERE doi IS NOT NULL {lim}""".format(
            s=subj, rdf=RDF, cx=CX, cls=WORK_CLASS, edoi=esc("doi"),
            label=_lit(subj, LABEL, htext("title")),
            container=_lit(subj, CX + "containerTitle", htext("container_title")),
            year=gyear(subj, CX + "issuedYear", "issued_year"),
            publisher=_lit(subj, CX + "publisherName", htext("publisher")),
            issn=_lit(subj, CX + "issn", "issn"),
            refby=intlit(subj, CX + "isReferencedByCount", "is_referenced_by_count"),
            W=W, lim=lim)

    def authors_sql(W):  # cap hyperauthorship (>~100 authors): bombs json_each RAM
        return """SELECT concat_ws(chr(10),
          {name},
          CASE WHEN {orc} <> '' THEN {s} || ' <{cx}authorORCID> <https://orcid.org/' || {orc} || '> .' END
        ) AS line
        FROM (SELECT doi, author_json FROM {W}
              WHERE doi IS NOT NULL AND author_json IS NOT NULL
                    AND length(author_json) <= 20000 {lim}),
             json_each(author_json) AS a""".format(
            name=_lit(subj, DCT_CREATOR, htext(aname)), orc=aorcid, s=subj, cx=CX, W=W, lim=lim)

    def funders_sql(W):
        return """SELECT concat_ws(chr(10),
          CASE WHEN {fdoi} IS NOT NULL THEN {s} || ' <{cx}fundedBy> ' || {firi} || ' .' END,
          CASE WHEN {fdoi} IS NOT NULL THEN {firi} || ' <{rdf}> <{cx}Funder> .' END,
          CASE WHEN {fdoi} IS NOT NULL AND {fname} IS NOT NULL
               THEN {firi} || ' <{label}> "' || {efname} || '" .' END
        ) AS line
        FROM (SELECT doi, funder_json FROM {W} WHERE doi IS NOT NULL AND funder_json IS NOT NULL {lim}),
             json_each(funder_json) AS f""".format(
            fdoi=fdoi, s=subj, cx=CX, firi=firi, rdf=RDF, label=LABEL,
            fname=fname, efname=esc(htext(fname)), W=W, lim=lim)

    def cites_sql(R):
        return """SELECT {s} || ' <{cx}cites> ' || {o} || ' .' AS line
        FROM {R} WHERE doi IS NOT NULL AND ref_doi IS NOT NULL {lim}""".format(
            s=doi_iri("doi"), cx=CX, o=doi_iri("ref_doi"), R=R, lim=lim)

    def ontology_nt():
        import rdflib
        g = rdflib.Graph()
        g.parse(args.ontology, format="turtle")
        nt = g.serialize(format="nt")
        return nt.encode("utf-8") if isinstance(nt, str) else nt

    # unit list: (name, callable producing bytes OR sql string)
    def build_units(ng_a, ng_w, ng_f, ng_c):
        units = [("ontology", None)]
        for i, gf in enumerate(groups(args.base, "works", ng_w)):
            units.append(("works-%03d" % i, works_sql(rp(gf))))
        for i, gf in enumerate(groups(args.base, "works", ng_a)):
            units.append(("authors-%03d" % i, authors_sql(rp(gf))))
        for i, gf in enumerate(groups(args.base, "works", ng_f)):
            units.append(("funders-%03d" % i, funders_sql(rp(gf))))
        for i, gf in enumerate(groups(args.base, "refs", ng_c)):
            units.append(("cites-%03d" % i, cites_sql(rp(gf))))
        return units

    # ==== RESUMABLE SHARD MODE (big build) =================================
    if args.shard_dir:
        os.makedirs(args.shard_dir, exist_ok=True)
        units = build_units(args.author_groups, args.works_groups,
                            args.funder_groups, args.cites_groups)
        done = skipped = 0
        for name, sql in units:
            shard = os.path.join(args.shard_dir, name + ".nt")
            if os.path.exists(shard):
                skipped += 1
                continue
            part = shard + ".partial"
            with open(part, "wb") as fh:
                if name == "ontology":
                    fh.write(ontology_nt())
                else:
                    emit(con, sql, fh)
            os.replace(part, shard)  # atomic: only a complete shard exists
            done += 1
            print("shard " + name + " done (" + format(done, ",") + " new, "
                  + str(skipped) + " skipped)", file=sys.stderr, flush=True)
        print("SHARDS COMPLETE: " + str(done) + " written, " + str(skipped)
              + " already present, " + str(len(units)) + " total", file=sys.stderr)
        return

    # ==== SINGLE-STREAM MODE (smoke / stdout) ==============================
    out = open(args.out, "wb") if args.out else sys.stdout.buffer
    try:
        out.write(ontology_nt())
    except Exception as e:  # noqa: BLE001 — ontology is additive, not fatal
        print("warning: ontology skipped: " + str(e), file=sys.stderr)
    W1 = rp(groups(args.base, "works", 1)[0])
    R1 = rp(groups(args.base, "refs", 1)[0])
    print("works emitted: " + format(emit(con, works_sql(W1), out), ","), file=sys.stderr)
    print("authors emitted: " + format(emit(con, authors_sql(W1), out), ","), file=sys.stderr)
    print("funders emitted: " + format(emit(con, funders_sql(W1), out), ","), file=sys.stderr)
    print("cites emitted: " + format(emit(con, cites_sql(R1), out), ","), file=sys.stderr)


if __name__ == "__main__":
    main()
