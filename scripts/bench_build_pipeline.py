#!/usr/bin/env python3
"""Strict, reproducible evidence for comparing immutable ``rete build`` binaries.

The harness deliberately accepts only a small JSON workload language.  It never
uses a shell: all build and query arguments are argv members, and the two
reserved query values are substituted as data immediately before process start.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import math
import os
import pathlib
import re
import statistics
import subprocess
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Iterable


SCHEMA_VERSION = 1
WORKLOAD_KEYS = frozenset(("name", "input", "sha256", "mode", "args", "gateClass", "queries"))
QUERY_KEYS = frozenset(("name", "args", "sha256"))
GATE_CLASSES = frozenset(("primary", "small-overhead", "louvain-no-regression", "external-primary"))
MODES = frozenset(("standard", "external"))
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SAFE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]*$")
RANGE_RE = re.compile(r"^bytes=(\d+)-(\d+)$")
EXTERNAL_BUDGETS = (64, 256, 1024)
RANGE_CHUNK_BYTES = 64 * 1024


@dataclasses.dataclass(frozen=True)
class Query:
    name: str
    args: tuple[str, ...]
    sha256: str


@dataclasses.dataclass(frozen=True)
class Workload:
    name: str
    input: str
    sha256: str
    mode: str
    args: tuple[str, ...]
    gate_class: str
    queries: tuple[Query, ...]


def _reject_duplicate_members(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, member in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON member: {key}")
        value[key] = member
    return value


def _read_json(path: pathlib.Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_reject_duplicate_members)
    except OSError as error:
        raise ValueError(f"cannot read workload {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid workload JSON {path}: {error}") from error


def _require_object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    return value


def _require_exact_keys(value: dict[str, Any], expected: frozenset[str], label: str) -> None:
    unknown = sorted(set(value) - expected)
    missing = sorted(expected - set(value))
    if unknown:
        raise ValueError(f"{label} has unknown key: {unknown[0]}")
    if missing:
        raise ValueError(f"{label} is missing key: {missing[0]}")


def _require_string(value: Any, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be a string")
    if not value:
        raise ValueError(f"{label} must not be empty")
    return value


def _require_sha256(value: Any, label: str) -> str:
    digest = _require_string(value, label)
    if not SHA256_RE.fullmatch(digest):
        raise ValueError(f"{label} must be a lowercase SHA-256 hex digest")
    return digest


def _require_args(value: Any, label: str) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise ValueError(f"{label} must be an array")
    args = tuple(value)
    if not all(isinstance(arg, str) for arg in args):
        raise ValueError(f"{label} members must be strings")
    return args


def _parse_query(value: Any, index: int) -> Query:
    query = _require_object(value, f"query {index}")
    _require_exact_keys(query, QUERY_KEYS, f"query {index}")
    name = _require_string(query["name"], f"query {index}.name")
    if not SAFE_NAME_RE.fullmatch(name):
        raise ValueError(f"query {index}.name contains unsafe filename characters")
    return Query(
        name=name,
        args=_require_args(query["args"], f"query {index}.args"),
        sha256=_require_sha256(query["sha256"], f"query {index}.sha256"),
    )


def load_workload(path: pathlib.Path) -> Workload:
    """Load a workload without silently accepting a schema or identity change."""
    raw = _require_object(_read_json(path), "workload")
    _require_exact_keys(raw, WORKLOAD_KEYS, "workload")
    name = _require_string(raw["name"], "workload.name")
    if not SAFE_NAME_RE.fullmatch(name):
        raise ValueError("workload.name contains unsafe filename characters")
    input_name = _require_string(raw["input"], "workload.input")
    input_path = pathlib.PurePosixPath(input_name)
    if input_path.is_absolute() or ".." in input_path.parts or "\\" in input_name:
        raise ValueError("workload.input must be a relative POSIX path")
    mode = _require_string(raw["mode"], "workload.mode")
    if mode not in MODES:
        raise ValueError(f"workload.mode must be one of {sorted(MODES)}")
    gate_class = _require_string(raw["gateClass"], "workload.gateClass")
    if gate_class not in GATE_CLASSES:
        raise ValueError(f"workload.gateClass must be one of {sorted(GATE_CLASSES)}")
    args = _require_args(raw["args"], "workload.args")
    has_memory_budget = any(
        arg == "--memory-budget-mb" or arg.startswith("--memory-budget-mb=") for arg in args
    )
    if mode == "external" and has_memory_budget:
        raise ValueError("external workload must not bake in --memory-budget-mb")
    if mode == "standard" and has_memory_budget:
        raise ValueError("standard workload must not carry --memory-budget-mb")
    if not isinstance(raw["queries"], list):
        raise ValueError("workload.queries must be an array")
    queries = tuple(_parse_query(query, index) for index, query in enumerate(raw["queries"]))
    if len({query.name for query in queries}) != len(queries):
        raise ValueError("query names must be unique")
    return Workload(
        name=name,
        input=input_name,
        sha256=_require_sha256(raw["sha256"], "workload.sha256"),
        mode=mode,
        args=args,
        gate_class=gate_class,
        queries=queries,
    )


def open_exclusive(path: pathlib.Path):
    """Create an evidence file once; a second run must not overwrite it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    return path.open("x", encoding="utf-8", newline="\n")


