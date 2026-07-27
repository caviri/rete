#!/usr/bin/env python3
"""Emit RDF-star N-Triples from the GH Archive Parquet tables for one UTC day.

Usage (Docker-only, from the repo root):
    MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
      -e DAY=2025-07-22 python:3.12-slim bash -c \
      "pip -q install duckdb pyarrow && python data/github-archive/scripts/to_rdf.py"

Writes:
    data/github-archive/rdf/gharchive-ontology.nt   (TBox, once)
    data/github-archive/rdf/gharchive-<DAY>.nt      (ABox for that day)

Model:
  - entity IRIs are real GitHub URLs: https://github.com/<login>,
    https://github.com/<owner>/<repo>, .../pull/<n>, .../issues/<n>,
    .../commit/<sha>, .../releases/tag/<tag>
  - every event is a node <https://w3id.org/rete/gharchive/event/<id>> typed
    gh:<EventType> (subclass of prov:Activity) with prov:atTime,
    prov:wasAssociatedWith (actor) and gh:repo
  - RDF-star provenance:
      * volatile repo metadata observed in embedded payload snapshots
        (stars/forks/open issues/pushed-at) is asserted plainly AND annotated
        << s p o >> prov:generatedAtTime t ; prov:wasGeneratedBy event
      * social edges gh:starred / gh:forked are annotated the same way
  - commit author emails are deliberately NOT emitted (kept in Parquet only)

The emitter streams; RAM is bounded by per-day dedup sets of entity names.
"""
import gzip
import os
import pathlib
import sys
import traceback
import urllib.parse

import duckdb

GZ = os.environ.get("GZIP") == "1"   # write .nt.gz (month-scale disk saver)

DAY = os.environ.get("DAY", "2025-07-22")
BASE = pathlib.Path(__file__).resolve().parent.parent
DATA = BASE / "data"
RDF = BASE / "rdf"
RDF.mkdir(exist_ok=True)

GH = "https://w3id.org/rete/gharchive#"
EV = "https://w3id.org/rete/gharchive/event/"
PROV = "http://www.w3.org/ns/prov#"
RDFNS = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
SCHEMA = "https://schema.org/"
XSD = "http://www.w3.org/2001/XMLSchema#"

EVENT_TYPES = [
    "PushEvent", "PullRequestEvent", "IssuesEvent", "IssueCommentEvent",
    "WatchEvent", "ForkEvent", "CreateEvent", "DeleteEvent", "ReleaseEvent",
    "PullRequestReviewEvent", "PullRequestReviewCommentEvent", "MemberEvent",
    "CommitCommentEvent", "PublicEvent", "GollumEvent", "DiscussionEvent",
]


def esc_lit(s: str) -> str:
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def iri_seg(s: str) -> str:
    return urllib.parse.quote(s, safe="/-._~")


def gh_iri(*segs) -> str:
    return "<https://github.com/" + "/".join(iri_seg(s) for s in segs) + ">"


def lit(s, maxlen=None):
    s = str(s)
    if maxlen and len(s) > maxlen:
        s = s[:maxlen] + "…"
    return f'"{esc_lit(s)}"'


def dt(t) -> str:
    s = str(t).replace(" ", "T")
    if not s.endswith("Z") and "+" not in s:
        s += "Z"
    return f'"{s}"^^<{XSD}dateTime>'


