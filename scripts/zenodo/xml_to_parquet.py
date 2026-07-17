"""Stream the Zenodo `records-xml.tar.gz` exporter dump into Parquet — without
ever extracting the archive.

The dump is one `<recid>.xml` per Zenodo record, each an OAI-wrapped DataCite
**kernel-4.5 XML** `<resource>`. We parse every record into the SAME column
set as the DataCite Public Data File Parquet (`scripts/datacite/`), so the two
`UNION BY NAME` cleanly and join on `doi`. The only added column is
`record_id` (the Zenodo recid — every version of a deposit has its own).

Nested DataCite fields are emitted as JSON strings with the DataCite JSON key
names (`creators_json[].nameIdentifiers[].nameIdentifierScheme`,
`related_identifiers_json[].relationType`, …) so the DataCite README queries
work verbatim here. Columns that never appear in the XML export (usage metrics,
client_id, state, container, …) are simply absent — `UNION BY NAME` fills them
with NULL against the DataCite tables.

Members are tiny (~3 KB), so they are read sequentially from the compressed tar
and parsed in batches by a process pool (one batch -> one Arrow RecordBatch),
then written as rolling zstd Parquet files. Peak disk = the tar + the Parquet.

Resumable: a checkpoint is written each time an output file closes; re-running
the same command skips records already in closed Parquet files and overwrites
the partial last file.

Usage:
  python xml_to_parquet.py                         # full run (defaults below)
  python xml_to_parquet.py --max-records 5000 --out /tmp/ztest --fresh
"""

import argparse
import json
import os
import tarfile
import time
from collections import deque
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow as pa
import pyarrow.parquet as pq
from lxml import etree

DEFAULT_TAR = r"D:\pro\rete\data\zenodo\records-xml-2026-07-10.tar.gz"
DEFAULT_OUT = r"D:\pro\rete\data\zenodo\parquet-metadata"

SCHEMA = pa.schema(
    [
        ("doi", pa.string()),
        ("record_id", pa.string()),   # Zenodo recid (the only Zenodo-specific column)
        ("prefix", pa.string()),
        ("publisher", pa.string()),
        ("publication_year", pa.int32()),
        ("published", pa.string()),   # date[@dateType=Issued]
        ("updated", pa.string()),     # date[@dateType=Updated]
        ("resource_type_general", pa.string()),
        ("resource_type", pa.string()),
        ("title", pa.string()),
        ("language", pa.string()),
        ("version", pa.string()),
        ("schema_version", pa.string()),
        ("url", pa.string()),         # landing page (https://zenodo.org/records/<id>)
        ("types_json", pa.string()),
        ("creators_json", pa.string()),
        ("titles_json", pa.string()),
        ("subjects_json", pa.string()),
        ("contributors_json", pa.string()),
        ("dates_json", pa.string()),
        ("related_identifiers_json", pa.string()),
        ("descriptions_json", pa.string()),
        ("geo_locations_json", pa.string()),
        ("funding_references_json", pa.string()),
        ("rights_list_json", pa.string()),
        ("alternate_identifiers_json", pa.string()),
        ("sizes_json", pa.string()),
        ("formats_json", pa.string()),
        ("extra_json", pa.string()),
    ]
)
COLUMNS = [f.name for f in SCHEMA]


def _dumps(v):
    return orjson.dumps(v).decode() if v else None


def _find(el, tag):
    return el.find("{*}" + tag)


def _findall(el, tag):
    return el.findall("{*}" + tag)


def _txt(el):
    if el is None:
        return None
    t = el.text
    return t.strip() if isinstance(t, str) and t.strip() else (t if t else None)


def _name_ids(el):
    out = []
    for ni in _findall(el, "nameIdentifier"):
        out.append({
            "nameIdentifier": _txt(ni),
            "nameIdentifierScheme": ni.get("nameIdentifierScheme"),
            "schemeUri": ni.get("schemeURI"),
        })
    return out or None


def _affiliations(el):
    out = [_txt(a) for a in _findall(el, "affiliation")]
    out = [a for a in out if a]
    return out or None


def _agent(el):
    """Shared shape for creator / contributor."""
    name_el = _find(el, "creatorName")
    if name_el is None:
        name_el = _find(el, "contributorName")
    obj = {"name": _txt(name_el)}
    if name_el is not None and name_el.get("nameType"):
        obj["nameType"] = name_el.get("nameType")
    gn, fn = _txt(_find(el, "givenName")), _txt(_find(el, "familyName"))
    if gn:
        obj["givenName"] = gn
    if fn:
        obj["familyName"] = fn
    nids = _name_ids(el)
    if nids:
        obj["nameIdentifiers"] = nids
    affs = _affiliations(el)
    if affs:
        obj["affiliation"] = affs
    return obj


