#!/usr/bin/env python3
"""Convert the harvested OSS GitHub Benchmark JSON into N-Triples.

Modelling rule: reuse Open-Pulse IRIs and terms wherever possible so the two graphs
merge into one. Repositories, GitHub organizations and people land on bare GitHub
URLs — the exact IRIs Open-Pulse mints — so shared entities become the *same node*.
Only benchmark-specific facts use the ossb: namespace (see ossbenchmark.ttl).

Entity model
  repository   <https://github.com/{org}/{repo}>   schema:SoftwareSourceCode
  organization <https://github.com/{org}>          org:Organization
  person       <https://github.com/{login}>        schema:Person
  institution  <https://w3id.org/rete/oss-benchmark/institution/{slug}>
                                                   org:Organization, ossb:Institution
  sector       <https://w3id.org/rete/oss-benchmark/sector/{Name}>  skos:Concept
"""
from __future__ import annotations

import gzip
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

BASE = Path(__file__).resolve().parents[2] / "data" / "digital-sustainability-oss-github-benchmark"
RAW = BASE / "raw"
OUT = BASE / "build"

OSSB = "https://w3id.org/rete/oss-benchmark#"
INST = "https://w3id.org/rete/oss-benchmark/institution/"
SECTOR = "https://w3id.org/rete/oss-benchmark/sector/"
SCHEMA = "http://schema.org/"
ORG = "http://www.w3.org/ns/org#"
PULSE = "https://open-pulse.epfl.ch/ontology#"
GME = "https://openpulse.science/git-metadata-extractor#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"
DCT = "http://purl.org/dc/terms/"

# The benchmark reports human license names; map to SPDX so the graph joins anything
# else that speaks SPDX. 'none'/'Other' carry no SPDX identity and are dropped.
SPDX = {
    "MIT License": "MIT",
    "Apache License 2.0": "Apache-2.0",
    "GNU General Public License v3.0": "GPL-3.0-only",
    "GNU General Public License v2.0": "GPL-2.0-only",
    "GNU Affero General Public License v3.0": "AGPL-3.0-only",
    "GNU Lesser General Public License v3.0": "LGPL-3.0-only",
    "GNU Lesser General Public License v2.1": "LGPL-2.1-only",
    'BSD 3-Clause "New" or "Revised" License': "BSD-3-Clause",
    'BSD 2-Clause "Simplified" License': "BSD-2-Clause",
    "BSD 3-Clause Clear License": "BSD-3-Clause-Clear",
    "BSD Zero Clause License": "0BSD",
    "Mozilla Public License 2.0": "MPL-2.0",
    "Creative Commons Attribution 4.0 International": "CC-BY-4.0",
    "Creative Commons Attribution Share Alike 4.0 International": "CC-BY-SA-4.0",
    "Creative Commons Zero v1.0 Universal": "CC0-1.0",
    "The Unlicense": "Unlicense",
    "ISC License": "ISC",
    "Eclipse Public License 1.0": "EPL-1.0",
    "Eclipse Public License 2.0": "EPL-2.0",
    "European Union Public License 1.2": "EUPL-1.2",
    "Open Data Commons Open Database License v1.0": "ODbL-1.0",
    "Open Software License 3.0": "OSL-3.0",
    "Boost Software License 1.0": "BSL-1.0",
    "Do What The F*ck You Want To Public License": "WTFPL",
    "SIL Open Font License 1.1": "OFL-1.1",
    "MIT No Attribution": "MIT-0",
    "Universal Permissive License v1.0": "UPL-1.0",
    "Academic Free License v3.0": "AFL-3.0",
    "zlib License": "Zlib",
}

ESCAPES = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_ESC_RE = re.compile(r'[\\"\n\r\t]')
_CTRL_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]")
_IRI_BAD = re.compile(r'[\x00-\x20<>"{}|\\^`]')


def esc(s: str) -> str:
    return _ESC_RE.sub(lambda m: ESCAPES[m.group()], _CTRL_RE.sub("", s))


def iri(u: str) -> str | None:
    """Percent-encode the characters N-Triples forbids; reject anything still broken."""
    u = u.strip()
    if not u or not u.startswith(("http://", "https://")):
        return None
    u = _IRI_BAD.sub(lambda m: "%%%02X" % ord(m.group()), u)
    return u


