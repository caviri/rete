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
import os
import pathlib
import socket
import tempfile
import unittest
import urllib.error
import urllib.request
from unittest import mock

import bench_build_pipeline as harness
from bench_build_pipeline import (
    Query,
    StrictRangeServer,
    _sample_output_path,
    _time_command,
    load_workload,
    main,
    run_sample,
    summarize,
)


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

    def test_run_sample_rejects_an_executable_changed_after_source_recording(self) -> None:
        """A binary replaced after SOURCE must not contribute a timed sample."""
        input_root, workload = self.make_input_workload()
        executable = self.make_executable("old")
        expected = hashlib.sha256(executable.read_bytes()).hexdigest()
        executable.write_text("#!/bin/sh\necho replaced\n", encoding="utf-8")
        executable.chmod(executable.stat().st_mode | 0o111)
        with self.assertRaisesRegex(ValueError, "executable SHA-256 drift"):
            run_sample(
                executable,
                workload,
                input_root,
                self.root / "artifacts",
                "baseline",
                0,
                expected_executable_sha256=expected,
            )

    def test_run_sample_rejects_an_executable_changed_during_build(self) -> None:
        """Replacing a binary during its timed process must invalidate the sample."""
        input_root, workload = self.make_input_workload()
        executable = self.make_executable("stable")
        expected = hashlib.sha256(executable.read_bytes()).hexdigest()
        artifacts = self.root / "artifacts"
        artifacts.mkdir()

        def replace_during_build(_command):
            executable.write_text("#!/bin/sh\necho changed\n", encoding="utf-8")
            executable.chmod(executable.stat().st_mode | 0o111)
            (artifacts / "fixture-baseline-r0.rete").write_bytes(b"rete")
            return 1, 2

        with mock.patch.object(harness, "_run_build", side_effect=replace_during_build):
            with self.assertRaisesRegex(ValueError, "executable SHA-256 drift"):
                run_sample(
                    executable, workload, input_root, artifacts, "baseline", 0,
                    expected_executable_sha256=expected,
                )

    def test_run_sample_rejects_an_executable_changed_by_final_query(self) -> None:
        """A query must not leave a different binary behind after a sample."""
        input_root, workload = self.make_input_workload()
        workload = dataclasses.replace(
            workload, queries=(Query("mutating", (), "0" * 64),)
        )
        executable = self.make_executable("stable")
        expected = hashlib.sha256(executable.read_bytes()).hexdigest()
        artifacts = self.root / "artifacts"
        artifacts.mkdir()

        def build_output(_command):
            (artifacts / "fixture-baseline-r0.rete").write_bytes(b"rete")
            return 1, 2

        def mutate_final_query(_exe, _query, _output):
            executable.write_text("#!/bin/sh\necho changed\n", encoding="utf-8")
            executable.chmod(executable.stat().st_mode | 0o111)
            return {"name": "mutating", "wallMs": 1, "resultSha256": "0" * 64}

        with mock.patch.object(harness, "_run_build", side_effect=build_output):
            with mock.patch.object(harness, "_run_query", side_effect=mutate_final_query):
                with self.assertRaisesRegex(ValueError, "executable SHA-256 drift"):
                    run_sample(
                        executable, workload, input_root, artifacts, "baseline", 0,
                        expected_executable_sha256=expected,
                    )

    def test_query_substitutes_output_as_one_literal_argv_member(self) -> None:
        """The output placeholder must reach the child unchanged and without a shell."""
        output = self.root / "file with spaces.rete"
        output.write_bytes(b"rete")
        script = self.root / "argv.py"
        script.write_text("import sys\nprint(sys.argv[1] == sys.argv[2])\n", encoding="utf-8")
        expected_stdout = f"True{os.linesep}".encode()
        query = Query(
            name="argv",
            args=(str(script), str(output), "{output}"),
            sha256=hashlib.sha256(expected_stdout).hexdigest(),
        )
        result = harness._run_query(pathlib.Path(os.sys.executable), query, output)
        self.assertEqual(result["resultSha256"], query.sha256)

    def test_range_server_requires_partial_ranges_and_streams_exact_bytes(self) -> None:
        """A full download or malformed request must not bypass range accounting."""
        source = self.root / "range.rete"
        source.write_bytes(b"0123456789")
        with StrictRangeServer(source) as server:
            request = urllib.request.Request(server.url, headers={"Range": "bytes=2-5"})
            with urllib.request.urlopen(request) as response:
                self.assertEqual(response.status, 206)
                self.assertEqual(response.headers["Content-Range"], "bytes 2-5/10")
                self.assertEqual(response.read(), b"2345")
            for header in (None, "bytes=bogus", "bytes=0-9"):
                request = urllib.request.Request(server.url, headers={} if header is None else {"Range": header})
                with self.assertRaises(urllib.error.HTTPError) as raised:
                    urllib.request.urlopen(request)
                self.assertEqual(raised.exception.code, 416)
            self.assertEqual(server.gets, 4)
            self.assertEqual(server.bytes_served, 4)
            self.assertEqual(server.rejected_gets, 3)
            port = server._server.server_address[1]
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", port), timeout=0.2)

    def test_main_writes_source_samples_summary_and_alternates_warmups(self) -> None:
        """Changing the sequence must not silently make paired evidence biased."""
        input_root, workload = self.make_input_workload()
        workload_path = self.write_json(self.workload_json(workload))
        baseline = self.make_executable("baseline")
        candidate = self.make_executable("candidate")
        evidence = self.root / "evidence.jsonl"
        calls: list[tuple[str, int]] = []

        def fake_sample(_exe, _workload, _input_root, _dir, implementation, repetition, **kwargs):
            calls.append((implementation, repetition))
            return self.sample_record(implementation, repetition, kwargs["expected_executable_sha256"])

        with mock.patch.object(harness, "run_sample", side_effect=fake_sample):
            self.assertEqual(
                main(
                    [
                        "--baseline", str(baseline), "--candidate", str(candidate),
                        "--workload", str(workload_path), "--input-root", str(input_root),
                        "--samples", "15", "--output", str(evidence),
                    ]
                ),
                0,
            )
        self.assertEqual(calls[:4], [("baseline", -2), ("candidate", -2), ("candidate", -1), ("baseline", -1)])
        self.assertEqual(calls[4:8], [("baseline", 0), ("candidate", 0), ("candidate", 1), ("baseline", 1)])
        records = [json.loads(line) for line in evidence.read_text(encoding="utf-8").splitlines()]
        self.assertEqual(records[0]["kind"], "SOURCE")
        self.assertEqual(sum(record["kind"] == "SAMPLE" for record in records), 30)
        self.assertEqual(records[-1]["kind"], "SUMMARY")

    def test_main_rejects_cross_budget_output_drift_in_warmups(self) -> None:
        """A changed external warmup must abort before a later budget resets identity."""
        input_root, workload = self.make_input_workload(mode="external")
        workload_path = self.write_json(self.workload_json(workload))
        baseline = self.make_executable("baseline")
        candidate = self.make_executable("candidate")
        evidence = self.root / "evidence.jsonl"

        def drifting_sample(_exe, configured, _input_root, _dir, implementation, repetition, **kwargs):
            record = self.sample_record(implementation, repetition, kwargs["expected_executable_sha256"])
            if configured.args[-1] == "256" and repetition == -2:
                record["outputSha256"] = "different"
            return record

        with mock.patch.object(harness, "run_sample", side_effect=drifting_sample):
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as raised:
                    main(
                        [
                            "--baseline", str(baseline), "--candidate", str(candidate),
                            "--workload", str(workload_path), "--input-root", str(input_root),
                            "--samples", "15", "--external-budgets", "64,256,1024", "--output", str(evidence),
                        ]
                    )
        self.assertEqual(raised.exception.code, 2)

    def make_input_workload(self, mode: str = "standard"):
        input_root = self.root / "inputs"
        input_root.mkdir(exist_ok=True)
        source = input_root / "fixture.nt"
        source.write_text("<s> <p> <o> .\n", encoding="utf-8")
        raw = {
            "name": "fixture",
            "input": "fixture.nt",
            "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            "mode": mode,
            "args": [],
            "gateClass": "external-primary" if mode == "external" else "primary",
            "queries": [],
        }
        return input_root, load_workload(self.write_json(raw))

    @staticmethod
    def workload_json(workload):
        return {
            "name": workload.name, "input": workload.input, "sha256": workload.sha256,
            "mode": workload.mode, "args": list(workload.args), "gateClass": workload.gate_class,
            "queries": [],
        }

    def make_executable(self, text: str) -> pathlib.Path:
        path = self.root / f"{text}.sh"
        path.write_text(f"#!/bin/sh\necho {text}\n", encoding="utf-8")
        path.chmod(path.stat().st_mode | 0o111)
        return path

    @staticmethod
    def sample_record(implementation: str, repetition: int, executable_sha256: str) -> dict:
        return {
            "schemaVersion": 1, "kind": "SAMPLE", "implementation": implementation,
            "repetition": repetition, "executableSha256": executable_sha256, "wallMs": 1,
            "peakRssKiB": 2, "outputSha256": f"{implementation}-output", "outputBytes": 3,
            "queries": [
                {"name": "probe", "wallMs": 1, "resultSha256": f"{implementation}-query"}
            ],
        }


if __name__ == "__main__":
    unittest.main(verbosity=2)
