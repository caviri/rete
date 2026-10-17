"""DataCite Parquet (metadata + PID Links) -> N-Triples on stdout, faithfully.

Vocabulary `dcite:` = https://w3id.org/rete/datacite# (scripts/datacite/datacite.ttl).
Pipe the output into `rete build - --format nt --strict --memory-budget-mb ...`.

Two subcommands:

  plan   decide which metadata row stands for each DOI (R9/R10) and write the
         decision to a small Parquet file. DuckDB does the grouping.
  emit   stream N-Triples to stdout, apply every rule below, and write a JSON
         tally of what each rule rescued, encoded or dropped (--report).

Principle: the graph says what DataCite says. Literals are written as DataCite
has them (U+FFFD, mojibake, stray whitespace included: they are upstream, see
dev/datacite-parquet-audit/REPORT.md). An identifier is rewritten only when the
rewrite is deterministic and names the same thing (`https://doi.org/10.x/y` is
the DOI `10.x/y`). A value that cannot be turned into a valid IRI that way is
dropped, never guessed, and every drop is counted. Every IRI written is checked
by pyoxigraph's IRI parser (oxiri, the same parser `rete build --strict` and
Oxigraph use), so validity is decided by a parser, not by our own classifier.

Rules (numbers follow the audit report, section 7):

  R1  One DOI normaliser for related identifiers, funder identifiers and link
      endpoints: trim, lower-case, strip doi.org / dx.doi.org URL prefixes,
      `doi:`, `doi/`, `org/`, leading `%20` and `%7C%5D`, spaces inside the
      `10.NNNN` prefix; require `10.<4-9 digits>/<non-space>`.
  R2  Percent-encode what RFC 3987 forbids (`[ ]`, a second `#`, a `%` without
      two hex digits, U+FFFD, C1 controls, private-use, noncharacters), exactly
      as `rete export --sanitize-iris` does, so a valid IRI never moves.
  R3  Funder IRI: ROR (checksum must pass) > Crossref Funder ID (DOI, or bare
      digits typed `Crossref Funder ID`) > record-scoped name IRI
      `<doi>/funder/<sha1(NFC(name))[:16]>`. The name stays a literal.
  R4  Placeholder or empty funder names are not names.
  R5  A link endpoint with no scheme is rescued by R1 or dropped.
  R6  `hdl:` -> https://hdl.handle.net/. ORCID IRIs only when the ISO 7064
      mod 11-2 check digit passes; otherwise the value is kept as text.
  R7  Every DataCite 4.6 relationType has its own property, for metadata and
      links alike.
  R8  A related identifier with no DOI after R1 is dropped; a DOI embedded in
      other text is extracted only when exactly one stands there, delimited.
  R9  One record per DOI: 2025 wins, else 2024, else 2023. dcite:fromSnapshot
      says which.
  R10 Within one snapshot the row with the latest `updated` wins.
  R11 `occurredAt` sentinel 0000-01-01 is not emitted. A publicationYear
      outside 1000-2027 is kept as text (dcite:publicationYearText), not gYear.
  R12 landingPage is typed xsd:anyURI only when it parses as an IRI.
  R13 Literals are not repaired.
  R14 rorId only when the ROR checksum passes; otherwise kept as text.
"""
import argparse
import glob
import hashlib
import json
import multiprocessing as mp
import os
import random
import re
import sys
import unicodedata
from collections import Counter, defaultdict

try:
    import pyoxigraph as _ox
except ImportError:  # the tests can still exercise the pure-Python rules
    _ox = None

D = "https://w3id.org/rete/datacite#"
DOIB = "https://doi.org/"
ORCIDB = "https://orcid.org/"
RORB = "https://ror.org/"
HDLB = "https://hdl.handle.net/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
DCT_SUBJECT = "http://purl.org/dc/terms/subject"
XSD = "http://www.w3.org/2001/XMLSchema#"

META_YEARS = ("2025", "2024", "2023")  # precedence order (R9)
LINK_SETS = ("2023", "may2025")
YEAR_MIN, YEAR_MAX = 1000, 2027  # R11

# resourceTypeGeneral (DataCite 4.6, all 32 values) -> dcite class
TYPE_CLASS = {
    "audiovisual": "Audiovisual", "award": "Award", "book": "Book",
    "bookchapter": "BookChapter", "collection": "Collection",
    "computationalnotebook": "ComputationalNotebook", "conferencepaper": "ConferencePaper",
    "conferenceproceeding": "ConferenceProceeding", "datapaper": "DataPaper",
    "dataset": "Dataset", "dissertation": "Dissertation", "event": "Event",
    "image": "Image", "instrument": "Instrument", "interactiveresource": "InteractiveResource",
    "journal": "Journal", "journalarticle": "JournalArticle", "model": "Model",
    "outputmanagementplan": "OutputManagementPlan", "peerreview": "PeerReview",
    "physicalobject": "PhysicalObject", "preprint": "Preprint", "project": "Project",
    "report": "Report", "service": "Service", "software": "Software", "sound": "Sound",
    "standard": "Standard", "studyregistration": "StudyRegistration", "text": "Text",
    "workflow": "Workflow", "other": "OtherResource",
}

# DataCite 4.6 relationType vocabulary (38 values, kernel-4.6 datacite-relationType-v4.xsd)
DATACITE_46_RELATIONS = (
    "IsCitedBy Cites IsSupplementTo IsSupplementedBy IsContinuedBy Continues "
    "IsNewVersionOf IsPreviousVersionOf IsPartOf HasPart IsPublishedIn IsReferencedBy "
    "References IsDocumentedBy Documents IsCompiledBy Compiles IsVariantFormOf "
    "IsOriginalFormOf IsIdenticalTo HasMetadata IsMetadataFor Reviews IsReviewedBy "
    "IsDerivedFrom IsSourceOf Describes IsDescribedBy HasVersion IsVersionOf Requires "
    "IsRequiredBy Obsoletes IsObsoletedBy Collects IsCollectedBy HasTranslation "
    "IsTranslationOf").split()