def claim_artifact_namespace(evidence_path: pathlib.Path) -> pathlib.Path:
    """Atomically reserve one evidence stem's sample-artifact namespace."""
    namespace = evidence_path.parent / f".{evidence_path.stem}.artifacts"
    namespace.mkdir(parents=True)
    return namespace


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _require_executable(path: pathlib.Path) -> pathlib.Path:
    executable = path.resolve()
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise ValueError(f"executable is not runnable: {executable}")
    return executable


def percentile90(values: Iterable[float]) -> float:
    ordered = sorted(values)
    if not ordered:
        raise ValueError("cannot calculate p90 of no values")
    return ordered[max(0, math.ceil(0.90 * len(ordered)) - 1)]


def _number(value: float) -> int | float:
    return int(value) if float(value).is_integer() else value


def _stable_single(values: Iterable[Any], label: str) -> list[Any]:
    distinct = sorted(set(values))
    if len(distinct) != 1:
        raise ValueError(f"{label} drift: {distinct}")
    return distinct


def summarize(rows: list[dict]) -> dict:
    """Summarize accepted samples, refusing output/query identity drift."""
    if not rows:
        raise ValueError("cannot summarize no samples")
    result = {
        "samples": len(rows),
        "wallMsMedian": _number(statistics.median(row["wallMs"] for row in rows)),
        "wallMsP90": _number(percentile90(row["wallMs"] for row in rows)),
        "peakRssKiBMedian": _number(statistics.median(row["peakRssKiB"] for row in rows)),
        "peakRssKiBP90": _number(percentile90(row["peakRssKiB"] for row in rows)),
        "outputHashes": _stable_single((row["outputSha256"] for row in rows), "output hash"),
        "outputBytes": _stable_single((row["outputBytes"] for row in rows), "output byte count")[0],
    }
    by_query: dict[str, list[dict]] = {}
    for row in rows:
        for query in row.get("queries", []):
            by_query.setdefault(query["name"], []).append(query)
    result["queries"] = {
        name: {
            "resultHashes": _stable_single(
                (query["resultSha256"] for query in values), f"query {name} result hash"
            ),
            "wallMsMedian": _number(statistics.median(query["wallMs"] for query in values)),
            "wallMsP90": _number(percentile90(query["wallMs"] for query in values)),
            "rangeGets": _stable_single(
                (query["rangeGets"] for query in values), f"query {name} range GET count"
            )[0]
            if "rangeGets" in values[0]
            else None,
            "rangeBytes": _stable_single(
                (query["rangeBytes"] for query in values), f"query {name} range byte count"
            )[0]
            if "rangeBytes" in values[0]
            else None,
        }
        for name, values in sorted(by_query.items())
    }
    return result


