"""Generate the machine-readable metadata of the DataCite datasets.

Parquet (metadata 2023/2024/2025 + PID Links 2023/May-2025):
  data/datacite/schema.json       JSON Schema (draft 2020-12): 'metadata' + 'pid_link'
  data/datacite/croissant.jsonld  Croissant 1.0: 5 FileSets + 5 RecordSets
                                  (+ the datacite.rete FileObject with --tally)
RDF (datacite.rete), with --tally <the converter's JSON report>:
  scripts/datacite/cards/datacite.card.json   the Dataset Card `rete build --card-file`
                                              embeds: the conversion rules and what each
                                              rescued, encoded, kept as text or dropped

Usage: python make_metadata.py [--out data/datacite] [--tally tally.json]
"""

import argparse
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
OUT = os.path.join(REPO, "data", "datacite")
CARD = os.path.join(HERE, "cards", "datacite.card.json")

METADATA_FULL = [  # 2024/2025 (51 cols)
    ("doi","VARCHAR"),("prefix","VARCHAR"),("state","VARCHAR"),("source","VARCHAR"),
    ("is_active","BOOLEAN"),("client_id","VARCHAR"),("created","VARCHAR"),
    ("registered","VARCHAR"),("updated","VARCHAR"),("published","VARCHAR"),
    ("publication_year","INTEGER"),("language","VARCHAR"),("version","VARCHAR"),
    ("metadata_version","INTEGER"),("schema_version","VARCHAR"),("url","VARCHAR"),
    ("publisher","VARCHAR"),("resource_type_general","VARCHAR"),("resource_type","VARCHAR"),
    ("reason","VARCHAR"),("title","VARCHAR"),
    ("citation_count","INTEGER"),("reference_count","INTEGER"),("view_count","INTEGER"),
    ("download_count","INTEGER"),("version_count","INTEGER"),("version_of_count","INTEGER"),
    ("part_count","INTEGER"),("part_of_count","INTEGER"),
    ("types_json","VARCHAR"),("container_json","VARCHAR"),("creators_json","VARCHAR"),
    ("titles_json","VARCHAR"),("subjects_json","VARCHAR"),("contributors_json","VARCHAR"),
    ("dates_json","VARCHAR"),("related_identifiers_json","VARCHAR"),
    ("related_items_json","VARCHAR"),("descriptions_json","VARCHAR"),
    ("geo_locations_json","VARCHAR"),("funding_references_json","VARCHAR"),
    ("rights_list_json","VARCHAR"),("identifiers_json","VARCHAR"),
    ("alternate_identifiers_json","VARCHAR"),("sizes_json","VARCHAR"),
    ("formats_json","VARCHAR"),("content_url_json","VARCHAR"),
    ("citations_over_time_json","VARCHAR"),("views_over_time_json","VARCHAR"),
    ("downloads_over_time_json","VARCHAR"),("extra_json","VARCHAR"),
]
METRIC_COLS = {"citation_count","reference_count","view_count","download_count",
               "version_count","version_of_count","part_count","part_of_count",
               "citations_over_time_json","views_over_time_json","downloads_over_time_json"}
METADATA_2023 = [(c, t) for c, t in METADATA_FULL if c not in METRIC_COLS]  # 40 cols

PIDLINK = [
    ("subj_id","VARCHAR"),("obj_id","VARCHAR"),("relation_type","VARCHAR"),
    ("source_id","VARCHAR"),("citation_type","VARCHAR"),("subj_type","VARCHAR"),
    ("obj_type","VARCHAR"),("subj_published","VARCHAR"),("obj_published","VARCHAR"),
    ("subj_year","INTEGER"),("obj_year","INTEGER"),("occurred_at","VARCHAR"),
    ("created_at","VARCHAR"),("updated_at","VARCHAR"),("uuid","VARCHAR"),
    ("subj_extra_json","VARCHAR"),("obj_extra_json","VARCHAR"),("extra_json","VARCHAR"),
]

