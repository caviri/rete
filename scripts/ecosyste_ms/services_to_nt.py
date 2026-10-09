#!/usr/bin/env python3
"""ecosyste-ms-services JSON -> N-Triples on stdout. Pipe into `rete build -`.

Vocabulary `eco:` = https://w3id.org/rete/ecosystems# (scripts/ecosyste_ms/
ecosystems.ttl). Canonical instance IRIs, so the three ecosyste.ms .rete files
merge with no owl:sameAs and join to any other DOI/repo-keyed graph:

    project / repository   the source URL itself   https://github.com/o/r
    package                ECO/package/{ecosystem}/{name}
    agent (owner, sponsor) https://github.com/{login}
    collective             https://opencollective.com/{slug}
    advisory               ECO/advisory/{uuid}
    awesome list           its own url
    topic                  ECO/topic/{slug}

EVERY IRI THIS EMITS IS A VALID ABSOLUTE IRI. The upstream records carry
free-text URL fields (a sponsor's ``website``, an advisory's ``references``, a
topic's ``wikipedia_url``) and some of those values are not IRIs. One reached the
published file: ``https://::1`` — an IPv6 loopback written without the brackets
an IP-literal host requires, typed into a GitHub profile's website field. It has
a scheme and no forbidden character, so a prefix test passed it, `rete build`
stored it, and Oxigraph refused the exported dump on it.

Each free-text URL now goes through :func:`data_iri`, which does what the
OpenAIRE converter (scripts/openaire/parquet_to_nt.py) does and nothing more:

1. **escape** what the IRIREF grammar forbids but the IRI still means — a space,
   a ``[``/``]`` outside an IP-literal host, a second ``#``, a ``%`` that opens
   no escape. The escaped IRI denotes the same resource;
2. **drop** the statement — and count it — when the value is still not an
   absolute ``http``/``https`` IRI with a well-formed authority (host and
   port). ``https://::1`` names no host this converter could write without
   inventing one, so it is never rewritten to some other host.

The tally goes to stderr at the end of every run, with up to ten examples per
field. A clean run says so in one line.

Streaming emitter: one ``<s> <p> <o> .`` per line, written binary so no stray
CR ever reaches the parser.

Usage:
  python scripts/ecosyste_ms/services_to_nt.py --raw data/ecosyste-ms-services/raw \\
    | rete build - scripts/ecosyste_ms/ecosystems.ttl --strict \\
        --card-file scripts/ecosyste_ms/ecosyste-ms-services.card.json \\
        -o ecosyste-ms-services.rete
"""
from __future__ import annotations

import argparse
import ipaddress
import json
import re
import sys
from collections import Counter, defaultdict
from urllib.parse import quote

ECO = "https://w3id.org/rete/ecosystems#"
BASE = "https://w3id.org/rete/ecosystems/"
XSD = "http://www.w3.org/2001/XMLSchema#"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_SEEALSO = "http://www.w3.org/2000/01/rdf-schema#seeAlso"
SKOS = "http://www.w3.org/2004/02/skos/core#"
DCT_IDENTIFIER = "http://purl.org/dc/terms/identifier"

ESC = {ord("\\"): "\\\\", ord('"'): '\\"', ord("\n"): "\\n",
       ord("\r"): "\\r", ord("\t"): "\\t"}

# --------------------------------------------------------------- IRI validity
# Escaping (step 1) is the same transformation as scripts/openaire/
# parquet_to_nt.py:iri_escape and as `rete export --sanitize-iris`; the
# authority check (step 2) is the part of RFC 3987 `ihier-part` that a prefix
# test cannot see, and the part the `https://::1` value fails.
_HEX = "0123456789abcdefABCDEF"
_SCHEMES = ("http://", "https://")
# reg-name: iunreserved / pct-encoded / sub-delims. ucschar (>= U+00A0) is legal
# in an IRI and passes through untouched.
_REG_NAME_RE = re.compile(
    r"^(?:[A-Za-z0-9\-._~!$&'()*+,;= -\U0010ffff]|%[0-9A-Fa-f]{2})*$")
_USERINFO_RE = re.compile(
    r"^(?:[A-Za-z0-9\-._~!$&'()*+,;=: -\U0010ffff]|%[0-9A-Fa-f]{2})*$")
_IPVFUTURE_RE = re.compile(r"^[vV][0-9A-Fa-f]+\.[A-Za-z0-9\-._~!$&'()*+,;=:]+$")


