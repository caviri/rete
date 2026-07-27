#!/usr/bin/env python3
"""Stream the hugging-face parquets -> N-Triples (all fields), per hugging-face.ttl.

IRI policy (scholar-aligned, see data/scholar/scholar.ttl):
  account       https://huggingface.co/{name}
  model         https://huggingface.co/{repo-id}
  dataset repo  https://huggingface.co/datasets/{repo-id}
  space         https://huggingface.co/spaces/{repo-id}
  paper         https://doi.org/10.48550/arxiv.{arxiv-id}   (lowercased DOI)
  post          its huggingface.co/posts/... URL

Left OUT of the graph (kept in the Parquet companions): cardData JSON blobs,
sibling file manifests, safetensors per-dtype breakdowns, BibTeX citation blobs,
AI summaries. Zero-valued counters are elided (absent = 0).

Usage:
  python build_nt.py > hugging-face.nt        # everything
  python build_nt.py --only models --limit 1000 > sample.nt
"""
import argparse
import json
import os
import re
import sys

import pyarrow.parquet as pq

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
HS = os.path.join(BASE, "raw", "hub-stats")
PQ = os.path.join(BASE, "parquet")

HF = "https://w3id.org/rete/huggingface#"
SCH = "https://schema.org/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD = "http://www.w3.org/2001/XMLSchema#"
HFCO = "https://huggingface.co/"

_IRI_BAD = re.compile(r'[\x00-\x20<>"{}|\\^`]')
_LIT = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_LIT_RE = re.compile(r'[\\"\n\r\t]')
_ARXIV = re.compile(r'^(\d{4}\.\d{4,5}(v\d+)?|[a-z-]+(\.[A-Z]{2})?/\d{7})$')

out = sys.stdout.buffer
n_emitted = 0


def ienc(s):
    return _IRI_BAD.sub(lambda m: "%%%02X" % ord(m.group()), str(s))


def lit(s):
    s = _LIT_RE.sub(lambda m: _LIT[m.group()], str(s))
    if any(ord(c) < 0x20 for c in s):
        s = "".join(c for c in s if ord(c) >= 0x20)
    return s


def emit(s, p, o):
    global n_emitted
    out.write(f"<{s}> <{p}> {o} .\n".encode("utf-8"))
    n_emitted += 1


def obj_iri(iri):
    return f"<{iri}>"


def s_lit(v):
    return f'"{lit(v)}"'


def i_lit(v):
    return f'"{int(v)}"^^<{XSD}integer>'


def d_lit(v):
    return f'"{v:g}"^^<{XSD}double>'


def b_lit(v):
    return f'"{"true" if v else "false"}"^^<{XSD}boolean>'


def dt_lit(ts):
    return f'"{ts.isoformat()}Z"^^<{XSD}dateTime>'


def acc(name):
    return HFCO + ienc(name)


def model_iri(rid):
    return HFCO + ienc(rid)


def ds_iri(rid):
    return HFCO + "datasets/" + ienc(rid)


def space_iri(rid):
    return HFCO + "spaces/" + ienc(rid)


def paper_iri(aid):
    return "https://doi.org/10.48550/arxiv." + ienc(str(aid).lower())


def avatar_iri(u):
    return u if u.startswith("http") else "https://huggingface.co" + u


def rows(path, columns, limit=0):
    pf = pq.ParquetFile(path)
    got = 0
    cols = [c for c in columns if c in pf.schema_arrow.names]
    for batch in pf.iter_batches(batch_size=8192, columns=cols):
        for row in batch.to_pylist():
            yield row
            got += 1
            if limit and got >= limit:
                return


def count_props(s, row, mapping):
    for col, prop in mapping:
        v = row.get(col)
        if v:
            emit(s, HF + prop, i_lit(v))


def emit_tags(s, tags):
    """license: -> schema:license; structured tags already edges -> skip; rest keywords."""
    for t in tags or []:
        if not t:
            continue
        if t.startswith(("dataset:", "arxiv:", "base_model:")):
            continue
        if t.startswith("license:"):
            emit(s, SCH + "license", s_lit(t[8:]))
        else:
            emit(s, SCH + "keywords", s_lit(t))


ACCOUNT_COUNTS = [
    ("num_models", "modelCount"), ("num_datasets", "datasetCount"),
    ("num_spaces", "spaceCount"), ("num_kernels", "kernelCount"),
    ("num_buckets", "bucketCount"), ("num_papers", "paperCount"),
    ("num_followers", "followerCount"),
]