def _camel(term):
    return term[0].lower() + term[1:]


def relkey(rt):
    """`IsCitedBy`, `is-cited-by` and `iscitedby` all key to `iscitedby`."""
    return re.sub(r"[^a-z]", "", str(rt or "").lower())


# relation key -> dcite property. Same table for metadata and PID links (R7).
REL_PROP = {relkey(t): _camel(t) for t in DATACITE_46_RELATIONS}
REL_PROP.update({
    # PID-Links relation types DataCite Event Data adds to the 4.6 vocabulary
    "isrelatedto": "isRelatedTo",
    "isauthoredby": "isAuthoredBy",      # work -> ORCID
    "isauthoredat": "isAuthoredAt",      # work -> ROR
    "isaffiliatedwith": "isAffiliatedWith",  # ORCID -> ROR
    "isfundedby": "fundedBy",            # work -> Crossref Funder ID, as fundingReference
})
FALLBACK_PROP = "isRelatedTo"


def rel_concept(rt):
    """`is-review-of` -> `rel-isReviewOf`; `IsCitedBy` -> `rel-isCitedBy`."""
    raw = str(rt or "").strip()
    parts = [p for p in re.split(r"[^A-Za-z0-9]+", raw) if p]
    if not parts:
        return None
    if len(parts) == 1:
        return "rel-" + _camel(parts[0])
    return "rel-" + parts[0].lower() + "".join(p[:1].upper() + p[1:].lower() for p in parts[1:])


# ------------------------------------------------------------------ IRI escaping (R2)
# Identical to scripts/openaire/parquet_to_nt.py iri_escape and to
# crates/rete-core/src/iri.rs `rete export --sanitize-iris`: a valid IRI comes out
# unchanged; `[ ]`, a second `#`, a stray `%`, controls and non-ucschar code points
# are percent-encoded byte by byte (the IRI -> URI mapping, meaning preserved).
_HEX = "0123456789abcdefABCDEF"
_SCHEME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.\-]*:")
_SAFE_RE = re.compile(
    r"[A-Za-z][A-Za-z0-9+.\-]*:(?:[A-Za-z0-9\-._~!$&'()*+,;=:@/?]|%[0-9A-Fa-f]{2})*"
    r"(?:#(?:[A-Za-z0-9\-._~!$&'()*+,;=:@/?]|%[0-9A-Fa-f]{2})*)?\Z")


def _ip_literal_brackets(b, colon):
    if b[colon + 1: colon + 3] != b"//":
        return None
    start = colon + 3
    if b[start: start + 1] != b"[":
        return None
    end = len(b)
    for p in range(start, len(b)):
        if b[p] in (0x2F, 0x3F, 0x23):
            end = p
            break
    close = b.find(b"]", start, end)
    return None if close < 0 else (start, close)


def _is_ucschar(cp):
    return (0xA0 <= cp <= 0xD7FF or 0xF900 <= cp <= 0xFDCF or 0xFDF0 <= cp <= 0xFFEF
            or (0x10000 <= cp <= 0xEFFFD and (cp & 0xFFFF) <= 0xFFFD))


def iri_escape(s):
    """Percent-encode every IRIREF / RFC 3987 defect escaping can repair."""
    if _SAFE_RE.match(s):
        return s
    b = s.encode("utf-8", "surrogatepass")
    m = _SCHEME_RE.match(s)
    brackets = _ip_literal_brackets(b, m.end() - 1) if m else None
    out = bytearray()
    i = 0
    seen_hash = False
    while i < len(b):
        c = b[i]
        if c >= 0x80:
            n = 2 if c < 0xE0 else 3 if c < 0xF0 else 4
            seq = b[i: i + n]
            try:
                cp = ord(seq.decode("utf-8", "surrogatepass"))
            except (UnicodeDecodeError, TypeError):
                cp = -1
            if _is_ucschar(cp):
                out += seq
            else:
                out += b"".join(b"%%%02X" % x for x in seq)
            i += len(seq)
            continue
        elif c <= 0x20 or c == 0x7F or c in b'<>"{}|^`\\':
            out += b"%%%02X" % c
        elif c in (0x5B, 0x5D) and not (brackets and i in brackets):
            out += b"%%%02X" % c
        elif c == 0x23:
            if seen_hash:
                out += b"%23"
            else:
                seen_hash = True
                out.append(c)
        elif c == 0x25:
            if i + 2 < len(b) and chr(b[i + 1]) in _HEX and chr(b[i + 2]) in _HEX:
                out += b[i: i + 3]
                i += 3
                continue
            out += b"%25"
        else:
            out.append(c)
        i += 1
    return out.decode("utf-8", "surrogatepass")


def iri_parses(s):
    """True when oxiri (via pyoxigraph) accepts `s` as an absolute IRI."""
    if _ox is None:
        raise RuntimeError("pyoxigraph is required to validate IRIs (pip install pyoxigraph)")
    try:
        _ox.NamedNode(s)
        return True
    except ValueError:
        return False


# ------------------------------------------------------------------ identifiers
_DOI_RE = re.compile(r"10\.\d{4,9}/\S+\Z")
_DOI_TOKEN = re.compile(r"10\.\d{4,9}/[^\s\"<>]+")
_DOI_URL_PREFIX = re.compile(r"(?:https?:/{1,3})?(?:dx\.|www\.)?doi\.org/+")
_DOI_SCHEME = re.compile(r"doi:\s*/*")
_ORCID_ANY = re.compile(r"(\d{4}-\d{4}-\d{4}-\d{3}[\dXx])")
_ORCID_FULL = re.compile(r"\d{4}-\d{4}-\d{4}-\d{3}[\dX]\Z")
_ROR_ID = re.compile(r"0[a-hj-km-np-tv-z0-9]{6}\d{2}\Z")
_ROR_URL = re.compile(r"(?:https?://)?(?:www\.)?ror\.org/([0-9a-z]{9})/?\Z")
_CROCK = "0123456789abcdefghjkmnpqrstvwxyz"
_SPACE_IN_PREFIX = re.compile(r"10(?:\s|%20)*\.(?:\s|%20)*(\d{4,9})(?:\s|%20)*/(?:\s|%20)*")
_EMBED_TAIL = re.compile(r"\Z|\s+(?:\Z|[^\w\s])")