def parse_record(recid, data):
    """bytes of one OAI/DataCite XML file -> dict of column values."""
    root = etree.fromstring(data)
    schema_version = _txt(_find(root, "schemaVersion"))
    datacentre = _txt(_find(root, "datacentreSymbol"))
    resource = root.find(".//{http://datacite.org/schema/kernel-4}resource")
    if resource is None:
        resource = root.find(".//{*}resource")
    if resource is None:
        raise ValueError("no <resource> element")

    row = {c: None for c in COLUMNS}
    row["record_id"] = recid
    row["schema_version"] = schema_version
    row["url"] = f"https://zenodo.org/records/{recid}"

    # doi (primary identifier)
    doi = None
    for idn in _findall(resource, "identifier"):
        if (idn.get("identifierType") or "").upper() == "DOI":
            doi = _txt(idn)
            break
    row["doi"] = doi
    if doi and "/" in doi:
        row["prefix"] = doi.split("/", 1)[0]

    row["publisher"] = _txt(_find(resource, "publisher"))
    row["language"] = _txt(_find(resource, "language"))
    row["version"] = _txt(_find(resource, "version"))

    py = _txt(_find(resource, "publicationYear"))
    if py:
        try:
            row["publication_year"] = int(py)
        except ValueError:
            pass

    # resourceType
    rt = _find(resource, "resourceType")
    if rt is not None:
        row["resource_type_general"] = rt.get("resourceTypeGeneral")
        row["resource_type"] = _txt(rt)
        row["types_json"] = _dumps({
            "resourceTypeGeneral": rt.get("resourceTypeGeneral"),
            "resourceType": _txt(rt),
        })

    # titles
    titles = []
    tw = _find(resource, "titles")
    if tw is not None:
        for t in _findall(tw, "title"):
            o = {"title": _txt(t)}
            if t.get("titleType"):
                o["titleType"] = t.get("titleType")
            titles.append(o)
    if titles:
        row["title"] = titles[0].get("title")
        row["titles_json"] = _dumps(titles)

    # creators
    cw = _find(resource, "creators")
    if cw is not None:
        creators = [_agent(c) for c in _findall(cw, "creator")]
        row["creators_json"] = _dumps(creators)

    # contributors
    conw = _find(resource, "contributors")
    if conw is not None:
        cons = []
        for c in _findall(conw, "contributor"):
            o = _agent(c)
            if c.get("contributorType"):
                o["contributorType"] = c.get("contributorType")
            cons.append(o)
        row["contributors_json"] = _dumps(cons)

    # subjects
    sw = _find(resource, "subjects")
    if sw is not None:
        subs = []
        for s in _findall(sw, "subject"):
            o = {"subject": _txt(s)}
            if s.get("subjectScheme"):
                o["subjectScheme"] = s.get("subjectScheme")
            if s.get("valueURI"):
                o["valueUri"] = s.get("valueURI")
            if s.get("schemeURI"):
                o["schemeUri"] = s.get("schemeURI")
            subs.append(o)
        row["subjects_json"] = _dumps(subs)

    # dates (+ published/updated convenience)
    dw = _find(resource, "dates")
    if dw is not None:
        dates = []
        for d in _findall(dw, "date"):
            dt = d.get("dateType")
            val = _txt(d)
            o = {"date": val, "dateType": dt}
            if d.get("dateInformation"):
                o["dateInformation"] = d.get("dateInformation")
            dates.append(o)
            if dt == "Issued" and not row["published"]:
                row["published"] = val
            elif dt == "Updated" and not row["updated"]:
                row["updated"] = val
        row["dates_json"] = _dumps(dates)

    # relatedIdentifiers
    rw = _find(resource, "relatedIdentifiers")
    if rw is not None:
        rels = []
        _RI_ATTRS = {"resourceTypeGeneral": "resourceTypeGeneral",
                     "relatedMetadataScheme": "relatedMetadataScheme",
                     "schemeURI": "schemeUri", "schemeType": "schemeType"}
        for r in _findall(rw, "relatedIdentifier"):
            o = {
                "relatedIdentifier": _txt(r),
                "relatedIdentifierType": r.get("relatedIdentifierType"),
                "relationType": r.get("relationType"),
            }
            for attr, key in _RI_ATTRS.items():
                if r.get(attr):
                    o[key] = r.get(attr)
            rels.append(o)
        row["related_identifiers_json"] = _dumps(rels)

    # descriptions
    dsw = _find(resource, "descriptions")
    if dsw is not None:
        descs = []
        for d in _findall(dsw, "description"):
            o = {"description": _txt(d)}
            if d.get("descriptionType"):
                o["descriptionType"] = d.get("descriptionType")
            descs.append(o)
        row["descriptions_json"] = _dumps(descs)

    # rightsList
    rlw = _find(resource, "rightsList")
    if rlw is not None:
        rights = []
        for r in _findall(rlw, "rights"):
            o = {"rights": _txt(r)}
            if r.get("rightsURI"):
                o["rightsUri"] = r.get("rightsURI")
            if r.get("rightsIdentifier"):
                o["rightsIdentifier"] = r.get("rightsIdentifier")
            if r.get("rightsIdentifierScheme"):
                o["rightsIdentifierScheme"] = r.get("rightsIdentifierScheme")
            if r.get("schemeURI"):
                o["schemeUri"] = r.get("schemeURI")
            rights.append(o)
        row["rights_list_json"] = _dumps(rights)

    # alternateIdentifiers
    aw = _find(resource, "alternateIdentifiers")
    if aw is not None:
        alts = []
        for a in _findall(aw, "alternateIdentifier"):
            alts.append({
                "alternateIdentifier": _txt(a),
                "alternateIdentifierType": a.get("alternateIdentifierType"),
            })
        row["alternate_identifiers_json"] = _dumps(alts)

    # geoLocations / fundingReferences (kept whole, generically)
    glw = _find(resource, "geoLocations")
    if glw is not None:
        row["geo_locations_json"] = _dumps([_elem_obj(g) for g in _findall(glw, "geoLocation")])
    fw = _find(resource, "fundingReferences")
    if fw is not None:
        row["funding_references_json"] = _dumps([_elem_obj(f) for f in _findall(fw, "fundingReference")])

    # sizes / formats
    szw = _find(resource, "sizes")
    if szw is not None:
        sizes = [_txt(s) for s in _findall(szw, "size") if _txt(s)]
        row["sizes_json"] = _dumps(sizes)
    fmw = _find(resource, "formats")
    if fmw is not None:
        fmts = [_txt(f) for f in _findall(fmw, "format") if _txt(f)]
        row["formats_json"] = _dumps(fmts)

    extra = {}
    if datacentre:
        extra["datacentreSymbol"] = datacentre
    if extra:
        row["extra_json"] = _dumps(extra)
    return row