def _ip_literal_brackets(b: bytes):
    """Byte offsets of an IP-literal host's ``[``/``]`` — the one legal pair."""
    for scheme in (b"http://", b"https://"):
        if b[: len(scheme)] == scheme:
            start = len(scheme)
            break
    else:
        return None
    end = len(b)
    for p in range(start, len(b)):
        if b[p] in (0x2F, 0x3F, 0x23):  # / ? #
            end = p
            break
    at = b.rfind(b"@", start, end)
    if at >= 0:
        start = at + 1
    if b[start: start + 1] != b"[":
        return None
    close = b.find(b"]", start, end)
    return None if close < 0 else (start, close)


def iri_escape(s: str) -> str:
    """Percent-encode every IRIREF defect escaping CAN repair.

    Lossless in the sense that matters: the escaped form denotes the same
    resource. It does not and cannot supply a missing scheme or a host.
    """
    b = s.encode("utf-8", "surrogatepass")
    brackets = _ip_literal_brackets(b)
    out = bytearray()
    i = 0
    seen_hash = False
    while i < len(b):
        c = b[i]
        if c >= 0x80:  # RFC 3987 ucschar — legal, never touched
            out.append(c)
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
                out += b[i: i + 3]
                i += 3
                continue
            out += b"%25"
        else:
            out.append(c)
        i += 1
    return out.decode("utf-8", "surrogatepass")


def authority_defect(authority: str) -> str | None:
    """Why ``authority`` is not a valid RFC 3987 ``iauthority``, or ``None``.

    ``iauthority = [ iuserinfo "@" ] ihost [ ":" port ]`` with
    ``ihost = IP-literal / IPv4address / ireg-name``. A reg-name cannot hold a
    ``:``, so the first ``:`` after the host opens the port, which is digits
    only — which is exactly where ``::1`` fails: an empty host, then the port
    ``:1``, which is not digits. Same verdict, same place, as the RFC 3987
    parser rete's gate and Oxigraph use (oxiri: "Invalid character ':'").
    """
    userinfo, at, hostport = authority.rpartition("@")
    if at and not _USERINFO_RE.match(userinfo):
        return "userinfo"
    if hostport.startswith("["):
        close = hostport.find("]")
        if close < 0:
            return "unclosed IP-literal"
        literal, rest = hostport[1:close], hostport[close + 1:]
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
    # An EMPTY host is syntactically legal (`ireg-name` may be empty, which is
    # what makes `file:///x` an IRI), so `http:///x` passes here exactly as it
    # passes Oxigraph. This function judges syntax, the thing a strict loader
    # rejects, not whether a URL would resolve.
    if port and not port.isdigit():
        return "port"
    return None


class IriAudit:
    """What the converter did to the free-text URLs the records handed it."""

    def __init__(self) -> None:
        self.seen = 0
        self.escaped = Counter()
        self.skipped = Counter()
        self.dropped = Counter()
        self.reasons = defaultdict(Counter)
        self.samples = defaultdict(list)

    def report(self, stream=sys.stderr) -> None:
        drops = sum(self.dropped.values())
        print(
            f"IRI audit: {self.seen:,} free-text URL(s) seen, "
            f"{sum(self.escaped.values()):,} percent-escaped, "
            f"{sum(self.skipped.values()):,} skipped (not an http(s) URL — never "
            f"emitted, as before), "
            f"{drops:,} DROPPED (an http(s) URL that is not a valid IRI and that "
            f"no escaping repairs).",
            file=stream,
        )
        for field, n in self.escaped.most_common():
            print(f"  escaped  {n:>7}  {field}", file=stream)
        for field, n in self.skipped.most_common():
            print(f"  skipped  {n:>7}  {field}", file=stream)
        for field, n in self.dropped.most_common():
            why = ", ".join(f"{r}={c}" for r, c in self.reasons[field].most_common())
            print(f"  dropped  {n:>7}  {field}  ({why})", file=stream)
            for raw in self.samples[field]:
                print(f"             e.g. {raw!r}", file=stream)