def norm_doi(raw, embedded=False):
    """R1 (+ R8 with `embedded`). Returns (doi | None, how).

    `how` names the rescue (`clean` when nothing was needed) or, for None, the
    reason. Rescues only strip wrappers that are not part of any DOI.
    """
    if raw is None:
        return None, "empty"
    s = str(raw).strip()
    if not s:
        return None, "empty"
    s = s.lower()
    if _DOI_RE.match(s):
        return s, "clean" if s == str(raw) or s == str(raw).lower() else "trimmed"
    t, how = s, []
    for _ in range(4):  # wrappers can nest: `doi:https://doi.org/10...`
        before = t
        m = _DOI_URL_PREFIX.match(t)
        if m:
            t = t[m.end():]
            how.append("doi_url")
        m = _DOI_SCHEME.match(t)
        if m:
            t = t[m.end():]
            how.append("doi_scheme")
        for junk, name in (("%20", "pct20"), ("%7c%5d", "pct7c5d"), ("doi/", "doi_slash"),
                           ("org/", "org_slash")):
            if t.startswith(junk):
                t = t[len(junk):]
                how.append(name)
        t = t.strip()
        if t == before:
            break
    m = _SPACE_IN_PREFIX.match(t)
    if m and m.group(0) != f"10.{m.group(1)}/":
        t = f"10.{m.group(1)}/" + t[m.end():]
        how.append("space_in_prefix")
    if "doi_url" in how and "/" not in t and "%2f" in t:
        t = t.replace("%2f", "/", 1)  # doi.org URL path with an encoded slash
        how.append("pct2f")
    if _DOI_RE.match(t):
        return t, "+".join(dict.fromkeys(how)) or "clean"
    if embedded:
        toks = list(_DOI_TOKEN.finditer(s))
        if len(toks) == 1:
            st, en = toks[0].start(), toks[0].end()
            # delimited on both sides: nothing alphanumeric glued before it, and after
            # it either the end or a space followed by punctuation (`… (DOI)`, `… ,`).
            # `10.3969/j.issn.1000 1565.2017…` is a DOI broken by a space: not taken.
            if (st == 0 or not s[st - 1].isalnum()) and _EMBED_TAIL.match(s, en):
                return toks[0].group(0), "embedded"
            return None, "embedded_undelimited"
        if len(toks) > 1:
            return None, "several_dois"
    return None, "no_doi"


def mod11_2(base15):
    total = 0
    for ch in base15:
        total = (total + int(ch)) * 2
    r = (12 - total % 11) % 11
    return "X" if r == 10 else str(r)


def orcid_ok(oid):
    """`0000-0002-1825-0097` -> True. ISO 7064 mod 11-2 (R6)."""
    if not _ORCID_FULL.match(oid):
        return False
    d = oid.replace("-", "")
    return mod11_2(d[:15]) == d[15]


_ORCID_BARE16 = re.compile(r"(?<![\dA-Za-z])(\d{15}[\dXx])(?![\dA-Za-z])")


def find_orcid(val):
    """The ORCID iD in `val`, canonical (hyphens, upper-case X), or None.

    Returns (candidate, matched_text). `candidate` is not yet checked: the caller
    applies `orcid_ok`. A 16-digit run without hyphens is the same iD written
    without its separators, so it is re-hyphenated (and counted by the caller).
    """
    m = _ORCID_ANY.search(val)
    if m:
        return m.group(1).upper(), m.group(1)
    m = _ORCID_BARE16.search(val)
    if m:
        d = m.group(1).upper()
        return f"{d[0:4]}-{d[4:8]}-{d[8:12]}-{d[12:16]}", m.group(1)
    return None, None


def ror_ok(rid):
    """ROR id `0abcdef12`: Crockford base32 body, 2-digit ISO 7064 mod 97-10 checksum."""
    rid = rid.lower()
    if not _ROR_ID.match(rid):
        return False
    n = 0
    for ch in rid[1:7]:
        n = n * 32 + _CROCK.index(ch)
    return 98 - ((n * 100) % 97) == int(rid[7:9])


_ROR_IN_URL = re.compile(r"ror\.org/?(0[0-9a-z]{8})(?![0-9a-z])")


def ror_from(value, typed_ror=False):
    """ROR id from a ror.org URL, or from a bare id when the scheme says ROR.

    A ror.org URL with broken punctuation (`https://ror.org00012xz55/`,
    `https://https://ror.org/0…`, a trailing `/ROR`) still names exactly one id;
    callers mint it only when its check digits pass (`ror_ok`).
    """
    v = str(value or "").strip().lower()
    m = _ROR_URL.match(v)
    if m:
        return m.group(1)
    if typed_ror and re.fullmatch(r"[0-9a-z]{9}", v):
        return v
    hits = _ROR_IN_URL.findall(v)
    if len(hits) == 1:
        return hits[0]
    return None


# ------------------------------------------------------------------ literals (R13)
_LIT = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_LIT_RE = re.compile(r'[\\"\n\r\t]')
_C0 = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")


def lit(s):
    """N-Triples string body. The text is DataCite's; only N-Triples syntax is added.

    C0 controls other than tab/newline/CR are escaped as \\uXXXX (N-Triples UCHAR),
    so nothing is removed from the value.
    """
    s = _LIT_RE.sub(lambda m: _LIT[m.group()], str(s))
    if _C0.search(s):
        s = _C0.sub(lambda m: "\\u%04X" % ord(m.group()), s)
    return s