def _elem_obj(el):
    """Generic {localname: text|[children]} for leftover nested blocks."""
    children = list(el)
    if not children:
        return _txt(el)
    out = {}
    for c in children:
        key = etree.QName(c).localname
        val = _elem_obj(c)
        if key in out:
            if not isinstance(out[key], list):
                out[key] = [out[key]]
            out[key].append(val)
        else:
            out[key] = val
    return out


def parse_batch(items):
    """Worker: list[(recid, bytes)] -> (RecordBatch, n_members, n_bad, first_err)."""
    cols = {c: [] for c in COLUMNS}
    n_bad = 0
    first_err = None
    for recid, data in items:
        try:
            row = parse_record(recid, data)
            for c in COLUMNS:
                cols[c].append(row[c])
        except Exception as e:  # noqa: BLE001 - count and move on
            n_bad += 1
            if first_err is None:
                first_err = f"{recid}: {e!r}"
    batch = pa.RecordBatch.from_pydict(cols, schema=SCHEMA)
    return batch, len(items), n_bad, first_err


class RollingWriter:
    def __init__(self, out_dir, rows_per_file, chunk_rows, checkpoint):
        self.out_dir = out_dir
        self.rows_per_file = rows_per_file
        self.chunk_rows = chunk_rows
        self.cp = checkpoint
        self.writer = None
        self.file_index = checkpoint["files"]
        self.pending = []
        self.pending_rows = 0
        self.file_rows = 0
        self.members_in_file = 0

    def _flush_chunk(self):
        if not self.pending:
            return
        if self.writer is None:
            path = os.path.join(self.out_dir, f"part-{self.file_index:05d}.parquet")
            self.writer = pq.ParquetWriter(path, SCHEMA, compression="zstd", compression_level=3)
        table = pa.Table.from_batches(self.pending, schema=SCHEMA)
        self.writer.write_table(table, row_group_size=self.chunk_rows)
        self.file_rows += table.num_rows
        self.pending = []
        self.pending_rows = 0

    def _close_file(self):
        self._flush_chunk()
        if self.writer is not None:
            self.writer.close()
            self.writer = None
            self.file_index += 1
            self.cp["files"] = self.file_index
            self.cp["members_done"] += self.members_in_file
            self.cp["rows"] += self.file_rows
            save_checkpoint(self.out_dir, self.cp)
        self.file_rows = 0
        self.members_in_file = 0

    def add_batch(self, batch, n_members):
        self.pending.append(batch)
        self.pending_rows += batch.num_rows
        self.members_in_file += n_members
        if self.pending_rows >= self.chunk_rows:
            self._flush_chunk()
        if self.file_rows >= self.rows_per_file:
            self._close_file()

    def finalize(self):
        self._close_file()


