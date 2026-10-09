"""Shared setup: a throwaway DATA_DIR with small .rete files built by the
installed rete-graph wheel, and the relay modules on sys.path.

Everything is configured through the environment BEFORE any relay module is
imported, because rete_service and app read DATA_DIR / RETE_CACHE_DIR /
CATALOG_FILE at import time. No test touches the network: the catalog is an
empty file and every dataset is a local file under DATA_DIR.
"""
from __future__ import annotations

import json
import os
import pathlib
import sys
import tempfile

import pytest

RELAY = pathlib.Path(__file__).resolve().parent.parent
ROOT = pathlib.Path(tempfile.mkdtemp(prefix="relay-test-"))
DATA = ROOT / "data"
DATA.mkdir()
CATALOG = ROOT / "catalog.json"
CATALOG.write_text(json.dumps({"datasets": []}), encoding="utf-8")

ENV = {
    "DATA_DIR": str(DATA),
    "RETE_CACHE_DIR": str(ROOT / "cache"),
    "CATALOG_FILE": str(CATALOG),
    "RETE_GENERATED_DIR": str(ROOT / "generated"),
}
os.environ.update(ENV)
os.environ.pop("ASK_MODEL", None)
os.environ.pop("JWT_TOKEN", None)
sys.path.insert(0, str(RELAY))

import rete_graph as rete  # noqa: E402

EX = "http://ex.org/"
ALICE, BOB, KNOWS, NAME = f"<{EX}alice>", f"<{EX}bob>", f"<{EX}knows>", f"<{EX}name>"
QUOTED_STAR = f"<<{ALICE} {KNOWS} {BOB}>>"  # rete's stored RDF-star token
QUOTED_RDF12 = f"<<( {ALICE} {KNOWS} {BOB} )>>"

# A quoted triple in OBJECT position only — what RDF 1.2 can spell.
PEOPLE_NT = f"""\
{ALICE} {NAME} "Alice" .
{BOB} {NAME} "Bob" .
{ALICE} {KNOWS} {BOB} .
<{EX}claim1> <{EX}asserts> << {ALICE} {KNOWS} {BOB} >> .
"""
# A quoted triple in SUBJECT position — RDF-star only, no RDF 1.2 spelling.
SUBJECT_NT = f"""\
<< {ALICE} {KNOWS} {BOB} >> <{EX}source> <{EX}wiki> .
"""

(DATA / "people.rete").write_bytes(rete.build(PEOPLE_NT, "nt"))
(DATA / "star-subject.rete").write_bytes(rete.build(SUBJECT_NT, "nt"))
PEOPLE = "local/people.rete"
STAR_SUBJECT = "local/star-subject.rete"

# The query an agent wrote when it concluded "CONTAINS is unsupported":
# CONTAINS on an IRI is a type error, FILTER makes it false, zero rows.
CONTAINS_ON_IRI = f'SELECT ?s WHERE {{ ?s {NAME} ?n FILTER(CONTAINS(?s, "alice")) }}'


@pytest.fixture(scope="session")
def client():
    from fastapi.testclient import TestClient

    import app

    with TestClient(app.app) as c:
        yield c
