"""Emit the ORCID summaries Parquet as N-Triples on stdout, modeled on the
rete ORCID ontology (https://w3id.org/rete/orcid#) — the feed for
`rete build - --format nt --memory-budget-mb N` (single-pass stdin build).

IRIs are chosen for federation:
  researcher  https://orcid.org/<id>            (canonical ORCID URL)
  work        https://w3id.org/rete/orcid/work/<orcid>/<put_code>
  affiliation https://w3id.org/rete/orcid/affiliation/<orcid>/<put_code>
  funding     https://w3id.org/rete/orcid/funding/<orcid>/<put_code>
  org         the ROR URL itself when org_id is one (joins ror.rete directly)

The heavy lifting is done in DuckDB SQL: each SELECT builds the COMPLETE
newline-joined N-Triples lines for one row (escaped literals, NULL parts
dropped by concat_ws), and Python just streams the single column out.

Usage:
  python scripts/orcid/orcid_to_nt.py              # everything (~1.3B triples)
  python scripts/orcid/orcid_to_nt.py --limit 200000   # smoke-test slice
"""

import argparse
import sys

import duckdb

BASE = "D:/pro/rete/data/orcid/parquet-summaries"
ONTOLOGY_TTL = r"D:\pro\rete\data\orcid\orcid.ttl"
O = "https://w3id.org/rete/orcid#"
R = "https://w3id.org/rete/orcid"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
DCT_CREATOR = "http://purl.org/dc/terms/creator"
XSD = "http://www.w3.org/2001/XMLSchema#"

# escape a column into an N-Triples literal body (backslash first, then quotes,
# then control chars) — pure SQL so DuckDB's threads do the work
ESC = (
    "replace(replace(replace(replace(replace({c}, '\\', '\\\\'), '\"', '\\\"'), "
    "chr(10), '\\n'), chr(13), '\\r'), chr(9), '\\t')"
)


def esc(col: str) -> str:
    return ESC.format(c=col)


def lit(subj: str, pred: str, col: str) -> str:
    """SQL for an optional literal triple line (NULL when col is NULL)."""
    return f"CASE WHEN {col} IS NOT NULL THEN {subj} || ' <{pred}> \"' || {esc(col)} || '\" .' END"


def gyear(subj: str, pred: str, col: str) -> str:
    return (
        f"CASE WHEN {col} IS NOT NULL THEN {subj} || ' <{pred}> \"' || {col} "
        f"|| '\"^^<{XSD}gYear> .' END"
    )