def users(limit):
    for r in rows(os.path.join(PQ, "users.parquet"),
                  ["name", "fullname", "avatar_url", "is_pro", "num_models",
                   "num_datasets", "num_spaces", "num_kernels", "num_buckets",
                   "num_discussions", "num_papers", "num_upvotes", "num_likes",
                   "num_followers", "num_following"], limit):
        s = acc(r["name"])
        emit(s, RDF_TYPE, obj_iri(HF + "User"))
        emit(s, HF + "accountName", s_lit(r["name"]))
        if r.get("fullname"):
            emit(s, SCH + "name", s_lit(r["fullname"]))
        if r.get("avatar_url"):
            emit(s, SCH + "image", obj_iri(ienc(avatar_iri(r["avatar_url"]))))
        if r.get("is_pro"):
            emit(s, HF + "isPro", b_lit(True))
        count_props(s, r, ACCOUNT_COUNTS + [
            ("num_discussions", "discussionCount"), ("num_upvotes", "upvoteCount"),
            ("num_likes", "likeCount"), ("num_following", "followingCount")])


def orgs(limit):
    for r in rows(os.path.join(PQ, "orgs.parquet"),
                  ["name", "fullname", "avatar_url", "details", "is_verified",
                   "plan", "num_users", "num_models", "num_datasets", "num_spaces",
                   "num_kernels", "num_buckets", "num_papers", "num_followers"], limit):
        s = acc(r["name"])
        emit(s, RDF_TYPE, obj_iri(HF + "Organization"))
        emit(s, HF + "accountName", s_lit(r["name"]))
        if r.get("fullname"):
            emit(s, SCH + "name", s_lit(r["fullname"]))
        if r.get("details"):
            emit(s, SCH + "description", s_lit(r["details"]))
        if r.get("avatar_url"):
            emit(s, SCH + "image", obj_iri(ienc(avatar_iri(r["avatar_url"]))))
        if r.get("is_verified"):
            emit(s, HF + "isVerified", b_lit(True))
        if r.get("plan"):
            emit(s, HF + "plan", s_lit(r["plan"]))
        count_props(s, r, ACCOUNT_COUNTS + [("num_users", "memberCount")])


def repo_common(s, r, iri_fn):
    emit(s, HF + "repoId", s_lit(r["id"]))
    if r.get("author"):
        emit(s, HF + "author", obj_iri(acc(r["author"])))
    if r.get("createdAt"):
        emit(s, SCH + "dateCreated", dt_lit(r["createdAt"]))
    if r.get("lastModified"):
        emit(s, SCH + "dateModified", dt_lit(r["lastModified"]))
    if r.get("likes"):
        emit(s, HF + "likes", i_lit(r["likes"]))
    if r.get("trendingScore"):
        emit(s, HF + "trendingScore", d_lit(r["trendingScore"]))
    if r.get("downloads"):
        emit(s, HF + "downloads", i_lit(r["downloads"]))
    if r.get("downloadsAllTime"):
        emit(s, HF + "downloadsAllTime", i_lit(r["downloadsAllTime"]))
    if r.get("gated") and str(r["gated"]).lower() not in ("false", "none"):
        emit(s, HF + "gated", s_lit(str(r["gated"]).lower()))
    if r.get("private"):
        emit(s, HF + "private", b_lit(True))
    if r.get("sha"):
        emit(s, HF + "sha", s_lit(r["sha"]))
    emit_tags(s, r.get("tags"))


def models(limit):
    for r in rows(os.path.join(HS, "models.parquet"),
                  ["id", "author", "createdAt", "lastModified", "likes",
                   "trendingScore", "downloads", "downloadsAllTime", "gated",
                   "pipeline_tag", "library_name", "tags", "safetensors", "gguf",
                   "inferenceProviderMapping"], limit):
        s = model_iri(r["id"])
        emit(s, RDF_TYPE, obj_iri(HF + "Model"))
        repo_common(s, r, model_iri)
        if r.get("pipeline_tag"):
            emit(s, HF + "pipelineTag", s_lit(r["pipeline_tag"]))
        if r.get("library_name"):
            emit(s, HF + "library", s_lit(r["library_name"]))
        st = r.get("safetensors")
        if st and st.get("total"):
            emit(s, HF + "parameterCount", i_lit(st["total"]))
        if r.get("gguf"):
            emit(s, HF + "hasGguf", b_lit(True))
        seen = set()
        for m in r.get("inferenceProviderMapping") or []:
            p = m.get("provider")
            if p and p not in seen:
                seen.add(p)
                emit(s, HF + "inferenceProvider", s_lit(p))


