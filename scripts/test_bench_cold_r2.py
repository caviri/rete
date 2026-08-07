#!/usr/bin/env python3

import importlib.util
import http.server
import json
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest


HARNESS = pathlib.Path(__file__).with_name("bench_cold_r2.py")


def load_harness():
    if not HARNESS.exists():
        raise AssertionError("bench_cold_r2.py has not been implemented")
    spec = importlib.util.spec_from_file_location("bench_cold_r2", HARNESS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class BenchColdR2Tests(unittest.TestCase):
    def test_select_workload_orders_rows_before_applying_limit(self):
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
        self.assertEqual(len(records), 9)
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

    def test_mode_order_reverses_on_alternating_runs(self):
        bench = load_harness()
        modes = ["baseline_lazy", "delegated_lazy", "eager_8"]

        self.assertEqual(bench.alternating_order(modes, 1), modes)
        self.assertEqual(bench.alternating_order(modes, 2), list(reversed(modes)))
        self.assertEqual(bench.alternating_order(modes, 3), modes)

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