class StrictRangeServer:
    """Serve exactly one local file, accepting only valid single byte ranges."""

    def __init__(self, file_path: pathlib.Path):
        self.file_path = file_path.resolve()
        self.gets = 0
        self.bytes_served = 0
        self.rejected_gets = 0
        handler = self._handler_type()
        self._server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    def _handler_type(self):
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, _format: str, *_args: object) -> None:
                pass

            def do_HEAD(self) -> None:
                self.send_error(405, "Range GET required")

            def do_GET(self) -> None:
                owner.gets += 1
                requested = urllib.parse.unquote(urllib.parse.urlparse(self.path).path).lstrip("/")
                if requested != owner.file_path.name:
                    owner._reject(self, owner.file_path.stat().st_size)
                    return
                range_header = self.headers.get("Range")
                match = RANGE_RE.fullmatch(range_header or "")
                size = owner.file_path.stat().st_size
                if match is None:
                    owner._reject(self, size)
                    return
                start, end = (int(match.group(1)), int(match.group(2)))
                if start > end or end >= size or (start == 0 and end == size - 1):
                    owner._reject(self, size)
                    return
                length = end - start + 1
                with owner.file_path.open("rb") as source:
                    source.seek(start)
                    self.send_response(206)
                    self.send_header("Content-Type", "application/octet-stream")
                    self.send_header("Accept-Ranges", "bytes")
                    self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
                    self.send_header("Content-Length", str(length))
                    self.end_headers()
                    remaining = length
                    while remaining:
                        chunk = source.read(min(RANGE_CHUNK_BYTES, remaining))
                        if not chunk:
                            owner._reject(self, size)
                            return
                        self.wfile.write(chunk)
                        owner.bytes_served += len(chunk)
                        remaining -= len(chunk)

        return Handler

    def _reject(self, handler: BaseHTTPRequestHandler, size: int) -> None:
        self.rejected_gets += 1
        handler.send_response(416)
        handler.send_header("Content-Range", f"bytes */{size}")
        handler.send_header("Content-Length", "0")
        handler.end_headers()

    @property
    def url(self) -> str:
        host, port = self._server.server_address[:2]
        return f"http://{host}:{port}/{urllib.parse.quote(self.file_path.name)}"

    def __enter__(self) -> "StrictRangeServer":
        self._thread.start()
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join()


