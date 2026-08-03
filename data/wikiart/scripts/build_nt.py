#!/usr/bin/env python3
"""Stream the whole WikiArt corpus as N-Triples on stdout, images embedded.

Every locally-mirrored WebP is emitted as an `xsd:base64Binary` literal, so the
resulting `.rete` is a single self-contained artifact: metadata AND pixels, no
external requests. Streamed (never buffers the corpus) so it can be piped
straight into `rete build -` without materialising a ~31 GB .nt file:

    python3 build_nt.py | rete build - -o data/wikiart/wikiart.rete --no-pyramid

Field coverage is driven by FIELDS.md (the census): every populated field of the
painting and artist records is modelled, plus the dictionaries and the facet
vocabularies. Env:

    WIKIART_LIMIT      stop after N paintings (0 = all)
    WIKIART_NO_IMAGES  emit metadata only (for a size baseline)
"""

import base64
import csv
import json
import os
import sys

RAW = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw"))
LIMIT = int(os.environ.get("WIKIART_LIMIT", "0"))
NO_IMAGES = os.environ.get("WIKIART_NO_IMAGES") == "1"

W = "https://w3id.org/rete/wikiart#"
PAGE = "https://www.wikiart.org/en/"
XSD = "http://www.w3.org/2001/XMLSchema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
DCT = "http://purl.org/dc/terms/"
SCHEMA = "https://schema.org/"
FOAF = "http://xmlns.com/foaf/0.1/"

out = sys.stdout
_esc = str.maketrans({'"': '\\"', "\\": "\\\\", "\n": "\\n", "\r": "\\r", "\t": "\\t"})


def lit(v):
    return '"' + str(v).translate(_esc) + '"'


def iri(u):
    # N-Triples forbids these raw inside <>; WikiArt slugs are otherwise safe
    return "<" + str(u).replace(" ", "%20").replace('"', "%22")\
        .replace("<", "%3C").replace(">", "%3E").replace("\\", "%5C").replace("`", "%60") + ">"


def emit(s, p, o):
    out.write(f"{s} {p} {o} .\n")


def T(s, p, v):                      # plain string literal
    if v not in (None, ""):
        emit(s, iri(W + p), lit(v))


def I(s, p, v):                      # integer
    if v is not None:
        emit(s, iri(W + p), f'{lit(v)}^^<{XSD}integer>')


def D(s, p, v):                      # decimal
    if v is not None:
        emit(s, iri(W + p), f'{lit(v)}^^<{XSD}decimal>')


def aspnet_date(v):
    """`/Date(ms)/` -> xsd:dateTime. 253402300799999 is the 'still alive' sentinel."""
    if not isinstance(v, str) or not v.startswith("/Date("):
        return None
    try:
        ms = int(v[6:v.index(")")])
    except Exception:
        return None
    if ms >= 253402300799999:
        return None
    import datetime
    try:
        return datetime.datetime.fromtimestamp(ms / 1000, datetime.timezone.utc)\
            .strftime("%Y-%m-%dT%H:%M:%SZ")
    except Exception:
        return None


def webp_path(cid):
    return os.path.join(RAW, "assets", "webp", f"{int(cid) & 0xFF:02x}", f"{cid}.webp")


# ---------------------------------------------------------------- vocabularies
def emit_dictionaries():
    d = os.path.join(RAW, "dictionaries")
    n = 0
    if not os.path.isdir(d):
        return 0
    for fn in sorted(os.listdir(d)):
        if not fn.startswith("group-"):
            continue
        facet = fn[:-5].split("-", 2)[2]
        scheme = iri(W + "scheme/" + facet)
        emit(scheme, iri(RDF + "type"), iri(SKOS + "ConceptScheme"))
        emit(scheme, iri(RDFS + "label"), lit(facet))
        for r in json.load(open(os.path.join(d, fn), encoding="utf-8")):
            c = iri(W + "concept/" + str(r.get("id")))
            emit(c, iri(RDF + "type"), iri(SKOS + "Concept"))
            emit(c, iri(SKOS + "inScheme"), scheme)
            if r.get("title"):
                emit(c, iri(SKOS + "prefLabel"), lit(r["title"]))
            T(c, "slug", r.get("url"))
            I(c, "dictionaryGroup", r.get("group"))
            n += 1
    return n