DESC = {
    "doi":"DOI of the record. Primary cross-dataset join key (OpenCitations / OpenAIRE / ORCID / DBLP); case per DataCite, lowercase on join.",
    "prefix":"DOI prefix (the registrant's namespace, e.g. 10.5281).",
    "state":"Record state; the public file contains only 'findable'.",
    "source":"How the record was registered (e.g. mds, api).",
    "is_active":"Whether the DOI is active.",
    "client_id":"Repository (DataCite client) that registered the DOI. Present in 2024+ files.",
    "created":"Record creation timestamp (ISO 8601).",
    "registered":"DOI registration timestamp (ISO 8601).",
    "updated":"Last update timestamp (ISO 8601).",
    "published":"Publication date string as supplied.",
    "publication_year":"Publication year.",
    "language":"Primary language of the resource.",
    "version":"Version label of the resource.",
    "metadata_version":"DataCite metadata version counter.",
    "schema_version":"DataCite metadata schema version (e.g. http://datacite.org/schema/kernel-4).",
    "url":"Landing-page URL the DOI resolves to.",
    "publisher":"Publisher name (2024+ publisher objects flattened to name; original in extra_json.publisher_obj).",
    "resource_type_general":"DataCite general resource type (Dataset, Text, Software, Image, PhysicalObject, Collection, …).",
    "resource_type":"Free-text resource type as supplied.",
    "reason":"Reason a DOI is not findable (usually null here).",
    "title":"First title (convenience; full list in titles_json).",
    "citation_count":"Citation count (2024+ files only).",
    "reference_count":"Reference count (2024+ only).",
    "view_count":"View count (2024+ only).",
    "download_count":"Download count (2024+ only).",
    "version_count":"Number of versions (2024+ only).",
    "version_of_count":"Number of works this is a version of (2024+ only).",
    "part_count":"Number of parts (2024+ only).",
    "part_of_count":"Number of works this is part of (2024+ only).",
    "types_json":"JSON object of the DataCite `types` block (schemaOrg, resourceTypeGeneral, citeproc, bibtex, ris).",
    "container_json":"JSON of the container (e.g. hosting repository/series).",
    "creators_json":"JSON array of creators: name, givenName, familyName, nameIdentifiers (ORCID), affiliation.",
    "titles_json":"JSON array of all titles.",
    "subjects_json":"JSON array of subjects/keywords.",
    "contributors_json":"JSON array of contributors with roles.",
    "dates_json":"JSON array of typed dates.",
    "related_identifiers_json":"JSON array of related identifiers: relationType, relatedIdentifier, type. The citation/derivation network.",
    "related_items_json":"JSON array of related items (structured citations).",
    "descriptions_json":"JSON array of descriptions/abstracts.",
    "geo_locations_json":"JSON array of geo locations.",
    "funding_references_json":"JSON array of funding references (funder, award number).",
    "rights_list_json":"JSON array of rights/licenses.",
    "identifiers_json":"JSON array of alternate identifiers of this resource.",
    "alternate_identifiers_json":"JSON array of legacy alternate identifiers.",
    "sizes_json":"JSON array of sizes.","formats_json":"JSON array of formats.",
    "content_url_json":"JSON array/string of content URLs.",
    "citations_over_time_json":"JSON time series of citations (2024+ only).",
    "views_over_time_json":"JSON time series of views (2024+ only).",
    "downloads_over_time_json":"JSON time series of downloads (2024+ only).",
    "extra_json":"JSON catch-all of any attribute key not promoted to its own column.",
    # pid_link
    "subj_id":"Subject PID (bare DOI when a doi.org URL, else full URL). Join key to metadata.doi.",
    "obj_id":"Object PID (bare DOI or URL). Join key to metadata.doi.",
    "relation_type":"Relation, e.g. references, cites, is-supplement-to, is-identical-to, is-derived-from, is-authored-by.",
    "source_id":"Provenance of the relation (crossref, datacite-related, datacite-crossref, …).",
    "citation_type":"schema.org type pair, e.g. ScholarlyArticle-Dataset.",
    "subj_type":"schema.org @type of the subject endpoint.",
    "obj_type":"schema.org @type of the object endpoint.",
    "subj_published":"Publication date of the subject endpoint.",
    "obj_published":"Publication date of the object endpoint.",
    "subj_year":"Publication year of the subject.","obj_year":"Publication year of the object.",
    "occurred_at":"When the relation occurred (0000-01-01… = unknown).",
    "created_at":"Event creation timestamp.","updated_at":"Event update timestamp.",
    "uuid":"Event UUID.",
    "subj_extra_json":"JSON of subject endpoint fields beyond id/@type/date_published.",
    "obj_extra_json":"JSON of object endpoint fields beyond id/@type/date_published.",
}

