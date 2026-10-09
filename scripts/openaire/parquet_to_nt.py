"""Stream the OpenAIRE Graph Parquet -> N-Triples on stdout. Pipe into `rete build -`.

Vocabulary `oaire:` = https://w3id.org/rete/openaire# (aligned to schema.org / DCAT
/ FaBiO via the scholar hub), so this graph joins the others by IRI rather than by
string matching:

  * a result with a DOI IS ``https://doi.org/{doi}``  — the same node crossref,
    datacite, opencitations, dblp and zenodo use;
  * an author with an ORCID IS ``https://orcid.org/{id}``;
  * an organization with a ROR IS ``https://ror.org/{id}``;
  * everything else keeps its native OpenAIRE id under ``https://w3id.org/rete/
    openaire/{50|10|20|40}/{hash}`` so nothing is lost and relations still resolve.

Authorship is REIFIED (oaire:AuthorRole with oaire:rank) rather than flattened to a
string, so "second author of X" is answerable — the same shape opencitations and
datacite use here.

`--only <entity>` converts ONE entity type, which is what makes a parallel sharded
build possible: the builder is single-threaded, so N converters feeding N builders
on N cores is the whole optimization (see docs/BENCHMARK.md).

EVERY IRI THIS EMITS IS A VALID ABSOLUTE IRI — see :func:`data_iri`. The dump's
own ``websiteurl`` / instance-``url`` columns are free text and carry values that
are not IRIs at all (``www.example.org`` with no scheme, ``http//x.y`` with no
colon, a raw ``#`` mid-path). Percent-escaping cannot repair a missing scheme, so
those statements are DROPPED and counted rather than emitted as something a
strict parser rejects: one such IRI costs a bulk loader the whole chunk, not the
line. The tally goes to stderr at the end of every run.

Usage:
  python parquet_to_nt.py --only publication | rete build - --format nt ...
  python parquet_to_nt.py --only relation --row-groups 2   # bounded sample
  python parquet_to_nt.py --only organization --parquet-root data/openaire
"""
import argparse
import glob
import ipaddress
import json
import os
import re
import sys
from collections import Counter, defaultdict

import pyarrow.parquet as pq

OA = "https://w3id.org/rete/openaire#"
N = "https://w3id.org/rete/openaire/"
DOI = "https://doi.org/"
ORCID = "https://orcid.org/"
ROR = "https://ror.org/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
DCT = "http://purl.org/dc/terms/"
XSD = "http://www.w3.org/2001/XMLSchema#"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

_ESC = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_ESC_RE = re.compile(r'[\\"\n\r\t]')
_CTRL = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")
_ORCID_RE = re.compile(r"(\d{4}-\d{4}-\d{4}-\d{3}[\dX])", re.I)
_ROR_RE = re.compile(r"(0[a-z0-9]{8})", re.I)

# OpenAIRE result types -> our classes (schema.org/FaBiO-aligned via the hub).
TYPES = {
    "publication": "Publication",
    "dataset": "Dataset",
    "software": "Software",
    "otherresearchproduct": "OtherResearchProduct",
}


def ienc(s):
    """Escape an id WE mint so that prefix + id is a valid IRI.

    Only ever applied under a constant prefix (``N``, ``DOI``, ``ORCID``,
    ``ROR``), so the scheme is ours and the result is absolute by construction.
    Free text out of the dump goes through :func:`data_iri` instead — this
    function alone is NOT enough to make an arbitrary string an IRI.

    It is :func:`iri_escape`, the same transformation ``rete export
    --sanitize-iris`` applies: an id that was already a valid IRI suffix comes
    out unchanged, so no published IRI moves, and an id with a ``[``/``]``, a
    second ``#`` or a stray ``%`` (DOIs carry all three) comes out exactly as
    the sanitized dumps already wrote it. Escaping only the IRIREF-forbidden
    characters, as this did first, let those through, and `rete build --strict`
    refuses them.
    """
    return iri_escape(str(s))