def emit_categories():
    d = os.path.join(RAW, "categories")
    n = 0
    if not os.path.isdir(d):
        return 0
    for fn in sorted(os.listdir(d)):
        if not fn.endswith(".json"):
            continue
        doc = json.load(open(os.path.join(d, fn), encoding="utf-8"))
        facet = doc.get("facet") or fn[:-5]
        for e in doc.get("entries") or []:
            if not e.get("Seo"):
                continue
            c = iri(W + "facet/" + facet + "/" + str(e["Seo"]))
            emit(c, iri(RDF + "type"), iri(SKOS + "Concept"))
            emit(c, iri(SKOS + "inScheme"), iri(W + "scheme/" + facet))
            if e.get("Title"):
                emit(c, iri(SKOS + "prefLabel"), lit(e["Title"]))
            T(c, "slug", e.get("Seo"))
            I(c, "artistCount", e.get("Count"))
            I(c, "dictionaryGroup", e.get("Group"))
            n += 1
        # multilingual group headings
        for cat in doc.get("categories") or []:
            oid = ((cat.get("_id") or {}).get("_oid"))
            if not oid:
                continue
            cc = iri(W + "category/" + oid)
            emit(cc, iri(RDF + "type"), iri(SKOS + "Collection"))
            titles = (((cat.get("Content") or {}).get("Title") or {}).get("Title")) or {}
            for lang, val in titles.items():
                if val:
                    out.write(f'{cc} <{SKOS}prefLabel> "{str(val).translate(_esc)}"@{lang} .\n')
            n += 1
    return n


# --------------------------------------------------------------------- artists
def emit_artists():
    rich = {}
    p = os.path.join(RAW, "artists.jsonl")
    if os.path.exists(p):
        for line in open(p, encoding="utf-8"):
            try:
                r = json.loads(line)
            except Exception:
                continue
            if r.get("url"):
                rich[r["url"]] = r

    n = 0
    ap = os.path.join(RAW, "artists_alphabet.json")
    for a in json.load(open(ap, encoding="utf-8")):
        slug = a.get("url")
        if not slug:
            continue
        s = iri(PAGE + slug)
        emit(s, iri(RDF + "type"), iri(W + "Artist"))
        emit(s, iri(RDF + "type"), iri(FOAF + "Person"))
        T(s, "slug", slug)
        if a.get("artistName"):
            emit(s, iri(FOAF + "name"), lit(a["artistName"]))
            emit(s, iri(RDFS + "label"), lit(a["artistName"]))
        T(s, "sortName", a.get("lastNameFirst"))
        I(s, "contentId", a.get("contentId"))
        for fld, prop in (("birthDay", "birthDate"), ("deathDay", "deathDate")):
            dt = aspnet_date(a.get(fld))
            if dt:
                emit(s, iri(W + prop), f'{lit(dt)}^^<{XSD}dateTime>')
        T(s, "birthDateText", a.get("birthDayAsString"))
        T(s, "deathDateText", a.get("deathDayAsString"))
        if a.get("image"):
            emit(s, iri(W + "portraitUrl"), iri(a["image"]))
        if a.get("wikipediaUrl"):
            emit(s, iri(RDFS + "seeAlso"), iri(a["wikipediaUrl"]))
        for did in a.get("dictonaries") or []:      # sic: misspelled upstream
            emit(s, iri(W + "dictionaryRef"), lit(did))

        r = rich.get(slug)
        if r:
            T(s, "mongoId", r.get("id"))
            T(s, "gender", r.get("gender"))
            T(s, "originalName", r.get("originalArtistName"))
            if r.get("biography"):
                emit(s, iri(W + "biography"), lit(r["biography"]))
            T(s, "activeYearsStart", r.get("activeYearsStart"))
            T(s, "activeYearsEnd", r.get("activeYearsCompletion"))
            for c in r.get("dictionaries") or []:
                emit(s, iri(W + "concept"), iri(W + "concept/" + str(c)))
            for rel in r.get("relatedArtists") or []:
                u = rel.get("url") if isinstance(rel, dict) else rel
                if u:
                    emit(s, iri(W + "relatedArtist"), iri(PAGE + str(u)))
            for per in r.get("periods") or []:
                t = per.get("title") if isinstance(per, dict) else per
                T(s, "period", t)
            for ser in r.get("series") or []:
                t = ser.get("title") if isinstance(ser, dict) else ser
                T(s, "series", t)
        n += 1
    return n


# -------------------------------------------------------------------- artworks
def artist_names_by_slug():
    """Real artist names keyed by their URL slug, for repairing broken records.

    21,641 of the 223,094 painting records (9.7%) carry `artistName` "￿" —
    U+FFFF, a permanent non-character. It is not an encoding accident on our
    side: it is what WikiArt's own image JSON serves for those works. Left
    alone it makes the single most prolific "artist" in the graph a piece of
    garbage, ahead of van Gogh.

    `artistUrl` survives intact on every one of them, and artists.jsonl carries
    the real name against that slug, so 96.4% (20,859) are recoverable by join.
    The 13 slugs with no artist entry are not artists at all —
    `ancient-greek-pottery`, `ancient-greek-painting` and the like — so those
    works keep no artistNameText rather than gaining a fabricated one.
    """
    by_slug = {}
    path = os.path.join(RAW, "artists.jsonl")
    if os.path.exists(path):
        for line in open(path, encoding="utf-8"):
            try:
                record = json.loads(line)
            except Exception:
                continue
            slug, name = record.get("url"), record.get("artistName")
            if slug and name and "￿" not in name:
                by_slug[slug] = name
    return by_slug


