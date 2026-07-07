#!/usr/bin/env python3
"""Vesuvius Challenge scrolls metadata -> N-Triples for the rete playground.

Source: the open-data registry the scrollprize.org data browser itself consumes
(https://scrollprize.org/data_browser -> villa src/data/atlasDataAccess.json ->
https://vesuvius-challenge-open-data.s3.us-east-1.amazonaws.com/metadata.json),
CC BY-NC 4.0 (a few EduceLab volumes under the EduceLab Data License). The bucket
is public-read; photos.json is a cached ListObjectsV2 of each sample's photos/
prefix (see the harvest snippet in the repo history / rerun with --photos).

Graph shape (vocab https://scrollprize.org/vocab#):
  Sample (Scroll/Fragment)  <- vs:sample -  Scan (X-ray tomography session)
  Volume (OME-Zarr recon)   - vs:fromScan -> Scan, vs:alignsTo -> Volume
  Segment (traced surface)  - vs:fromVolume -> Volume, vs:coversVolume -> Volume
  DataAsset (file on S3)    - vs:assetOf -> Volume|Segment, vs:usedModel -> Model
  Model (ML: ink detection / surface prediction) - vs:compatibleWith -> Sample

Asset IRIs are their public HTTPS download URLs (the browser's s3->https rewrite
rules), so every asset in the playground is a working link; image assets get a
vs:thumbnail via the same Thumbor service the data browser uses.
"""
import json
import os
import re
import sys
import urllib.request

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(HERE, "data", "scrolls")
META = os.path.join(DATA, "metadata.json")
PHOTOS = os.path.join(DATA, "photos.json")
OUT = os.path.join(DATA, "scrolls.nt")

ID = "https://scrollprize.org/id/"
VS = "https://scrollprize.org/vocab#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
DCT = "http://purl.org/dc/terms/"
PROV = "http://www.w3.org/ns/prov#"
XSD = "http://www.w3.org/2001/XMLSchema#"
FOAF = "http://xmlns.com/foaf/0.1/"

OPEN_BUCKET = "https://vesuvius-challenge-open-data.s3.us-east-1.amazonaws.com/"
# the browser's s3:// -> https rewrite rules (villa atlasDataAccess.json)
REWRITES = {
    "s3://vesuvius-challenge-open-data": OPEN_BUCKET,
    "s3://vesuvius-challenge": "https://data.aws.ash2txt.org/samples/",
}
# Thumbor thumbnail services, keyed by the https prefix they serve
THUMBS = {
    OPEN_BUCKET: "https://thumbs.aws.ash2txt.org/",
    "https://data.aws.ash2txt.org/samples/": "https://thumbs-vc.aws.ash2txt.org/",
}
IMG_EXT = (".jpg", ".jpeg", ".png", ".tif", ".tiff", ".webp")


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", " ").replace("\r", " ").replace("\t", " ")).strip()


def lit(s):
    return '"' + esc(re.sub(r"\s+", " ", str(s))) + '"'


def num(v):
    return f'"{v}"^^<{XSD}double>'


def integer(v):
    return f'"{int(v)}"^^<{XSD}integer>'


def dt(s):
    s = s.strip()
    # some registry dates carry microseconds; xsd:dateTime accepts them as-is
    return f'"{esc(s)}"^^<{XSD}dateTime>'


def pretty_sample(sid):
    m = re.match(r"PHercParis(\d+)(?:Fr(\d+))?$", sid)
    if m:
        base = f"PHerc. Paris {m.group(1)}"
        return base + (f" fr. {m.group(2)}" if m.group(2) else "")
    m = re.match(r"PHercMAN(.+)$", sid)
    if m:
        return f"PHerc. MAN {m.group(1)}"
    m = re.match(r"PHerc(\d+)([A-Za-z0-9]*)$", sid)
    if m:
        return f"PHerc. {int(m.group(1))}{(' ' + m.group(2)) if m.group(2) else ''}"
    return sid


def scroll_number(legacy_url):
    if not legacy_url:
        return None
    m = re.search(r"/full-scrolls/Scroll(\d+)/", legacy_url)
    if m:
        return f"Scroll {m.group(1)}"
    m = re.search(r"/fragments/Frag(\d+)/", legacy_url)
    if m:
        return f"Fragment {m.group(1)}"
    return None


def asset_url(data_entry):
    """First origin's path resolved against its s3 access root -> public HTTPS."""
    for origin in data_entry.get("origins") or []:
        path = (origin.get("path") or "").lstrip("/")
        for root in origin.get("access_roots") or []:
            base = REWRITES.get((root.get("url") or "").rstrip("/"))
            if base and path:
                return base + path
    return None


