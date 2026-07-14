#!/usr/bin/env python3
"""Tests for reconstructing ignored playground inputs on clean CI runners."""

import base64
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "stage_playground_datasets.py"
DATASET_NAMES = (
    "scholar",
    "scholar-noisy",
    "causal",
    "linked-jazz",
    "nomisma",
    "mimotext",
    "openalex-astrocytes",
    "antarctic-expeditions",
    "theographic-graph",
    "monarch",
    "opencitations",
)


def rete_payload(marker: bytes) -> bytes:
    return b"RETE\x05" + marker + b"RETE"


class StagePlaygroundDatasetsTests(unittest.TestCase):
    def run_stage(self, html: Path, web_dir: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--html", str(html), "--web-dir", str(web_dir)],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )

    def write_html(self, path: Path, payloads: dict[str, bytes]) -> None:
        encoded = {
            key: base64.b64encode(value).decode("ascii")
            for key, value in payloads.items()
        }
        path.write_text(
            "<script>\nconst RETE_DATASETS_B64 = "
            + json.dumps(encoded, separators=(",", ":"), sort_keys=True)
            + ";\n</script>\n",
            encoding="utf-8",
        )

    def test_stages_every_required_dataset_from_tracked_html(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            html = root / "playground.html"
            web_dir = root / "web"
            payloads = {name: rete_payload(name.encode()) for name in DATASET_NAMES}
            self.write_html(html, payloads)

            result = self.run_stage(html, web_dir)

            self.assertEqual(result.returncode, 0, result.stderr)
            for name, payload in payloads.items():
                self.assertEqual((web_dir / f"{name}.rete").read_bytes(), payload)

    def test_existing_matching_dataset_is_not_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            html = root / "playground.html"
            web_dir = root / "web"
            web_dir.mkdir()
            payloads = {name: rete_payload(name.encode()) for name in DATASET_NAMES}
            self.write_html(html, payloads)
            existing = payloads["scholar"]
            (web_dir / "scholar.rete").write_bytes(existing)

            result = self.run_stage(html, web_dir)

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((web_dir / "scholar.rete").read_bytes(), existing)

    def test_refuses_to_overwrite_a_conflicting_dataset(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            html = root / "playground.html"
            web_dir = root / "web"
            web_dir.mkdir()
            payloads = {name: rete_payload(name.encode()) for name in DATASET_NAMES}
            self.write_html(html, payloads)
            existing = rete_payload(b"different")
            (web_dir / "scholar.rete").write_bytes(existing)

            result = self.run_stage(html, web_dir)

            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((web_dir / "scholar.rete").read_bytes(), existing)
            self.assertIn("does not match", result.stderr)

    def test_rejects_a_dataset_that_is_not_v5(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            html = root / "playground.html"
            payloads = {name: rete_payload(name.encode()) for name in DATASET_NAMES}
            payloads["causal"] = b"RETE\x04oldRETE"
            self.write_html(html, payloads)

            result = self.run_stage(html, root / "web")

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("causal", result.stderr)
            self.assertIn("format v5", result.stderr)


if __name__ == "__main__":
    unittest.main()