def write_tbox(path: pathlib.Path) -> None:
    lines = []
    def t(s, p, o):
        lines.append(f"{s} {p} {o} .")
    ont = f"<{GH[:-1]}>"
    t(ont, f"<{RDFNS}type>", "<http://www.w3.org/2002/07/owl#Ontology>")
    t(ont, f"<{RDFS}label>", '"GH Archive events ontology"')
    for c, label, parent in (
        [("User", "GitHub user", f"<{PROV}Agent>"),
         ("Bot", "GitHub bot account", f"<{GH}User>"),
         ("Organization", "GitHub organization", f"<{PROV}Agent>"),
         ("Repository", "GitHub repository", None),
         ("PullRequest", "pull request", None),
         ("Issue", "issue", None),
         ("Commit", "commit", None),
         ("Release", "release", None)]
        + [(e, e.replace("Event", " event"), f"<{PROV}Activity>")
           for e in EVENT_TYPES]):
        t(f"<{GH}{c}>", f"<{RDFNS}type>", "<http://www.w3.org/2002/07/owl#Class>")
        t(f"<{GH}{c}>", f"<{RDFS}label>", f'"{label}"')
        if parent:
            t(f"<{GH}{c}>", f"<{RDFS}subClassOf>", parent)
    for p, label in [
        ("repo", "event happened in repository"), ("inRepo", "belongs to repository"),
        ("inOrg", "repository of organization"), ("action", "payload action"),
        ("starred", "starred"), ("forked", "forked"), ("forkOf", "is fork of"),
        ("pushedTo", "pushed to"), ("ref", "git ref"), ("refType", "ref type"),
        ("commit", "push contains commit"), ("authorName", "commit author name"),
        ("merged", "PR merged flag"), ("mergedBy", "merged by"),
        ("additions", "added lines"), ("deletions", "deleted lines"),
        ("changedFiles", "changed files"), ("author", "authored by"),
        ("label", "issue label"), ("tag", "release tag"),
        ("stars", "stargazer count (observed)"), ("forks", "fork count (observed)"),
        ("openIssues", "open issue count (observed)"),
        ("pushedAt", "last push (observed)"), ("sizeKB", "repo size in KB"),
        ("isFork", "repository is a fork"), ("archived", "repository archived"),
        ("defaultBranch", "default branch"), ("pullRequest", "event about PR"),
        ("issue", "event about issue"), ("release", "event about release"),
    ]:
        t(f"<{GH}{p}>", f"<{RDFS}label>", f'"{label}"')
    path.write_bytes(("\n".join(lines) + "\n").encode())


class Emitter:
    def __init__(self, out):
        self.out = out
        self.n = 0
        self.seen_users, self.seen_repos, self.seen_orgs = set(), set(), set()
        self.seen_pairs = set()   # (actor, repo) pushedTo dedup

    def t(self, s, p, o):
        self.out.write(f"{s} {p} {o} .\n".encode())
        self.n += 1

    def star(self, s, p, o, *annots):
        self.t(s, p, o)
        for ap, ao in annots:
            self.out.write(f"<< {s} {p} {o} >> {ap} {ao} .\n".encode())
            self.n += 1

    def user(self, login):
        if login and login not in self.seen_users:
            self.seen_users.add(login)
            u = gh_iri(login)
            cls = "Bot" if login.endswith("[bot]") or login.endswith("-bot") else "User"
            self.t(u, f"<{RDFNS}type>", f"<{GH}{cls}>")
            self.t(u, f"<{RDFS}label>", lit(login))
        return gh_iri(login)

    def repo(self, full_name, org=None):
        r = gh_iri(*full_name.split("/", 1))
        if full_name not in self.seen_repos:
            self.seen_repos.add(full_name)
            self.t(r, f"<{RDFNS}type>", f"<{GH}Repository>")
            self.t(r, f"<{RDFS}label>", lit(full_name))
        if org and org not in self.seen_orgs:
            self.seen_orgs.add(org)
            o = gh_iri(org)
            self.t(o, f"<{RDFNS}type>", f"<{GH}Organization>")
            self.t(o, f"<{RDFS}label>", lit(org))
        if org:
            self.t(r, f"<{GH}inOrg>", gh_iri(org))
        return r


def batches(con, sql):
    reader = con.execute(sql).fetch_record_batch(65536)
    for batch in reader:
        yield from batch.to_pylist()