def thumb(url, size=640):
    for prefix, service in THUMBS.items():
        if url.startswith(prefix):
            return (f"{service}unsafe/fit-in/{size}x{size}/"
                    f"filters:format(webp)/{url[len(prefix):]}")
    return None


class Emitter:
    def __init__(self, fh):
        self.fh = fh
        self.seen = set()
        self.n = 0

    def t(self, s, p, o):
        line = f"{s} <{p}> {o} ."
        if line not in self.seen:
            self.seen.add(line)
            self.fh.write(line + "\n")
            self.n += 1


def emit_tbox(e):
    classes = {
        "Sample": "A physical Herculaneum sample scanned by the Vesuvius Challenge",
        "Scroll": "A carbonized, still-rolled Herculaneum scroll",
        "Fragment": "A detached scroll fragment with exposed writing (ink ground truth)",
        "Scan": "An X-ray micro-CT scanning session at a synchrotron facility",
        "Volume": "A reconstructed 3D CT volume (OME-Zarr) of a sample",
        "Segment": "A traced papyrus surface extracted from a volume",
        "DataAsset": "A downloadable file/directory on the open-data bucket",
        "Model": "A machine-learning model (ink detection / surface prediction)",
        "Facility": "A synchrotron scanning facility",
    }
    for c, comment in classes.items():
        s = f"<{VS}{c}>"
        e.t(s, RDF + "type", f"<{RDFS}Class>")
        e.t(s, RDFS + "label", lit(c))
        e.t(s, RDFS + "comment", lit(comment))
    e.t(f"<{VS}Scroll>", RDFS + "subClassOf", f"<{VS}Sample>")
    e.t(f"<{VS}Fragment>", RDFS + "subClassOf", f"<{VS}Sample>")
    props = {
        "sample": "the physical sample this entity belongs to",
        "sampleType": "scroll or fragment",
        "scrollNumber": "canonical Vesuvius Challenge number (Scroll 1-5, Fragment 1-6)",
        "legacyDataUrl": "the sample's legacy volpkg URL on dl.ash2txt.org",
        "facility": "synchrotron facility where the scan was taken",
        "energyKeV": "X-ray beam energy (keV)",
        "pixelSizeUm": "voxel/pixel size (micrometers)",
        "detectorDistanceMm": "detector distance (mm)",
        "fromScan": "scan this volume was reconstructed from",
        "fromVolume": "volume this segment was traced in",
        "alignsTo": "a registration transform exists onto this volume",
        "coversVolume": "the segment's bounding box overlaps this volume",
        "process": "the process that created this entity",
        "dataFormat": "voxel data format (e.g. uint8)",
        "shape": "volume shape in voxels, z x y x x",
        "publicRelease": "whether the volume is publicly released",
        "widthPx": "flattened segment width in pixels",
        "heightPx": "flattened segment height in pixels",
        "bbox": "axis-aligned bounding box in volume coordinates, 'x0 y0 z0 - x1 y1 z1'",
        "assetOf": "the volume or segment this file belongs to",
        "assetType": "asset kind (ome-zarr, obj, tifxyz, ink-detection, ...)",
        "usedModel": "ML model that produced this asset",
        "targetVolume": "volume the asset's coordinates refer to",
        "thumbnail": "webp thumbnail via the data browser's Thumbor service",
        "photo": "photograph of the physical sample",
        "architecture": "model architecture (timesformer, nnunet, ...)",
        "task": "model task (ink-detection, surface-prediction)",
        "compatibleWith": "sample the model is known to work on",
        "modelIdentifier": "human-readable model identifier",
        "targetResolutionMinUm": "minimum working resolution (um)",
        "targetResolutionMaxUm": "maximum working resolution (um)",
    }
    for p, comment in props.items():
        s = f"<{VS}{p}>"
        e.t(s, RDF + "type", f"<{RDF}Property>")
        e.t(s, RDFS + "label", lit(re.sub(r"(?<=[a-z])(?=[A-Z])", " ", p).lower()))
        e.t(s, RDFS + "comment", lit(comment))


