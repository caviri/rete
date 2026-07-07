"""MARCXML parsing + normalization to the unified BCUL digital-twin record.

Namespace-agnostic (matches on local tag names) so it handles both:
  - OAI-PMH marcxml (Patrinum / TIND)  -> iter_oai_records()
  - Alma SRU marcxml (Renouvaud)       -> iter_marc_records()

Normalization maps a MARC record to the schema in
data/bcul/schema/bcul.record.schema.json.
"""
from __future__ import annotations

import re
import xml.etree.ElementTree as ET

# ---------------------------------------------------------------- low-level parse


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def parse_record(el) -> dict | None:
    """Parse a MARC <record> element -> {leader, ctrl:{tag:val}, fields:[...]}. """
    leader = None
    ctrl: dict[str, str] = {}
    fields: list[dict] = []
    for c in el:
        lt = _local(c.tag)
        if lt == "leader":
            leader = (c.text or "").strip()
        elif lt == "controlfield":
            ctrl[c.get("tag")] = c.text or ""
        elif lt == "datafield":
            subs = [(s.get("code"), (s.text or "")) for s in c if _local(s.tag) == "subfield"]
            fields.append({
                "tag": c.get("tag"),
                "ind1": c.get("ind1", " "),
                "ind2": c.get("ind2", " "),
                "subs": subs,
            })
    if leader is None and not ctrl and not fields:
        return None
    return {"leader": leader, "ctrl": ctrl, "fields": fields}


def iter_marc_records(xml_bytes: bytes):
    """Yield parsed MARC records found anywhere in the document (SRU responses)."""
    root = ET.fromstring(xml_bytes)
    for el in root.iter():
        if _local(el.tag) == "record":
            rec = parse_record(el)
            if rec is not None:  # skips OAI wrappers (no leader/ctrl/fields)
                yield rec


def iter_oai_records(xml_bytes: bytes):
    """Yield (header, marc_rec|None) for each OAI <record> wrapper.

    header = {"identifier":.., "datestamp":.., "sets":[..], "deleted":bool}
    """
    root = ET.fromstring(xml_bytes)
    for wrapper in root.iter():
        if _local(wrapper.tag) != "record":
            continue
        header = None
        marc = None
        for child in wrapper:
            lt = _local(child.tag)
            if lt == "header":
                header = {
                    "identifier": None, "datestamp": None, "sets": [],
                    "deleted": (child.get("status") == "deleted"),
                }
                for h in child:
                    ht = _local(h.tag)
                    if ht == "identifier":
                        header["identifier"] = (h.text or "").strip()
                    elif ht == "datestamp":
                        header["datestamp"] = (h.text or "").strip()
                    elif ht == "setSpec":
                        header["sets"].append((h.text or "").strip())
            elif lt == "metadata":
                for m in child.iter():
                    if _local(m.tag) == "record":
                        marc = parse_record(m)
                        if marc is not None:
                            break
        if header is not None:
            yield header, marc


def get_resumption_token(xml_bytes: bytes) -> str | None:
    m = re.search(rb"<resumptionToken[^>]*>([^<]*)</resumptionToken>", xml_bytes)
    if not m:
        return None
    tok = m.group(1).decode("utf-8", "replace").strip()
    return tok or None


# ---------------------------------------------------------------- accessors


def fields(rec, tag):
    return [f for f in rec["fields"] if f["tag"] == tag]


def sub_vals(field, codes):
    return [v for (c, v) in field["subs"] if c in codes and v]


def first_val(rec, tag, codes):
    for f in rec["fields"]:
        if f["tag"] == tag:
            for c, v in f["subs"]:
                if c in codes and v:
                    return v
    return None


def all_vals(rec, tags, codes):
    out = []
    tagset = {tags} if isinstance(tags, str) else set(tags)
    for f in rec["fields"]:
        if f["tag"] in tagset:
            for c, v in f["subs"]:
                if c in codes and v:
                    out.append(v)
    return out


def joined(field, codes, sep=" "):
    return sep.join(sub_vals(field, codes)).strip()


# ---------------------------------------------------------------- normalization

# MARC leader/06 -> coarse resource type
_LEADER_TYPE = {
    "a": "text", "c": "notated-music", "d": "manuscript-music",
    "e": "cartographic", "f": "manuscript-cartographic", "g": "moving-image",
    "i": "sound-nonmusic", "j": "sound-music", "k": "still-image",
    "m": "electronic", "o": "kit", "p": "mixed-material", "r": "object",
    "t": "manuscript-text",
}
# leader/07 bibliographic level
_LEADER_LEVEL = {
    "a": "article", "b": "serial-part", "c": "collection", "d": "subunit",
    "i": "integrating", "m": "monograph", "s": "serial",
}

_YEAR_RE = re.compile(r"(1[0-9]{3}|20[0-9]{2})")


def _digit_year(s):
    if not s:
        return None
    s = s.replace("u", "0").replace("x", "0").replace("?", "0").replace(" ", "")
    if s.isdigit() and len(s) == 4 and s != "0000":
        return int(s)
    return None