def sec_events(em, con, q):
    for r in batches(con, q("events")):
            if not r["actor_login"] or not r["repo_name"]:
                continue
            ev = f"<{EV}{r['id']}>"
            actor = em.user(r["actor_login"])
            repo = em.repo(r["repo_name"], r["org_login"])
            em.t(ev, f"<{RDFNS}type>", f"<{GH}{r['type']}>")
            em.t(ev, f"<{PROV}atTime>", dt(r["created_at"]))
            em.t(ev, f"<{PROV}wasAssociatedWith>", actor)
            em.t(ev, f"<{GH}repo>", repo)
            if r["action"]:
                em.t(ev, f"<{GH}action>", lit(r["action"]))
            if r["ref"] and r["type"] in ("CreateEvent", "DeleteEvent", "PushEvent"):
                em.t(ev, f"<{GH}ref>", lit(r["ref"], 200))
            if r["ref_type"]:
                em.t(ev, f"<{GH}refType>", lit(r["ref_type"]))
            ann = ((f"<{PROV}atTime>", dt(r["created_at"])),
                   (f"<{PROV}wasGeneratedBy>", ev))
            if r["type"] == "WatchEvent":
                em.star(actor, f"<{GH}starred>", repo, *ann)
            elif r["type"] == "ForkEvent":
                em.star(actor, f"<{GH}forked>", repo, *ann)
                if r["forkee_full_name"]:
                    fork = em.repo(r["forkee_full_name"])
                    em.t(fork, f"<{GH}forkOf>", repo)
            elif r["type"] == "PushEvent":
                pair = (r["actor_login"], r["repo_name"])
                if pair not in em.seen_pairs:
                    em.seen_pairs.add(pair)
                    em.t(actor, f"<{GH}pushedTo>", repo)

def sec_commits(em, con, q):
    for r in batches(con, q("push_commits")):
            if not r["sha"] or not r["repo_name"]:
                continue
            c = gh_iri(*r["repo_name"].split("/", 1), "commit", r["sha"])
            em.t(c, f"<{RDFNS}type>", f"<{GH}Commit>")
            em.t(c, f"<{GH}inRepo>", em.repo(r["repo_name"]))
            em.t(f"<{EV}{r['event_id']}>", f"<{GH}commit>", c)
            if r["message"]:
                em.t(c, f"<{RDFS}comment>", lit(r["message"], 200))
            if r["author_name"]:
                em.t(c, f"<{GH}authorName>", lit(r["author_name"], 100))

def sec_prs(em, con, q):
    for r in batches(con, q("pull_requests")):
            if not r["number"] or not r["repo_name"]:
                continue
            pr = gh_iri(*r["repo_name"].split("/", 1), "pull", str(r["number"]))
            em.t(pr, f"<{RDFNS}type>", f"<{GH}PullRequest>")
            em.t(pr, f"<{GH}inRepo>", em.repo(r["repo_name"]))
            em.t(f"<{EV}{r['event_id']}>", f"<{GH}pullRequest>", pr)
            if r["title"]:
                em.t(pr, f"<{RDFS}label>", lit(r["title"], 300))
            if r["pr_author"]:
                em.t(pr, f"<{GH}author>", em.user(r["pr_author"]))
            if r["action"] == "closed" and r["merged"]:
                em.t(pr, f"<{GH}merged>", '"true"^^<' + XSD + "boolean>")
                if r["merged_by"]:
                    em.t(pr, f"<{GH}mergedBy>", em.user(r["merged_by"]))
            for k, p in (("additions", "additions"), ("deletions", "deletions"),
                         ("changed_files", "changedFiles")):
                if r[k] is not None:
                    em.t(pr, f"<{GH}{p}>", f'"{r[k]}"^^<{XSD}integer>')

def sec_issues(em, con, q):
    for r in batches(con, q("issues")):
            if not r["number"] or not r["repo_name"]:
                continue
            iss = gh_iri(*r["repo_name"].split("/", 1), "issues", str(r["number"]))
            em.t(iss, f"<{RDFNS}type>", f"<{GH}Issue>")
            em.t(iss, f"<{GH}inRepo>", em.repo(r["repo_name"]))
            em.t(f"<{EV}{r['event_id']}>", f"<{GH}issue>", iss)
            if r["title"]:
                em.t(iss, f"<{RDFS}label>", lit(r["title"], 300))
            if r["issue_author"]:
                em.t(iss, f"<{GH}author>", em.user(r["issue_author"]))
            for lab in (r["labels"] or []):
                if lab:
                    em.t(iss, f"<{GH}label>", lit(lab, 100))

def sec_releases(em, con, q):
    for r in batches(con, q("releases")):
            if not r["tag_name"] or not r["repo_name"]:
                continue
            rel = gh_iri(*r["repo_name"].split("/", 1), "releases", "tag",
                         r["tag_name"])
            em.t(rel, f"<{RDFNS}type>", f"<{GH}Release>")
            em.t(rel, f"<{GH}inRepo>", em.repo(r["repo_name"]))
            em.t(rel, f"<{GH}tag>", lit(r["tag_name"], 200))
            em.t(f"<{EV}{r['event_id']}>", f"<{GH}release>", rel)
            if r["release_name"]:
                em.t(rel, f"<{RDFS}label>", lit(r["release_name"], 200))