COUNTS = {"metadata_2023":52863283,"metadata_2024":72019577,"metadata_2025":108468906,
          "pidlinks_2023":167844248,"pidlinks_may2025":592958301}
GLOBS = {"metadata_2023":"parquet-2023/part-*.parquet","metadata_2024":"parquet-2024/part-*.parquet",
         "metadata_2025":"parquet-2025/part-*.parquet","pidlinks_2023":"parquet-links-2023/part-*.parquet",
         "pidlinks_may2025":"parquet-links-may2025/part-*.parquet"}
RS_COLS = {"metadata_2023":METADATA_2023,"metadata_2024":METADATA_FULL,"metadata_2025":METADATA_FULL,
           "pidlinks_2023":PIDLINK,"pidlinks_may2025":PIDLINK}

JSON_T = {"VARCHAR":"string","INTEGER":"integer","BOOLEAN":"boolean"}
CR_T = {"VARCHAR":"sc:Text","INTEGER":"sc:Integer","BOOLEAN":"sc:Boolean"}
LICENSE = "https://creativecommons.org/publicdomain/zero/1.0/"


def props(cols, required_null_ok):
    out = {}
    for c, dt in cols:
        typ = JSON_T[dt] if c in required_null_ok else [JSON_T[dt], "null"]
        p = {"type": typ, "description": DESC[c]}
        if c.endswith("_json"):
            p["contentMediaType"] = "application/json"
        out[c] = p
    return out


def json_schema():
    return {
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$id":"https://w3id.org/rete/datacite/schema.json",
        "title":"DataCite Public Data Files — Parquet row schemas",
        "description":"Row schemas for the DataCite Public Data Files converted to "
                      "Parquet. 'metadata' = one DOI record (metric columns present in "
                      "2024+ files only); 'pid_link' = one PID Graph relationship event. "
                      "Source: https://datafiles.datacite.org/ (CC0).",
        "$defs":{
            "metadata":{"type":"object","title":"DataCite metadata record",
                "description":"One row per DOI. Metric columns (citation_count, …, "
                              "*_over_time_json) appear only in the 2024 and 2025 files.",
                "properties":props(METADATA_FULL,{"doi"}),"required":["doi"],
                "additionalProperties":False},
            "pid_link":{"type":"object","title":"DataCite PID Graph relationship",
                "description":"One row per relationship event between two PIDs.",
                "properties":props(PIDLINK,{"subj_id","obj_id","relation_type"}),
                "required":["subj_id","obj_id","relation_type"],
                "additionalProperties":False},
        },
    }


CR_CONTEXT = {
    "@language":"en","@vocab":"https://schema.org/","citeAs":"cr:citeAs","column":"cr:column",
    "conformsTo":"dct:conformsTo","cr":"http://mlcommons.org/croissant/","rai":"http://mlcommons.org/croissant/RAI/",
    "data":{"@id":"cr:data","@type":"@json"},"dataType":{"@id":"cr:dataType","@type":"@vocab"},
    "dct":"http://purl.org/dc/terms/","examples":{"@id":"cr:examples","@type":"@json"},
    "equivalentProperty":{"@id":"cr:equivalentProperty","@type":"@vocab"},"samplingRate":"cr:samplingRate",
    "extract":"cr:extract","field":"cr:field","fileProperty":"cr:fileProperty","fileObject":"cr:fileObject",
    "fileSet":"cr:fileSet","format":"cr:format","includes":"cr:includes","isLiveDataset":"cr:isLiveDataset",
    "jsonPath":"cr:jsonPath","key":"cr:key","md5":"cr:md5","parentField":"cr:parentField","path":"cr:path",
    "recordSet":"cr:recordSet","references":"cr:references","regex":"cr:regex","repeated":"cr:repeated",
    "replace":"cr:replace","sc":"https://schema.org/","separator":"cr:separator","source":"cr:source",
    "subField":"cr:subField","transform":"cr:transform",
}