def data_iri(value, audit: IriAudit, field: str) -> str | None:
    """A free-text URL from a record -> ``<iri>``, or ``None`` (dropped, counted).

    Non-string and empty values are absent data, not defects: they are neither
    seen nor counted, exactly as before.
    """
    if not value or not isinstance(value, str):
        return None
    raw = value.strip()
    if not raw:
        return None
    audit.seen += 1
    why = None
    if not raw.startswith(_SCHEMES):  # case-sensitive, as it always was
        why = "scheme"
    else:
        s = iri_escape(raw)
        rest = s.split("://", 1)[1]
        authority = re.split(r"[/?#]", rest, maxsplit=1)[0]
        why = authority_defect(authority)
    if why is None:
        if s != raw:
            audit.escaped[field] += 1
        return f"<{s}>"
    if why == "scheme":
        # A non-http(s) value (a bare domain, mailto:, prose) was always skipped
        # by this converter and still is. Counted apart from the drops, so the
        # tally separates "not a web URL" from "a broken one".
        audit.skipped[field] += 1
        return None
    audit.dropped[field] += 1
    audit.reasons[field][why] += 1
    if len(audit.samples[field]) < 10:
        audit.samples[field].append(raw)
    return None


def minted(path: str) -> str:
    """An IRI WE mint under a constant https prefix — valid by construction."""
    return f"<{path}>"


def lit(v, dt: str | None = None) -> str | None:
    if v is None or v == "":
        return None
    if isinstance(v, bool):
        return f'"{str(v).lower()}"^^<{XSD}boolean>'
    if isinstance(v, int):
        return f'"{v}"^^<{XSD}integer>'
    if isinstance(v, float):
        return f'"{v}"^^<{XSD}decimal>'
    s = str(v).translate(ESC)
    return f'"{s}"^^<{XSD}{dt}>' if dt else f'"{s}"'


def dt_lit(v):
    """dateTime literal; upstream stamps are ISO-8601 with a Z or offset."""
    if not v or not isinstance(v, str):
        return None
    return f'"{v.translate(ESC)}"^^<{XSD}dateTime>'


class Emitter:
    def __init__(self, out) -> None:
        self.out = out
        self.n = 0

    def __call__(self, s: str | None, p: str, o: str | None) -> None:
        if s and o:
            self.out.write(f"{s} <{p}> {o} .\n".encode("utf-8"))
            self.n += 1


def pkg_iri(ecosystem, name):
    if not ecosystem or not name:
        return None
    return minted(f"{BASE}package/{quote(str(ecosystem), safe='')}/{quote(str(name), safe='')}")


def load(path):
    try:
        with open(path, "rb") as fh:
            return json.load(fh)
    except Exception as e:  # noqa: BLE001
        print(f"skip {path}: {e}", file=sys.stderr)
        return []