def _time_command(command: list[str]) -> tuple[subprocess.CompletedProcess[bytes], int]:
    start = time.perf_counter_ns()
    try:
        completed = subprocess.run(command, check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    except OSError as error:
        raise RuntimeError(f"required benchmark tool is unavailable: {command[0]} ({error})") from error
    wall_ms = round((time.perf_counter_ns() - start) / 1_000_000, 3)
    return completed, wall_ms


def _peak_rss_kib(stderr: bytes) -> int:
    match = re.search(rb"Maximum resident set size \(kbytes\):\s*(\d+)", stderr)
    if match is None:
        raise ValueError("/usr/bin/time -v did not report peak RSS")
    return int(match.group(1))


def _run_build(command: list[str]) -> tuple[int | float, int]:
    completed, wall_ms = _time_command(["/usr/bin/time", "-v", *command])
    if completed.returncode:
        stderr = completed.stderr.decode("utf-8", "replace")
        raise RuntimeError(f"build failed ({completed.returncode}): {stderr}")
    return wall_ms, _peak_rss_kib(completed.stderr)


def _resolve_input(workload: Workload, input_root: pathlib.Path) -> pathlib.Path:
    root = input_root.resolve()
    input_path = (root / pathlib.PurePosixPath(workload.input)).resolve()
    if os.path.commonpath((str(root), str(input_path))) != str(root):
        raise ValueError("resolved workload input escapes --input-root")
    if not input_path.is_file():
        raise ValueError(f"workload input does not exist: {input_path}")
    actual = sha256_file(input_path)
    if actual != workload.sha256:
        raise ValueError(f"input SHA-256 mismatch for {input_path}: expected {workload.sha256}, got {actual}")
    return input_path


def _external_budget(args: tuple[str, ...]) -> int | None:
    indexes = [index for index, arg in enumerate(args) if arg == "--memory-budget-mb"]
    if not indexes:
        return None
    if len(indexes) != 1 or indexes[0] + 1 >= len(args):
        raise ValueError("external sample must carry exactly one --memory-budget-mb value")
    value = args[indexes[0] + 1]
    if not value.isdecimal() or int(value) <= 0:
        raise ValueError("--memory-budget-mb must have a positive integer value")
    return int(value)


def _sample_output_path(
    workload: Workload, output_dir: pathlib.Path, implementation: str, repetition: int
) -> pathlib.Path:
    """Derive an exclusive artifact name for one implementation/configuration."""
    budget = _external_budget(workload.args)
    suffix = f"-mb{budget}" if budget is not None else ""
    return output_dir / f"{workload.name}-{implementation}{suffix}-r{repetition}.rete"


class IdentityLedger:
    """Reject mutable executables and any output/query identity split immediately."""

    def __init__(self, executable_sha256: dict[str, str]):
        self.executable_sha256 = executable_sha256
        self.output_sha256: dict[str, str] = {}
        self.query_sha256: dict[str, dict[str, str]] = {}

    def verify(self, row: dict) -> None:
        implementation = row.get("implementation")
        expected_executable = self.executable_sha256.get(implementation)
        if expected_executable is None:
            raise ValueError(f"sample has unknown implementation: {implementation}")
        if row.get("executableSha256") != expected_executable:
            raise ValueError(f"executable SHA-256 drift for {implementation}")
        output_sha256 = row.get("outputSha256")
        expected_output = self.output_sha256.setdefault(implementation, output_sha256)
        if output_sha256 != expected_output:
            raise ValueError(f"output hash drift for {implementation} across benchmark matrix")
        query_sha256 = {query["name"]: query["resultSha256"] for query in row.get("queries", [])}
        if len(query_sha256) != len(row.get("queries", [])):
            raise ValueError("duplicate query result name in sample")
        expected_queries = self.query_sha256.setdefault(implementation, query_sha256)
        if query_sha256 != expected_queries:
            raise ValueError(f"query hash drift for {implementation} across benchmark matrix")


def _run_query(executable: pathlib.Path, query: Query, output_path: pathlib.Path) -> dict:
    has_url = "{url}" in query.args
    if has_url and "{output}" in query.args:
        raise ValueError(f"query {query.name} cannot use both {{output}} and {{url}}")
    if has_url:
        with StrictRangeServer(output_path) as server:
            args = [server.url if arg == "{url}" else arg for arg in query.args]
            completed, wall_ms = _time_command([str(executable), *args])
            result = {
                "name": query.name,
                "wallMs": wall_ms,
                "resultSha256": hashlib.sha256(completed.stdout).hexdigest(),
                "rangeGets": server.gets,
                "rangeBytes": server.bytes_served,
                "rangeRejectedGets": server.rejected_gets,
            }
    else:
        args = [str(output_path) if arg == "{output}" else arg for arg in query.args]
        completed, wall_ms = _time_command([str(executable), *args])
        result = {
            "name": query.name,
            "wallMs": wall_ms,
            "resultSha256": hashlib.sha256(completed.stdout).hexdigest(),
        }
    if completed.returncode:
        stderr = completed.stderr.decode("utf-8", "replace")
        raise RuntimeError(f"query {query.name} failed ({completed.returncode}): {stderr}")
    if result["resultSha256"] != query.sha256:
        raise ValueError(
            f"query {query.name} SHA-256 mismatch: expected {query.sha256}, got {result['resultSha256']}"
        )
    return result


def run_sample(
    executable: pathlib.Path,
    workload: Workload,
    input_root: pathlib.Path,
    output_dir: pathlib.Path,
    implementation: str,
    repetition: int,
    expected_executable_sha256: str | None = None,
) -> dict:
    """Run one isolated build and its isolated query processes."""
    executable = _require_executable(executable)
    executable_sha256 = sha256_file(executable)
    if expected_executable_sha256 is not None and executable_sha256 != expected_executable_sha256:
        raise ValueError(f"executable SHA-256 drift for {implementation}")
    if not SAFE_NAME_RE.fullmatch(implementation):
        raise ValueError("implementation contains unsafe filename characters")
    input_path = _resolve_input(workload, input_root)
    budget = _external_budget(workload.args)
    if workload.mode == "external" and budget is None:
        raise ValueError("external samples require --memory-budget-mb")
    if workload.mode == "standard" and budget is not None:
        raise ValueError("standard samples must not carry --memory-budget-mb")
    if not output_dir.is_dir():
        raise ValueError(f"sample artifact namespace is not claimed: {output_dir}")
    output_path = _sample_output_path(workload, output_dir, implementation, repetition)
    if output_path.exists():
        raise FileExistsError(f"refusing to overwrite sample output: {output_path}")
    wall_ms, peak_rss_kib = _run_build(
        [str(executable), "build", str(input_path), "-o", str(output_path), *workload.args]
    )
    if sha256_file(executable) != executable_sha256:
        raise ValueError(f"executable SHA-256 drift for {implementation}")
    if not output_path.is_file():
        raise RuntimeError(f"build completed without output: {output_path}")
    queries = [_run_query(executable, query, output_path) for query in workload.queries]
    if sha256_file(executable) != executable_sha256:
        raise ValueError(f"executable SHA-256 drift for {implementation}")
    result = {
        "schemaVersion": SCHEMA_VERSION,
        "kind": "SAMPLE",
        "workload": workload.name,
        "mode": workload.mode,
        "gateClass": workload.gate_class,
        "implementation": implementation,
        "repetition": repetition,
        "inputSha256": workload.sha256,
        "executableSha256": executable_sha256,
        "wallMs": wall_ms,
        "peakRssKiB": peak_rss_kib,
        "outputSha256": sha256_file(output_path),
        "outputBytes": output_path.stat().st_size,
        "queries": queries,
    }
    if budget is not None:
        result["memoryBudgetMb"] = budget
    return result


def _write_record(evidence, record: dict) -> None:
    evidence.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n")
    evidence.flush()


def _with_external_budget(workload: Workload, budget: int) -> Workload:
    return dataclasses.replace(workload, args=(*workload.args, "--memory-budget-mb", str(budget)))


def _implementation_order(repetition: int) -> tuple[str, str]:
    return ("baseline", "candidate") if repetition % 2 == 0 else ("candidate", "baseline")


def _parse_budgets(value: str) -> tuple[int, ...]:
    try:
        budgets = tuple(int(part) for part in value.split(","))
    except ValueError as error:
        raise argparse.ArgumentTypeError("budgets must be comma-separated integers") from error
    if not budgets or any(budget <= 0 for budget in budgets):
        raise argparse.ArgumentTypeError("budgets must be positive integers")
    return budgets


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=pathlib.Path, required=True, help="immutable baseline rete executable")
    parser.add_argument("--candidate", type=pathlib.Path, required=True, help="immutable candidate rete executable")
    parser.add_argument("--workload", type=pathlib.Path, required=True, help="strict workload JSON")
    parser.add_argument("--input-root", type=pathlib.Path, required=True, help="root containing pinned inputs")
    parser.add_argument("--samples", type=int, default=15, help="accepted samples per implementation (minimum: 15)")
    parser.add_argument("--output", type=pathlib.Path, required=True, help="new JSONL evidence path")
    parser.add_argument(
        "--external-budgets", type=_parse_budgets, help="required external memory matrix: 64,256,1024"
    )
    args = parser.parse_args(argv)
    if args.samples < 15:
        parser.error("--samples must be at least 15")
    try:
        workload = load_workload(args.workload)
        if workload.mode == "external":
            if args.external_budgets != EXTERNAL_BUDGETS:
                parser.error("external workloads require --external-budgets 64,256,1024")
            budgets: tuple[int | None, ...] = args.external_budgets
        elif args.external_budgets is not None:
            parser.error("--external-budgets is only valid for an external workload")
        else:
            budgets = (None,)
        executables = {
            "baseline": _require_executable(args.baseline),
            "candidate": _require_executable(args.candidate),
        }
        executable_sha256 = {name: sha256_file(path) for name, path in executables.items()}
        identity = IdentityLedger(executable_sha256)
        with open_exclusive(args.output) as evidence:
            artifact_dir = claim_artifact_namespace(args.output)
            _write_record(
                evidence,
                {
                    "schemaVersion": SCHEMA_VERSION,
                    "kind": "SOURCE",
                    "workload": workload.name,
                    "input": workload.input,
                    "inputSha256": workload.sha256,
                    "baseline": {
                        "path": str(executables["baseline"]),
                        "sha256": executable_sha256["baseline"],
                    },
                    "candidate": {
                        "path": str(executables["candidate"]),
                        "sha256": executable_sha256["candidate"],
                    },
                    "samples": args.samples,
                    "externalBudgets": list(budget for budget in budgets if budget is not None),
                },
            )
            for budget in budgets:
                configured = _with_external_budget(workload, budget) if budget is not None else workload
                rows: dict[str, list[dict]] = {"baseline": [], "candidate": []}
                for warmup in (-2, -1):
                    for implementation in _implementation_order(warmup):
                        warmup_row = run_sample(
                            executables[implementation],
                            configured,
                            args.input_root,
                            artifact_dir,
                            implementation,
                            warmup,
                            expected_executable_sha256=executable_sha256[implementation],
                        )
                        identity.verify(warmup_row)
                for repetition in range(args.samples):
                    for implementation in _implementation_order(repetition):
                        row = run_sample(
                            executables[implementation],
                            configured,
                            args.input_root,
                            artifact_dir,
                            implementation,
                            repetition,
                            expected_executable_sha256=executable_sha256[implementation],
                        )
                        identity.verify(row)
                        rows[implementation].append(row)
                        _write_record(evidence, row)
                summary = {
                    "schemaVersion": SCHEMA_VERSION,
                    "kind": "SUMMARY",
                    "workload": workload.name,
                    "gateClass": workload.gate_class,
                    "baseline": summarize(rows["baseline"]),
                    "candidate": summarize(rows["candidate"]),
                }
                if budget is not None:
                    summary["memoryBudgetMb"] = budget
                _write_record(evidence, summary)
    except (ValueError, FileExistsError, RuntimeError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
