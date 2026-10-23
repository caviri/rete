"""``rete.federate``: union over several graphs, routing, warnings."""

from __future__ import annotations

import warnings

import pytest

import rete_graph as rete

EX = "http://ex.org/"
A = f'<{EX}a1> <{EX}name> "Ann" .\n<{EX}shared> <{EX}name> "Both" .\n'
B = f'<{EX}b1> <{EX}name> "Bea" .\n<{EX}shared> <{EX}name> "Both" .\n<{EX}b1> <{EX}age> "7" .\n'
C = f'<{EX}c1> <{EX}other> "x" .\n'
Q = f"SELECT ?n WHERE {{ ?s <{EX}name> ?n }}"


@pytest.fixture(scope="module")
def graphs():
    return [rete.open(rete.build(t, "nt")) for t in (A, B, C)]


def names(res):
    return sorted(r["n"].value for r in res)


def test_select_unions_and_dedups(graphs):
    got = rete.federate(graphs[:2], Q)
    assert names(got.result) == ["Ann", "Bea", "Both"]
    assert [s["contributed"] for s in got.sources] == [2, 1]
    assert got.warnings == []


def test_routing_prunes_disjoint_source(graphs):
    got = rete.federate(graphs, Q)
    assert [s["pruned"] for s in got.sources] == [False, False, True]
    off = rete.federate(graphs, Q, route=False)
    assert not any(s["pruned"] for s in off.sources)
    assert names(off.result) == names(got.result)


def test_ask_is_or(graphs):
    ask = f'ASK {{ ?s <{EX}age> "7" }}'
    assert rete.federate(graphs, ask).result is True
    assert rete.federate(graphs[:1], ask).result is False


def test_accepts_paths(tmp_path):
    paths = []
    for i, t in enumerate((A, B)):
        p = tmp_path / f"{i}.rete"
        p.write_bytes(rete.build(t, "nt"))
        paths.append(str(p))
    assert names(rete.federate(paths, Q).result) == ["Ann", "Bea", "Both"]


def test_per_source_features_warn(graphs):
    with pytest.warns(rete.FederationWarning, match="limit"):
        got = rete.federate(graphs[:2], Q + " LIMIT 1")
    assert {w["kind"] for w in got.warnings} == {"limit"}
    with pytest.warns(rete.FederationWarning):
        agg = rete.federate(
            graphs[:2], f"SELECT (COUNT(*) AS ?c) WHERE {{ ?s <{EX}name> ?n }}"
        )
    assert {w["kind"] for w in agg.warnings} == {"aggregate"}


def test_one_source_is_silent(graphs):
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        got = rete.federate(graphs[:1], Q + " ORDER BY ?n LIMIT 1")
    assert got.warnings == []


def test_bad_query_raises(graphs):
    with pytest.raises(ValueError):
        rete.federate(graphs, "SELEKT nothing")
