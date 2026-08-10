#!/usr/bin/env python3

import importlib.util
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest


HARNESS = pathlib.Path(__file__).with_name("bench_cold_r2.py")
WORKLOADS = pathlib.Path(__file__).with_name("cold-r2-workloads")


def load_harness():
    if not HARNESS.exists():
        raise AssertionError("bench_cold_r2.py has not been implemented")
    spec = importlib.util.spec_from_file_location("bench_cold_r2", HARNESS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def valid_workload_payload():
    return {
        "name": "custom",
        "source": "https://example.test/custom.rete",
        "expected_length": 1234,
        "expected_etag": '"custom-etag"',
        "queries": [
            {
                "name": "bounded-select",
                "sparql": (
                    "SELECT ?s ?p ?o WHERE { ?s ?p ?o } "
                    "ORDER BY ?s ?p ?o LIMIT 10"
                ),
            }
        ],
    }


def write_workload(directory, payload):
    path = pathlib.Path(directory) / "custom.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


class BenchColdR2Tests(unittest.TestCase):
    def test_every_limited_select_has_its_complete_approved_ordering(self):
        bench = load_harness()
        workloads = (
            bench.CHEMOTION_WORKLOAD,
            bench.load_workload(WORKLOADS / "boe.json"),
            bench.load_workload(WORKLOADS / "chebi-full.json"),
        )
        limited_selects = {
            f"{workload.name}:{query_name}": sparql
            for workload in workloads
            for query_name, sparql in workload.queries
            if "SELECT" in sparql.upper() and "LIMIT" in sparql.upper()
        }
        approved_orderings = {
            "chemotion:select": "ORDER BY ?name ?formula ?smiles LIMIT 200",
            "chemotion:aggregate": (
                "ORDER BY DESC(?molecules) ?formula LIMIT 20"
            ),
            "chemotion:path": "ORDER BY ?name LIMIT 200",
            "boe:bound-law": "ORDER BY ?p ?o LIMIT 100",
            "boe:type-counts": "ORDER BY DESC(?count) ?type LIMIT 50",
            "chebi-full:bound-entity": "ORDER BY ?p ?o LIMIT 200",
        }

        self.assertEqual(set(limited_selects), set(approved_orderings))
        for query_name, ordering in approved_orderings.items():
            with self.subTest(query=query_name):
                self.assertIn(ordering, limited_selects[query_name])

    def test_load_workload_accepts_the_exact_pinned_schema(self):
        bench = load_harness()
        payload = valid_workload_payload()

        with tempfile.TemporaryDirectory() as directory:
            workload = bench.load_workload(write_workload(directory, payload))

        self.assertEqual(
            workload,
            bench.Workload(
                name="custom",
                source="https://example.test/custom.rete",
                expected_length=1234,
                expected_etag='"custom-etag"',
                queries=(
                    (
                        "bounded-select",
                        "SELECT ?s ?p ?o WHERE { ?s ?p ?o } "
                        "ORDER BY ?s ?p ?o LIMIT 10",
                    ),
                ),
            ),
        )

    def test_load_workload_rejects_missing_or_unknown_fields(self):
        bench = load_harness()
        cases = []
        for field in valid_workload_payload():
            payload = valid_workload_payload()
            del payload[field]
            cases.append((f"missing root {field}", payload))
        payload = valid_workload_payload()
        payload["expected_etagg"] = payload["expected_etag"]
        cases.append(("unknown root field", payload))
        for field in ("name", "sparql"):
            payload = valid_workload_payload()
            del payload["queries"][0][field]
            cases.append((f"missing query {field}", payload))
        payload = valid_workload_payload()
        payload["queries"][0]["query"] = payload["queries"][0]["sparql"]
        cases.append(("unknown query field", payload))

        with tempfile.TemporaryDirectory() as directory:
            for label, payload in cases:
                with self.subTest(label=label):
                    with self.assertRaisesRegex(ValueError, "fields"):
                        bench.load_workload(write_workload(directory, payload))

    def test_load_workload_rejects_blank_or_invalid_values(self):
        bench = load_harness()
        cases = []
        for field in ("name", "source", "expected_etag"):
            for value in ("", " \t\n", None, 7):
                payload = valid_workload_payload()
                payload[field] = value
                cases.append((f"invalid {field} {value!r}", payload))
        for value in (0, -1, True, 1.5, "1234", None):
            payload = valid_workload_payload()
            payload["expected_length"] = value
            cases.append((f"invalid expected_length {value!r}", payload))
        for value in ([], {}, "query", None):
            payload = valid_workload_payload()
            payload["queries"] = value
            cases.append((f"invalid queries {value!r}", payload))
        for field in ("name", "sparql"):
            for value in ("", " \t\n", None, 7):
                payload = valid_workload_payload()
                payload["queries"][0][field] = value
                cases.append((f"invalid query {field} {value!r}", payload))
        payload = valid_workload_payload()
        payload["queries"] = ["not an object"]
        cases.append(("query is not an object", payload))

        with tempfile.TemporaryDirectory() as directory:
            for label, payload in cases:
                with self.subTest(label=label):
                    with self.assertRaises(ValueError):
                        bench.load_workload(write_workload(directory, payload))

    def test_load_workload_rejects_duplicate_query_names(self):
        bench = load_harness()
        payload = valid_workload_payload()
        payload["queries"].append(
            {
                "name": "bounded-select",
                "sparql": "ASK WHERE { ?s ?p ?o }",
            }
        )

        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "duplicate query name"):
                bench.load_workload(write_workload(directory, payload))

    def test_load_workload_rejects_duplicate_json_fields(self):
        bench = load_harness()
        duplicate_root = """{
          "name": "custom",
          "name": "shadow",
          "source": "https://example.test/custom.rete",
          "expected_length": 1234,
          "expected_etag": "custom-etag",
          "queries": [{"name": "ask", "sparql": "ASK { ?s ?p ?o }"}]
        }"""
        duplicate_query = """{
          "name": "custom",
          "source": "https://example.test/custom.rete",
          "expected_length": 1234,
          "expected_etag": "custom-etag",
          "queries": [{
            "name": "ask",
            "name": "shadow",
            "sparql": "ASK { ?s ?p ?o }"
          }]
        }"""

        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "duplicate.json"
            for payload in (duplicate_root, duplicate_query):
                with self.subTest(payload=payload):
                    path.write_text(payload, encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, "duplicate JSON field"):
                        bench.load_workload(path)

    def test_source_and_workload_are_exclusive_and_one_is_required(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = pathlib.Path(directory)
            workload = write_workload(directory, valid_workload_payload())
            common = [
                sys.executable,
                str(HARNESS),
                "--candidate",
                str(directory / "candidate"),
                "--thresholds",
                "0",
                "--samples",
                "1",
                "--out",
                str(directory / "samples.jsonl"),
            ]
            missing = subprocess.run(
                common,
                capture_output=True,
                text=True,
                check=False,
            )
            conflicting = subprocess.run(
                [
                    *common,
                    "--source",
                    "https://example.test/data.rete",
                    "--workload",
                    str(workload),
                ],
                capture_output=True,
                text=True,
                check=False,
            )

        self.assertNotEqual(missing.returncode, 0)
        self.assertIn(
            "one of the arguments --source --workload is required", missing.stderr
        )
        self.assertNotEqual(conflicting.returncode, 0)
        self.assertIn("not allowed with argument", conflicting.stderr)

    def test_workload_file_controls_pins_queries_processes_and_record_identity(self):
        head_probes = []
        expected_length = 4321
        expected_etag = '"custom-pin"'

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_HEAD(self):
                head_probes.append((expected_length, expected_etag))
                self.send_response(200)
                self.send_header("Content-Length", str(expected_length))
                self.send_header("ETag", expected_etag)
                self.end_headers()

            def log_message(self, _format, *_args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                directory = pathlib.Path(directory)
                process_log = directory / "processes.log"
                executable = directory / "fake-rete"
                executable.write_text(
                    """#!/usr/bin/env python3
import os
import pathlib
import sys
import time
time.sleep(0.03)
query = sys.argv[3]
with pathlib.Path(os.environ["PROCESS_LOG"]).open("a", encoding="utf-8") as log:
    log.write(os.environ["RETE_EAGER_MAX_MB"] + "\\t" + query + "\\n")
sys.stdout.write("alpha result\\n" if "urn:alpha" in query else "beta result\\n")
sys.stderr.write("(fetched 321 bytes in 2 range request(s); file is 4321 bytes)\\n")
""",
                    encoding="utf-8",
                )
                executable.chmod(0o755)
                source = f"http://127.0.0.1:{server.server_port}/custom.rete"
                payload = {
                    "name": "custom-workload",
                    "source": source,
                    "expected_length": expected_length,
                    "expected_etag": expected_etag,
                    "queries": [
                        {"name": "alpha", "sparql": "ASK { <urn:alpha> ?p ?o }"},
                        {"name": "beta", "sparql": "ASK { <urn:beta> ?p ?o }"},
                    ],
                }
                workload = write_workload(directory, payload)
                output = directory / "samples.jsonl"
                environment = os.environ.copy()
                environment["PROCESS_LOG"] = str(process_log)
                result = subprocess.run(
                    [
                        sys.executable,
                        str(HARNESS),
                        "--candidate",
                        str(executable),
                        "--thresholds",
                        "0,8",
                        "--samples",
                        "2",
                        "--workload",
                        str(workload),
                        "--out",
                        str(output),
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                    env=environment,
                )
                records = (
                    [json.loads(line) for line in output.read_text().splitlines()]
                    if output.exists()
                    else []
                )
                process_lines = (
                    process_log.read_text(encoding="utf-8").splitlines()
                    if process_log.exists()
                    else []
                )
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(head_probes, [(expected_length, expected_etag)] * 2)
        self.assertEqual(len(process_lines), 2 * 2 * 2)
        self.assertEqual(
            [line.split("\t", 1)[0] for line in process_lines],
            ["0", "8", "8", "0", "0", "8", "8", "0"],
        )
        self.assertEqual(sum("urn:alpha" in line for line in process_lines), 4)
        self.assertEqual(sum("urn:beta" in line for line in process_lines), 4)
        self.assertFalse(any("CHEBI_23367" in line for line in process_lines))
        self.assertEqual(len(records), 2 * 2 * 2)
        self.assertEqual({record["workload"] for record in records}, {"custom-workload"})
        self.assertEqual({record["query"] for record in records}, {"alpha", "beta"})
        self.assertEqual({record["mode"] for record in records}, {"threshold_0", "threshold_8"})
        self.assertEqual({record["length"] for record in records}, {expected_length})
        self.assertEqual({record["etag"] for record in records}, {expected_etag})
        for query_name in ("alpha", "beta"):
            self.assertEqual(
                len(
                    {
                        record["sha256"]
                        for record in records
                        if record["query"] == query_name
                    }
                ),
                1,
            )
        source_lines = [
            json.loads(line.removeprefix("SOURCE "))
            for line in result.stdout.splitlines()
            if line.startswith("SOURCE ")
        ]
        self.assertEqual(
            source_lines,
            [
                {
                    "workload": "custom-workload",
                    "source": source,
                    "length": expected_length,
                    "etag": expected_etag,
                }
            ],
        )

    def test_select_workload_orders_rows_before_applying_limit(self):
        head_probes = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_HEAD(self):
                head_probes.append(
                    (7_566_404, '"6cefd111dee3c59c063f0bede9cd60f9"')
                )
                self.send_response(200)
                self.send_header("Content-Length", "7566404")
                self.send_header("ETag", '"6cefd111dee3c59c063f0bede9cd60f9"')
                self.end_headers()

            def log_message(self, _format, *_args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                directory = pathlib.Path(directory)
                executable = directory / "order-sensitive-rete"
                executable.write_text(
                    """#!/usr/bin/env python3
import os
import sys
import time
time.sleep(0.03)
query = sys.argv[3]
select = "SELECT ?name ?formula ?smiles" in query
ordered = "ORDER BY ?name ?formula ?smiles LIMIT 200" in query
if select and not ordered:
    output = "eager subset\\n" if os.environ.get("RETE_EAGER_MAX_MB") == "8" else "lazy subset\\n"
else:
    output = "stable result\\n"
sys.stdout.write(output)
sys.stderr.write("(fetched 100 bytes in 2 range request(s); file is 7566404 bytes)\\n")
""",
                    encoding="utf-8",
                )
                executable.chmod(0o755)
                output = directory / "samples.jsonl"
                url = f"http://127.0.0.1:{server.server_port}/chemotion.rete"
                result = subprocess.run(
                    [
                        sys.executable,
                        str(HARNESS),
                        "--baseline",
                        str(executable),
                        "--candidate",
                        str(executable),
                        "--samples",
                        "1",
                        "--source",
                        url,
                        "--out",
                        str(output),
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                records = [json.loads(line) for line in output.read_text().splitlines()]
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            head_probes,
            [
                (7_566_404, '"6cefd111dee3c59c063f0bede9cd60f9"'),
                (7_566_404, '"6cefd111dee3c59c063f0bede9cd60f9"'),
            ],
        )
        self.assertEqual(len(records), 9)
        self.assertEqual({record["workload"] for record in records}, {"chemotion"})
        self.assertEqual(
            len({record["sha256"] for record in records if record["query"] == "select"}),
            1,
        )

    def test_hash_mismatch_preserves_the_fresh_process_record_before_failing(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_HEAD(self):
                self.send_response(200)
                self.send_header("Content-Length", "7566404")
                self.send_header("ETag", '"6cefd111dee3c59c063f0bede9cd60f9"')
                self.end_headers()

            def log_message(self, _format, *_args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                directory = pathlib.Path(directory)
                executable = directory / "fake-rete"
                executable.write_text(
                    """#!/usr/bin/env python3
import os
import sys
import time
time.sleep(0.03)
eager = os.environ.get("RETE_EAGER_MAX_MB") == "8"
sys.stdout.write("eager output\\n" if eager else "lazy output\\n")
sys.stderr.write(
    "(fetched 7566404 bytes in 1 range request(s); file is 7566404 bytes)\\n"
    if eager
    else "(fetched 100 bytes in 2 range request(s); file is 7566404 bytes)\\n"
)
""",
                    encoding="utf-8",
                )
                executable.chmod(0o755)
                output = directory / "samples.jsonl"
                url = f"http://127.0.0.1:{server.server_port}/chemotion.rete"
                result = subprocess.run(
                    [
                        sys.executable,
                        str(HARNESS),
                        "--baseline",
                        str(executable),
                        "--candidate",
                        str(executable),
                        "--samples",
                        "1",
                        "--source",
                        url,
                        "--out",
                        str(output),
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                records = [json.loads(line) for line in output.read_text().splitlines()]
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("output hash mismatch", result.stderr)
        self.assertEqual([record["mode"] for record in records], [
            "baseline_lazy",
            "delegated_lazy",
            "eager_8",
        ])

    def test_head_metadata_identifies_the_benchmark_client(self):
        bench = load_harness()
        observed = {}

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_HEAD(self):
                observed["user_agent"] = self.headers.get("User-Agent")
                self.send_response(200)
                self.send_header("Content-Length", "7566404")
                self.send_header("ETag", '"6cefd111dee3c59c063f0bede9cd60f9"')
                self.end_headers()

            def log_message(self, _format, *_args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_port}/chemotion.rete"
            self.assertEqual(
                bench.head_metadata(url),
                (7_566_404, '"6cefd111dee3c59c063f0bede9cd60f9"'),
            )
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

        self.assertEqual(observed["user_agent"], "rete-cold-r2-benchmark/1")

    def test_parse_transfer_stats_extracts_physical_fetch_counts(self):
        bench = load_harness()
        stderr = (
            b"diagnostic before stats\n"
            b"(fetched 7566404 bytes in 1 range request(s); file is 7566404 bytes)\n"
        )

        self.assertEqual(bench.parse_transfer_stats(stderr), (7_566_404, 1))

    def test_parse_transfer_stats_rejects_missing_or_ambiguous_stats(self):
        bench = load_harness()

        with self.assertRaisesRegex(ValueError, "exactly one transfer-stat line"):
            bench.parse_transfer_stats(b"no transfer stats here\n")
        with self.assertRaisesRegex(ValueError, "exactly one transfer-stat line"):
            bench.parse_transfer_stats(
                b"fetched 1 bytes in 2 range request(s)\n"
                b"fetched 3 bytes in 4 range request(s)\n"
            )

    def test_nearest_rank_p90_uses_the_ceiling_rank(self):
        bench = load_harness()

        self.assertEqual(bench.nearest_rank_p90([15, 3, 9, 1, 11]), 15)
        self.assertEqual(bench.nearest_rank_p90(list(range(1, 16))), 14)

    def test_mode_order_rotates_every_mode_through_every_position(self):
        bench = load_harness()
        modes = ["baseline_lazy", "delegated_lazy", "eager_8"]

        self.assertEqual(bench.rotating_order(modes, 1), modes)
        self.assertEqual(
            bench.rotating_order(modes, 2),
            ["delegated_lazy", "eager_8", "baseline_lazy"],
        )
        self.assertEqual(
            bench.rotating_order(modes, 3),
            ["eager_8", "baseline_lazy", "delegated_lazy"],
        )
        self.assertEqual(bench.rotating_order(modes, 4), modes)

    def test_summary_reports_median_nearest_rank_p90_and_peak_rss(self):
        bench = load_harness()
        records = [
            {
                "wall_ms": value,
                "bytes": 7_566_404,
                "gets": 1,
                "peak_rss_kib": 30_000 + value,
            }
            for value in range(1, 16)
        ]

        self.assertEqual(
            bench.summarize(records),
            {
                "median_ms": 8,
                "p90_ms": 14,
                "bytes": 7_566_404,
                "gets": 1,
                "median_peak_rss_kib": 30_008,
                "p90_peak_rss_kib": 30_014,
                "max_peak_rss_kib": 30_015,
            },
        )

    def test_summary_rejects_unstable_transfer_counts(self):
        bench = load_harness()
        records = [
            {"wall_ms": 10, "bytes": 1, "gets": 2, "peak_rss_kib": 3},
            {"wall_ms": 11, "bytes": 2, "gets": 2, "peak_rss_kib": 4},
        ]

        with self.assertRaisesRegex(ValueError, "transfer counts changed"):
            bench.summarize(records)


if __name__ == "__main__":
    unittest.main()