def jload(s):
    if not s:
        return None
    try:
        return json.loads(s)
    except ValueError:
        return None


_PLACEHOLDER = re.compile(r"#?placeholder[_ ]parent[_ ]metadata[_ ]value#?\Z", re.I)


def usable_name(name):
    """R4: None for an empty or placeholder funder name."""
    if name is None:
        return None
    s = str(name)
    if not s.strip() or _PLACEHOLDER.match(s.strip()):
        return None
    return s


def name_key(name):
    return hashlib.sha1(unicodedata.normalize("NFC", name).encode("utf-8")).hexdigest()[:16]


# ------------------------------------------------------------------ tally
class Tally:
    """Counts every rescue / encoding / drop, per set and rule; keeps a few examples."""

    def __init__(self):
        self.c = Counter()
        self.ex = defaultdict(list)

    def add(self, key, n=1, example=None):
        self.c[key] += n
        if example is not None and len(self.ex[key]) < 5:
            v = str(example)[:200]
            if v not in self.ex[key]:
                self.ex[key].append(v)

    def merge(self, other_c, other_ex):
        self.c.update(other_c)
        for k, v in other_ex.items():
            for e in v:
                if len(self.ex[k]) < 5 and e not in self.ex[k]:
                    self.ex[k].append(e)

    def as_json(self):
        out = {}
        for key, n in sorted(self.c.items()):
            st, rule, what = key.split("|", 2)
            out.setdefault(st, {}).setdefault(rule, {})[what] = n
        return {"counts": out, "examples": {k: v for k, v in sorted(self.ex.items())}}


class Ctx:
    """Per-task state: the set name, the tally, and IRI minting with validation."""

    def __init__(self, setname, tally):
        self.set = setname
        self.t = tally

    def count(self, rule, what, n=1, example=None):
        self.t.add(f"{self.set}|{rule}|{what}", n, example)

    def mint(self, raw, rule, field):
        """R2: escape, then let the parser decide. None (counted) if it still fails."""
        iri = iri_escape(raw)
        if iri != raw:
            self.count("R2", f"encoded.{field}", example=raw)
        if not iri_parses(iri):
            self.count(rule, f"dropped.{field}.invalid_iri", example=raw)
            return None
        return iri

    def doi_iri(self, doi, field):
        return self.mint(DOIB + doi, "R2", field)


# ------------------------------------------------------------------ metadata
META_COLS = ["doi", "prefix", "title", "resource_type", "resource_type_general", "publisher",
             "version", "schema_version", "state", "client_id", "language", "url", "registered",
             "created", "updated", "publication_year", "citation_count", "reference_count",
             "view_count", "download_count", "creators_json", "related_identifiers_json",
             "funding_references_json", "subjects_json"]
LINK_COLS = ["subj_id", "obj_id", "relation_type", "source_id", "occurred_at"]