# --------------------------------------------------------------- IRI validity
# The N-Triples/N-Quads IRIREF production plus RFC 3987, i.e. exactly what
# `rete build`'s audit counts and `rete export --sanitize-iris` repairs. Kept in
# step with crates/rete-core/src/iri.rs — one definition, five classes:
#   NotAbsolute (no scheme)  ForbiddenChar  Bracket  ExtraHash  BadPercent
# Only the first is unrepairable: resolving a relative IRI needs a base IRI
# neither the dump nor the .rete records.
_HEX = "0123456789abcdefABCDEF"
_SCHEME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.\-]*:")
# `http//x`, `https:/x`, `HTTP: //x` — the scheme is STATED, only its punctuation
# is broken, so restoring `://` recovers the value the dump meant rather than
# guessing one. A bare `www.x.org` names no scheme and is never repaired.
_LAME_SCHEME_RE = re.compile(r"^([A-Za-z][A-Za-z0-9+.\-]*)\s*:?\s*/{0,2}\s*(?=[^/\s])")
_KNOWN_SCHEMES = ("http", "https", "ftp", "ftps")


def _ip_literal_brackets(b, colon):
    """Byte offsets of an IP-literal host's ``[``/``]`` — the one legal pair."""
    if b[colon + 1 : colon + 3] != b"//":
        return None
    start = colon + 3
    if b[start : start + 1] != b"[":
        return None
    end = len(b)
    for p in range(start, len(b)):
        if b[p] in (0x2F, 0x3F, 0x23):  # / ? #
            end = p
            break
    close = b.find(b"]", start, end)
    return None if close < 0 else (start, close)


def _is_ucschar(cp):
    """RFC 3987 ``ucschar``: what an IRI may carry unescaped above U+007F."""
    return (0xA0 <= cp <= 0xD7FF or 0xF900 <= cp <= 0xFDCF or 0xFDF0 <= cp <= 0xFFEF
            or (0x10000 <= cp <= 0xEFFFD and (cp & 0xFFFF) <= 0xFFFD))


def iri_escape(s):
    """Percent-encode every IRIREF defect escaping CAN repair.

    Lossless in the sense that matters here: the escaped form denotes the same
    resource. Does not and cannot supply a missing scheme.
    """
    b = s.encode("utf-8", "surrogatepass")
    m = _SCHEME_RE.match(s)
    brackets = _ip_literal_brackets(b, m.end() - 1) if m else None
    out = bytearray()
    i = 0
    seen_hash = False
    while i < len(b):
        c = b[i]
        if c >= 0x80:
            # One whole UTF-8 sequence. A `ucschar` (RFC 3987) is legal and
            # never touched; anything else -- a C1 control, a noncharacter, a
            # private-use or lone-surrogate code point -- is percent-encoded
            # byte by byte, which is the IRI -> URI mapping and keeps the meaning.
            n = 2 if c < 0xE0 else 3 if c < 0xF0 else 4
            seq = b[i : i + n]
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
        elif c == 0x23:  # only the FIRST '#' starts a fragment
            if seen_hash:
                out += b"%23"
            else:
                seen_hash = True
                out.append(c)
        elif c == 0x25:  # '%' must open a pct-encoded triplet
            if i + 2 < len(b) and chr(b[i + 1]) in _HEX and chr(b[i + 2]) in _HEX:
                out += b[i : i + 3]
                i += 3
                continue
            out += b"%25"
        else:
            out.append(c)
        i += 1
    return out.decode("utf-8", "surrogatepass")


# A scheme followed by `//` opens an authority, and RFC 3987 constrains it:
# `[iuserinfo "@"] ihost [":" port]`, the port digits only, an IP-literal in
# brackets. A value can carry a perfectly good scheme and still fail here --
# `http//:host.hu` repaired to `http://:host.hu` has an empty host and the port
# `host.hu`. The RFC 3987 parser behind rete's gate (#255) and Oxigraph rejects
# it, so it is dropped and counted like a missing scheme, never rewritten.
_AUTH_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.\-]*://([^/?#]*)")
_REG_NAME_RE = re.compile(
    r"^(?:[A-Za-z0-9\-._~!$&'()*+,;=\u00a0-\U0010ffff]|%[0-9A-Fa-f]{2})*$")
_USERINFO_RE = re.compile(
    r"^(?:[A-Za-z0-9\-._~!$&'()*+,;=:\u00a0-\U0010ffff]|%[0-9A-Fa-f]{2})*$")
_IPVFUTURE_RE = re.compile(r"^[vV][0-9A-Fa-f]+\.[A-Za-z0-9\-._~!$&'()*+,;=:]+$")


