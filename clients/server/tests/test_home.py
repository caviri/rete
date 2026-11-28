"""The home page shell is open, everything with data stays closed, and
``POST /api/uploads`` turns a file into a queryable dataset."""
from __future__ import annotations

import pytest
import rete_graph as rete

from conftest import PEOPLE, PEOPLE_NT, TOKEN, serve

BEARER = {"Authorization": f"Bearer {TOKEN}"}
TTL = """@prefix ex: <https://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:ada a ex:Person ; rdfs:label "Ada" ; ex:knows ex:bob .
ex:bob a ex:Person ; rdfs:label "Bob" .
"""
COUNT = "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"
SHELL = ["/", "/home/home.css", "/home/home.js", "/home/tools.js"]


# --------------------------------------------------------------------------- #
# The shell is open, nothing else is
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("path", SHELL)
def test_shell_loads_without_a_token(locked, path):
    r = locked.get(path)
    assert r.status_code == 200 and len(r.content) > 500
    assert TOKEN not in r.text and PEOPLE not in r.text  # nothing server-side is baked in


def test_shell_files_have_the_right_types(locked):
    assert locked.get("/").headers["content-type"].startswith("text/html")
    assert locked.get("/home/home.css").headers["content-type"].startswith("text/css")
    assert locked.get("/home/home.js").headers["content-type"].startswith("text/javascript")


@pytest.mark.parametrize("path", [
    "/api/datasets", f"/api/datasets/{PEOPLE}", f"/api/datasets/{PEOPLE}/card", f"/api/datasets/{PEOPLE}/schema",
    "/api/status", "/api/uploads", "/docs", "/openapi.json", "/mcp", "/generated/x.rete",
    "/home", "/home/", "/home/index.html", "/home/other.js", "/home/../app.py", "/home/%2e%2e/app.py",
    "/home/home.js/", "//home/home.js",
])
def test_data_and_everything_else_stays_closed(locked, path):
    assert locked.get(path).status_code == 401, path


def test_only_get_opens_the_shell(locked):
    assert locked.post("/", content=b"x").status_code == 401
    assert locked.post("/home/home.js", content=b"x").status_code == 401
    assert locked.get("/health").status_code == 200


def test_uploads_need_the_token(locked):
    r = locked.post("/api/uploads?name=a.ttl", content=TTL.encode())
    assert r.status_code == 401


def test_shell_query_token_is_not_needed_and_data_still_is():
    with serve(tokens=[TOKEN], query_token=True) as c:
        assert c.get("/").status_code == 200
        assert c.get("/api/datasets").status_code == 401
        assert c.get(f"/api/datasets?token={TOKEN}").status_code == 200


# --------------------------------------------------------------------------- #
# Uploads
# --------------------------------------------------------------------------- #

def test_upload_ttl_becomes_a_queryable_dataset():
    with serve(tokens=[TOKEN]) as c:
        r = c.post("/api/uploads?name=people.ttl", content=TTL.encode(), headers=BEARER)
        assert r.status_code == 200, r.text
        doc = r.json()
        assert doc["key"].startswith("generated/") and doc["key"].endswith(".rete")
        assert doc["statements"] == 5 and doc["label"] == "people.ttl" and doc["query"].startswith("SELECT")
        keys = [d["key"] for d in c.get("/api/datasets", headers=BEARER).json()]
        assert doc["key"] in keys
        q = c.post("/api/query", json={"dataset": doc["key"], "query": COUNT}, headers=BEARER).json()
        assert q["table"] == [{"n": 5}]
        starter = c.post("/api/query", json={"dataset": doc["key"], "query": doc["query"]}, headers=BEARER)
        assert starter.status_code == 200 and len(starter.json()["table"]) == 5


@pytest.mark.parametrize("name,fmt", [("a.nt", "nt"), ("a.nq", "nq")])
def test_upload_other_rdf_suffixes(name, fmt):
    body = "".join(line + "\n" for line in PEOPLE_NT.splitlines() if "<<" not in line)
    if fmt == "nq":
        body = body.replace(" .\n", " <http://ex.org/g> .\n")
    with serve() as c:
        r = c.post(f"/api/uploads?name={name}", content=body.encode())
        assert r.status_code == 200, r.text
        assert r.json()["statements"] == 3


def test_upload_a_rete_file_is_kept_as_is():
    data = rete.build(TTL, "ttl")
    with serve() as c:
        r = c.post("/api/uploads?name=ready.rete", content=data)
        assert r.status_code == 200, r.text
        assert r.json()["bytes"] == len(data)
        q = c.post("/api/query", json={"dataset": r.json()["key"], "query": COUNT}).json()
        assert q["table"] == [{"n": 5}]


def test_upload_name_is_reduced_to_a_file_name():
    with serve() as c:
        r = c.post("/api/uploads?name=..%2F..%2Fetc%2Fpeople.ttl", content=TTL.encode())
        assert r.status_code == 200 and r.json()["label"] == "people.ttl"


def test_upload_junk_is_a_400_that_says_why():
    with serve() as c:
        before = {d["key"] for d in c.get("/api/datasets").json()}
        r = c.post("/api/uploads?name=bad.ttl", content=b"this is not turtle at all {{{")
        assert r.status_code == 400 and "bad.ttl" in r.json()["detail"]
        r = c.post("/api/uploads?name=bad.rete", content=b"not a rete file")
        assert r.status_code == 400 and "bad.rete" in r.json()["detail"]
        r = c.post("/api/uploads?name=notes.txt", content=b"hello")
        assert r.status_code == 400 and "unknown file type" in r.json()["detail"]
        r = c.post("/api/uploads?name=empty.ttl", content=b"")
        assert r.status_code == 400
        r = c.post("/api/uploads?name=bin.ttl", content=b"\xff\xfe\x00\x01")
        assert r.status_code == 400
        # nothing junk was kept
        assert {d["key"] for d in c.get("/api/datasets").json()} == before


def test_upload_over_the_cap_is_a_413():
    with serve(upload_max_mb=1) as c:
        r = c.post("/api/uploads?name=big.ttl", content=b"#" * ((1 << 20) + 1))
        assert r.status_code == 413 and "1 MB" in r.json()["detail"]
        r = c.post("/api/uploads?name=big.rete", content=b"x" * ((1 << 20) + 1))
        assert r.status_code == 413


def test_upload_cap_holds_without_a_content_length():
    def chunks():
        for _ in range(3):
            yield b"#" * (512 << 10)

    with serve(upload_max_mb=1) as c:
        r = c.post("/api/uploads?name=big.ttl", content=chunks())  # chunked: no Content-Length
        assert r.status_code == 413


def test_upload_is_refused_when_authoring_is_off():
    with serve(author=False) as c:
        r = c.post("/api/uploads?name=a.ttl", content=TTL.encode())
        assert r.status_code == 403 and "disabled" in r.json()["detail"]
        info = c.get("/api/uploads").json()
        assert info["enabled"] is False and "off" in info["note"].lower()


def test_upload_info_lists_limits_and_formats():
    with serve(upload_max_mb=16) as c:
        info = c.get("/api/uploads").json()
        assert info["enabled"] is True and info["max_mb"] == 16
        assert {".ttl", ".nt", ".jsonld", ".rete", ".jelly"} <= set(info["formats"])