def emit_metadata(out, r, snapshot, x):
    """One DataCite record (dict of META_COLS) -> N-Triples lines appended to `out`."""
    raw_doi = r.get("doi")
    doi, how = norm_doi(raw_doi)
    if doi is None:
        # the record's own DOI: not ours to drop on shape alone, if it escapes to an IRI
        raw = str(raw_doi or "").strip().lower()
        if not raw:
            x.count("R1", "dropped.record.empty_doi")
            return
        doi = raw
        x.count("R1", "record.nonstandard_doi", example=raw_doi)
    elif how != "clean":
        x.count("R1", f"rescued.record.{how}", example=raw_doi)
    subj = x.doi_iri(doi, "record_doi")
    if subj is None:
        return
    x.count("R9", f"records.from_{snapshot}")
    S = f"<{subj}>"
    ap = out.append

    def t(p, o):
        ap(f"{S} <{p}> {o} .\n")

    rtg = (r.get("resource_type_general") or "").replace(" ", "").lower()
    cls = TYPE_CLASS.get(rtg)
    if cls is None:
        cls = "Resource"
        if rtg:
            x.count("types", "unknown_resource_type_general.kept_as_text",
                    example=r.get("resource_type_general"))
            t(f"{D}resourceTypeGeneralText", f'"{lit(r.get("resource_type_general"))}"')
    t(RDF_TYPE, f"<{D}{cls}>")
    t(f"{D}fromSnapshot", f"<{D}snapshot-{snapshot}>")
    t(f"{D}doi", f'"{lit(str(raw_doi).strip().lower())}"')
    for col, prop, dt in (
        ("prefix", "prefix", None), ("title", "title", None),
        ("resource_type", "resourceType", None), ("publisher", "publisherName", None),
        ("version", "version", None), ("schema_version", "schemaVersion", None),
        ("state", "state", None), ("client_id", "clientId", None),
        ("language", "language", None),
        ("registered", "registered", "dateTime"), ("created", "created", "dateTime"),
        ("updated", "updated", "dateTime"),
    ):
        v = r.get(col)
        if v:
            t(f"{D}{prop}", f'"{lit(v)}"^^<{XSD}{dt}>' if dt else f'"{lit(v)}"')
    url = r.get("url")
    if url:  # R12
        u = str(url)
        if _SCHEME_RE.match(u) and "://" in u and iri_parses(u):
            t(f"{D}landingPage", f'"{lit(u)}"^^<{XSD}anyURI>')
        else:
            t(f"{D}landingPage", f'"{lit(u)}"')
            x.count("R12", "untyped_landing_page", example=u)
    py = r.get("publication_year")
    if py not in (None, ""):  # R11
        try:
            y = int(str(py).strip())
        except ValueError:
            y = None
        if y is not None and YEAR_MIN <= y <= YEAR_MAX:
            t(f"{D}publicationYear", f'"{y:04d}"^^<{XSD}gYear>')
        else:
            t(f"{D}publicationYearText", f'"{lit(str(py).strip())}"')
            x.count("R11", "publication_year_kept_as_text", example=py)
    for col, prop in (("citation_count", "citationCount"), ("reference_count", "referenceCount"),
                      ("view_count", "viewCount"), ("download_count", "downloadCount")):
        v = r.get(col)
        if v not in (None, ""):
            try:
                t(f"{D}{prop}", f'"{int(v)}"^^<{XSD}integer>')
            except (TypeError, ValueError):
                x.count("metrics", "dropped.not_integer", example=v)

    # creators: reified role + agent, keyed to orcid.org when the ORCID is valid (R6)
    for k, cr in enumerate(jload(r.get("creators_json")) or [], 1):
        if not isinstance(cr, dict):
            continue
        orcid, raw_ids = None, []
        nids = cr.get("nameIdentifiers") or []
        if isinstance(nids, dict):
            nids = [nids]
        for nid in nids:
            if not isinstance(nid, dict):
                continue
            sch = str(nid.get("nameIdentifierScheme") or "")
            val = str(nid.get("nameIdentifier") or "")
            if sch.lower() == "orcid" or "orcid.org" in val.lower():
                cand, seen = find_orcid(val)
                if cand and orcid_ok(cand):
                    if orcid is None:
                        orcid = cand
                        if seen != cand:
                            x.count("R6", "rescued.creator_orcid_" + (
                                "lowercase_x" if seen.upper() == cand else "unhyphenated"), example=val)
                    elif cand != orcid:
                        raw_ids.append((val, sch))
                elif val.strip():
                    raw_ids.append((val, sch or "ORCID"))
                    x.count("R6", "creator_orcid_failed_check_kept_as_text", example=val)
        role_iri = f"{subj}/creator/{k}"
        airi = (ORCIDB + orcid) if orcid else (role_iri + "/agent")
        t(f"{D}creator", f"<{airi}>")
        ap(f"{S} <{D}hasContributor> <{role_iri}> .\n")
        ap(f"<{role_iri}> <{RDF_TYPE}> <{D}AgentRole> .\n")
        ap(f"<{role_iri}> <{D}isHeldBy> <{airi}> .\n")
        ap(f"<{airi}> <{RDF_TYPE}> <{D}Agent> .\n")
        nm = cr.get("name")
        if nm:
            ap(f'<{airi}> <{D}agentName> "{lit(nm)}" .\n')
        if orcid:
            ap(f'<{airi}> <{D}orcid> "{orcid}" .\n')
        for val, sch in raw_ids:
            ap(f'<{airi}> <{D}rawIdentifier> "{lit(val)}" .\n')
            ap(f'<{airi}> <{D}rawIdentifierScheme> "{lit(sch)}" .\n')
        affs = cr.get("affiliation") or []
        if not isinstance(affs, list):
            affs = [affs]
        for aff in affs:
            an = aff.get("name") if isinstance(aff, dict) else aff
            if an:
                ap(f'<{airi}> <{D}affiliationName> "{lit(an)}" .\n')
            if isinstance(aff, dict):
                ai = str(aff.get("affiliationIdentifier") or "")
                ais = str(aff.get("affiliationIdentifierScheme") or "")
                if "ror.org" in ai.lower() or (ais.upper() == "ROR" and ai.strip()):
                    rid = ror_from(ai, typed_ror=ais.upper() == "ROR")
                    if rid and ror_ok(rid) and ai == RORB + rid:
                        ap(f'<{airi}> <{D}rorId> "{ai}"^^<{XSD}anyURI> .\n')
                    elif rid and ror_ok(rid):
                        ap(f'<{airi}> <{D}rorId> "{RORB}{rid}"^^<{XSD}anyURI> .\n')
                        x.count("R14", "rescued.ror_bare_or_untidy_url", example=ai)
                    else:
                        ap(f'<{airi}> <{D}rawIdentifier> "{lit(ai)}" .\n')
                        ap(f'<{airi}> <{D}rawIdentifierScheme> "{lit(ais or "ROR")}" .\n')
                        x.count("R14", "ror_failed_check_kept_as_text", example=ai)

    # related identifiers -> typed relation edges (R1, R2, R7, R8)
    for rel in jload(r.get("related_identifiers_json")) or []:
        if not isinstance(rel, dict):
            continue
        if str(rel.get("relatedIdentifierType") or "").upper() != "DOI":
            x.count("relid", "not_doi_typed.not_converted")
            continue
        rk = relkey(rel.get("relationType"))
        prop = REL_PROP.get(rk)
        if prop is None:
            prop = FALLBACK_PROP
            x.count("R7", "unknown_relation_type.as_isRelatedTo", example=rel.get("relationType"))
        rdoi, how = norm_doi(rel.get("relatedIdentifier"), embedded=True)
        if rdoi is None:
            if how == "empty":
                x.count("R8", "skipped.empty")
            else:
                x.count("R8", f"dropped.{how}", example=rel.get("relatedIdentifier"))
            continue
        if how != "clean":
            x.count("R1", f"rescued.relid.{how}", example=rel.get("relatedIdentifier"))
        o = x.doi_iri(rdoi, "relid")
        if o is None:
            continue
        x.count("R7", f"relid.{prop}")
        t(f"{D}{prop}", f"<{o}>")

    # funding (R1, R3, R4)
    for fr in jload(r.get("funding_references_json")) or []:
        if not isinstance(fr, dict):
            continue
        fname = usable_name(fr.get("funderName"))
        if fname is None and fr.get("funderName") not in (None, ""):
            x.count("R4", "placeholder_or_blank_name", example=fr.get("funderName"))
        fid_raw = str(fr.get("funderIdentifier") or "")
        fid = fid_raw.strip()
        ftype = str(fr.get("funderIdentifierType") or "").strip()
        firi, keep_raw = None, False
        if fid:
            rid = ror_from(fid, typed_ror=ftype.upper() == "ROR")
            if rid is not None:
                if ror_ok(rid):
                    firi = RORB + rid
                    x.count("R3", "funder.ror")
                else:
                    keep_raw = True
                    x.count("R3", "funder.ror_failed_check", example=fid)
            else:
                fdoi, how = norm_doi(fid)
                if fdoi is None and ftype.lower() == "crossref funder id":
                    # a Funder Registry number written without its 10.13039/ DOI prefix,
                    # bare or as a doi.org URL path (`https://doi.org/501100003407`)
                    m = _DOI_URL_PREFIX.match(fid.lower())
                    num = fid[m.end():] if m else fid
                    if re.fullmatch(r"100\d{6}|5011\d{8}", num):
                        fdoi, how = "10.13039/" + num, "bare_funder_id"
                if fdoi is not None:
                    firi = x.doi_iri(fdoi, "funder")
                    x.count("R3", "funder.doi")
                    if how == "bare_funder_id":
                        x.count("R3", "rescued.funder.bare_funder_registry_number", example=fid)
                    elif how != "clean":
                        x.count("R1", f"rescued.funder.{how}", example=fid)
                    if firi is None:
                        keep_raw = True
                else:
                    keep_raw = True
                    x.count("R3", f"funder.id_not_minted.{ftype or 'untyped'}", example=fid)
        if firi is None:
            if fname is None:
                x.count("R4", "dropped.funder_without_name_or_usable_id",
                        example=fid or fr.get("funderName"))
                continue
            firi = f"{subj}/funder/{name_key(fname)}"
            x.count("R3", "funder.name_minted")
        t(f"{D}fundedBy", f"<{firi}>")
        ap(f"<{firi}> <{RDF_TYPE}> <{D}Funder> .\n")
        if fname is not None:
            ap(f'<{firi}> <{D}agentName> "{lit(fname)}" .\n')
        if keep_raw and fid:
            ap(f'<{firi}> <{D}rawIdentifier> "{lit(fid_raw)}" .\n')
            if ftype:
                ap(f'<{firi}> <{D}rawIdentifierScheme> "{lit(ftype)}" .\n')

    for sub in jload(r.get("subjects_json")) or []:
        s = sub.get("subject") if isinstance(sub, dict) else sub
        if s:
            t(DCT_SUBJECT, f'"{lit(s)}"')