def authority_defect(iri):
    """Why ``iri``'s authority is not a valid RFC 3987 ``iauthority``, or ``None``.

    ``None`` too when the IRI has no authority at all (no ``//`` after the
    scheme). An EMPTY host is legal syntax (``file:///x``), exactly as for the
    parser the gate uses; what fails is a malformed one.
    """
    m = _AUTH_RE.match(iri)
    if not m:
        return None
    userinfo, at, hostport = m.group(1).rpartition("@")
    if at and not _USERINFO_RE.match(userinfo):
        return "userinfo"
    if hostport.startswith("["):
        close = hostport.find("]")
        if close < 0:
            return "unclosed IP-literal"
        literal, rest = hostport[1:close], hostport[close + 1 :]
        try:
            if "%" in literal:  # Python takes a zone id; RFC 3986 does not
                raise ValueError(literal)
            ipaddress.IPv6Address(literal)
        except ValueError:
            if not _IPVFUTURE_RE.match(literal):
                return "IP-literal"
        port = rest[1:] if rest.startswith(":") else None
        if rest and port is None:
            return "text after IP-literal"
    else:
        host, colon, port = hostport.partition(":")
        if not colon:
            port = None
        if not _REG_NAME_RE.match(host):
            return "host"
    if port and not port.isdigit():
        return "port"
    return None


# What a dropped value looked like -- the shapes measured on the refused 2021
# dumps (empty / bare host / not a URL; the broken-scheme shape is REPAIRED),
# so a rebuild's tally can be checked against them one by one.
_BARE_HOST_RE = re.compile(r"^[^\s/:@]+\.[A-Za-z][^\s/:@]*(?::\d+)?(?:[/?#]|$)")


def _shape(raw):
    if not raw:
        return "empty"
    if _BARE_HOST_RE.match(raw):
        return "bare host"
    return "not a URL"


class IriAudit:
    """What the converter did to the free-text URLs the dump handed it."""

    def __init__(self):
        self.seen = 0
        self.escaped = 0
        self.rescued = Counter()
        self.dropped = Counter()
        self.shapes = Counter()
        self.samples = defaultdict(list)

    def _sample(self, field, raw):
        if len(self.samples[field]) < 10:
            self.samples[field].append(raw)

    def report(self, stream=sys.stderr):
        if not self.seen:
            return
        drops = sum(self.dropped.values())
        print(
            f"IRI audit: {self.seen:,} free-text URL(s) seen, "
            f"{self.escaped:,} percent-escaped, "
            f"{sum(self.rescued.values()):,} scheme punctuation repaired, "
            f"{drops:,} DROPPED (empty, no scheme, or an invalid authority — "
            f"not repairable).",
            file=stream,
        )
        for field, n in self.rescued.most_common():
            print(f"  repaired {n:>7}  {field}", file=stream)
        for field, n in self.dropped.most_common():
            print(f"  dropped  {n:>7}  {field}", file=stream)
            for raw in self.samples[field]:
                print(f"             e.g. {raw!r}", file=stream)
        # One machine-readable line: the per-shape numbers a rebuild is checked
        # against. `repaired` is the broken-scheme shape (`http//x`).
        shapes = " ".join(f"{k.replace(' ', '_')}={v}" for k, v in sorted(self.shapes.items()))
        print(f"IRI audit shapes: repaired={sum(self.rescued.values())} {shapes}".rstrip(),
              file=stream)