def emit(con, sql, out):
    reader = con.execute(sql).fetch_record_batch(200_000)
    while True:
        try:
            batch = reader.read_next_batch()
        except StopIteration:
            break
        col = batch.column(0)
        lines = col.to_pylist()
        if lines:
            out.write(("\n".join(lines) + "\n").encode("utf-8"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--base", default=BASE)
    args = ap.parse_args()
    lim = f"LIMIT {args.limit}" if args.limit else ""

    out = sys.stdout.buffer

    # 0. the ontology itself (a few hundred triples) so the schema view works
    try:
        import rdflib

        g = rdflib.Graph()
        g.parse(ONTOLOGY_TTL, format="turtle")
        nt = g.serialize(format="nt")
        if isinstance(nt, str):
            out.write(nt.encode("utf-8"))
        else:
            out.write(nt)
    except Exception as e:  # noqa: BLE001 — ontology is additive, not fatal
        print(f"warning: ontology skipped: {e}", file=sys.stderr)

    con = duckdb.connect()
    con.execute("SET threads=8")

    # the parquet `orcid` column carries a leading '/' (an artifact of the XML
    # path it was extracted from) — strip it here or every IRI gets '//'
    def src(glob: str) -> str:
        return (
            "(SELECT * REPLACE (ltrim(orcid, '/') AS orcid) "
            f"FROM read_parquet('{args.base}/{glob}/part-*.parquet'))"
        )

    p = src("person")
    w = src("work")
    a = src("affiliation")
    f = src("funding")

    # ---- person -----------------------------------------------------------
    subj = "'<https://orcid.org/' || orcid || '>'"
    label = "coalesce(credit_name, nullif(trim(coalesce(given_names,'') || ' ' || coalesce(family_name,'')), ''))"
    emit(
        con,
        f"""
        SELECT concat_ws(chr(10),
          {subj} || ' <{RDF}> <{O}Researcher> .',
          {subj} || ' <{O}orcidId> "' || orcid || '" .',
          {lit(subj, O + "givenNames", "given_names")},
          {lit(subj, O + "familyName", "family_name")},
          {lit(subj, LABEL, label)},
          {lit(subj, O + "countryCode", "country")}
        ) AS line FROM {p} WHERE orcid IS NOT NULL {lim}
        """,
        out,
    )
    print("person emitted", file=sys.stderr)

    # ---- work -------------------------------------------------------------
    subj = f"'<{R}/work/' || orcid || '/' || put_code || '>'"
    person = "'<https://orcid.org/' || orcid || '>'"
    # type → ontology class: keep the raw token as a class local name when it is
    # a clean slug (ORCID types are), else fall back to the Work superclass only
    cls = (
        "CASE WHEN type IS NOT NULL AND regexp_matches(type, '^[a-z][a-z-]*$') "
        f"THEN {subj} || ' <{RDF}> <{O}' || type || '> .' END"
    )
    emit(
        con,
        f"""
        SELECT concat_ws(chr(10),
          {subj} || ' <{RDF}> <{O}Work> .',
          {cls},
          {subj} || ' <{DCT_CREATOR}> ' || {person} || ' .',
          {lit(subj, LABEL, "title")},
          {lit(subj, O + "doi", "doi")},
          {gyear(subj, O + "publicationYear", "pub_year")},
          {lit(subj, O + "journalTitle", "journal_title")}
        ) AS line FROM {w}
        WHERE orcid IS NOT NULL AND put_code IS NOT NULL {lim}
        """,
        out,
    )
    print("work emitted", file=sys.stderr)

    # ---- affiliation ------------------------------------------------------
    subj = f"'<{R}/affiliation/' || orcid || '/' || put_code || '>'"
    aff_cls = (
        "CASE aff_type WHEN 'employment' THEN 'Employment' WHEN 'education' THEN 'Education' "
        "WHEN 'qualification' THEN 'Qualification' WHEN 'distinction' THEN 'Distinction' "
        "WHEN 'invited-position' THEN 'InvitedPosition' WHEN 'membership' THEN 'Membership' "
        "WHEN 'service' THEN 'Service' ELSE 'Affiliation' END"
    )
    ror_org = (
        "CASE WHEN org_id LIKE 'https://ror.org/%' THEN "
        f"{subj} || ' <{O}affiliationWith> <' || org_id || '> .' END"
    )
    emit(
        con,
        f"""
        SELECT concat_ws(chr(10),
          {subj} || ' <{RDF}> <{O}' || {aff_cls} || '> .',
          {subj} || ' <{O}affiliationOf> ' || {person} || ' .',
          {ror_org},
          {lit(subj, O + "organizationName", "org_name")},
          {lit(subj, O + "roleTitle", "role_title")},
          {lit(subj, O + "countryCode", "org_country")},
          {gyear(subj, O + "startYear", "start_year")},
          {gyear(subj, O + "endYear", "end_year")}
        ) AS line FROM {a}
        WHERE orcid IS NOT NULL AND put_code IS NOT NULL {lim}
        """,
        out,
    )
    print("affiliation emitted", file=sys.stderr)

    # ---- funding ----------------------------------------------------------
    subj = f"'<{R}/funding/' || orcid || '/' || put_code || '>'"
    emit(
        con,
        f"""
        SELECT concat_ws(chr(10),
          {subj} || ' <{RDF}> <{O}Funding> .',
          {subj} || ' <{O}awardedTo> ' || {person} || ' .',
          {lit(subj, LABEL, "title")},
          {lit(subj, O + "organizationName", "org_name")},
          {gyear(subj, O + "startYear", "start_year")},
          {gyear(subj, O + "endYear", "end_year")}
        ) AS line FROM {f}
        WHERE orcid IS NOT NULL AND put_code IS NOT NULL {lim}
        """,
        out,
    )
    print("funding emitted", file=sys.stderr)


if __name__ == "__main__":
    main()
