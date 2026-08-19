#!/usr/bin/env python3
"""Behavior tests for the strict build-pipeline benchmark harness.

Each test names a contract break it catches: accepting an accidental workload
shape would make benchmark evidence incomparable, and accepting output identity
drift would turn an apparent speedup into a correctness regression.
"""

from __future__ import annotations

import json
import hashlib
import dataclasses
import contextlib
import io
import pathlib
import tempfile
import unittest

from bench_build_pipeline import _sample_output_path, _time_command, load_workload, main, summarize


class BenchBuildPipelineTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tempdir.name)

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def write_json(self, value: object) -> pathlib.Path:
        path = self.root / "workload.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def write_raw(self, value: str) -> pathlib.Path:
        path = self.root / "workload.json"
        path.write_text(value, encoding="utf-8")
        return path

    @staticmethod
    def sample_rows(
        *, times: list[int], rss: list[int], output_hash: str
    ) -> list[dict]:
        return [
            {
                "wallMs": time,
                "peakRssKiB": peak_rss,
                "outputSha256": output_hash,
                "outputBytes": 42,
                "queries": [],
            }
            for time, peak_rss in zip(times, rss, strict=True)
        ]

    def test_workload_rejects_unknown_keys_and_duplicate_json_members(self) -> None:
        """Adding an unrecognised workload field must stop evidence generation."""
        bad = {
            "name": "x",
            "input": "a",
            "sha256": "0" * 64,
            "mode": "standard",
            "args": [],
            "gateClass": "primary",
            "queries": [],
            "extra": 1,
        }
        with self.assertRaisesRegex(ValueError, "unknown key"):
            load_workload(self.write_json(bad))
        with self.assertRaisesRegex(ValueError, "duplicate JSON member"):
            load_workload(self.write_raw('{"name":"x","name":"y"}'))

    def test_workload_rejects_wrong_member_types_and_unknown_query_keys(self) -> None:
        """A malformed build or query command must not be coerced into a run."""
        bad = {
            "name": "x",
            "input": "a",
            "sha256": "0" * 64,
            "mode": "standard",
            "args": "--card",
            "gateClass": "primary",
            "queries": [],
        }
        with self.assertRaisesRegex(ValueError, "args must be an array"):
            load_workload(self.write_json(bad))

        bad["args"] = []
        bad["queries"] = [{"name": "q", "args": [], "sha256": "0" * 64, "extra": 1}]
        with self.assertRaisesRegex(ValueError, "query.*unknown key"):
            load_workload(self.write_json(bad))

    def test_external_workload_rejects_inline_memory_budget(self) -> None:
        """The required budget matrix must be the sole source of external budgets."""
        external = {
            "name": "x",
            "input": "a",
            "sha256": "0" * 64,
            "mode": "external",
            "args": ["--memory-budget-mb=64"],
            "gateClass": "external-primary",
            "queries": [],
        }
        with self.assertRaisesRegex(ValueError, "must not bake in --memory-budget-mb"):
            load_workload(self.write_json(external))

    def test_workload_allows_reserved_data_substitution_tokens(self) -> None:
        """Local and ranged query commands retain literal, non-shell placeholders."""
        workload = {
            "name": "x",
            "input": "fixture.nt",
            "sha256": "0" * 64,
            "mode": "standard",
            "args": [],
            "gateClass": "primary",
            "queries": [
                {"name": "local", "args": ["sparql", "{output}", "ASK {}"], "sha256": "1" * 64},
                {"name": "range", "args": ["sparql-url", "{url}", "ASK {}"], "sha256": "2" * 64},
            ],
        }
        got = load_workload(self.write_json(workload))
        self.assertEqual(got.queries[0].args[1], "{output}")
        self.assertEqual(got.queries[1].args[1], "{url}")

    def test_summary_uses_median_p90_and_requires_stable_hashes(self) -> None:
        """A changed output bytestring must not be summarized as a valid sample set."""
        rows = self.sample_rows(times=[100, 90, 110], rss=[50, 45, 55], output_hash="abc")
        got = summarize(rows)
        self.assertEqual(got["wallMsMedian"], 100)
        self.assertEqual(got["wallMsP90"], 110)
        self.assertEqual(got["peakRssKiBMedian"], 50)
        self.assertEqual(got["outputHashes"], ["abc"])

        drifting = self.sample_rows(times=[1, 2], rss=[3, 4], output_hash="abc")
        drifting[1]["outputSha256"] = "def"
        with self.assertRaisesRegex(ValueError, "output hash drift"):
            summarize(drifting)

    def test_external_budget_samples_do_not_reuse_output_paths(self) -> None:
        """Each external budget must build a separate artifact for the same repetition."""
        input_root = self.root / "inputs"
        input_root.mkdir()
        source = input_root / "social.nt"
        source.write_text("<s> <p> <o> .\n", encoding="utf-8")
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        external = {
            "name": "external",
            "input": "social.nt",
            "sha256": digest,
            "mode": "external",
            "args": [],
            "gateClass": "external-primary",
            "queries": [],
        }
        workload = load_workload(self.write_json(external))
        workload_64 = dataclasses.replace(workload, args=("--memory-budget-mb", "64"))
        workload_256 = dataclasses.replace(workload, args=("--memory-budget-mb", "256"))
        output_dir = self.root / "outputs"
        first = _sample_output_path(workload_64, output_dir, "baseline", 0)
        second = _sample_output_path(workload_256, output_dir, "baseline", 0)
        self.assertNotEqual(first, second)

    def test_main_rejects_missing_executables_before_creating_evidence(self) -> None:
        """A typo in an immutable binary path must fail cleanly before benchmark work."""
        workload = {
            "name": "x",
            "input": "fixture.nt",
            "sha256": "0" * 64,
            "mode": "standard",
            "args": [],
            "gateClass": "primary",
            "queries": [],
        }
        workload_path = self.write_json(workload)
        evidence = self.root / "evidence.jsonl"
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as raised:
                main(
                    [
                        "--baseline",
                        str(self.root / "missing-baseline"),
                        "--candidate",
                        str(self.root / "missing-candidate"),
                        "--workload",
                        str(workload_path),
                        "--input-root",
                        str(self.root),
                        "--output",
                        str(evidence),
                    ]
                )
        self.assertEqual(raised.exception.code, 2)
        self.assertFalse(evidence.exists())

    def test_missing_external_tool_has_a_clean_harness_error(self) -> None:
        """A missing required timing executable must not leak a Python traceback."""
        with self.assertRaisesRegex(RuntimeError, "required benchmark tool is unavailable"):
            _time_command(["/definitely-not-a-benchmark-tool"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