def checkpoint_path(out_dir):
    return os.path.join(out_dir, "_checkpoint.json")


def save_checkpoint(out_dir, cp):
    tmp = checkpoint_path(out_dir) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(cp, f)
    os.replace(tmp, checkpoint_path(out_dir))


def load_checkpoint(out_dir, fresh):
    if not fresh and os.path.exists(checkpoint_path(out_dir)):
        with open(checkpoint_path(out_dir), encoding="utf-8") as f:
            return json.load(f)
    return {"members_done": 0, "files": 0, "rows": 0}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--tar", default=DEFAULT_TAR)
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--workers", type=int, default=min(20, max(4, os.cpu_count() - 4)))
    ap.add_argument("--batch-size", type=int, default=4000, help="records per pool task")
    ap.add_argument("--rows-per-file", type=int, default=500_000)
    ap.add_argument("--chunk-rows", type=int, default=131_072)
    ap.add_argument("--max-records", type=int, default=None, help="test mode")
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    cp = load_checkpoint(args.out, args.fresh)
    skip = cp["members_done"]
    if skip:
        print(f"resuming: skipping {skip:,} records "
              f"({cp['rows']:,} rows in {cp['files']} closed files)", flush=True)

    tar_size = os.path.getsize(args.tar)
    raw = open(args.tar, "rb")
    tf = tarfile.open(fileobj=raw, mode="r|gz")

    writer = RollingWriter(args.out, args.rows_per_file, args.chunk_rows, cp)
    total_bad = 0
    first_err = None
    n_seen = 0
    n_submitted = 0
    n_written = skip
    t0 = time.time()
    inflight = deque()
    max_inflight = args.workers * 2
    batch = []

    def drain_one():
        nonlocal total_bad, first_err, n_written
        b, n_members, n_bad, err = inflight.popleft().result()
        total_bad += n_bad
        if err and first_err is None:
            first_err = err
        writer.add_batch(b, n_members)
        n_written += n_members
        frac = raw.tell() / tar_size
        elapsed = time.time() - t0
        eta = elapsed / frac * (1 - frac) if frac > 0 else 0
        print(
            f"[{elapsed/60:6.1f} min] records {n_written:>9,}  "
            f"rows {cp['rows'] + writer.file_rows + writer.pending_rows:>10,}  "
            f"tar {frac*100:5.1f}%  eta {eta/60:5.1f} min  bad {total_bad}",
            flush=True,
        )

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for member in tf:
            if not (member.isfile() and member.name.endswith(".xml")):
                continue
            n_seen += 1
            if n_seen <= skip:
                continue
            if args.max_records is not None and n_submitted >= args.max_records:
                break
            recid = os.path.splitext(os.path.basename(member.name))[0]
            batch.append((recid, tf.extractfile(member).read()))
            n_submitted += 1
            if len(batch) >= args.batch_size:
                inflight.append(pool.submit(parse_batch, batch))
                batch = []
                if len(inflight) >= max_inflight:
                    drain_one()
        if batch:
            inflight.append(pool.submit(parse_batch, batch))
        while inflight:
            drain_one()

    writer.finalize()
    tf.close()
    raw.close()

    elapsed = time.time() - t0
    print(
        f"DONE in {elapsed/60:.1f} min: {cp['rows']:,} rows total, "
        f"{cp['files']} parquet files, {total_bad} bad records",
        flush=True,
    )
    if first_err:
        print(f"first bad record: {first_err}", flush=True)


if __name__ == "__main__":
    main()