class Writer:
    def __init__(self, path: Path):
        self.f = gzip.open(path, "wt", encoding="utf-8", newline="\n")
        self.n = 0
        self.seen: set[int] = set()

    def raw(self, line: str) -> None:
        h = hash(line)
        if h in self.seen:
            return
        self.seen.add(h)
        self.f.write(line)
        self.n += 1

    def rel(self, s: str, p: str, o: str) -> None:
        if s and o:
            self.raw(f"<{s}> <{p}> <{o}> .\n")

    def lit(self, s: str, p: str, v, dt: str | None = None, lang: str | None = None) -> None:
        if not s or v is None:
            return
        if isinstance(v, bool):
            v, dt = ("true" if v else "false"), XSD + "boolean"
        elif isinstance(v, int):
            v, dt = str(v), XSD + "integer"
        else:
            v = str(v).strip()
            if not v:
                return
        tail = f"^^<{dt}>" if dt else (f"@{lang}" if lang else "")
        self.raw(f'<{s}> <{p}> "{esc(v)}"{tail} .\n')

    def close(self) -> None:
        self.f.close()


def dt(v: str | None) -> str | None:
    """Normalize the two timestamp shapes the API emits to xsd:dateTime."""
    if not v or not isinstance(v, str):
        return None
    v = v.strip().replace(" ", "T")
    if v.endswith("Z") and "." in v:
        v = v.split(".")[0] + "Z"
    return v if re.match(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}", v) else None


def slug(s: str) -> str:
    """Institution shortname -> IRI path segment. Case-preserving: 'Swisstopo' and
    'swisstopo' are distinct institutions and must not collide."""
    return re.sub(r"[^A-Za-z0-9._~-]", lambda m: "%%%02X" % ord(m.group()), s.strip())


