#!/usr/bin/env python3
"""Benchmark cold native CLI reads against a strictly pinned R2 workload."""

import argparse
import hashlib
import json
import math
import os
import pathlib
import re
import statistics
import subprocess
import tempfile
import time
import urllib.request
from dataclasses import dataclass


EXPECTED_LENGTH = 7_566_404
EXPECTED_ETAG = '"6cefd111dee3c59c063f0bede9cd60f9"'
TRANSFER_STATS = re.compile(rb"fetched (\d+) bytes in (\d+) range request\(s\)")

QUERIES = (
    (
        "select",
        """PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX obo: <http://purl.obolibrary.org/obo/>
PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>
SELECT ?name ?formula ?smiles WHERE {
  ?m a obo:CHEBI_23367 ; rdfs:label ?name ; chebi:formula ?formula ; chebi:smiles ?smiles
} ORDER BY ?name ?formula ?smiles LIMIT 200""",
    ),
    (
        "aggregate",
        """PREFIX chebi: <http://purl.obolibrary.org/obo/chebi/>
SELECT ?formula (COUNT(?m) AS ?molecules) WHERE {
  ?m chebi:formula ?formula
} GROUP BY ?formula ORDER BY DESC(?molecules) ?formula LIMIT 20""",
    ),
    (
        "path",
        """PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?name WHERE {
  ?sub rdfs:subClassOf+ <http://purl.obolibrary.org/obo/CHMO_0000228> ; rdfs:label ?name
} ORDER BY ?name LIMIT 200""",
    ),
)


@dataclass(frozen=True)
class Workload:
    name: str
    source: str
    expected_length: int
    expected_etag: str
    queries: tuple[tuple[str, str], ...]


CHEMOTION_WORKLOAD = Workload(
    name="chemotion",
    source="",
    expected_length=EXPECTED_LENGTH,
    expected_etag=EXPECTED_ETAG,
    queries=QUERIES,
)


def require_exact_fields(value, expected, context):
    if not isinstance(value, dict):
        raise ValueError(f"{context} must be a JSON object")
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        unknown = sorted(actual - expected)
        raise ValueError(
            f"invalid {context} fields: missing={missing}, unknown={unknown}"
        )


def require_nonblank_string(value, context):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{context} must be a non-blank string")
    return value


def reject_duplicate_json_fields(pairs):
    value = {}
    for key, member in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON field: {key!r}")
        value[key] = member
    return value


def load_workload(path: pathlib.Path) -> Workload:
    payload = json.loads(
        path.read_text(encoding="utf-8"),
        object_pairs_hook=reject_duplicate_json_fields,
    )
    require_exact_fields(
        payload,
        {"name", "source", "expected_length", "expected_etag", "queries"},
        "workload",
    )
    name = require_nonblank_string(payload["name"], "workload name")
    source = require_nonblank_string(payload["source"], "workload source")
    expected_etag = require_nonblank_string(
        payload["expected_etag"], "workload expected_etag"
    )
    expected_length = payload["expected_length"]
    if (
        not isinstance(expected_length, int)
        or isinstance(expected_length, bool)
        or expected_length <= 0
    ):
        raise ValueError("workload expected_length must be a positive integer")
    query_values = payload["queries"]
    if not isinstance(query_values, list) or not query_values:
        raise ValueError("workload queries must be a non-empty array")
    queries = []
    query_names = set()
    for index, query in enumerate(query_values):
        context = f"workload query {index}"
        require_exact_fields(query, {"name", "sparql"}, context)
        query_name = require_nonblank_string(query["name"], f"{context} name")
        sparql = require_nonblank_string(query["sparql"], f"{context} sparql")
        if query_name in query_names:
            raise ValueError(f"duplicate query name: {query_name!r}")
        query_names.add(query_name)
        queries.append((query_name, sparql))
    return Workload(
        name=name,
        source=source,
        expected_length=expected_length,
        expected_etag=expected_etag,
        queries=tuple(queries),
    )


@dataclass(frozen=True)
class Mode:
    name: str
    executable: pathlib.Path
    eager_max_mb: int
    read_policy: str
    git_revision: str


def parse_transfer_stats(stderr):
    matches = TRANSFER_STATS.findall(stderr)
    if len(matches) != 1:
        raise ValueError(
            f"expected exactly one transfer-stat line, found {len(matches)}"
        )
    byte_count, get_count = matches[0]
    return int(byte_count), int(get_count)


def nearest_rank_p90(values):
    if not values:
        raise ValueError("cannot compute p90 of an empty sample")
    ordered = sorted(values)
    return ordered[math.ceil(0.9 * len(ordered)) - 1]


def rotating_order(modes, run):
    ordered = list(modes)
    if not ordered:
        return ordered
    offset = (run - 1) % len(ordered)
    return ordered[offset:] + ordered[:offset]


