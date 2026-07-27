#!/usr/bin/env python3
"""Stream deps-dev Parquet -> N-Triples for one registry system (per deps-dev.ttl).

DuckDB reads/filters the Parquet (fast: the tables are clustered on System, so
WHERE System=? prunes). One ecosystem at a time keeps it disk-bounded and lets the
same converter scale to a sharded full build.

Env:  SYSTEM (default CARGO)  OUT (default /w/data/deps-dev/deps-dev-<sys>.nt)
IRIs: package version = https://deps.dev/<sys>/<name>/<version> ; deps:purl = join key.
"""
import json
import os
import re
import sys
from urllib.parse import quote

import duckdb


def jarr(s):
    """JSON string (from DuckDB to_json) -> list; [] for null/empty."""
    if not s:
        return []
    v = json.loads(s)
    return v if isinstance(v, list) else ([] if v is None else [v])


def jobj(s):
    """JSON string -> dict; None for null/empty."""
    if not s:
        return None
    v = json.loads(s)
    return v if isinstance(v, dict) else None

RAW = "/w/data/deps-dev/raw"
SYS = os.environ.get("SYSTEM", "CARGO")
OUT = os.environ.get("OUT", f"/w/data/deps-dev/deps-dev-{SYS.lower()}.nt")

DEPS = "https://w3id.org/rete/deps-dev#"
SCHEMA = "https://schema.org/"
DCT = "http://purl.org/dc/terms/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
XSD = "http://www.w3.org/2001/XMLSchema#"
SPDX = "http://spdx.org/licenses/"
SPDX_ID = re.compile(r"^[A-Za-z0-9.\-+]+$")
_BAD = ' <>"{}|\\^`'

# Optional name-hash sharding for a huge ecosystem (npm): CHUNK_MOD>1 splits by
# hash of the package name so a version and its out-edges land in the same chunk.
# Projects/advisories are emitted only in chunk 0 (federation merges them in).
CHUNK_MOD = int(os.environ.get("CHUNK_MOD", "0"))
CHUNK_IDX = int(os.environ.get("CHUNK_IDX", "0"))


def chunk_cond(col):
    return f" AND (hash({col}) % {CHUNK_MOD}) = {CHUNK_IDX}" if CHUNK_MOD > 1 else ""

con = duckdb.connect()
con.execute("PRAGMA threads=4")
w = sys.stdout if OUT == "-" else open(OUT, "w", encoding="utf-8", newline="")
N = [0]


def esc(s):
    return (str(s).replace("\\", "\\\\").replace('"', '\\"')
            .replace("\n", "\\n").replace("\r", "").replace("\t", "\\t"))


def L(s):
    return f'"{esc(s)}"'


def Ld(s, dt):
    return f'"{esc(s)}"^^<{dt}>'


def I(u):
    # Percent-encode every control/whitespace char (< 0x21) and NT delimiters;
    # leave real Unicode (>= 0x80) intact — NT IRIs allow it. This is what keeps a
    # junk value (e.g. a multi-line "homepage") from breaking the line.
    out = []
    for c in str(u):
        out.append("%%%02X" % ord(c) if ord(c) < 0x21 or c in '<>"{}|\\^`' else c)
    return "<" + "".join(out) + ">"


def pv(system, name, version):
    return f"https://deps.dev/{system.lower()}/{quote(str(name), safe='')}/{quote(str(version), safe='')}"


def proj_iri(ptype, name):
    base = {"GITHUB": "https://github.com/", "GITLAB": "https://gitlab.com/",
            "BITBUCKET": "https://bitbucket.org/"}.get(ptype)
    return base + str(name) if base else \
        f"https://w3id.org/rete/deps-dev/project/{quote(str(ptype) + '/' + str(name), safe='')}"


def adv_iri(sid):
    return f"https://w3id.org/rete/deps-dev/advisory/{quote(str(sid), safe='')}"


def T(s, p, o):
    w.write(f"{s} <{p}> {o} .\n")
    N[0] += 1