def croissant():
    distribution, record_sets = [], []
    for rs, cols in RS_COLS.items():
        fs = rs + "_files"
        distribution.append({"@type":"cr:FileSet","@id":fs,"name":fs,
            "description":f"Parquet shards for {rs}.","encodingFormat":"application/x-parquet",
            "includes":GLOBS[rs]})
        key = "doi" if rs.startswith("metadata") else "subj_id"
        fields = [{"@type":"cr:Field","@id":f"{rs}/{c}","name":c,"description":DESC[c],
                   "dataType":CR_T[dt],"source":{"fileSet":{"@id":fs},"extract":{"column":c}}}
                  for c, dt in cols]
        record_sets.append({"@type":"cr:RecordSet","@id":rs,"name":rs,
            "description":f"{'DOI metadata records' if rs.startswith('metadata') else 'PID Graph relationship events'} — {COUNTS[rs]:,} rows.",
            "field":fields})
    return {
        "@context":CR_CONTEXT,"@type":"sc:Dataset","conformsTo":"http://mlcommons.org/croissant/1.0",
        "name":"datacite-public-data-files","description":"The DataCite Public Data Files "
            "converted to Parquet: DOI metadata for 2023, 2024 and 2025, plus the PID Graph "
            "'PID Links' relationship events for 2023 and May 2025. Metadata rows carry the DOI, "
            "resource type, creators (with ORCIDs), publisher and the related-identifier network; "
            "PID Links are the citation/derivation edge tables.",
        "version":"1.0.0","datePublished":"2026-01-06",
        "license":LICENSE,"url":"https://datafiles.datacite.org/",
        "citeAs":"DataCite (2023–2025). DataCite Public Data File. https://datafiles.datacite.org/",
        "keywords":["DOI","research data","citations","persistent identifiers","metadata","PID graph"],
        "creator":{"@type":"sc:Organization","name":"DataCite","url":"https://datacite.org"},
        "distribution":distribution,"recordSet":record_sets,
    }


# ----------------------------------------------------------------------- the .rete card
RETE_URL = "https://data.graphplaza.com/datacite/datacite.rete"
# relation properties the 2026-07 build already had; everything else in the tally is new (R7)
OLD_PROPS = {"cites", "isCitedBy", "references", "isReferencedBy", "isSupplementTo",
             "isSupplementedBy", "isVersionOf", "hasVersion", "isNewVersionOf", "isPartOf",
             "hasPart", "isDerivedFrom", "isSourceOf", "isIdenticalTo", "isRelatedTo",
             "isAuthoredBy"}