def datasets(limit):
    for r in rows(os.path.join(HS, "datasets.parquet"),
                  ["id", "author", "createdAt", "lastModified", "likes",
                   "trendingScore", "downloads", "downloadsAllTime", "gated",
                   "private", "disabled", "sha", "description", "mainSize",
                   "paperswithcode_id", "tags"], limit):
        s = ds_iri(r["id"])
        emit(s, RDF_TYPE, obj_iri(HF + "DatasetRepo"))
        repo_common(s, r, ds_iri)
        if r.get("description"):
            emit(s, SCH + "description", s_lit(r["description"]))
        if r.get("mainSize"):
            emit(s, SCH + "contentSize", i_lit(r["mainSize"]))
        if r.get("paperswithcode_id"):
            emit(s, HF + "paperswithcodeId", s_lit(r["paperswithcode_id"]))


def spaces(limit):
    for r in rows(os.path.join(HS, "spaces.parquet"),
                  ["id", "author", "createdAt", "lastModified", "likes",
                   "trendingScore", "private", "sha", "subdomain", "sdk",
                   "tags"], limit):
        s = space_iri(r["id"])
        emit(s, RDF_TYPE, obj_iri(HF + "Space"))
        repo_common(s, r, space_iri)
        if r.get("sdk"):
            emit(s, HF + "sdk", s_lit(r["sdk"]))
        if r.get("subdomain"):
            emit(s, HF + "subdomain", s_lit(r["subdomain"]))


def papers(limit):
    for r in rows(os.path.join(HS, "arxiv_papers.parquet"),
                  ["id", "title", "thumbnailUrl", "upvotes", "publishedAt",
                   "authors", "summary", "projectPage", "githubRepo",
                   "organization"], limit):
        aid = str(r["id"])
        if not _ARXIV.match(aid):
            continue
        s = paper_iri(aid)
        emit(s, RDF_TYPE, obj_iri(HF + "Paper"))
        emit(s, HF + "doi", s_lit("10.48550/arxiv." + aid.lower()))
        emit(s, HF + "arxivId", s_lit(aid))
        emit(s, HF + "hfUrl", obj_iri(HFCO + "papers/" + ienc(aid)))
        if r.get("title"):
            emit(s, SCH + "name", s_lit(r["title"]))
        if r.get("summary"):
            emit(s, SCH + "abstract", s_lit(r["summary"]))
        if r.get("publishedAt"):
            emit(s, SCH + "datePublished", dt_lit(r["publishedAt"]))
        if r.get("upvotes"):
            emit(s, HF + "upvotes", i_lit(r["upvotes"]))
        if r.get("thumbnailUrl"):
            emit(s, SCH + "image", obj_iri(ienc(r["thumbnailUrl"])))
        if r.get("projectPage"):
            emit(s, HF + "projectPage", obj_iri(ienc(r["projectPage"])))
        if r.get("githubRepo"):
            g = r["githubRepo"]
            if not g.startswith("http"):
                g = "https://github.com/" + g
            emit(s, HF + "githubRepo", obj_iri(ienc(g)))
        if r.get("organization"):
            emit(s, HF + "organization", obj_iri(acc(r["organization"])))
        au = r.get("authors")
        if au:
            try:
                names = json.loads(au) if isinstance(au, str) else au
            except json.JSONDecodeError:
                names = []
            for nm in names:
                if isinstance(nm, str) and nm.strip():
                    emit(s, HF + "authorName", s_lit(nm.strip()))


def daily_papers(limit):
    for r in rows(os.path.join(HS, "daily_papers.parquet"),
                  ["paper_id", "submittedBy", "paper_authors"], limit):
        aid = str(r.get("paper_id") or "")
        if not _ARXIV.match(aid):
            continue
        s = paper_iri(aid)
        sub = r.get("submittedBy")
        if sub and sub.get("name"):
            emit(s, HF + "submittedBy", obj_iri(acc(sub["name"])))
        for a in r.get("paper_authors") or []:
            u = (a or {}).get("user")
            if u and u.get("name") and not (a.get("hidden") or False):
                emit(s, HF + "paperAuthor", obj_iri(acc(u["name"])))