def load_json(p: Path):
    return json.loads(p.read_text(encoding="utf-8"))


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    w = Writer(OUT / "oss-github-benchmark.nt.gz")
    stats = defaultdict(int)

    latest = load_json(RAW / "latest_update.json").get("updatedDate")
    paged = load_json(RAW / "institutions_paginated.json")
    files = load_json(RAW / "institution_files.json")["shortname_to_file"]

    # ---------------------------------------------------------------- dataset node
    ds = "https://w3id.org/rete/oss-benchmark/dataset"
    w.rel(ds, RDF + "type", SCHEMA + "Dataset")
    w.lit(ds, SCHEMA + "name", "Digital Sustainability OSS GitHub Benchmark")
    w.lit(ds, SCHEMA + "description",
          "Open-source activity of Swiss institutions on GitHub, as measured by "
          "ossbenchmark.com (Bern University of Applied Sciences).")
    w.rel(ds, SCHEMA + "url", "https://ossbenchmark.com")
    w.rel(ds, SCHEMA + "codeRepository",
          "https://github.com/digital-sustainability/oss-github-benchmark")
    w.lit(ds, DCT + "modified", dt(latest), XSD + "dateTime")

    # ------------------------------------------------------------------- sectors
    for name, count in paged.get("sectors", {}).items():
        s = SECTOR + slug(name)
        w.rel(s, RDF + "type", SKOS + "Concept")
        w.rel(s, RDF + "type", OSSB + "Sector")
        w.rel(s, SKOS + "inScheme", "https://w3id.org/rete/oss-benchmark/sectors")
        w.lit(s, SKOS + "prefLabel", name.replace("_", " "), lang="en")
        w.lit(s, RDFS + "label", name.replace("_", " "), lang="en")
        w.lit(s, SKOS + "notation", name)
        stats["sectors"] += 1

    # -------------------------------------------------------------- institutions
    summary = {i["shortname"]: i for i in paged["institutions"]}
    known_institutions = set(files)
    org_of_inst: dict[str, set[str]] = defaultdict(set)

    # GitHub org names are case-insensitive, but IRIs are not. The institution
    # listing carries GitHub's canonical display casing (…/EPFL-IC) while the
    # repository records store a lowercased handle ('epfl-ic'). Minting both would
    # split one organization into two nodes and break institution -> org -> repo
    # (13 orgs affected). Canonicalize on the listing's casing, which is also the
    # casing Open-Pulse uses, so the graphs keep matching.
    canon_org: dict[str, str] = {}
    for sn, stem in files.items():
        f = RAW / "institutions" / f"{stem}.json"
        if not f.exists():
            continue
        for o in load_json(f).get("orgs") or []:
            nm = (o.get("name") or "").strip()
            u = iri(o.get("url") or (f"https://github.com/{nm}" if nm else ""))
            if u:
                canon_org[u.rsplit("/", 1)[-1].lower()] = u

    def org_iri(handle: str) -> str | None:
        """Resolve a GitHub org handle to its canonical IRI."""
        h = (handle or "").strip()
        if not h:
            return None
        return canon_org.get(h.lower()) or iri(f"https://github.com/{h}")

    for sn, stem in files.items():
        f = RAW / "institutions" / f"{stem}.json"
        if not f.exists():
            print(f"  WARN missing institution detail: {sn}", file=sys.stderr)
            continue
        d = load_json(f)
        su = summary.get(sn, {})
        i = INST + slug(sn)
        stats["institutions"] += 1

        w.rel(i, RDF + "type", ORG + "Organization")
        w.rel(i, RDF + "type", OSSB + "Institution")
        w.lit(i, OSSB + "shortName", sn)
        label = su.get("name_de") or sn
        w.lit(i, SCHEMA + "name", label)
        w.lit(i, RDFS + "label", label)
        w.lit(i, GME + "location", su.get("location"))
        w.lit(i, SCHEMA + "dateCreated", dt(su.get("created_at")), XSD + "dateTime")
        if av := iri(d.get("avatar") or su.get("avatar") or ""):
            w.rel(i, GME + "avatar_url", av)
        if sec := d.get("sector") or su.get("sector"):
            w.rel(i, OSSB + "sector", SECTOR + slug(sec))
        w.rel(i, SCHEMA + "includedInDataCatalog", ds)

        for key, prop in [
            ("num_repos", "numRepos"), ("num_orgs", "numOrgs"),
            ("num_members", "numMembers"), ("total_num_contributors", "totalContributors"),
            ("total_num_commits", "totalCommits"), ("total_num_stars", "totalStars"),
            ("total_num_watchers", "totalWatchers"),
            ("total_num_forks_in_repos", "totalForksInRepos"),
            ("total_issues", "totalIssues"), ("total_issues_closed", "totalIssuesClosed"),
            ("total_pull_requests", "totalPullRequests"),
            ("total_pull_requests_closed", "totalPullRequestsClosed"),
            ("total_comments", "totalComments"),
        ]:
            w.lit(i, OSSB + prop, d.get(key))

        # ---- GitHub organizations owned by this institution (many-to-many)
        for o in d.get("orgs") or []:
            name = (o.get("name") or "").strip()
            ou = iri(o.get("url") or (f"https://github.com/{name}" if name else ""))
            if not ou:
                continue
            org_of_inst[ou].add(i)
            w.rel(ou, RDF + "type", ORG + "Organization")
            w.rel(ou, ORG + "unitOf", i)
            w.lit(ou, SCHEMA + "name", name)
            w.lit(ou, RDFS + "label", name)
            w.lit(ou, PULSE + "githubOrganizationHandle", name)
            w.lit(ou, SCHEMA + "description", o.get("description"))
            w.lit(ou, GME + "location", o.get("locations"))
            w.lit(ou, SCHEMA + "email", o.get("email"))
            w.lit(ou, GME + "github_created_at", dt(o.get("created_at")), XSD + "dateTime")
            w.rel(ou, SCHEMA + "url", ou)
            if a := iri(o.get("avatar") or ""):
                w.rel(ou, GME + "avatar_url", a)
            stats["orgs"] += 1

    # ------------------------------------------------------------- repositories
    # A repository claimed by several institutions yields several records with
    # different crawl dates; collapse to one node and let the freshest supply metrics.
    best: dict[str, dict] = {}
    claims: dict[str, set[str]] = defaultdict(set)
    for f in sorted((RAW / "repositories").glob("page_*.json")):
        for r in load_json(f)["repositories"]:
            u = iri((r.get("url") or "").rstrip("/"))
            if not u:
                continue
            claims[u].add(r["institution"])
            prev = best.get(u)
            if prev is None or (r.get("timestamp") or "") > (prev.get("timestamp") or ""):
                best[u] = r
    stats["repo_records"] = sum(len(v) for v in claims.values())
    stats["repo_multi_claimed"] = sum(1 for v in claims.values() if len(v) > 1)

    for u, r in best.items():
        stats["repositories"] += 1
        w.rel(u, RDF + "type", SCHEMA + "SoftwareSourceCode")
        w.lit(u, SCHEMA + "name", r.get("name"))
        w.lit(u, RDFS + "label", r.get("name"))
        w.lit(u, SCHEMA + "description", r.get("description"))
        w.rel(u, SCHEMA + "url", u)
        w.rel(u, GME + "html_url", u)
        w.lit(u, SCHEMA + "identifier", r.get("uuid"))
        w.lit(u, PULSE + "githubRepositoryHandle",
              f"{r.get('organization')}/{r.get('name')}" if r.get("organization") else None)
        created = dt(r.get("created_at"))
        w.lit(u, SCHEMA + "dateCreated", created, XSD + "dateTime")
        w.lit(u, GME + "github_created_at", created, XSD + "dateTime")
        updated = dt(r.get("updated_at"))
        w.lit(u, SCHEMA + "dateModified", updated, XSD + "dateTime")
        w.lit(u, GME + "github_updated_at", updated, XSD + "dateTime")
        w.lit(u, OSSB + "crawledAt", dt(r.get("timestamp")), XSD + "dateTime")

        w.lit(u, PULSE + "githubRepoStars", r.get("num_stars"))
        w.lit(u, PULSE + "githubRepoForks", r.get("num_forks"))
        w.lit(u, GME + "watchers_count", r.get("num_watchers"))
        w.lit(u, GME + "archived", bool(r.get("archived")))
        # 'fork' arrives as the STRING 'true'/'false', not a JSON boolean.
        w.lit(u, OSSB + "isFork", str(r.get("fork")).lower() == "true")

        for key, prop in [
            ("num_commits", "numCommits"), ("num_contributors", "numContributors"),
            ("has_own_commits", "hasOwnCommits"), ("issues_all", "issuesAll"),
            ("issues_closed", "issuesClosed"), ("pull_requests_all", "pullRequestsAll"),
            ("pull_requests_closed", "pullRequestsClosed"), ("comments", "comments"),
        ]:
            w.lit(u, OSSB + prop, r.get(key))

        lic = r.get("license")
        if lic and lic not in ("none", "Other"):
            w.lit(u, GME + "license_name", lic)
            if spdx := SPDX.get(lic):
                w.rel(u, SCHEMA + "license", f"https://spdx.org/licenses/{spdx}")
            else:
                stats["license_unmapped"] += 1
        if logo := iri(r.get("logo") or ""):
            w.rel(u, GME + "avatar_url", logo)

        if org := (r.get("organization") or "").strip():
            ou = org_iri(org)
            if ou:
                w.rel(u, PULSE + "ownedBy", ou)
                w.rel(ou, PULSE + "owns", u)
                # Repos reference orgs the institution listing never mentioned.
                w.rel(ou, RDF + "type", ORG + "Organization")
                if ou not in org_of_inst:
                    w.lit(ou, SCHEMA + "name", org)
                    w.lit(ou, PULSE + "githubOrganizationHandle", org)
                    org_of_inst[ou] = set()
                    stats["orgs_from_repos"] += 1
        # Repository records reference 11 institution keys that the institution
        # listing does not contain ('appuio', 'Bertschi', 'Winterthur', …) — stale or
        # renamed keys upstream, 286 records. Pointing attributedToInstitution at a
        # node that is never typed ossb:Institution would leave dangling references
        # (SHACL sh:class flags exactly these), so keep the raw key as a literal
        # instead of inventing an institution.
        for sn in claims[u]:
            if sn in known_institutions:
                w.rel(u, OSSB + "attributedToInstitution", INST + slug(sn))
            else:
                w.lit(u, OSSB + "unlistedInstitutionKey", sn)
                stats["orphan_institution_edges"] += 1

    # -------------------------------------------------------------------- people
    # As with repositories, upstream stores one document per (user, claiming
    # institution), so ~0.2% of logins appear twice with different crawl states.
    # Collapse to the freshest, otherwise a person ends up with two conflicting
    # followers_count / public_repos values.
    people: dict[str, dict] = {}
    for f in sorted((RAW / "users").glob("page_*.json")):
        for p in load_json(f)["users"]:
            login = (p.get("login") or "").strip()
            if not login:
                continue
            stats["user_records"] += 1
            prev = people.get(login)
            if prev is None or (p.get("updated_at") or "") > (prev.get("updated_at") or ""):
                people[login] = p
    stats["people_duplicate_logins"] = stats["user_records"] - len(people)

    for login, p in people.items():
        pu = iri(f"https://github.com/{login}")
        if not pu:
            continue
        stats["people"] += 1
        w.rel(pu, RDF + "type", SCHEMA + "Person")
        w.lit(pu, PULSE + "githubUsername", login)
        w.lit(pu, SCHEMA + "name", p.get("name") or login)
        w.lit(pu, RDFS + "label", p.get("name") or login)
        w.rel(pu, SCHEMA + "url", pu)
        w.lit(pu, GME + "company", p.get("company"))
        w.lit(pu, GME + "location", p.get("location"))
        w.lit(pu, GME + "twitter_username", p.get("twitter_username"))
        w.lit(pu, GME + "followers_count", p.get("followers"))
        w.lit(pu, GME + "public_repos", p.get("public_repos"))
        w.lit(pu, OSSB + "publicGists", p.get("public_gists"))
        w.lit(pu, GME + "github_created_at", dt(p.get("created_at")), XSD + "dateTime")
        w.lit(pu, GME + "github_updated_at", dt(p.get("updated_at")), XSD + "dateTime")
        if a := iri(p.get("avatar_url") or ""):
            w.rel(pu, GME + "avatar_url", a)

    w.close()
    stats["orgs_total"] = len(org_of_inst)
    print(f"wrote {OUT / 'oss-github-benchmark.nt.gz'}  {w.n:,} triples")
    for k in sorted(stats):
        print(f"  {k:24} {stats[k]:,}")


if __name__ == "__main__":
    main()