def stream(sql, fn):
    # STREAM in record batches (bounded memory) — materializing the whole stage
    # (fetch_arrow_table) OOMs on big ecosystems like npm. This is safe now that
    # every column is scalar via to_json(): DuckDB's earlier fetch crashes were
    # nested-column-specific ("integer cast" / ReplenishBuffer), which to_json
    # eliminated, so the streaming reader no longer trips them.
    reader = con.execute(sql).fetch_record_batch(50000)
    for batch in reader:
        cols = batch.schema.names
        for row in batch.to_pylist():
            fn(*(row[c] for c in cols))


# ---- 1. package-version nodes -------------------------------------------------
def emit_pv(system, name, version, purl, lics, vinfo, regs, published, advs, links):
    lics, vinfo, regs, advs, links = jarr(lics), jobj(vinfo), jarr(regs), jarr(advs), jarr(links)
    s = I(pv(system, name, version))
    T(s, RDF + "type", I(DEPS + "PackageVersion"))
    T(s, DEPS + "system", L(system))
    T(s, DEPS + "packageName", L(name))
    T(s, SCHEMA + "softwareVersion", L(version))
    if purl:
        T(s, DEPS + "purl", L(purl))
    for lic in (lics or []):
        if lic:
            T(s, SCHEMA + "license", I(SPDX + lic) if SPDX_ID.match(lic) else L(lic))
    if vinfo:
        if vinfo.get("IsRelease") is not None:
            T(s, DEPS + "isRelease", Ld("true" if vinfo["IsRelease"] else "false", XSD + "boolean"))
        if vinfo.get("Ordinal") is not None:
            T(s, DEPS + "versionOrdinal", Ld(vinfo["Ordinal"], XSD + "integer"))
    for reg in (regs or []):
        if reg:
            T(s, DEPS + "registry", I(reg))
    if published:
        T(s, SCHEMA + "datePublished", Ld(str(published).replace(" ", "T"), XSD + "dateTime"))
    for lk in (links or []):
        lbl, url = lk.get("Label"), lk.get("URL")
        if url and lbl == "SOURCE_REPO":
            T(s, SCHEMA + "codeRepository", I(url))
        elif url and lbl == "HOMEPAGE":
            T(s, SCHEMA + "url", I(url))
    for a in (advs or []):
        if a.get("SourceID"):
            T(s, DEPS + "hasAdvisory", I(adv_iri(a["SourceID"])))


# ---- 2. dependency edges ------------------------------------------------------
def emit_edge(system, fname, fver, dsys, tname, tver):
    T(I(pv(system, fname, fver)), DEPS + "dependsOn", I(pv(dsys, tname, tver)))


# ---- 3. package -> project links ----------------------------------------------
def emit_p2p(system, name, version, ptype, pname):
    if pname:
        T(I(pv(system, name, version)), DEPS + "hasProject", I(proj_iri(ptype, pname)))


# ---- 4. project metadata (only projects referenced by this ecosystem) ---------
def emit_project(ptype, name, stars, forks, issues, homepage, lics, desc):
    lics = jarr(lics)
    s = I(proj_iri(ptype, name))
    T(s, RDF + "type", I(DEPS + "Project"))
    T(s, SCHEMA + "name", L(name))
    if stars is not None:
        T(s, DEPS + "starsCount", Ld(stars, XSD + "integer"))
    if forks is not None:
        T(s, DEPS + "forksCount", Ld(forks, XSD + "integer"))
    if issues is not None:
        T(s, DEPS + "openIssuesCount", Ld(issues, XSD + "integer"))
    if homepage:
        T(s, SCHEMA + "url", I(homepage))
    for lic in (lics or []):
        if lic:
            T(s, SCHEMA + "license", I(SPDX + lic) if SPDX_ID.match(lic) else L(lic))
    if desc:
        T(s, DCT + "description", L(desc))