def convert(raw: str, out, audit: IriAudit) -> int:
    emit = Emitter(out)

    def url(v, field):
        return data_iri(v, audit, field)

    def agent_iri(login, field):
        return url(f"https://github.com/{login}", field) if login else None

    def common(s, r, kind):
        """Fields shared by nearly every ecosyste.ms record."""
        emit(s, ECO + "name", lit(r.get("name")))
        emit(s, ECO + "description", lit(r.get("description")))
        emit(s, ECO + "createdAt", dt_lit(r.get("created_at")))
        emit(s, ECO + "updatedAt", dt_lit(r.get("updated_at")))
        emit(s, ECO + "lastSyncedAt", dt_lit(r.get("last_synced_at")))
        emit(s, ECO + "apiUrl", url(r.get("api_url"), f"{kind}.api_url"))
        emit(s, ECO + "htmlUrl", url(r.get("html_url"), f"{kind}.html_url"))

    # ------------------------------------------------------------ advisories
    for r in load(f"{raw}/advisories/advisories.json"):
        s = minted(f"{BASE}advisory/{quote(str(r.get('uuid')), safe='')}")
        emit(s, RDF_TYPE, f"<{ECO}Advisory>")
        emit(s, ECO + "name", lit(r.get("title")))
        emit(s, ECO + "description", lit(r.get("description")))
        emit(s, ECO + "severity", lit(r.get("severity")))
        emit(s, ECO + "cvssScore", lit(r.get("cvss_score")))
        emit(s, ECO + "cvssVector", lit(r.get("cvss_vector")))
        emit(s, ECO + "epssPercentage", lit(r.get("epss_percentage")))
        emit(s, ECO + "blastRadius", lit(r.get("blast_radius")))
        emit(s, ECO + "createdAt", dt_lit(r.get("published_at") or r.get("created_at")))
        emit(s, ECO + "updatedAt", dt_lit(r.get("updated_at")))
        emit(s, ECO + "htmlUrl", url(r.get("html_url"), "advisory.html_url"))
        emit(s, ECO + "sourceUrl", url(r.get("repository_url"), "advisory.repository_url"))
        for ident in r.get("identifiers") or []:
            emit(s, DCT_IDENTIFIER, lit(ident))
        for ref in (r.get("references") or [])[:20]:
            emit(s, RDFS_SEEALSO, url(ref, "advisory.references"))
        for p in r.get("packages") or []:
            pi = pkg_iri(p.get("ecosystem"), p.get("package_name"))
            if pi:
                emit(s, ECO + "affectsPackage", pi)
                emit(pi, RDF_TYPE, f"<{ECO}Package>")
                emit(pi, ECO + "ecosystem", lit(p.get("ecosystem")))
                emit(pi, ECO + "name", lit(p.get("package_name")))
                emit(pi, ECO + "purl", lit(p.get("purl")))

    # -------------------------------------------------------------- sponsors
    for r in load(f"{raw}/sponsors/accounts.json"):
        s = agent_iri(r.get("login"), "sponsor.login")
        if not s:
            continue
        emit(s, RDF_TYPE, f"<{ECO}SponsorAccount>")
        emit(s, ECO + "login", lit(r.get("login")))
        emit(s, ECO + "sponsorsCount", lit(r.get("sponsors_count")))
        emit(s, ECO + "activeSponsorsCount", lit(r.get("active_sponsors_count")))
        emit(s, ECO + "minimumSponsorshipAmount", lit(r.get("minimum_sponsorship_amount")))
        emit(s, ECO + "htmlUrl", url(r.get("html_url"), "sponsor.html_url"))
        emit(s, ECO + "lastSyncedAt", dt_lit(r.get("last_synced_at")))
        d = r.get("data") or {}
        emit(s, ECO + "name", lit(d.get("name")))
        emit(s, ECO + "description", lit(d.get("description")))
        emit(s, ECO + "agentKind", lit(d.get("kind")))
        emit(s, ECO + "company", lit(d.get("company")))
        emit(s, ECO + "location", lit(d.get("location")))
        emit(s, ECO + "followers", lit(d.get("followers")))
        emit(s, ECO + "homepage", url(d.get("website"), "sponsor.website"))

    # -------------------------------------------------------- opencollective
    for r in load(f"{raw}/opencollective/collectives.json"):
        slug = r.get("slug")
        s = url(f"https://opencollective.com/{slug}", "collective.slug") if slug else None
        if not s:
            continue
        emit(s, RDF_TYPE, f"<{ECO}Collective>")
        common(s, r, "collective")
        emit(s, ECO + "currency", lit(r.get("currency")))
        emit(s, ECO + "totalDonations", lit(r.get("total_donations")))
        emit(s, ECO + "currentBalance", lit(r.get("current_balance")))
        emit(s, ECO + "homepage", url(r.get("website"), "collective.website"))
        emit(s, ECO + "sourceUrl", url(r.get("github"), "collective.github"))
        o = r.get("owner") or {}
        oi = agent_iri(o.get("login"), "collective.owner.login")
        if oi:
            emit(s, ECO + "ownedBy", oi)
            emit(oi, RDF_TYPE, f"<{ECO}Owner>")
            emit(oi, ECO + "login", lit(o.get("login")))
            emit(oi, ECO + "name", lit(o.get("name")))
            emit(oi, ECO + "agentKind", lit(o.get("kind")))
            emit(oi, ECO + "location", lit(o.get("location")))

    for r in load(f"{raw}/opencollective/projects.json"):
        s = url(r.get("url"), "ocproject.url")
        if not s:
            continue
        emit(s, RDF_TYPE, f"<{ECO}Project>")
        emit(s, ECO + "programmingLanguage", lit(r.get("language")))
        emit(s, ECO + "lastSyncedAt", dt_lit(r.get("last_synced_at")))
        emit(s, ECO + "htmlUrl", url(r.get("html_url"), "ocproject.html_url"))
        for k in (r.get("keywords") or [])[:30]:
            emit(s, ECO + "keyword", lit(k))
        c = r.get("collective") or {}
        if c.get("slug"):
            ci = url(f"https://opencollective.com/{c['slug']}", "ocproject.collective.slug")
            emit(ci, ECO + "fundsProject", s)
        rep = r.get("repository") or {}
        if rep.get("full_name"):
            ri = url(f"https://github.com/{rep['full_name']}", "ocproject.repository")
            emit(s, ECO + "hasRepository", ri)
            emit(ri, RDF_TYPE, f"<{ECO}Repository>")
            emit(ri, ECO + "name", lit(rep.get("full_name")))
            emit(ri, ECO + "description", lit(rep.get("description")))
            emit(ri, ECO + "stars", lit(rep.get("stargazers_count")))
            emit(ri, ECO + "forks", lit(rep.get("forks_count")))
            emit(ri, ECO + "archived", lit(rep.get("archived")))
            oi = agent_iri(rep.get("owner"), "ocproject.repository.owner")
            if oi:
                emit(ri, ECO + "ownedBy", oi)
                emit(oi, RDF_TYPE, f"<{ECO}Owner>")
                emit(oi, ECO + "login", lit(rep.get("owner")))

    # --------------------------------------------------------------- awesome
    for r in load(f"{raw}/awesome/lists.json"):
        s = url(r.get("url"), "awesomelist.url")
        if not s:
            continue
        emit(s, RDF_TYPE, f"<{ECO}AwesomeList>")
        common(s, r, "awesomelist")
        emit(s, ECO + "programmingLanguage", lit(r.get("primary_language")))
        for c in (r.get("categories") or [])[:60]:
            emit(s, ECO + "keyword", lit(c))

    for r in load(f"{raw}/awesome/topics.json"):
        slug = r.get("slug")
        s = minted(f"{BASE}topic/{quote(str(slug), safe='')}") if slug else None
        if not s:
            continue
        emit(s, RDF_TYPE, f"<{ECO}Topic>")
        emit(s, SKOS + "prefLabel", lit(r.get("name")))
        emit(s, ECO + "description", lit(r.get("short_description")))
        emit(s, ECO + "htmlUrl", url(r.get("html_url"), "topic.html_url"))
        emit(s, RDFS_SEEALSO, url(r.get("wikipedia_url"), "topic.wikipedia_url"))
        emit(s, ECO + "sourceUrl", url(r.get("github_url"), "topic.github_url"))
        for a in (r.get("aliases") or [])[:20]:
            emit(s, SKOS + "altLabel", lit(a))
        for t in (r.get("related_topics") or [])[:30]:
            emit(s, SKOS + "related", minted(f"{BASE}topic/{quote(str(t), safe='')}"))

    # ------------------------------------------------------------------- ost
    for r in load(f"{raw}/ost/projects.json"):
        s = url(r.get("url"), "ostproject.url")
        if not s:
            continue
        emit(s, RDF_TYPE, f"<{ECO}Project>")
        common(s, r, "ostproject")
        emit(s, ECO + "programmingLanguage", lit(r.get("language")))
        emit(s, ECO + "rank", lit(r.get("score")))
        emit(s, ECO + "monthlyDownloads", lit(r.get("monthly_downloads")))
        emit(s, ECO + "dependentRepositories", lit(r.get("total_dependent_repos")))
        emit(s, ECO + "dependentPackages", lit(r.get("total_dependent_packages")))
        for k in (r.get("keywords") or [])[:30]:
            emit(s, ECO + "keyword", lit(k))
        if r.get("category"):
            emit(s, ECO + "inField", minted(f"{BASE}field/{quote(str(r['category']), safe='')}"))
        rep = r.get("repository") or {}
        if rep.get("full_name"):
            ri = url(f"https://github.com/{rep['full_name']}", "ostproject.repository")
            emit(s, ECO + "hasRepository", ri)
            emit(ri, RDF_TYPE, f"<{ECO}Repository>")
            emit(ri, ECO + "name", lit(rep.get("full_name")))
            emit(ri, ECO + "stars", lit(rep.get("stargazers_count")))
            emit(ri, ECO + "forks", lit(rep.get("forks_count")))
            emit(ri, ECO + "archived", lit(rep.get("archived")))
            oi = agent_iri(rep.get("owner"), "ostproject.repository.owner")
            if oi:
                emit(ri, ECO + "ownedBy", oi)
                emit(oi, RDF_TYPE, f"<{ECO}Owner>")
                emit(oi, ECO + "login", lit(rep.get("owner")))
        for p in (r.get("packages") or [])[:40]:
            pi = pkg_iri(p.get("ecosystem"), p.get("name"))
            if pi:
                emit(s, ECO + "hasPackage", pi)
                emit(pi, RDF_TYPE, f"<{ECO}Package>")
                emit(pi, ECO + "ecosystem", lit(p.get("ecosystem")))
                emit(pi, ECO + "name", lit(p.get("name")))

    return emit.n


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    ap.add_argument("--raw", default="data/ecosyste-ms-services/raw",
                    help="the harvest's raw/ directory (default: %(default)s)")
    args = ap.parse_args(argv)
    audit = IriAudit()
    n = convert(args.raw, sys.stdout.buffer, audit)
    sys.stdout.buffer.flush()
    audit.report()
    print(f"emitted {n} triples", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