def emit_paintings():
    n = imgs = 0
    repaired = dropped = 0
    artist_name = artist_names_by_slug()
    src = os.path.join(RAW, "paintings_imagejson.jsonl")
    for line in open(src, encoding="utf-8"):
        try:
            p = json.loads(line)
        except Exception:
            continue
        au, u = p.get("artistUrl"), p.get("url")
        if not au or not u:
            continue
        s = iri(PAGE + au + "/" + u)
        emit(s, iri(RDF + "type"), iri(W + "Artwork"))
        emit(s, iri(W + "artist"), iri(PAGE + au))
        if p.get("title"):
            emit(s, iri(DCT + "title"), lit(p["title"]))
            emit(s, iri(RDFS + "label"), lit(p["title"]))
        T(s, "slug", u)
        I(s, "contentId", p.get("contentId"))
        I(s, "artistContentId", p.get("artistContentId"))
        # Repair the U+FFFF artist names via the intact artistUrl slug; emit
        # nothing when even that cannot be resolved, so a query never sees a
        # non-character where a name belongs.
        raw_name = p.get("artistName") or ""
        if "￿" in raw_name or not raw_name.strip():
            resolved = artist_name.get(au)
            if resolved:
                repaired += 1
            else:
                dropped += 1
            raw_name = resolved
        T(s, "artistNameText", raw_name)
        I(s, "completionYear", p.get("completitionYear"))     # sic upstream
        T(s, "yearText", p.get("yearAsString"))
        T(s, "style", p.get("style"))
        T(s, "genre", p.get("genre"))
        T(s, "material", p.get("material"))
        T(s, "technique", p.get("technique"))
        T(s, "period", p.get("period"))
        T(s, "series", p.get("serie"))
        T(s, "location", p.get("location"))
        T(s, "gallery", p.get("galleryName"))
        T(s, "auction", p.get("auction"))
        T(s, "yearOfTrade", p.get("yearOfTrade"))
        T(s, "lastPrice", p.get("lastPrice"))
        if p.get("description"):
            emit(s, iri(DCT + "description"), lit(p["description"]))
        D(s, "widthCm", p.get("sizeX"))
        D(s, "heightCm", p.get("sizeY"))
        D(s, "diameterCm", p.get("diameter"))
        I(s, "pixelWidth", p.get("width"))
        I(s, "pixelHeight", p.get("height"))
        if p.get("image"):
            emit(s, iri(SCHEMA + "image"), iri(p["image"]))
        for tag in str(p.get("tags") or "").split(","):
            tag = tag.strip()
            if tag:
                emit(s, iri(W + "tag"), lit(tag))
        for did in p.get("dictionaries") or []:
            emit(s, iri(W + "concept"), iri(W + "concept/" + str(did)))

        if not NO_IMAGES:
            fp = webp_path(p["contentId"])
            try:
                with open(fp, "rb") as fh:
                    blob = fh.read()
            except OSError:
                blob = None
            if blob:
                enc = base64.b64encode(blob).decode("ascii")
                emit(s, iri(W + "imageData"),
                     f'"{enc}"^^<{XSD}base64Binary>')
                emit(s, iri(W + "imageFormat"), lit("image/webp"))
                imgs += 1
        n += 1
        if n % 5000 == 0:
            sys.stderr.write(f"\r  paintings {n:,}  images {imgs:,}")
            sys.stderr.flush()
        if LIMIT and n >= LIMIT:
            break
    sys.stderr.write("\n")
    # Report the repair rather than leaving it silent: a number that shifts
    # between harvests is worth noticing.
    if repaired or dropped:
        sys.stderr.write(
            f"  artistName: repaired {repaired:,} U+FFFF record(s) via artistUrl, "
            f"{dropped:,} unresolvable (non-artist slugs) left without a name\n"
        )
    return n, imgs


def main():
    nd = emit_dictionaries()
    sys.stderr.write(f"  dictionaries/concepts: {nd:,}\n")
    nc = emit_categories()
    sys.stderr.write(f"  facet concepts:        {nc:,}\n")
    na = emit_artists()
    sys.stderr.write(f"  artists:               {na:,}\n")
    np_, ni = emit_paintings()
    sys.stderr.write(f"  artworks:              {np_:,}  (with embedded image: {ni:,})\n")


if __name__ == "__main__":
    main()