def data_iri(value, audit, field):
    """A free-text URL from the dump -> a valid absolute IRI, or ``None``.

    Three outcomes, in order, and the third is the point:

    1. **escape** what the IRIREF grammar forbids (a raw ``#`` past the first, a
       space, ``[``/``]``, a lone ``%``) — the IRI still denotes the same thing;
    2. **repair** a scheme the value itself states but mispunctuates
       (``http//x`` -> ``http://x``): the intended value is in the data, not
       invented;
    3. **drop** anything still relative, empty, or with an authority RFC 3987
       rejects (see :func:`authority_defect`). ``www.x.org`` does not say whether it
       meant ``http`` or ``https``, and a converter that picks one is inventing
       a fact. Emitting it verbatim is worse still: `rete build` stores it, the
       `.nq` export carries it, and Oxigraph then rejects the entire ~102,000
       line chunk it landed in (rete issue #233).
    """
    raw = str(value).strip()
    audit.seen += 1
    if not raw:
        # An absent URL. The 2021 dump writes it as "" inside instance.url, and
        # the first converter emitted it as the IRI `<>`. Dropped, and COUNTED,
        # so the tally accounts for every statement a rebuild no longer has.
        return _drop(audit, field, raw, "empty")
    s = iri_escape(raw)
    if s != raw:
        audit.escaped += 1
    if _SCHEME_RE.match(s):
        if authority_defect(s) is not None:
            return _drop(audit, field, raw, "invalid authority")
        return s
    m = _LAME_SCHEME_RE.match(s)
    if m and m.group(1).lower() in _KNOWN_SCHEMES:
        s = m.group(1).lower() + "://" + s[m.end() :]
        if _SCHEME_RE.match(s):
            if authority_defect(s) is not None:
                return _drop(audit, field, raw, "invalid authority")
            audit.rescued[field] += 1
            return s
    return _drop(audit, field, raw, _shape(raw))


def _drop(audit, field, raw, shape):
    audit.dropped[field] += 1
    audit.shapes[shape] += 1
    audit._sample(field, raw)
    return None


def lit(x):
    s = _ESC_RE.sub(lambda m: _ESC[m.group()], str(x))
    return _CTRL.sub("", s)


def jload(cell):
    if not cell:
        return None
    try:
        return json.loads(cell)
    except Exception:
        return None


def node(oa_id):
    """Native OpenAIRE id -> IRI. Ids look like `50|doi_________::hash`."""
    return N + ienc(str(oa_id).replace("|", "/", 1))


class W:
    """Buffered N-Triples writer — one flush per ~20k lines keeps memory flat."""

    def __init__(self):
        self.buf = []
        self.n = 0

    def t(self, s, p, o):
        self.buf.append(f"{s} <{p}> {o} .\n")
        self.n += 1

    def lit(self, s, p, v, dt=None):
        if v is None or v == "":
            return
        o = f'"{lit(v)}"' + (f"^^<{dt}>" if dt else "")
        self.t(s, p, o)

    def flush(self, out, force=False):
        if self.buf and (force or len(self.buf) >= 20000):
            out.write("".join(self.buf).encode("utf-8"))
            self.buf.clear()


def batches(entity, cols, batch, row_groups):
    files = sorted(glob.glob(os.path.join(ROOT, f"parquet-{entity}", "*.parquet")))
    if not files:
        raise SystemExit(f"no parquet for {entity} under {ROOT}")
    seen_rg = 0
    for f in files:
        pf = pq.ParquetFile(f)
        avail = [c for c in cols if c in pf.schema_arrow.names]
        for b in pf.iter_batches(batch_size=batch, columns=avail):
            yield {c: b.column(c).to_pylist() for c in avail}, len(b)
            seen_rg += 1
            if row_groups and seen_rg >= row_groups:
                return
        print(f"  ...{os.path.basename(f)} done", file=sys.stderr, flush=True)


def result_iri(oa_id, doi):
    return f"<{DOI}{ienc(str(doi).strip().lower())}>" if doi else f"<{node(oa_id)}>"