# ------------------------------------------------------------------ PID links
def pid_iri(pid, x, end):
    """Link endpoint -> IRI, or None (counted). R1, R2, R5, R6."""
    p = str(pid or "").strip()
    if not p:
        x.count("R5", f"dropped.{end}.empty")
        return None
    low = p.lower()
    if _DOI_URL_PREFIX.match(low) or low.startswith("doi:") or low.startswith("10."):
        doi, how = norm_doi(p)
        if doi is not None:
            if how != "clean":
                x.count("R1", f"rescued.link_{end}.{how}", example=p)
            return x.doi_iri(doi, f"link_{end}")
        if _SCHEME_RE.match(p) and "://" in p:
            # a doi.org URL whose path is not a DOI: it is still the URL the source states
            x.count("R1", f"link_{end}.doi_url_without_doi_kept_as_url", example=p)
            return x.mint(p, "R5", f"link_{end}")
        x.count("R5", f"dropped.{end}.not_a_doi", example=p)
        return None
    if "orcid.org/" in low or _ORCID_FULL.match(p.upper()):
        cand, seen = find_orcid(p)
        canonical = bool(_ORCID_FULL.match(p.upper()) or (seen and re.fullmatch(
            r"(?:https?://)?(?:www\.)?orcid\.org/" + re.escape(seen) + r"/?", p, re.I)))
        if cand and orcid_ok(cand) and canonical:
            if seen.upper() != cand:
                x.count("R6", f"rescued.{end}.orcid_unhyphenated", example=p)
            return ORCIDB + cand
        if _SCHEME_RE.match(p):
            why = "url_failed_check" if not (cand and orcid_ok(cand)) else "url_not_bare_id"
            x.count("R6", f"{end}.orcid_{why}.kept_verbatim", example=p)
            return x.mint(p, "R6", f"link_{end}")
        x.count("R6", f"dropped.{end}.bare_orcid_failed_check", example=p)
        return None
    if low.startswith("hdl:"):
        x.count("R6", f"rescued.{end}.hdl")
        return x.mint(HDLB + p[4:].lstrip("/"), "R6", f"link_{end}")
    if _SCHEME_RE.match(p):
        return x.mint(p, "R5", f"link_{end}")
    doi, how = norm_doi(p)  # scheme-less: `%2010.1039/...`, `doi/10...`, `org/10...`
    if doi is not None:
        x.count("R5", f"rescued.{end}.schemeless.{how}", example=p)
        return x.doi_iri(doi, f"link_{end}")
    x.count("R5", f"dropped.{end}.no_scheme", example=p)
    return None


def emit_link(out, r, x):
    s_id, o_id = r.get("subj_id"), r.get("obj_id")
    subj = pid_iri(s_id, x, "subj")
    obj = pid_iri(o_id, x, "obj")
    if not subj or not obj:
        x.count("links", "dropped.event")
        return
    rt = r.get("relation_type")
    rk = relkey(rt)
    prop = REL_PROP.get(rk)
    if prop is None:
        prop = FALLBACK_PROP
        x.count("R7", f"link.outside_vocabulary.as_isRelatedTo.{str(rt)[:40]}")
    else:
        x.count("R7", f"link.{prop}")
    ap = out.append
    ap(f"<{subj}> <{D}{prop}> <{obj}> .\n")
    rel_iri = x.mint(f"{subj}/rel/{rk}/{str(o_id).strip().lower()}", "R2", "pid_relation")
    if rel_iri is None:
        return
    ap(f"<{rel_iri}> <{RDF_TYPE}> <{D}PidRelation> .\n")
    ap(f"<{rel_iri}> <{D}relationSubject> <{subj}> .\n")
    ap(f"<{rel_iri}> <{D}relationObject> <{obj}> .\n")
    concept = rel_concept(rt)
    if concept:
        c_iri = x.mint(D + concept, "R7", "relation_concept")
        if c_iri:
            ap(f"<{rel_iri}> <{D}relationType> <{c_iri}> .\n")
    src = r.get("source_id")
    if src:
        s_iri = x.mint(f"{D}source-{src}", "R2", "source")
        if s_iri:
            ap(f"<{rel_iri}> <{D}assertedBy> <{s_iri}> .\n")
    occ = r.get("occurred_at")
    if occ and str(occ).strip():
        if str(occ).strip().startswith("0000-01-01"):
            x.count("R11", "dropped.occurred_at_sentinel")
        else:
            ap(f'<{rel_iri}> <{D}occurredAt> "{lit(occ)}"^^<{XSD}dateTime> .\n')