def _resource_type(rec) -> str:
    leader = rec.get("leader") or ""
    t6 = leader[6] if len(leader) > 6 else " "
    lvl = leader[7] if len(leader) > 7 else " "
    base = _LEADER_TYPE.get(t6, "text")
    level = _LEADER_LEVEL.get(lvl, "")
    if base == "text" and level == "serial":
        return "serial"
    if base == "text" and level in ("c", "collection"):
        return "collection"
    if base == "mixed-material":
        return "archive"
    if base == "text" and level == "collection":
        return "archive"
    return base


def _dates(rec):
    """Return (start, end, display)."""
    start = end = None
    disp = None
    f008 = rec["ctrl"].get("008")
    if f008 and len(f008) >= 15:
        start = _digit_year(f008[7:11])
        end = _digit_year(f008[11:15])
    # TIND custom 269 $a..$b holds ISO dates for archival material
    for f in fields(rec, "269"):
        a = sub_vals(f, ["a"])
        b = sub_vals(f, ["b"])
        if a and start is None:
            m = _YEAR_RE.search(a[0])
            if m:
                start = int(m.group(1))
        if b and end is None:
            m = _YEAR_RE.search(b[0])
            if m:
                end = int(m.group(1))
    # display from 264/260 $c
    c = first_val(rec, "264", ["c"]) or first_val(rec, "260", ["c"])
    if c:
        disp = c.strip()
        if start is None:
            m = _YEAR_RE.search(c)
            if m:
                start = int(m.group(1))
    return start, end, disp


def _creators(rec):
    out = []
    for tag, main in (("100", True), ("110", True), ("111", True),
                      ("700", False), ("710", False), ("711", False)):
        for f in fields(rec, tag):
            name = joined(f, ["a", "b", "c", "d", "q"], sep=" ").strip(" ,.")
            if not name:
                continue
            role = None
            roles = sub_vals(f, ["e"]) or sub_vals(f, ["4"])
            if roles:
                role = roles[0].strip(" ,.")
            out.append({"name": name, "role": role, "main": main})
    # dedupe by name keeping first
    seen, dedup = set(), []
    for c in out:
        if c["name"] not in seen:
            seen.add(c["name"])
            dedup.append(c)
    return dedup


_SUBJECT_TAGS = ["600", "610", "611", "630", "648", "650", "651", "653", "655", "656", "690"]


def _subjects(rec):
    subs, genres, places = [], [], []
    for f in rec["fields"]:
        if f["tag"] not in _SUBJECT_TAGS:
            continue
        parts = [v for (c, v) in f["subs"] if c in ("a", "b", "c", "d", "v", "x", "y", "z") and v]
        if not parts:
            continue
        label = " -- ".join(parts)
        if f["tag"] == "655":
            genres.append(label)
        elif f["tag"] == "651":
            places.append(parts[0])
            subs.append(label)
        else:
            subs.append(label)
    return _uniq(subs), _uniq(genres), _uniq(places)


def _uniq(seq):
    seen, out = set(), []
    for x in seq:
        if x and x not in seen:
            seen.add(x)
            out.append(x)
    return out


def _languages(rec):
    langs = []
    f008 = rec["ctrl"].get("008")
    if f008 and len(f008) >= 38:
        code = f008[35:38].strip()
        if code and code.isalpha():
            langs.append(code)
    langs += all_vals(rec, "041", ["a", "d"])
    return _uniq(langs)


def _holdings(rec):
    """Alma physical (AVA) + electronic (AVE) inventory — WHERE the item lives."""
    out = []
    for f in fields(rec, "AVA"):
        h = {
            "library": first_val_field(f, ["q"]),          # human name, e.g. "BCUL site Riponne"
            "library_code": first_val_field(f, ["b"]),      # e.g. "bcur"
            "location": first_val_field(f, ["c"]),          # shelving location
            "call_number": first_val_field(f, ["d"]),
            "availability": first_val_field(f, ["e"]),      # available / unavailable / check_holdings
            "institution": first_val_field(f, ["a"]),
            "kind": "physical",
        }
        if h["library"] or h["call_number"] or h["location"]:
            out.append(h)
    for f in fields(rec, "AVE"):
        h = {
            "library": first_val_field(f, ["m"]) or first_val_field(f, ["n"]),
            "location": "Online",
            "url": first_val_field(f, ["u"]),
            "availability": "online",
            "kind": "electronic",
        }
        if h["library"] or h["url"]:
            out.append(h)
    return out


def _shelfmark(rec):
    for tag, codes in (("852", ["j", "h", "c", "b"]), ("099", ["a"]),
                       ("090", ["a"]), ("084", ["a"]), ("950", ["a"])):
        v = first_val(rec, tag, codes)
        if v:
            return v.strip()
    return None