RULES = [  # (rule, one line on what it does) -- kept in step with parquet_to_nt.py's docstring
    ("R0", "One identity per DOI: the registered string, ASCII whitespace trimmed, lower-cased; nothing else (an NBSP inside or at the end of a registered DOI is part of it). The plan, the record's IRI and every pointer to it use this key; a record's own DOI never goes through R1. The build refuses to run if two registered DOIs would mint one IRI."),
    ("R1", "One DOI normaliser for related identifiers, funder ids and link endpoints: strip doi.org / dx.doi.org URL prefixes, `doi:`, `doi/`, `org/`, a leading `%20` / `%7C%5D`, spaces inside the `10.NNNN` prefix. Lower-case (DOIs are case-insensitive). A value that is a registered DOI is that DOI, untouched."),
    ("R2", "Percent-encode what RFC 3987 forbids (`[ ]`, a second `#`, a stray `%`, U+FFFD, C1, private-use, noncharacters) exactly as `rete export --sanitize-iris` does; a valid IRI never moves. Every IRI is then checked by oxiri (pyoxigraph); one it still rejects is dropped."),
    ("R3", "Funder IRI: ROR (check digits must pass) > Crossref Funder ID (`10.13039/…`, also a bare Funder Registry number) > a record-scoped IRI `<doi>/funder/<sha1(NFC(name))[:16]>`."),
    ("R4", "A funder name that is empty or `#PLACEHOLDER_PARENT_METADATA_VALUE#` is not a name; a reference with neither a name nor a usable id is dropped."),
    ("R5", "A link endpoint with no scheme is rescued by R1 or dropped (with its whole event); a URL the IRI parser rejects is dropped."),
    ("R6", "`hdl:` -> `https://hdl.handle.net/`. An `https://orcid.org/` IRI is minted only when the ISO 7064 mod 11-2 check digit passes (16 digits without hyphens are re-hyphenated); a creator's failing value is kept as `dcite:rawIdentifier`, a link URL that fails is kept verbatim."),
    ("R7", "All 38 DataCite 4.6 relationType values have their own property, for metadata and PID links alike; `is-funded-by` is `dcite:fundedBy`, `is-authored-at` / `is-affiliated-with` have their own properties; any other link type is `dcite:isRelatedTo` and keeps its exact type on the reified `dcite:PidRelation`."),
    ("R8", "A DOI-typed related identifier with no DOI after R1 is dropped; a DOI inside other text is taken only when exactly one stands there, not glued to other characters."),
    ("R9", "One record per DOI: the 2025 file wins, else 2024, else 2023; `dcite:fromSnapshot` names the file each record came from."),
    ("R10", "Within one file, the row with the latest `updated` wins."),
    ("R11", "`occurredAt` `0000-01-01` (DataCite's 'unknown') is not emitted. A publicationYear outside 1000-2027 is kept as `dcite:publicationYearText`, not typed `xsd:gYear`."),
    ("R12", "`landingPage` is typed `xsd:anyURI` only when it parses as an IRI; otherwise it is kept as a plain string."),
    ("R13", "Literals are written as DataCite has them: no trimming, no Unicode normalisation, no mojibake repair (U+FFFD and double-encoded UTF-8 are upstream). C0 controls are written as N-Triples `\\u` escapes, not removed."),
    ("R14", "`rorId` only when the ROR check digits pass; otherwise the value is kept as `dcite:rawIdentifier`."),
    ("R15", "Under `10.13039/` (the Funder Registry) a DOI is `10.13039/<digits>`. A value that names exactly one Funder Registry number (`10.13039/http://dx.doi.org/10.13039/501100001809`, `10.13039/501100002569\"`) is that Funder ID; a placeholder (`10.13039/#`, `funder%20id`, `xx`) is not minted: kept as `dcite:rawIdentifier` in metadata, its event dropped in the PID links."),
    ("R16", "Every fundingReference is a record-scoped `dcite:FundingReference` (`<doi>/funding/<n>`) carrying what that record says: `dcite:funderName` and any id not minted. The funder IRI, shared by every record that names it, carries no name, so one record's name is never asserted of another's funder."),
    ("R17", "A DOI written with percent-escapes (`%28`, or double-encoded `%2528` as in `10.1016/s0020-7519%252899%252900141-1`) is kept as written and counted, never decoded: DataCite registers DOIs with literal escapes (`10.18716/nmrshiftdb2/60004113/mrc_methanol-d4%20%28cd3od%29` resolves only as written), so decoding is not lossless. Such a value does not join the unescaped DOI."),
    ("R18", "A PID Links event is its `uuid`: one `dcite:PidRelation` `<https://api.datacite.org/events/{uuid}>` per event, with exactly one subject, object, type, source and time. An event in both files is taken from May 2025 (as R9). The build refuses a missing or repeated uuid."),
    ("R19", "What a record says about a creator (name as `dcite:creatorName`, affiliations, ROR, ids not minted) is on its record-scoped `dcite:AgentRole` (`<doi>/creator/<n>`); an `https://orcid.org/` agent, shared by every record naming that ORCID, carries only `dcite:orcid`."),
]


def _bucket(what):
    if what.startswith("rescued"):
        return "rescued"
    if what.startswith("encoded"):
        return "encoded"
    if what.startswith("skipped"):
        return "empty"
    if what.startswith("dropped"):
        return "dropped"
    if any(k in what for k in ("kept_as_text", "kept_verbatim", "failed_check", "id_not_minted",
                               "untyped_landing_page", "nonstandard_doi", "doi_url_without_doi")):
        return "kept"
    return None


def summarize(tally):
    """Aggregate the converter's per-set tally into per-rule totals."""
    rules, info = {}, {}
    for st, per_rule in tally["counts"].items():
        for rule, kv in per_rule.items():
            for what, n in kv.items():
                b = _bucket(what)
                if b:
                    rules.setdefault(rule, {}).setdefault(b, 0)
                    rules[rule][b] += n
                info.setdefault(rule, {}).setdefault(what, 0)
                info[rule][what] += n
    return rules, info


def _n(x):
    return f"{x:,}"


