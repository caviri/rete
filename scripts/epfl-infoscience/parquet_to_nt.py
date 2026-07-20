"""EPFL Infoscience Parquet -> N-Triples, for `rete build`.

Two passes over the entity tables:
  1. build uuid -> canonical IRI map for every item (so DSpace `authority`
     links between items become real graph edges);
  2. stream each row -> rdf:type + datatype properties (from metadata_json,
     mapped to the infs: ontology terms) + object-property relations resolved
     through the uuid map, + the harvested full text where present.

IRIs follow the scholar canonical policy (free cross-dataset joins):
  publication+doi -> https://doi.org/{doi};  person+orcid -> https://orcid.org/{orcid};
  else https://infoscience.epfl.ch/handle/{handle}  (or /item/{uuid} if no handle).

Usage:
  python scripts/epfl-infoscience/parquet_to_nt.py --out data/epfl-infoscience/infoscience.nt
"""

import argparse
import glob
import json
import os
import re

import pyarrow.parquet as pq

INFS = "https://w3id.org/rete/infoscience#"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
BASE = r"D:\pro\rete\data\epfl-infoscience"

ENTITY_CLASS = {"Publication": "Publication", "Person": "Person", "OrgUnit": "OrgUnit",
                "Journal": "Journal", "Event": "Event", "Patent": "Patent",
                "Product": "Product", "Funding": "Funding"}

# DSpace metadata field -> infs datatype-property local name
LIT = {
    "dc.title": "title", "dc.type": "type", "dc.date.issued": "dateIssued",
    "dc.date.accepted": "dateAccepted", "dc.description.abstract": "abstract",
    "dc.language.iso": "language", "dc.publisher": "publisher",
    "dc.publisher.place": "publisherPlace", "dc.source": "source", "dc.size": "size",
    "dc.description.sponsorship": "sponsorship", "dc.subject": "subject",
    "dc.date.created": "dateCreated", "dc.date.modified": "dateModified",
    "dc.date.accessioned": "dateAccessioned", "dc.date.available": "dateAvailable",
    "dc.identifier.uri": "uri", "datacite.rights": "rights",
    "person.givenName": "givenName", "person.familyName": "familyName",
    "person.email": "email", "person.jobTitle": "jobTitle",
    "person.identifier.scopus-author-id": "scopusAuthorId",
    "person.identifier.rid": "researcherId", "person.identifier.openalex": "openalexAuthorId",
    "epfl.sciper.active": "sciperActive", "epfl.synchronization.date": "synchronizationDate",
    "crisrp.name.variant": "nameVariant", "person.affiliation.name": "affiliationName",
    "oairecerif.affiliation.role": "affiliationRole",
    "oairecerif.affiliation.startDate": "affiliationStartDate",
    "oairecerif.affiliation.endDate": "affiliationEndDate",
    "oairecerif.acronym": "acronym", "epfl.unit.code": "unitCode",
    "epfl.unit.infoscienceCode": "unitInfoscienceCode", "epfl.orgUnit.active": "orgUnitActive",
    "epfl.orgUnit.level": "orgUnitLevel", "organization.foundingDate": "foundingDate",
    "dc.identifier.issn": "issn", "dc.identifier.eissn": "eissn",
    "oairecerif.event.place": "eventPlace", "oaire.citation.conferenceDate": "conferenceDate",
    "dc.identifier.patentno": "patentNumber", "oairecerif.patent.country": "patentCountry",
    "epfl.patent.kindcode": "patentKindCode", "epfl.patent.date": "patentDate",
    "epfl.thesis.doctoralSchool": "doctoralSchool", "epfl.thesis.faculty": "faculty",
    "epfl.thesis.institute": "institute", "epfl.thesis.number": "thesisNumber",
    "epfl.thesis.jury": "jury", "epfl.thesis.publicDefenseYear": "publicDefenseYear",
    "epfl.writtenAt": "writtenAt",
}
# DSpace field -> (infs object-property, fallback literal predicate) resolved via authority uuid
REL = {
    "dc.contributor.author": ("hasAuthor", "http://purl.org/dc/terms/creator"),
    "dc.contributor.advisor": ("hasAdvisor", "http://purl.org/dc/terms/contributor"),
    "oairecerif.person.affiliation": ("affiliatedWith", None),
    "organization.parentOrganization": ("parentOrg", None),
    "crisou.director": ("director", None),
}

_BAD_IRI = re.compile(r'[\x00-\x20<>"{}|\\^`]')


def iri(u):
    return f"<{u}>"


def lit(v):
    v = str(v).replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")
    return f'"{v}"'