def _collections(rec, oai_sets=None):
    cols = []
    # TIND collection tag 980, and 037 $a for ISADG/archival typing
    cols += all_vals(rec, "980", ["a", "b"])
    cols += all_vals(rec, "710", ["5"])  # holding institution sometimes
    if oai_sets:
        cols += list(oai_sets)
    return _uniq(cols)


def _identifiers(rec):
    ids = {}
    if rec["ctrl"].get("001"):
        ids["marc001"] = rec["ctrl"]["001"].strip()
    isbn = all_vals(rec, "020", ["a"])
    if isbn:
        ids["isbn"] = _uniq([re.sub(r"\s.*$", "", x) for x in isbn])
    issn = all_vals(rec, "022", ["a"])
    if issn:
        ids["issn"] = _uniq(issn)
    doi = [v for v in all_vals(rec, "024", ["a"]) if "/" in v]
    if doi:
        ids["doi"] = _uniq(doi)
    for v in all_vals(rec, "035", ["a"]):
        vv = v.strip()
        if "(RERO)" in vv or vv.startswith("R00"):
            ids.setdefault("rero", [])
            ids["rero"].append(re.sub(r"^\(RERO\)", "", vv))
        elif "NETWORK)" in vv:
            m = re.search(r"\)(\d+)$", vv)
            if m:
                ids["mms_id"] = m.group(1)
    return ids


def _files(rec):
    out = []
    for f in fields(rec, "856"):
        u = first_val_field(f, ["u"])
        if not u:
            continue
        label = " ".join(sub_vals(f, ["y", "3", "z"])).strip() or None
        fmt = first_val_field(f, ["q"])
        out.append({"url": u.strip(), "label": label, "format": fmt})
    return out


def first_val_field(field, codes):
    for c, v in field["subs"]:
        if c in codes and v:
            return v
    return None


def title(rec):
    for f in fields(rec, "245"):
        t = joined(f, ["a", "b"], sep=" ").strip(" /:;,.")
        full = joined(f, ["a", "b", "c", "n", "p"], sep=" ").strip()
        return t or None, (full or None)
    # fallback: uniform title / archival 245 absent
    t = first_val(rec, "240", ["a"]) or first_val(rec, "246", ["a"])
    return (t, t) if t else (None, None)


def normalize(rec, source: str, *, oai_header=None, provider="Bibliothèque cantonale et universitaire - Lausanne"):
    """MARC record dict -> unified BCUL record dict."""
    ctrl = rec["ctrl"]
    local_id = (ctrl.get("001") or "").strip()
    if oai_header and oai_header.get("identifier") and not local_id:
        local_id = oai_header["identifier"]
    t, tfull = title(rec)
    start, end, disp = _dates(rec)
    subs, genres, places = _subjects(rec)
    oai_sets = oai_header.get("sets") if oai_header else None
    files = _files(rec)

    if source == "patrinum":
        record_url = f"https://patrinum.ch/record/{local_id}"
        # nanna needs ?redirect=1 to 302 to the actual JPEG (else it returns text/html)
        thumb = f"https://patrinum.ch/nanna/thumbnail/v2/{local_id}?redirect=1"
    elif source == "renouvaud":
        record_url = f"https://renouvaud1.primo.exlibrisgroup.com/permalink/41BCULAUSA_LIB/VU2/{local_id}"
        thumb = None
    else:
        record_url = None
        thumb = None

    rt = _resource_type(rec)
    holdings = _holdings(rec)
    libraries = _uniq([h["library"] for h in holdings if h.get("library")])
    shelf = _shelfmark(rec)
    if not shelf and holdings:
        shelf = next((h["call_number"] for h in holdings if h.get("call_number")), None)
    out = {
        "id": f"{source}:{local_id}",
        "source": source,
        "local_id": local_id,
        "record_url": record_url,
        "type": rt,
        "title": t,
        "title_full": tfull if tfull and tfull != t else None,
        "creators": _creators(rec),
        "publication": {
            "place": first_val(rec, "264", ["a"]) or first_val(rec, "260", ["a"]),
            "publisher": first_val(rec, "264", ["b"]) or first_val(rec, "260", ["b"]),
            "date": disp,
        },
        "date_start": start,
        "date_end": end,
        "languages": _languages(rec),
        "subjects": subs,
        "genres": genres,
        "places": places,
        "shelfmark": shelf,
        "holdings": holdings,
        "libraries": libraries,
        "collections": _collections(rec, oai_sets),
        "extent": first_val(rec, "300", ["a"]),
        "description": first_val(rec, "520", ["a"]),
        "notes": _uniq(all_vals(rec, ["500", "545", "561"], ["a"]))[:8],
        "identifiers": _identifiers(rec),
        "files": files,
        "iiif_manifest": None,
        "thumbnail_url": thumb,
        "thumbnail_local": None,
        "has_digital": bool(files),
        "rights": first_val(rec, "540", ["a"]) or first_val(rec, "506", ["a"]),
        "provider": provider,
    }
    if oai_header:
        out["oai_datestamp"] = oai_header.get("datestamp")
    return out