def sec_snapshots(em, con, q):
    # repo metadata snapshots: stable facts plain, volatile facts RDF-star
    for r in batches(con, q("repo_snapshots")):
            if not r["full_name"]:
                continue
            repo = em.repo(r["full_name"])
            ev = f"<{EV}{r['event_id']}>"
            if r["owner_type"] == "Organization" and r["owner_login"]:
                em.repo(r["full_name"], r["owner_login"])
            if r["description"]:
                em.t(repo, f"<{SCHEMA}description>", lit(r["description"], 500))
            if r["language"]:
                em.t(repo, f"<{SCHEMA}programmingLanguage>", lit(r["language"]))
            if r["license_spdx"] and r["license_spdx"] not in ("NOASSERTION",):
                em.t(repo, f"<{SCHEMA}license>",
                     f"<https://spdx.org/licenses/{iri_seg(r['license_spdx'])}>")
            if r["homepage"] and r["homepage"].startswith("http"):
                em.t(repo, f"<{SCHEMA}url>", lit(r["homepage"], 300))
            for topic in (r["topics"] or []):
                if topic:
                    em.t(repo, f"<{SCHEMA}keywords>", lit(topic, 100))
            if r["repo_created_at"]:
                em.t(repo, f"<{SCHEMA}dateCreated>", dt(r["repo_created_at"]))
            if r["is_fork"]:
                em.t(repo, f"<{GH}isFork>", f'"true"^^<{XSD}boolean>')
            if r["archived"]:
                em.t(repo, f"<{GH}archived>", f'"true"^^<{XSD}boolean>')
            if r["default_branch"]:
                em.t(repo, f"<{GH}defaultBranch>", lit(r["default_branch"], 100))
            ann = ((f"<{PROV}generatedAtTime>", dt(r["observed_at"])),
                   (f"<{PROV}wasGeneratedBy>", ev))
            for k, p in (("stars", "stars"), ("forks", "forks"),
                         ("open_issues", "openIssues"), ("size_kb", "sizeKB")):
                if r[k] is not None:
                    em.star(repo, f"<{GH}{p}>", f'"{r[k]}"^^<{XSD}integer>', *ann)
            if r["repo_pushed_at"]:
                em.star(repo, f"<{GH}pushedAt>", dt(r["repo_pushed_at"]), *ann)

SECTIONS = [("events", sec_events), ("commits", sec_commits),
            ("prs", sec_prs), ("issues", sec_issues),
            ("releases", sec_releases), ("snapshots", sec_snapshots)]


def main():
    con = duckdb.connect()
    con.execute("SET memory_limit='8GB'; SET temp_directory='/tmp/spill'; "
                "SET threads=2;")
    tbox = RDF / "gharchive-ontology.nt"
    if not tbox.exists():
        write_tbox(tbox)
    outdir = RDF / DAY
    outdir.mkdir(exist_ok=True)
    q = lambda tbl: f"SELECT * FROM '{(DATA / tbl).as_posix()}/{DAY}-*.parquet'"

    suffix = ".nt.gz" if GZ else ".nt"
    failed = False
    for name, fn in SECTIONS:
        final = outdir / f"{name}{suffix}"
        if final.exists() or (outdir / f"{name}.nt").exists():
            print(f"{name}: already emitted, skipping", flush=True)
            continue
        part = outdir / f"{name}{suffix}.part"
        opener = gzip.open(part, "wb", compresslevel=6) if GZ else open(part, "wb")
        with opener as f:
            em = Emitter(f)
            try:
                fn(em, con, q)
            except Exception:
                traceback.print_exc()
                print(f"SECTION {name} FAILED after {em.n:,} triples "
                      f"(left as {part.name})", flush=True)
                failed = True
                continue
        part.rename(final)
        print(f"{name}: {em.n:,} triples, "
              f"{os.path.getsize(final)/1e9:.2f} GB", flush=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