def mint(etype, doi, orcid, handle, uuid):
    cand = None
    if etype == "Publication" and doi:
        cand = f"https://doi.org/{doi}"
    elif etype == "Person" and orcid:
        cand = f"https://orcid.org/{orcid}"
    elif handle:
        cand = f"https://infoscience.epfl.ch/handle/{handle}"
    if cand and not _BAD_IRI.search(cand):
        return cand
    if handle and not _BAD_IRI.search(f"https://infoscience.epfl.ch/handle/{handle}"):
        return f"https://infoscience.epfl.ch/handle/{handle}"
    return f"https://infoscience.epfl.ch/item/{uuid}"


def tables(base):
    for d in sorted(glob.glob(os.path.join(base, "parquet-*"))):
        t = os.path.basename(d).replace("parquet-", "")
        if t == "fulltext":
            continue
        yield t, d


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base", default=BASE)
    ap.add_argument("--out", default=os.path.join(BASE, "infoscience.nt"))
    args = ap.parse_args()

    # --- pass 1: uuid -> IRI (+ entity class) ---
    print("pass 1: building uuid -> IRI map", flush=True)
    umap = {}
    for t, d in tables(args.base):
        for f in glob.glob(os.path.join(d, "*.parquet")):
            tb = pq.read_table(f, columns=["uuid", "entity_type", "doi", "orcid", "handle"])
            for uuid, et, doi, orcid, handle in zip(tb["uuid"].to_pylist(), tb["entity_type"].to_pylist(),
                                                    tb["doi"].to_pylist(), tb["orcid"].to_pylist(),
                                                    tb["handle"].to_pylist()):
                if uuid:
                    umap[uuid] = mint(et or t.capitalize(), doi, orcid, handle, uuid)
    print(f"  {len(umap):,} items mapped", flush=True)

    n_triples = 0
    out = open(args.out, "w", encoding="utf-8", newline="\n")

    def emit(s, p, o):
        nonlocal n_triples
        out.write(f"{s} {p} {o} .\n")
        n_triples += 1

    # --- pass 2: rows -> triples ---
    print("pass 2: emitting triples", flush=True)
    for t, d in tables(args.base):
        cls = {"orgunit": "OrgUnit"}.get(t, t.capitalize())
        n = 0
        for f in glob.glob(os.path.join(d, "*.parquet")):
            tb = pq.read_table(f)
            cols = {c: tb[c].to_pylist() for c in tb.column_names}
            for i in range(tb.num_rows):
                uuid = cols["uuid"][i]
                s_iri = umap.get(uuid)
                if not s_iri:
                    continue
                s = iri(s_iri)
                doi, orcid, sciper = cols["doi"][i], cols["orcid"][i], cols["sciper"][i]
                name, handle = cols["name"][i], cols["handle"][i]
                md = json.loads(cols["metadata_json"][i]) if cols["metadata_json"][i] else {}
                # type: Thesis subtype for theses
                dc_type = (md.get("dc.type", [{}])[0].get("value", "") or "").lower()
                klass = "Thesis" if (cls == "Publication" and dc_type.startswith("thesis")) else cls
                emit(s, iri(RDF_TYPE), iri(INFS + klass))
                if name:
                    emit(s, iri(RDFS_LABEL), lit(name))
                emit(s, iri(INFS + "uuid"), lit(uuid))
                if handle:
                    emit(s, iri(INFS + "handle"), lit(handle))
                if doi:
                    emit(s, iri(INFS + "doi"), lit(doi))
                if orcid:
                    emit(s, iri(INFS + "orcid"), lit(orcid))
                if sciper:
                    emit(s, iri(INFS + "sciper"), lit(sciper))
                # datatype properties from metadata
                for field, entries in md.items():
                    local = LIT.get(field)
                    if local:
                        for e in entries:
                            v = e.get("value")
                            if v not in (None, ""):
                                emit(s, iri(INFS + local), lit(v))
                    rel = REL.get(field)
                    if rel:
                        prop, fallback = rel
                        for e in entries:
                            auth, v = e.get("authority"), e.get("value")
                            tgt = umap.get(auth) if auth else None
                            if tgt:
                                emit(s, iri(INFS + prop), iri(tgt))
                            elif fallback and v:
                                emit(s, iri(fallback), lit(v))
                n += 1
        print(f"  {t:12s} {n:>8,} items", flush=True)

    # --- full text (read from the JSONL — still harvesting; text lives there) ---
    ftjsonl = os.path.join(args.base, "jsonl", "fulltext.jsonl")
    nft = 0
    if os.path.exists(ftjsonl):
        with open(ftjsonl, encoding="utf-8") as ff:
            for line in ff:
                line = line.strip()
                if not line:
                    continue
                r = json.loads(line)
                s_iri = umap.get(r.get("uuid"))
                if s_iri and r.get("text"):
                    emit(iri(s_iri), iri(INFS + "fullText"), lit(r["text"]))
                    nft += 1
        print(f"  fulltext     {nft:>8,} texts", flush=True)

    out.close()
    print(f"DONE: {n_triples:,} triples -> {args.out}", flush=True)


if __name__ == "__main__":
    main()