def emit_result(w, out, entity, args):
    cls = TYPES.get(entity, "Result")
    cols = ["id", "pid_doi", "maintitle", "subtitle", "publication_year", "publicationdate",
            "publisher", "language_code", "bestaccessright_label", "type",
            "author_json", "container_json", "subjects_json", "instance_json", "pid_json"]
    for d, k in batches(entity, cols, args.batch, args.row_groups):
        g = lambda c: d.get(c, [None] * k)  # noqa: E731
        for i in range(k):
            oid = g("id")[i]
            if not oid:
                continue
            s = result_iri(oid, g("pid_doi")[i])
            w.t(s, RDF_TYPE, f"<{OA}{cls}>")
            w.lit(s, f"{OA}openaireId", oid)
            if g("pid_doi")[i]:
                w.lit(s, f"{OA}doi", str(g("pid_doi")[i]).strip().lower())
            title = g("maintitle")[i]
            if title:
                w.lit(s, f"{OA}title", title)
                w.lit(s, RDFS_LABEL, title)
            w.lit(s, f"{OA}subtitle", g("subtitle")[i])
            if g("publication_year")[i]:
                w.lit(s, f"{OA}publicationYear", g("publication_year")[i], f"{XSD}gYear")
            w.lit(s, f"{DCT}issued", g("publicationdate")[i])
            w.lit(s, f"{OA}publisherName", g("publisher")[i])
            w.lit(s, f"{OA}language", g("language_code")[i])
            w.lit(s, f"{OA}accessRight", g("bestaccessright_label")[i])
            # reified authorship: the rank is the point — "second author of X"
            for a in (jload(g("author_json")[i]) or [])[:200]:
                if not isinstance(a, dict):
                    continue
                name = a.get("fullname") or a.get("surname")
                if not name:
                    continue
                orcid = None
                for pid in (a.get("pid") or []) if isinstance(a.get("pid"), list) else []:
                    m = _ORCID_RE.search(str(pid))
                    if m:
                        orcid = m.group(1).upper()
                        break
                if not orcid:
                    m = _ORCID_RE.search(str(a.get("orcid") or ""))
                    orcid = m.group(1).upper() if m else None
                agent = f"<{ORCID}{orcid}>" if orcid else f'<{node(oid)}/agent/{a.get("rank") or 0}>'
                role = f'<{node(oid)}/role/{a.get("rank") or 0}>'
                w.t(s, f"{OA}hasAuthor", role)
                w.t(role, RDF_TYPE, f"<{OA}AuthorRole>")
                w.t(role, f"{OA}isHeldBy", agent)
                if a.get("rank") is not None:
                    w.lit(role, f"{OA}rank", a["rank"], f"{XSD}integer")
                w.t(agent, RDF_TYPE, f"<{OA}Agent>")
                w.lit(agent, f"{OA}agentName", name)
                if orcid:
                    w.lit(agent, f"{OA}orcid", orcid)
            c = jload(g("container_json")[i]) or {}
            if isinstance(c, dict) and c.get("name"):
                w.lit(s, f"{OA}venueName", c["name"])
                for key, prop in (("issnPrinted", "issn"), ("issnOnline", "issnOnline"),
                                  ("vol", "volume"), ("iss", "issue")):
                    w.lit(s, f"{OA}{prop}", c.get(key))
            subs = jload(g("subjects_json")[i]) or []
            for sub in subs[:60] if isinstance(subs, list) else []:
                v = sub.get("value") if isinstance(sub, dict) else sub
                w.lit(s, f"{DCT}subject", v)
            insts = jload(g("instance_json")[i]) or []
            for inst in insts[:20] if isinstance(insts, list) else []:
                if not isinstance(inst, dict):
                    continue
                for u in (inst.get("url") or [])[:5] if isinstance(inst.get("url"), list) else []:
                    iri = data_iri(u, AUDIT, "instance.url")
                    if iri:
                        w.t(s, f"{OA}fulltextUrl", f"<{iri}>")
            w.flush(out)


def emit_relation(w, out, args):
    for d, k in batches("relation", ["source_id", "target_id", "rel_name", "rel_type",
                                     "provenance", "trust"], args.batch, args.row_groups):
        g = lambda c: d.get(c, [None] * k)  # noqa: E731
        for i in range(k):
            s, t, name = g("source_id")[i], g("target_id")[i], g("rel_name")[i]
            if not s or not t or not name:
                continue
            prop = re.sub(r"[^A-Za-z0-9]", "", str(name))
            if not prop:
                continue
            w.t(f"<{node(s)}>", f"{OA}{prop[0].lower() + prop[1:]}", f"<{node(t)}>")
            w.flush(out)


def emit_organization(w, out, args):
    for d, k in batches("organization", ["id", "legalname", "legalshortname", "websiteurl",
                                         "country_code", "pid_json"], args.batch, args.row_groups):
        g = lambda c: d.get(c, [None] * k)  # noqa: E731
        for i in range(k):
            oid = g("id")[i]
            if not oid:
                continue
            ror = None
            for pid in (jload(g("pid_json")[i]) or []):
                if isinstance(pid, dict) and "ror" in str(pid.get("scheme", "")).lower():
                    m = _ROR_RE.search(str(pid.get("value", "")))
                    if m:
                        ror = m.group(1)
                        break
            s = f"<{ROR}{ror}>" if ror else f"<{node(oid)}>"
            w.t(s, RDF_TYPE, f"<{OA}Organization>")
            w.lit(s, f"{OA}openaireId", oid)
            for c, p in (("legalname", "legalName"), ("legalshortname", "legalShortName"),
                         ("country_code", "countryCode")):
                w.lit(s, f"{OA}{p}", g(c)[i])
            if g("legalname")[i]:
                w.lit(s, RDFS_LABEL, g("legalname")[i])
            if g("websiteurl")[i]:
                iri = data_iri(g("websiteurl")[i], AUDIT, "organization.websiteurl")
                if iri:
                    w.t(s, f"{OA}website", f"<{iri}>")
            w.flush(out)