# ---- 5. advisories affecting this ecosystem -----------------------------------
def emit_advisory(source, sid, url, title, desc, severity, cvss, aliases, disclosed):
    aliases = jarr(aliases)
    s = I(adv_iri(sid))
    T(s, RDF + "type", I(DEPS + "Advisory"))
    if source:
        T(s, DEPS + "advisorySource", L(source))
    if title:
        T(s, DCT + "title", L(title))
    if desc:
        T(s, DCT + "description", L(desc))
    if severity:
        T(s, DEPS + "severity", L(severity))
    if cvss is not None:
        T(s, DEPS + "cvss3Score", Ld(cvss, XSD + "decimal"))
    if url:
        T(s, SCHEMA + "url", I(url))
    if disclosed:
        T(s, DCT + "date", Ld(str(disclosed).replace(" ", "T"), XSD + "dateTime"))
    for al in (aliases or []):
        if al:
            T(s, DEPS + "alias", L(al))


print(f"[{SYS}] 1/5 package versions ...", file=sys.stderr, flush=True)
stream(f"""SELECT System,Name,Version,Purl,
                  to_json(Licenses) AS Licenses, to_json(VersionInfo) AS VersionInfo,
                  to_json(Registries) AS Registries,
                  CAST(UpstreamPublishedAt AS VARCHAR) AS UpstreamPublishedAt,
                  to_json(Advisories) AS Advisories, to_json(Links) AS Links
           FROM read_parquet('{RAW}/PackageVersions.parquet')
           WHERE System='{SYS}'{chunk_cond('Name')}""",
       emit_pv)
print(f"  {N[0]:,} triples", file=sys.stderr, flush=True)

print(f"[{SYS}] 2/5 dependency edges ...", file=sys.stderr, flush=True)
stream(f"""SELECT System,from_name,from_version,dep_system,to_name,to_version
           FROM read_parquet('{RAW}/dependency_edges.parquet')
           WHERE System='{SYS}'{chunk_cond('from_name')}""",
       emit_edge)
print(f"  {N[0]:,} triples", file=sys.stderr, flush=True)

print(f"[{SYS}] 3/5 package->project ...", file=sys.stderr, flush=True)
stream(f"""SELECT System,Name,Version,ProjectType,ProjectName
           FROM read_parquet('{RAW}/PackageVersionToProject.parquet')
           WHERE System='{SYS}'{chunk_cond('Name')}""",
       emit_p2p)
print(f"  {N[0]:,} triples", file=sys.stderr, flush=True)

if CHUNK_MOD <= 1 or CHUNK_IDX == 0:  # projects/advisories once (chunk 0)
    print(f"[{SYS}] 4/5 projects ...", file=sys.stderr, flush=True)
    stream(f"""SELECT DISTINCT p.Type,p.Name,p.StarsCount,p.ForksCount,p.OpenIssuesCount,
                      p.Homepage,p.Licenses,p.Description
               FROM (SELECT Type,Name,StarsCount,ForksCount,OpenIssuesCount,Homepage,
                            to_json(Licenses) AS Licenses,Description
                     FROM read_parquet('{RAW}/Projects.parquet')) p
               JOIN (SELECT DISTINCT ProjectType,ProjectName
                     FROM read_parquet('{RAW}/PackageVersionToProject.parquet')
                     WHERE System='{SYS}') x
                 ON p.Type=x.ProjectType AND p.Name=x.ProjectName""",
           emit_project)
    print(f"  {N[0]:,} triples", file=sys.stderr, flush=True)

    print(f"[{SYS}] 5/5 advisories ...", file=sys.stderr, flush=True)
    stream(f"""SELECT Source,SourceID,SourceURL,Title,Description,Severity,CVSS3Score,
                      to_json(Aliases) AS Aliases, CAST(Disclosed AS VARCHAR) AS Disclosed
               FROM read_parquet('{RAW}/Advisories.parquet')
               WHERE len(list_filter(Packages, x -> x.System='{SYS}')) > 0""",
           emit_advisory)
w.close()
print(f"[{SYS}] DONE: {N[0]:,} triples -> {OUT}", file=sys.stderr, flush=True)
