"""Shared setup: small .rete files built by the installed rete-graph wheel in a
throwaway folder, and a factory for servers over them.

No test touches the network: the catalog is a local JSON file and every
dataset is a local file.
"""
from __future__ import annotations

import json
import pathlib
import tempfile
from contextlib import contextmanager

import pytest
import rete_graph as rete

from rete_server.settings import Settings

ROOT = pathlib.Path(tempfile.mkdtemp(prefix="rete-server-test-"))
DATA = ROOT / "data"
DATA.mkdir()
GENERATED = ROOT / "generated"

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

# A published catalog that points at a local file, as a self-hosted
# catalog.json beside its .rete files would.
(ROOT / "published").mkdir()
(ROOT / "published" / "people-copy.rete").write_bytes(rete.build(PEOPLE_NT, "nt"))
CATALOG = ROOT / "published" / "catalog.json"
CATALOG.write_text(json.dumps({"datasets": [{
    "key": "people", "label": "People", "description": "Alice and Bob.",
    "url": "people-copy.rete", "triples": 4,
    "examples": [{"title": "Names", "sparql": f"SELECT ?n WHERE {{ ?s {NAME} ?n }}"}],
}]}), encoding="utf-8")

# The query an agent wrote when it concluded "CONTAINS is unsupported":
# CONTAINS on an IRI is a type error, FILTER makes it false, zero rows.
CONTAINS_ON_IRI = f'SELECT ?s WHERE {{ ?s {NAME} ?n FILTER(CONTAINS(?s, "alice")) }}'

TOKEN = "test-token-0123456789"


def settings(**overrides) -> Settings:
    s = Settings(catalogs=[str(CATALOG)], data_dir=DATA, generated_dir=GENERATED,
                 catalog_refresh_s=3600)
    for key, value in overrides.items():
        setattr(s, key, value)
    return s


@contextmanager
def serve(**overrides):
    """A TestClient over a fresh app (its lifespan runs, as in production)."""
    from fastapi.testclient import TestClient

    from rete_server.app import create_app

    with TestClient(create_app(settings(**overrides))) as c:
        yield c


@pytest.fixture
def client():
    with serve() as c:
        yield c


@pytest.fixture
def locked():
    """A server that needs TOKEN."""
    with serve(tokens=[TOKEN]) as c:
        yield c