def summarize(records):
    if not records:
        raise ValueError("cannot summarize an empty sample")
    byte_counts = {record["bytes"] for record in records}
    get_counts = {record["gets"] for record in records}
    if len(byte_counts) != 1 or len(get_counts) != 1:
        raise ValueError("transfer counts changed within one query/mode sample")
    walls = [record["wall_ms"] for record in records]
    rss = [record["peak_rss_kib"] for record in records]
    return {
        "median_ms": statistics.median(walls),
        "p90_ms": nearest_rank_p90(walls),
        "bytes": next(iter(byte_counts)),
        "gets": next(iter(get_counts)),
        "median_peak_rss_kib": statistics.median(rss),
        "p90_peak_rss_kib": nearest_rank_p90(rss),
        "max_peak_rss_kib": max(rss),
    }


def head_metadata(source):
    request = urllib.request.Request(
        source,
        headers={"User-Agent": "rete-cold-r2-benchmark/1"},
        method="HEAD",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        length_text = response.headers.get("Content-Length")
        etag = response.headers.get("ETag")
    if length_text is None or etag is None:
        raise RuntimeError("HEAD response is missing Content-Length or ETag")
    try:
        length = int(length_text)
    except ValueError as error:
        raise RuntimeError(f"invalid HEAD Content-Length: {length_text!r}") from error
    return length, etag


def require_pinned_metadata(workload, phase):
    length, etag = head_metadata(workload.source)
    if length != workload.expected_length or etag != workload.expected_etag:
        raise RuntimeError(
            f"{phase} HEAD metadata changed: got length={length}, etag={etag!r}; "
            f"expected length={workload.expected_length}, "
            f"etag={workload.expected_etag!r}"
        )
    return length, etag


def executable_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as executable:
        for chunk in iter(lambda: executable.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_vm_hwm_kib(pid):
    try:
        status = pathlib.Path(f"/proc/{pid}/status").read_text(encoding="ascii")
    except FileNotFoundError:
        return 0
    match = re.search(r"^VmHWM:\s+(\d+)\s+kB$", status, re.MULTILINE)
    return int(match.group(1)) if match else 0


def run_one(mode, source, query):
    environment = os.environ.copy()
    environment.pop("RETE_BLOCK_KB", None)
    environment["RETE_EAGER_MAX_MB"] = str(mode.eager_max_mb)
    environment["RETE_BENCH_READ_POLICY"] = mode.read_policy
    command = [str(mode.executable), "sparql-url", source, query, "--json"]
    with tempfile.TemporaryFile() as stdout_file, tempfile.TemporaryFile() as stderr_file:
        start = time.monotonic_ns()
        process = subprocess.Popen(
            command,
            env=environment,
            stdout=stdout_file,
            stderr=stderr_file,
        )
        peak_rss_kib = 0
        while process.poll() is None:
            peak_rss_kib = max(peak_rss_kib, read_vm_hwm_kib(process.pid))
            time.sleep(0.005)
        peak_rss_kib = max(peak_rss_kib, read_vm_hwm_kib(process.pid))
        wall_ms = (time.monotonic_ns() - start) // 1_000_000
        stdout_file.seek(0)
        stderr_file.seek(0)
        stdout = stdout_file.read()
        stderr = stderr_file.read()
    if process.returncode != 0:
        raise RuntimeError(
            f"command failed with exit {process.returncode}: {' '.join(command)}\n"
            f"{stderr.decode('utf-8', errors='replace')}"
        )
    if peak_rss_kib == 0:
        raise RuntimeError(f"could not observe VmHWM for process {process.pid}")
    byte_count, get_count = parse_transfer_stats(stderr)
    return wall_ms, byte_count, get_count, peak_rss_kib, stdout


def parse_thresholds(raw):
    thresholds = []
    for text in raw.split(","):
        try:
            threshold = int(text)
        except ValueError as error:
            raise argparse.ArgumentTypeError(
                f"thresholds must be comma-separated non-negative integers, got {raw!r}"
            ) from error
        if threshold < 0:
            raise argparse.ArgumentTypeError("thresholds must be non-negative")
        if threshold in thresholds:
            raise argparse.ArgumentTypeError(f"duplicate threshold: {threshold}")
        thresholds.append(threshold)
    if not thresholds:
        raise argparse.ArgumentTypeError("at least one threshold is required")
    return thresholds


def build_modes(arguments):
    candidate = pathlib.Path(arguments.candidate)
    candidate_revision = require_nonblank_string(
        arguments.candidate_revision, "candidate revision"
    )
    if arguments.thresholds is not None:
        return [
            Mode(
                f"threshold_{threshold}",
                candidate,
                threshold,
                "static_lazy" if threshold == 0 else "owned_memory",
                candidate_revision,
            )
            for threshold in arguments.thresholds
        ]
    if arguments.baseline is None:
        raise ValueError("--baseline is required unless --thresholds is supplied")
    baseline_revision = require_nonblank_string(
        arguments.baseline_revision, "baseline revision"
    )
    return [
        Mode(
            "baseline_lazy",
            pathlib.Path(arguments.baseline),
            0,
            "static_lazy",
            baseline_revision,
        ),
        Mode("adaptive_lazy", candidate, 0, "adaptive_lazy", candidate_revision),
    ]


def parse_arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline")
    parser.add_argument("--baseline-revision")
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--candidate-revision", required=True)
    parser.add_argument("--thresholds", type=parse_thresholds)
    parser.add_argument("--samples", type=int, default=15)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--source")
    source.add_argument("--workload", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    arguments = parser.parse_args()
    if arguments.samples <= 0:
        parser.error("--samples must be positive")
    if arguments.thresholds is None and arguments.baseline_revision is None:
        parser.error("--baseline-revision is required with --baseline")
    return arguments


def main():
    arguments = parse_arguments()
    if arguments.workload is not None:
        workload = load_workload(arguments.workload)
    else:
        workload = Workload(
            name=CHEMOTION_WORKLOAD.name,
            source=arguments.source,
            expected_length=CHEMOTION_WORKLOAD.expected_length,
            expected_etag=CHEMOTION_WORKLOAD.expected_etag,
            queries=CHEMOTION_WORKLOAD.queries,
        )
    modes = build_modes(arguments)
    for mode in modes:
        if not mode.executable.is_file():
            raise RuntimeError(f"executable does not exist: {mode.executable}")
        if not os.access(mode.executable, os.X_OK):
            raise RuntimeError(f"executable is not executable: {mode.executable}")

    length, etag = require_pinned_metadata(workload, "before")
    print(
        "SOURCE "
        + json.dumps(
            {
                "workload": workload.name,
                "source": workload.source,
                "length": length,
                "etag": etag,
            },
            sort_keys=True,
        ),
        flush=True,
    )
    executable_hashes = {
        path: executable_sha256(path)
        for path in dict.fromkeys(mode.executable for mode in modes)
    }
    for mode in modes:
        print(
            "EXECUTABLE "
            + json.dumps(
                {
                    "mode": mode.name,
                    "path": str(mode.executable),
                    "sha256": executable_hashes[mode.executable],
                    "git_revision": mode.git_revision,
                    "read_policy": mode.read_policy,
                },
                sort_keys=True,
            ),
            flush=True,
        )

    records = []
    expected_hashes = {}
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    with arguments.out.open("x", encoding="utf-8") as output:
        for label, query in workload.queries:
            for run in range(1, arguments.samples + 1):
                for mode in rotating_order(modes, run):
                    wall_ms, byte_count, get_count, peak_rss_kib, stdout = run_one(
                        mode, workload.source, query
                    )
                    result_hash = hashlib.sha256(stdout).hexdigest()
                    expected = expected_hashes.get(label)
                    if expected is None:
                        expected_hashes[label] = result_hash
                    record = {
                        "workload": workload.name,
                        "query": label,
                        "mode": mode.name,
                        "read_policy": mode.read_policy,
                        "git_revision": mode.git_revision,
                        "executable_sha256": executable_hashes[mode.executable],
                        "run": run,
                        "wall_ms": wall_ms,
                        "bytes": byte_count,
                        "gets": get_count,
                        "peak_rss_kib": peak_rss_kib,
                        "sha256": result_hash,
                        "length": length,
                        "etag": etag,
                    }
                    output.write(json.dumps(record, sort_keys=True) + "\n")
                    output.flush()
                    records.append(record)
                    print("SAMPLE " + json.dumps(record, sort_keys=True), flush=True)
                    if expected is not None and result_hash != expected:
                        raise RuntimeError(
                            f"output hash mismatch for {label}: {mode.name} run {run} "
                            f"produced {result_hash}, expected {expected}"
                        )

    after = require_pinned_metadata(workload, "after")
    if after != (length, etag):
        raise RuntimeError(f"source metadata changed during benchmark: {after!r}")

    for label, _query in workload.queries:
        for mode in modes:
            sample = [
                record
                for record in records
                if record["query"] == label and record["mode"] == mode.name
            ]
            summary = {
                "workload": workload.name,
                "query": label,
                "mode": mode.name,
                "read_policy": mode.read_policy,
                "git_revision": mode.git_revision,
                "executable_sha256": executable_hashes[mode.executable],
                **summarize(sample),
            }
            print("SUMMARY " + json.dumps(summary, sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