def posts(limit):
    for r in rows(os.path.join(HS, "posts.parquet"),
                  ["url", "name", "publishedAt", "rawContent", "mentions",
                   "totalUniqueImpressions"], limit):
        if not r.get("url"):
            continue
        s = ienc(r["url"])
        emit(s, RDF_TYPE, obj_iri(HF + "Post"))
        if r.get("name"):
            emit(s, HF + "author", obj_iri(acc(r["name"])))
        if r.get("publishedAt"):
            emit(s, SCH + "datePublished", dt_lit(r["publishedAt"]))
        if r.get("rawContent"):
            emit(s, SCH + "text", s_lit(r["rawContent"]))
        if r.get("totalUniqueImpressions"):
            emit(s, HF + "impressions", i_lit(r["totalUniqueImpressions"]))
        for m in r.get("mentions") or []:
            if m.get("name"):
                emit(s, HF + "mentions", obj_iri(acc(m["name"])))


REL_PROP = {"finetune": "finetunedFrom", "quantized": "quantizedFrom",
            "adapter": "adapterFor", "merge": "mergedFrom"}


def edges(limit):
    for r in rows(os.path.join(PQ, "followers.parquet"),
                  ["follower", "followee"], limit):
        emit(acc(r["follower"]), HF + "follows", obj_iri(acc(r["followee"])))
    for r in rows(os.path.join(PQ, "following.parquet"),
                  ["follower", "followee"], limit):
        emit(acc(r["follower"]), HF + "follows", obj_iri(acc(r["followee"])))
    for r in rows(os.path.join(PQ, "org_members.parquet"), ["org", "user"], limit):
        emit(acc(r["user"]), HF + "memberOf", obj_iri(acc(r["org"])))
    for r in rows(os.path.join(PQ, "model_base_models.parquet"),
                  ["model_id", "relation", "base_model_id"], limit):
        prop = REL_PROP.get(r.get("relation") or "", "baseModel")
        emit(model_iri(r["model_id"]), HF + prop, obj_iri(model_iri(r["base_model_id"])))
    for r in rows(os.path.join(PQ, "model_datasets.parquet"),
                  ["model_id", "dataset_id"], limit):
        emit(model_iri(r["model_id"]), HF + "trainedOn", obj_iri(ds_iri(r["dataset_id"])))
    for r in rows(os.path.join(PQ, "space_links.parquet"),
                  ["space_id", "ref_type", "ref_id"], limit):
        if r["ref_type"] == "model":
            emit(space_iri(r["space_id"]), HF + "usesModel", obj_iri(model_iri(r["ref_id"])))
        else:
            emit(space_iri(r["space_id"]), HF + "usesDataset", obj_iri(ds_iri(r["ref_id"])))
    for r in rows(os.path.join(PQ, "repo_papers.parquet"),
                  ["repo_type", "repo_id", "arxiv_id"], limit):
        if not _ARXIV.match(str(r["arxiv_id"])):
            continue
        s = model_iri(r["repo_id"]) if r["repo_type"] == "model" else ds_iri(r["repo_id"])
        emit(s, HF + "citesPaper", obj_iri(paper_iri(r["arxiv_id"])))
    for r in rows(os.path.join(PQ, "paper_hf_authors.parquet"),
                  ["paper_id", "hf_user", "hidden"], limit):
        if r.get("hf_user") and not r.get("hidden") and _ARXIV.match(str(r["paper_id"])):
            emit(paper_iri(r["paper_id"]), HF + "paperAuthor", obj_iri(acc(r["hf_user"])))


SECTIONS = {"users": users, "orgs": orgs, "models": models, "datasets": datasets,
            "spaces": spaces, "papers": papers, "daily_papers": daily_papers,
            "posts": posts, "edges": edges}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", choices=sorted(SECTIONS), default=None)
    ap.add_argument("--limit", type=int, default=0, help="rows per table (sample)")
    args = ap.parse_args()
    todo = [args.only] if args.only else list(SECTIONS)
    for name in todo:
        before = n_emitted
        SECTIONS[name](args.limit)
        print(f"{name}: {n_emitted - before:,} triples", file=sys.stderr, flush=True)
    print(f"TOTAL: {n_emitted:,} triples", file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