def emit_assets(e, parent_iri, parent_long, entries, sample_iri_fn, models):
    for da in entries or []:
        url = asset_url(da)
        if not url:
            continue
        a = f"<{url}>"
        atype = da.get("type") or "data"
        e.t(a, RDF + "type", f"<{VS}DataAsset>")
        e.t(a, RDFS + "label", lit(f"{atype} · {parent_long}"))
        e.t(a, VS + "assetType", lit(atype))
        e.t(a, VS + "assetOf", parent_iri)
        params = da.get("parameters") or {}
        if params.get("model_id") and params["model_id"] in models:
            e.t(a, VS + "usedModel", f"<{ID}model/{params['model_id']}>")
        if params.get("target_volume"):
            e.t(a, VS + "targetVolume", sample_iri_fn("volume", params["target_volume"]))
        ci = da.get("creation_info") or {}
        if ci.get("date"):
            e.t(a, DCT + "created", dt(ci["date"]))
        if url.lower().endswith(IMG_EXT):
            tu = thumb(url)
            if tu:
                e.t(a, VS + "thumbnail", f"<{tu}>")


def main():
    d = json.load(open(META, encoding="utf-8"))
    photos = json.load(open(PHOTOS, encoding="utf-8")) if os.path.exists(PHOTOS) else {}
    samples, models = d["samples"], d["models"]

    fh = open(OUT, "w", encoding="utf-8", newline="\n")
    e = Emitter(fh)
    emit_tbox(e)

    facilities = {}

    def facility_iri(name):
        if name not in facilities:
            slug = re.sub(r"[^A-Za-z0-9]+", "", name.split("(")[0].split()[0])
            iri = f"<{ID}facility/{slug}>"
            facilities[name] = iri
            e.t(iri, RDF + "type", f"<{VS}Facility>")
            e.t(iri, RDFS + "label", lit(name))
        return facilities[name]

    for mid, m in models.items():
        s = f"<{ID}model/{mid}>"
        p = m.get("properties") or {}
        e.t(s, RDF + "type", f"<{VS}Model>")
        e.t(s, RDFS + "label", lit(m.get("long_id") or mid))
        c = m.get("creation") or {}
        if c.get("date"):
            e.t(s, DCT + "created", dt(c["date"]))
        if p.get("architecture"):
            e.t(s, VS + "architecture", lit(p["architecture"]))
        if p.get("task"):
            e.t(s, VS + "task", lit(p["task"]))
        if p.get("model_identifier"):
            e.t(s, VS + "modelIdentifier", lit(p["model_identifier"]))
        for res_key, pred in (("target_resolution_um_min", "targetResolutionMinUm"),
                              ("target_resolution_um_max", "targetResolutionMaxUm")):
            if p.get(res_key) is not None:
                e.t(s, VS + pred, num(p[res_key]))
        for cs in p.get("compatible_samples") or []:
            e.t(s, VS + "compatibleWith", f"<{ID}sample/{cs}>")

    for sid, sample in samples.items():
        s_iri = f"<{ID}sample/{sid}>"

        def scoped(kind, eid, _sid=sid):
            return f"<{ID}sample/{_sid}/{kind}/{eid}>"

        props = (sample.get("sample") or {}).get("properties") or {}
        stype = props.get("type")
        cls = {"scroll": "Scroll", "fragment": "Fragment"}.get(stype, "Sample")
        e.t(s_iri, RDF + "type", f"<{VS}{cls}>")
        legacy = props.get("legacy_data_url")
        number = scroll_number(legacy)
        label = pretty_sample(sid)
        if number:
            label = f"{label} ({number})"
        e.t(s_iri, RDFS + "label", lit(label))
        e.t(s_iri, DCT + "identifier", lit(sid))
        if stype:
            e.t(s_iri, VS + "sampleType", lit(stype))
        if number:
            e.t(s_iri, VS + "scrollNumber", lit(number))
        if legacy:
            e.t(s_iri, VS + "legacyDataUrl", f"<{legacy}>")
        desc = (props.get("description") or "").strip()
        if desc and desc != label:
            e.t(s_iri, DCT + "description", lit(desc))
        for key in photos.get(sid, []):
            url = OPEN_BUCKET + key
            pred = "photo" if "_photo" in key else "thumbnail"
            if pred == "photo":
                e.t(s_iri, FOAF + "depiction", f"<{url}>")
                tu = thumb(url)
                if tu:
                    e.t(s_iri, VS + "thumbnail", f"<{tu}>")

        for scid, sc in (sample.get("scans") or {}).items():
            si = scoped("scan", scid)
            e.t(si, RDF + "type", f"<{VS}Scan>")
            c = sc.get("creation") or {}
            md = c.get("metadata") or {}
            e.t(si, RDFS + "label", lit(md.get("scan_name") or sc.get("long_id") or scid))
            e.t(si, VS + "sample", s_iri)
            if c.get("date"):
                e.t(si, DCT + "created", dt(c["date"]))
            if md.get("location"):
                e.t(si, VS + "facility", facility_iri(md["location"]))
            p = sc.get("properties") or {}
            if p.get("energy_keV") is not None:
                e.t(si, VS + "energyKeV", num(p["energy_keV"]))
            if p.get("pixel_size_um") is not None:
                e.t(si, VS + "pixelSizeUm", num(p["pixel_size_um"]))
            if p.get("detector_distance_mm") is not None:
                e.t(si, VS + "detectorDistanceMm", num(p["detector_distance_mm"]))

        for vid, v in (sample.get("volumes") or {}).items():
            vi = scoped("volume", vid)
            e.t(vi, RDF + "type", f"<{VS}Volume>")
            e.t(vi, RDFS + "label", lit(v.get("long_id") or vid))
            e.t(vi, VS + "sample", s_iri)
            c = v.get("creation") or {}
            if c.get("date"):
                e.t(vi, DCT + "created", dt(c["date"]))
            if c.get("process"):
                e.t(vi, VS + "process", lit(c["process"]))
            derived = c.get("derived_from") or {}
            if derived.get("type") == "scan" and derived.get("id"):
                e.t(vi, VS + "fromScan", scoped("scan", derived["id"]))
                e.t(vi, PROV + "wasDerivedFrom", scoped("scan", derived["id"]))
            p = v.get("properties") or {}
            if p.get("energy_keV") is not None:
                e.t(vi, VS + "energyKeV", num(p["energy_keV"]))
            if p.get("pixel_size_um") is not None:
                e.t(vi, VS + "pixelSizeUm", num(p["pixel_size_um"]))
            if p.get("data_format"):
                e.t(vi, VS + "dataFormat", lit(p["data_format"]))
            if p.get("shape"):
                e.t(vi, VS + "shape", lit("×".join(str(x) for x in p["shape"])))
            lic = p.get("license") or {}
            if lic.get("url"):
                e.t(vi, DCT + "license", f"<{lic['url']}>")
            pr = p.get("public_release")
            if pr is not None:
                e.t(vi, VS + "publicRelease",
                    f'"{str(pr).lower() if not isinstance(pr, str) else pr}"^^<{XSD}boolean>')
            for tr in p.get("transforms") or []:
                if tr.get("to_volume_id"):
                    e.t(vi, VS + "alignsTo", scoped("volume", tr["to_volume_id"]))
            emit_assets(e, vi, v.get("long_id") or vid, v.get("data"), scoped, models)

        # sample-level registration graph (from_volume -> to_volume)
        for vt in props.get("volume_transforms") or []:
            f = vt.get("from_volume_id")
            for tr in vt.get("transforms") or []:
                if f and tr.get("to_volume_id"):
                    e.t(scoped("volume", f), VS + "alignsTo",
                        scoped("volume", tr["to_volume_id"]))

        for gid, g in (sample.get("segments") or {}).items():
            gi = scoped("segment", gid)
            e.t(gi, RDF + "type", f"<{VS}Segment>")
            e.t(gi, RDFS + "label", lit(g.get("long_id") or gid))
            e.t(gi, VS + "sample", s_iri)
            if g.get("original_volume_id"):
                ov = scoped("volume", g["original_volume_id"])
                e.t(gi, VS + "fromVolume", ov)
                e.t(gi, PROV + "wasDerivedFrom", ov)
            c = g.get("creation") or {}
            if c.get("date"):
                e.t(gi, DCT + "created", dt(c["date"]))
            if c.get("process"):
                e.t(gi, VS + "process", lit(c["process"]))
            md = c.get("metadata") or {}
            if md.get("bbox"):
                (x0, y0, z0), (x1, y1, z1) = md["bbox"]
                e.t(gi, VS + "bbox",
                    lit(f"{x0:.0f} {y0:.0f} {z0:.0f} - {x1:.0f} {y1:.0f} {z1:.0f}"))
            p = g.get("properties") or {}
            if p.get("width") is not None:
                e.t(gi, VS + "widthPx", integer(p["width"]))
            if p.get("height") is not None:
                e.t(gi, VS + "heightPx", integer(p["height"]))
            for cvid, cov in (p.get("volume_coverage") or {}).items():
                if (cov or {}).get("overlap_ratio"):
                    e.t(gi, VS + "coversVolume", scoped("volume", cvid))
            emit_assets(e, gi, g.get("long_id") or gid, g.get("data"), scoped, models)

    fh.close()
    print(f"{e.n} triples -> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