def emit_project(w, out, args):
    for d, k in batches("project", ["id", "code", "acronym", "title", "startdate", "enddate",
                                    "funded_amount", "currency", "funding_json"],
                        args.batch, args.row_groups):
        g = lambda c: d.get(c, [None] * k)  # noqa: E731
        for i in range(k):
            oid = g("id")[i]
            if not oid:
                continue
            s = f"<{node(oid)}>"
            w.t(s, RDF_TYPE, f"<{OA}Project>")
            for c, p in (("code", "projectCode"), ("acronym", "acronym"), ("title", "title"),
                         ("startdate", "startDate"), ("enddate", "endDate"),
                         ("currency", "currency")):
                w.lit(s, f"{OA}{p}", g(c)[i])
            if g("title")[i]:
                w.lit(s, RDFS_LABEL, g("title")[i])
            if g("funded_amount")[i] is not None:
                w.lit(s, f"{OA}fundedAmount", g("funded_amount")[i], f"{XSD}decimal")
            for f in (jload(g("funding_json")[i]) or []):
                if isinstance(f, dict) and f.get("funder"):
                    w.lit(s, f"{OA}funderName", f["funder"])
            w.flush(out)


def emit_datasource(w, out, args):
    for d, k in batches("datasource", ["id", "officialname", "englishname", "websiteurl",
                                       "datasourcetype_value", "openairecompatibility"],
                        args.batch, args.row_groups):
        g = lambda c: d.get(c, [None] * k)  # noqa: E731
        for i in range(k):
            oid = g("id")[i]
            if not oid:
                continue
            s = f"<{node(oid)}>"
            w.t(s, RDF_TYPE, f"<{OA}Datasource>")
            for c, p in (("officialname", "officialName"), ("englishname", "englishName"),
                         ("datasourcetype_value", "datasourceType"),
                         ("openairecompatibility", "openaireCompatibility")):
                w.lit(s, f"{OA}{p}", g(c)[i])
            if g("officialname")[i]:
                w.lit(s, RDFS_LABEL, g("officialname")[i])
            if g("websiteurl")[i]:
                iri = data_iri(g("websiteurl")[i], AUDIT, "datasource.websiteurl")
                if iri:
                    w.t(s, f"{OA}website", f"<{iri}>")
            w.flush(out)


ENTITIES = ["publication", "dataset", "software", "otherresearchproduct",
            "relation", "organization", "project", "datasource"]

# One tally for the whole run, reported on stderr by main().
AUDIT = IriAudit()


def main():
    global ROOT
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--only", choices=ENTITIES, help="convert ONE entity (shard a parallel build)")
    ap.add_argument("--batch", type=int, default=20000)
    ap.add_argument("--row-groups", type=int, default=0, help="stop after N batches (sampling)")
    ap.add_argument("--parquet-root", default=os.environ.get("OPENAIRE_PARQUET_ROOT"),
                    help="directory holding parquet-<entity>/ (default: the script's "
                         "grandparent, which is data/openaire when run from there)")
    args = ap.parse_args()
    if args.parquet_root:
        ROOT = os.path.abspath(args.parquet_root)
    out = sys.stdout.buffer
    w = W()
    todo = [args.only] if args.only else ENTITIES
    for e in todo:
        if e in TYPES:
            emit_result(w, out, e, args)
        elif e == "relation":
            emit_relation(w, out, args)
        elif e == "organization":
            emit_organization(w, out, args)
        elif e == "project":
            emit_project(w, out, args)
        elif e == "datasource":
            emit_datasource(w, out, args)
    w.flush(out, force=True)
    AUDIT.report()
    print(f"DONE: {w.n:,} triples emitted", file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
