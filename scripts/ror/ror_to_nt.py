"""Emit N-Triples for ROR from data/ror/parquet/ror.parquet, using the rete ROR
ontology (https://w3id.org/rete/ror#). Each organization's IRI is its ROR URL
(https://ror.org/<id>) — the canonical identifier other datasets already use,
so the graph joins for free.

Output: data/ror/ror.nt
"""

import os
import orjson
import pyarrow.parquet as pq

SRC = r"D:\pro\rete\data\ror\parquet\ror.parquet"
OUT = r"D:\pro\rete\data\ror\ror.nt"
ROR = "https://w3id.org/rete/ror#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"

TYPE_CLASS = {
    "education": "Education", "company": "Company", "healthcare": "Healthcare",
    "government": "Government", "nonprofit": "Nonprofit", "archive": "Archive",
    "facility": "Facility", "funder": "Funder", "other": "OtherOrganization",
}
REL_PROP = {
    "parent": "hasParent", "child": "hasChild", "related": "relatedOrganization",
    "predecessor": "predecessor", "successor": "successor",
}
_ESC = str.maketrans({"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"})


def lit(v, dt=None):
    s = '"' + str(v).translate(_ESC) + '"'
    return s + f"^^<{XSD}{dt}>" if dt else s


def main():
    t = pq.read_table(SRC)
    cols = {n: t.column(n).to_pylist() for n in t.column_names}
    n = len(cols["id"])
    triples = 0
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        buf = []
        for i in range(n):
            iri = cols["id"][i]
            if not iri:
                continue
            s = f"<{iri}>"

            def emit(p, o):
                nonlocal triples
                buf.append(f"{s} <{p}> {o} .\n")
                triples += 1

            # types: base + subclasses from types_json (fallback primary_type)
            emit(f"{RDF}type", f"<{ROR}Organization>")
            types = []
            tj = cols["types_json"][i]
            if tj:
                try:
                    types = [x for x in orjson.loads(tj)]
                except Exception:
                    types = []
            if not types and cols["primary_type"][i]:
                types = [cols["primary_type"][i]]
            for ty in types:
                cls = TYPE_CLASS.get(str(ty).lower())
                if cls:
                    emit(f"{RDF}type", f"<{ROR}{cls}>")

            def sval(col, prop, dt=None):
                v = cols[col][i]
                if v is not None and v != "":
                    emit(f"{ROR}{prop}", lit(v, dt))

            if cols["name"][i]:
                emit(f"{ROR}name", lit(cols["name"][i]))
                emit(f"{RDFS}label", lit(cols["name"][i]))
            sval("ror_id", "rorId")
            sval("status", "status")
            if cols["established"][i] is not None:
                emit(f"{ROR}established", lit(cols["established"][i], "gYear"))
            sval("country_code", "countryCode")
            sval("country_name", "countryName")
            sval("location_name", "locationName")
            if cols["lat"][i] is not None:
                emit(f"{ROR}lat", lit(cols["lat"][i], "double"))
            if cols["lng"][i] is not None:
                emit(f"{ROR}long", lit(cols["lng"][i], "double"))
            if cols["geonames_id"][i] is not None:
                emit(f"{ROR}geonamesId", lit(cols["geonames_id"][i], "integer"))
            if cols["website"][i]:
                emit(f"{ROR}website", lit(cols["website"][i], "anyURI"))
            if cols["wikipedia"][i]:
                emit(f"{ROR}wikipedia", lit(cols["wikipedia"][i], "anyURI"))
            sval("fundref", "fundref")
            sval("grid", "grid")
            sval("isni", "isni")
            sval("wikidata", "wikidata")

            # relationships -> object properties to other ROR IRIs
            rj = cols["relationships_json"][i]
            if rj:
                try:
                    for rel in orjson.loads(rj):
                        prop = REL_PROP.get(rel.get("type"))
                        tgt = rel.get("id")
                        if prop and tgt:
                            emit(f"{ROR}{prop}", f"<{tgt}>")
                except Exception:
                    pass

            if len(buf) >= 50000:
                f.write("".join(buf)); buf.clear()
        f.write("".join(buf))
    print(f"wrote {OUT}: {triples:,} triples for {n:,} organizations "
          f"({os.path.getsize(OUT)/1e6:.0f} MB)")


if __name__ == "__main__":
    main()