# ------------------------------------------------------------------ plan (R9, R10)
def build_plan(root, out_path, years=META_YEARS, mem="12GB", threads=8, tmp="/tmp"):
    """Write (year, file, row, action) for every metadata row that is NOT simply
    emitted: `skip` rows of the top year, `keep` rows of the older years.

    Precedence per lower(trim(doi)): snapshot year desc, `updated` desc, then the
    later file and row (so a tie is broken the same way on every run).
    """
    import duckdb

    con = duckdb.connect()
    con.execute(f"SET memory_limit='{mem}'; SET threads={threads}; "
                f"SET temp_directory='{tmp}'; SET preserve_insertion_order=false")
    parts = []
    for y in years:
        g = os.path.join(root, f"parquet-{y}", "*.parquet")
        if glob.glob(g):
            parts.append(f"SELECT lower(trim(doi)) k, {int(y)} y, parse_filename(filename) f, "
                         f"file_row_number r, updated u FROM read_parquet('{g}', filename=true, "
                         f"file_row_number=true) WHERE doi IS NOT NULL AND trim(doi) <> ''")
    top = max(int(y) for y in years if glob.glob(os.path.join(root, f"parquet-{y}", "*.parquet")))
    con.execute(f"""CREATE TEMP TABLE ranked AS SELECT y, f, r, row_number() OVER (
        PARTITION BY k ORDER BY y DESC, u DESC NULLS LAST, f DESC, r DESC) rn
        FROM ({' UNION ALL '.join(parts)})""")
    con.execute(f"""COPY (SELECT y, f, r, 'keep' AS action FROM ranked WHERE rn = 1 AND y < {top}
        UNION ALL SELECT y, f, r, 'skip' AS action FROM ranked WHERE rn > 1 AND y = {top})
        TO '{out_path}' (FORMAT parquet)""")
    stats = dict(con.execute(f"""SELECT 'rows_read', count(*) FROM ranked UNION ALL
        SELECT 'dois', count(*) FROM ranked WHERE rn = 1 UNION ALL
        SELECT 'records_from_' || y, count(*) FROM ranked WHERE rn = 1 GROUP BY y UNION ALL
        SELECT 'rows_not_emitted_' || y, count(*) FROM ranked WHERE rn > 1 GROUP BY y""").fetchall())
    # R10 within one snapshot: DOIs with more than one row in the same year
    for y, n_dup_dois, n_rows in con.execute("""SELECT y, count(*), sum(c) FROM (
            SELECT k, y, count(*) c FROM (""" + " UNION ALL ".join(parts) + """)
            GROUP BY k, y HAVING count(*) > 1) GROUP BY y""").fetchall():
        stats[f"within_year_duplicate_dois_{y}"] = n_dup_dois
        stats[f"within_year_duplicate_rows_{y}"] = int(n_rows)
    stats["top_year"] = top
    stats = {k: int(v) for k, v in stats.items()}
    with open(out_path + ".json", "w") as fh:
        json.dump(stats, fh, indent=1, sort_keys=True)
    return stats


def load_plan(path):
    """(year, file basename) -> set of file row numbers named by the plan."""
    import pyarrow.parquet as pq

    t = pq.read_table(path).to_pydict()
    plan = defaultdict(set)
    for y, f, r in zip(t["y"], t["f"], t["r"]):
        plan[(str(y), f)].add(int(r))
    stats = {}
    if os.path.exists(path + ".json"):
        with open(path + ".json") as fh:
            stats = json.load(fh)
    return plan, stats


# ------------------------------------------------------------------ driver
_LOCK = None
_OUT_FD = 1


def _init_worker(lock, fd):
    global _LOCK, _OUT_FD
    _LOCK, _OUT_FD = lock, fd


def _flush(buf):
    data = "".join(buf).encode("utf-8")
    buf.clear()
    if not data:
        return
    if _LOCK is not None:
        _LOCK.acquire()
    try:
        view = memoryview(data)
        while view:
            n = os.write(_OUT_FD, view)
            view = view[n:]
    finally:
        if _LOCK is not None:
            _LOCK.release()


# Rows per streamed batch. Some metadata row groups are 12 GB uncompressed (one
# column of related identifiers), so a row group is never materialised whole.
BATCH_ROWS = {"meta": 200, "links": 5000}