def rete_card(tally):
    rules, info = summarize(tally)
    plan = tally.get("plan", {}).get("stats", {})
    trip = sum(v for k, v in info.get("output", {}).items() if k == "triples")
    edges = {k.split(".", 1)[1]: v for k, v in info.get("R7", {}).items()
             if k.startswith(("relid.", "link.")) and not k.startswith("link.outside")}
    new_edges = sum(v for k, v in info.get("R7", {}).items()
                    if k.startswith(("relid.", "link.")) and k.split(".", 1)[1] not in OLD_PROPS
                    and not k.startswith("link.outside"))
    outside = sum(v for k, v in info.get("R7", {}).items() if k.startswith("link.outside"))
    recs = {y: plan.get(f"records_from_{y}", 0) for y in ("2025", "2024", "2023")}
    link_rows = sum(v for st, r in tally["counts"].items() if st.startswith("links-")
                    for k, v in r.get("input", {}).items() if k == "rows")
    # rules whose numbers come from the plan or from edge counts rather than from buckets
    rules.setdefault("R7", {})["rescued"] = new_edges
    rules.setdefault("R9", {})["dropped"] = (plan.get("rows_not_emitted_2023", 0)
                                             + plan.get("rows_not_emitted_2024", 0))
    rules.setdefault("R10", {})["dropped"] = sum(
        plan.get(f"within_year_duplicate_rows_{y}", 0) - plan.get(f"within_year_duplicate_dois_{y}", 0)
        for y in ("2023", "2024", "2025"))
    rows = ["| Rule | What it does | Rescued | Encoded | Kept as text | Dropped |",
            "|---|---|---:|---:|---:|---:|"]
    for rule, text in RULES:
        r = rules.get(rule, {})
        rows.append(f"| {rule} | {text} | {_n(r.get('rescued', 0))} | {_n(r.get('encoded', 0))} | "
                    f"{_n(r.get('kept', 0))} | {_n(r.get('dropped', 0))} |")
    f3 = info.get("R3", {})
    desc = [
        f"The **DataCite Public Data Files** (CC0) as one range-queryable graph: **{_n(sum(recs.values()))} DOI records**, one per DOI, and the **PID Graph** ({_n(link_rows)} relation events from the 2023 and May 2025 PID Links files), reified as provenance-bearing `dcite:PidRelation`s. Converted by `scripts/datacite/parquet_to_nt.py`; the converter wrote {_n(trip)} statements.",
        "",
        "## What is in it",
        "",
        "- One `dcite:Resource` per DOI at `https://doi.org/{doi}`, typed by its resourceTypeGeneral (all 32 DataCite 4.6 values), with title, publisher, publication year, dates, version, language, landing page, usage metrics (2024+), subjects, reified creators (a `dcite:AgentRole` with the name and affiliations the record gives, held by an `https://orcid.org/` agent when the ORCID is valid), funders (`dcite:fundedBy`) and DOI-typed related identifiers as typed edges.",
        "- The PID Graph: every event is a direct typed edge **and** a `dcite:PidRelation` `<https://api.datacite.org/events/{uuid}>` with one `relationSubject`, `relationObject`, exact `relationType`, `assertedBy` (the source) and `occurredAt`. Events from both PID Links files are kept; an event (uuid) present in both is taken once, from May 2025.",
        "- Funding: `dcite:fundedBy` links a record to its funder (ROR, Crossref Funder ID, or a record-scoped IRI when the record gives no usable id); what the record says about the funder (`dcite:funderName`, an id not minted) sits on its `dcite:FundingReference`, never on the shared funder IRI.",
        "- Works key by DOI and people by ORCID, so the graph joins crossref, opencitations, openaire, dblp, zenodo and orcid by IRI.",
        "",
        "## One record per DOI, and where it came from",
        "",
        f"DataCite published three snapshots. Each DOI is converted **once**, from the newest file that has it (R9), and within one file from the row with the latest `updated` (R10: {_n(plan.get('within_year_duplicate_dois_2024', 0))} DOIs in 2024, {_n(plan.get('within_year_duplicate_dois_2025', 0))} in 2025). `dcite:fromSnapshot` says which file:",
        "",
        f"- `dcite:snapshot-2025`: {_n(recs['2025'])} records",
        f"- `dcite:snapshot-2024`: {_n(recs['2024'])} records whose DOI is not in the 2025 file",
        f"- `dcite:snapshot-2023`: {_n(recs['2023'])} records whose DOI is in neither later file",
        "",
        "Filter with `?r dcite:fromSnapshot dcite:snapshot-2025` for the 2025 state only. The 2026-07 build emitted every row of all three files, so a DOI present in several years carried several titles, creators and relation sets at once; that is gone.",
        "",
        "## Conversion rules and what each did",
        "",
        "The principle is **faithfulness**: the graph says what DataCite says. An identifier is rewritten only when the rewrite is deterministic and names the same thing (`https://doi.org/10.x/y` is the DOI `10.x/y`); anything that cannot be made a valid IRI that way is **dropped and counted**, never guessed. Literals are never cleaned. Counts are occurrences, summed over all five files.",
        "",
        *rows,
        "",
        "For R9 and R10, *Dropped* counts metadata **rows** not converted because a newer row of the same DOI was (no DOI is lost); for R7, *Rescued* counts relation edges that use a property this build added.",
        "",
        f"R7 in numbers: {_n(new_edges)} relation edges now use one of the properties added in this build (they were dropped, or for links folded into `isRelatedTo`, before); {_n(outside)} link events carry a type outside the DataCite 4.6 vocabulary and are `dcite:isRelatedTo` with their exact type on the `dcite:PidRelation`.",
        f"R3 routes: {_n(f3.get('funder.ror', 0))} ROR, {_n(f3.get('funder.doi', 0))} Crossref Funder ID / DOI, {_n(f3.get('funder.name_minted', 0))} name-minted; {_n(info.get('R16', {}).get('funding_reference', 0))} funding references. R18: {_n(info.get('R18', {}).get('pid_relation', 0))} PidRelations, one per event; {_n(plan.get('link_events_superseded_2023', 0))} 2023 events are the same uuid as a May 2025 event. R19: {_n(info.get('R19', {}).get('creator.orcid_agent', 0))} creator roles held by an orcid.org agent, {_n(info.get('R19', {}).get('creator.record_local_agent', 0))} by a record-local one.",
        f"R0: {_n(plan.get('identity_checked_non_ascii_or_escaped_dois', 0))} DOIs that are not plain ASCII were re-keyed and checked: no two registered DOIs share an IRI; {_n(plan.get('registered_exceptions', 0))} registered DOIs that R1 would have rewritten keep their own IRI (e.g. `10.18454/<NBSP>pmi.2016.5.712` is not `10.18454/pmi.2016.5.712`).",
        "",
        "The full per-file tally, with examples of every rescued, kept and dropped shape, is `datacite.tally.json` beside this file.",
        "",
        "## Not converted",
        "",
        "Related identifiers that are not DOI-typed (URL, Handle, ISSN, arXiv, PMID, …), descriptions/abstracts, typed dates, rights, contributors other than creators, geo locations, sizes and formats are in the Parquet but not in this graph.",
        "",
        "## Build",
        "",
        "`parquet_to_nt.py emit | rete build - --strict --memory-budget-mb`, so the build refuses any IRI the N-Triples grammar or RFC 3987 rejects; every emitted statement was also parsed by Oxigraph 0.5.11 before the build. Ontology `dcite:` 1.2.0 (`https://w3id.org/rete/datacite#`, OWL 2 QL-safe) travels in the file. No pyramid at this scale: keep queries selective.",
    ]
    queries = [
        "# Research-output cards straight from the DOI registry\nPREFIX dcite: <https://w3id.org/rete/datacite#>\nSELECT ?doi ?title ?year ?publisher ?type WHERE {\n  ?doi dcite:title ?title ;\n       dcite:publicationYear ?year ;\n       dcite:publisherName ?publisher ;\n       dcite:resourceType ?type .\n}\nLIMIT 25",
        "# Records whose DOI exists only in the 2023 snapshot\nPREFIX dcite: <https://w3id.org/rete/datacite#>\nSELECT ?doi ?title WHERE {\n  ?doi dcite:fromSnapshot dcite:snapshot-2023 ;\n       dcite:title ?title .\n}\nLIMIT 25",
        "# The PID Graph: what a work points at, and why\nPREFIX dcite: <https://w3id.org/rete/datacite#>\nSELECT ?from ?type ?to ?assertedBy WHERE {\n  ?rel a dcite:PidRelation ;\n       dcite:relationSubject ?from ;\n       dcite:relationType ?type ;\n       dcite:relationObject ?to .\n  OPTIONAL { ?rel dcite:assertedBy ?assertedBy }\n}\nLIMIT 25",
        "# Funders of one record as the record names them, joined to Crossref Funder IDs / ROR by IRI\nPREFIX dcite: <https://w3id.org/rete/datacite#>\nSELECT ?funder ?name WHERE {\n  <https://doi.org/10.17638/00008533> dcite:hasFundingReference ?ref .\n  ?ref dcite:funder ?funder .\n  OPTIONAL { ?ref dcite:funderName ?name }\n}",
    ]
    # `extra` may nest two container levels at most (rete refuses deeper card fields)
    extra = {
        "conversion": {"converter": "scripts/datacite/parquet_to_nt.py", "ontology_version": "1.2.0",
                       "statements_emitted": trip, "tally_file": "datacite.tally.json"},
        "conversion_records_by_snapshot": recs,
        "conversion_plan": plan,
        "conversion_rules": {rule: rules.get(rule, {}) for rule, _ in RULES},
        "conversion_relation_edges": dict(sorted(edges.items(), key=lambda kv: -kv[1])),
    }
    return {
        "title": "DataCite",
        "license": "CC0-1.0",
        "source": "https://datafiles.datacite.org/",
        "version": "2026-10 faithful rebuild (Public Data Files 2023, 2024, 2025; PID Links 2023, May 2025)",
        "publisher": {"name": "rete"},
        "canonical_url": RETE_URL,
        "cite_as": "DataCite (2023-2025). DataCite Public Data File. https://datafiles.datacite.org/",
        "keywords": ["DataCite", "DOI", "PID graph", "research data", "software", "citations", "provenance"],
        "description": desc,
        "extra": extra,
        "example_queries": queries,
    }


