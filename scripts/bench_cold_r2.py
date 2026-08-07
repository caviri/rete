#!/usr/bin/env python3
"""Benchmark cold native CLI reads against the pinned Chemotion R2 object."""

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
} GROUP BY ?formula ORDER BY DESC(?molecules) LIMIT 20""",
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
class Mode:
    name: str
    executable: pathlib.Path
    eager_max_mb: int


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


def require_pinned_metadata(source, phase):
    length, etag = head_metadata(source)
    if length != EXPECTED_LENGTH or etag != EXPECTED_ETAG:
        raise RuntimeError(
            f"{phase} HEAD metadata changed: got length={length}, etag={etag!r}; "
            f"expected length={EXPECTED_LENGTH}, etag={EXPECTED_ETAG!r}"
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
    if arguments.thresholds is not None:
        return [
            Mode(f"threshold_{threshold}", candidate, threshold)
            for threshold in arguments.thresholds
        ]
    if arguments.baseline is None:
        raise ValueError("--baseline is required unless --thresholds is supplied")
    return [
        Mode("baseline_lazy", pathlib.Path(arguments.baseline), 0),
        Mode("delegated_lazy", candidate, 0),
        Mode("eager_8", candidate, 8),
    ]


def parse_arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline")
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--thresholds", type=parse_thresholds)
    parser.add_argument("--samples", type=int, default=15)
    parser.add_argument("--source", required=True)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    arguments = parser.parse_args()
    if arguments.samples <= 0:
        parser.error("--samples must be positive")
    return arguments


def main():
    arguments = parse_arguments()
    modes = build_modes(arguments)
    for mode in modes:
        if not mode.executable.is_file():
            raise RuntimeError(f"executable does not exist: {mode.executable}")
        if not os.access(mode.executable, os.X_OK):
            raise RuntimeError(f"executable is not executable: {mode.executable}")

    length, etag = require_pinned_metadata(arguments.source, "before")
    print(
        "SOURCE "
        + json.dumps(
            {"source": arguments.source, "length": length, "etag": etag},
            sort_keys=True,
        ),
        flush=True,
    )
    for path in dict.fromkeys(mode.executable for mode in modes):
        print(
            "EXECUTABLE "
            + json.dumps(
                {"path": str(path), "sha256": executable_sha256(path)},
                sort_keys=True,
            ),
            flush=True,
        )

    records = []
    expected_hashes = {}
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    with arguments.out.open("x", encoding="utf-8") as output:
        for label, query in QUERIES:
            for run in range(1, arguments.samples + 1):
                for mode in rotating_order(modes, run):
                    wall_ms, byte_count, get_count, peak_rss_kib, stdout = run_one(
                        mode, arguments.source, query
                    )
                    result_hash = hashlib.sha256(stdout).hexdigest()
                    expected = expected_hashes.get(label)
                    if expected is None:
                        expected_hashes[label] = result_hash
                    record = {
                        "query": label,
                        "mode": mode.name,
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

    after = require_pinned_metadata(arguments.source, "after")
    if after != (length, etag):
        raise RuntimeError(f"source metadata changed during benchmark: {after!r}")

    for label, _query in QUERIES:
        for mode in modes:
            sample = [
                record
                for record in records
                if record["query"] == label and record["mode"] == mode.name
            ]
            summary = {"query": label, "mode": mode.name, **summarize(sample)}
            print("SUMMARY " + json.dumps(summary, sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
