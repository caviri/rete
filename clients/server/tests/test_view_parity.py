"""views.py copies a few lists from the MCPB view's JavaScript by hand: the card
field types (cards.js), the hive axes and orders (hive.js) and the tabs a
query hint may name (clients/chrome/lib/hints.js). This test reads those files
and fails when a copy has drifted.

It needs the repository, not only clients/server: CI mounts it at RETE_REPO.
Run from a checkout, it finds the repository by itself.
"""
from __future__ import annotations

import json
import os
import re
import typing
from pathlib import Path

import pytest

from rete_server import views


def _repo() -> Path | None:
    given = os.environ.get("RETE_REPO")
    candidates = [Path(given)] if given else list(Path(__file__).resolve().parents)
    for root in candidates:
        if (root / "clients/mcpb/src/view/hive.js").is_file():
            return root
    return None


REPO = _repo()
pytestmark = pytest.mark.skipif(REPO is None, reason="the repository is not mounted (set RETE_REPO)")


def _js_array(text: str, pattern: str) -> list:
    m = re.search(pattern, text, re.S)
    assert m, f"no match for {pattern}"
    return json.loads(m.group(1))


def test_hive_axes_and_orders_match_hive_js():
    js = (REPO / "clients/mcpb/src/view/hive.js").read_text(encoding="utf-8")
    axes = _js_array(js, r"export const HIVE_AXES = (\[[^\]]*\]);")
    orders = _js_array(js, r"export const HIVE_ORDERS = (\[[^\]]*\]);")
    assert list(typing.get_args(views.HiveAxes)) == axes
    assert list(typing.get_args(views.HiveOrder)) == orders


def test_card_field_types_match_cards_js():
    js = (REPO / "clients/mcpb/src/view/cards.js").read_text(encoding="utf-8")
    block = re.search(r"const FIELD_TYPES = \[(.*?)\n\];", js, re.S)
    assert block, "FIELD_TYPES not found in cards.js"
    names = re.findall(r'^\s*\["([\w-]+)"', block.group(1), re.M)
    assert names, "no field types parsed from cards.js"
    assert list(views.FIELD_TYPE_NAMES) == names


def test_hint_views_match_hints_js():
    js = (REPO / "clients/chrome/lib/hints.js").read_text(encoding="utf-8")
    m = re.search(r"const VIEWS = new Set\((\[[^\]]*\])\)", js)
    assert m, "VIEWS not found in hints.js"
    assert views._HINT_VIEWS == set(json.loads(m.group(1)))