def rete_fileobject(tally, sha256=None, size=None):
    rules, _ = summarize(tally)
    extra = {}
    if sha256:  # Croissant wants an md5/sha256 on every FileObject
        extra["sha256"] = sha256
    if size:
        extra["contentSize"] = f"{size} B"
    return {**extra, "@type": "cr:FileObject", "@id": "datacite.rete", "name": "datacite.rete",
            "contentUrl": RETE_URL, "encodingFormat": "application/vnd.rete",
            "description": "The metadata files (one record per DOI, newest snapshot wins, "
                           "provenance in dcite:fromSnapshot) and both PID Links files as one RDF "
                           "graph in the rete format. Conversion rules R0-R18 and their counts are "
                           "in the file's Dataset Card and in datacite.tally.json. Totals: "
                           + ", ".join(f"{r} " + "/".join(f"{k}={v}" for k, v in sorted(rules.get(r, {}).items()))
                                       for r, _ in RULES if rules.get(r))}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--tally", help="the converter's JSON report (parquet_to_nt.py emit --report)")
    ap.add_argument("--card", default=CARD)
    ap.add_argument("--rete-sha256", help="sha256 of the built datacite.rete (Croissant FileObject)")
    ap.add_argument("--rete-bytes", type=int, help="its size in bytes")
    a = ap.parse_args()
    tally = json.load(open(a.tally, encoding="utf-8")) if a.tally else None
    with open(os.path.join(a.out,"schema.json"),"w",encoding="utf-8") as f:
        json.dump(json_schema(),f,indent=2,ensure_ascii=False)
    cr = croissant()
    if tally:
        cr["distribution"].append(rete_fileobject(tally, a.rete_sha256, a.rete_bytes))
    with open(os.path.join(a.out,"croissant.jsonld"),"w",encoding="utf-8") as f:
        json.dump(cr,f,indent=2,ensure_ascii=False)
    print("schema.json: 2 $defs (metadata 51 / pid_link 18); croissant:",
          len(RS_COLS),"FileSets +",len(RS_COLS),"RecordSets" + (" + datacite.rete" if tally else ""))
    if tally:
        os.makedirs(os.path.dirname(a.card), exist_ok=True)
        with open(a.card, "w", encoding="utf-8") as f:
            json.dump(rete_card(tally), f, indent=2, ensure_ascii=False)
            f.write("\n")
        print("card:", a.card)


if __name__ == "__main__":
    main()