def run_task(task):
    """task = (kind, setname, path, row_groups|None, plan_rows|None, plan_action)."""
    import pyarrow.parquet as pq

    kind, setname, path, rgs, plan_rows, action = task
    tally = Tally()
    x = Ctx(("meta-" if kind == "meta" else "links-") + setname, tally)
    pf = pq.ParquetFile(path, pre_buffer=False)
    cols = META_COLS if kind == "meta" else LINK_COLS
    avail = [c for c in cols if c in pf.schema_arrow.names]
    offsets, o = [], 0
    for i in range(pf.num_row_groups):
        offsets.append(o)
        o += pf.metadata.row_group(i).num_rows
    keep = kind == "meta" and action == "keep"
    buf, lines = [], 0
    for rg in (rgs if rgs is not None else range(pf.num_row_groups)):
        base = offsets[rg]
        n_rg = pf.metadata.row_group(rg).num_rows
        if keep and not any(base <= i < base + n_rg for i in plan_rows or ()):
            continue
        pos = base
        for b in pf.iter_batches(batch_size=BATCH_ROWS[kind], row_groups=[rg], columns=avail,
                                 use_threads=False):
            n = b.num_rows
            if kind == "meta" and (keep or plan_rows):
                idx = [i for i in range(n) if ((pos + i) in plan_rows) == keep]
                pos += n
                if not idx:
                    continue
                if len(idx) != n:
                    b = b.take(idx)
            else:
                pos += n
            C = {c: b.column(c).to_pylist() for c in avail}
            x.count("input", "rows", b.num_rows)
            for i in range(b.num_rows):
                r = {c: C[c][i] for c in avail}
                before = len(buf)
                if kind == "meta":
                    emit_metadata(buf, r, setname, x)
                else:
                    emit_link(buf, r, x)
                lines += len(buf) - before
                if len(buf) >= 20000:
                    _flush(buf)
            del C, b
    _flush(buf)
    x.count("output", "triples", lines)
    return dict(tally.c), dict(tally.ex)


def make_tasks(root, kinds, plan, sample_rgs=0, seed=20261016):
    """One task per Parquet file. Metadata files of the newest snapshot present are
    emitted minus the plan's `skip` rows; older snapshots emit only `keep` rows."""
    import pyarrow.parquet as pq

    tasks = []
    rng = random.Random(seed)
    sets = []
    present = [y for y in META_YEARS if glob.glob(os.path.join(root, f"parquet-{y}", "*.parquet"))]
    top = present[0] if present else None
    if "meta" in kinds:
        sets += [("meta", y, os.path.join(root, f"parquet-{y}")) for y in present]
    if "links" in kinds:
        sets += [("links", s, os.path.join(root, f"parquet-links-{s}")) for s in LINK_SETS]
    for kind, name, d in sets:
        files = sorted(glob.glob(os.path.join(d, "*.parquet")))
        if not files:
            continue
        action = ("skip" if name == top else "keep") if kind == "meta" else None
        chosen = None
        if sample_rgs:
            pool = [(f, i) for f in files for i in range(pq.ParquetFile(f).num_row_groups)]
            chosen = defaultdict(list)
            for f, i in sorted(rng.sample(pool, min(sample_rgs, len(pool)))):
                chosen[f].append(i)
        for f in files:
            if chosen is not None and f not in chosen:
                continue
            rows = plan.get((name, os.path.basename(f))) if kind == "meta" else None
            if kind == "meta" and action == "keep" and not rows:
                continue  # an older snapshot file none of whose DOIs survives R9
            tasks.append((kind, name, f, chosen[f] if chosen is not None else None,
                          rows, action))
    return tasks


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("plan", help="R9/R10 record selection -> Parquet")
    p.add_argument("--root", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--mem", default="12GB")
    p.add_argument("--threads", type=int, default=8)
    p.add_argument("--tmp", default="/tmp")
    e = sub.add_parser("emit", help="N-Triples to stdout")
    e.add_argument("--root", required=True)
    e.add_argument("--plan", required=True, help="output of `plan` over the same --root")
    e.add_argument("--report", required=True, help="JSON tally path")
    e.add_argument("--mode", choices=["all", "metadata", "links"], default="all")
    e.add_argument("--workers", type=int, default=8)
    e.add_argument("--sample-row-groups", type=int, default=0,
                   help="per set, convert this many seeded-random row groups only")
    e.add_argument("--seed", type=int, default=20261016)
    a = ap.parse_args(argv)

    if a.cmd == "plan":
        stats = build_plan(a.root, a.out, mem=a.mem, threads=a.threads, tmp=a.tmp)
        print(json.dumps(stats, indent=1), file=sys.stderr)
        return 0

    if _ox is None:
        sys.exit("pyoxigraph is required: every IRI is checked by its parser")
    plan, plan_stats = load_plan(a.plan)
    kinds = {"all": ("meta", "links"), "metadata": ("meta",), "links": ("links",)}[a.mode]
    tasks = make_tasks(a.root, kinds, plan, a.sample_row_groups, a.seed)
    # biggest files first, so the pool does not end on one long straggler
    tasks.sort(key=lambda t: -os.path.getsize(t[2]))
    tally = Tally()
    sys.stdout.flush()
    lock = mp.Lock()
    done = 0
    # ProcessPoolExecutor, not Pool: a worker that dies (OOM kill) raises
    # BrokenProcessPool here instead of silently losing its file.
    from concurrent.futures import ProcessPoolExecutor, as_completed

    with ProcessPoolExecutor(a.workers, mp_context=mp.get_context("fork"),
                             initializer=_init_worker,
                             initargs=(lock, sys.stdout.fileno())) as pool:
        for fut in as_completed([pool.submit(run_task, t) for t in tasks]):
            c, ex = fut.result()
            tally.merge(c, ex)
            done += 1
            trip = sum(v for k, v in tally.c.items() if k.endswith("|output|triples"))
            print(f"  ...{done}/{len(tasks)} files done, {trip:,} triples", file=sys.stderr,
                  flush=True)
    rep = tally.as_json()
    rep["tasks"] = len(tasks)
    rep["plan"] = {"file": os.path.basename(a.plan), "stats": plan_stats}
    rep["sample_row_groups"] = a.sample_row_groups
    rep["seed"] = a.seed
    with open(a.report, "w", encoding="utf-8") as fh:
        json.dump(rep, fh, indent=1, ensure_ascii=False)
    print(f"DONE: {len(tasks)} tasks; tally -> {a.report}", file=sys.stderr, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
