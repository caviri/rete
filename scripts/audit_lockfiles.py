#!/usr/bin/env python3
"""Audit every lockfile in the repository against lockfile-audit.toml.

`cargo audit` used to run on the workspace Cargo.lock only. The Python wheels,
the R package, the Java client's wasm, the desktop explorer and the fuzz
harness each lock their own dependency set, and nothing checked them: the 0.3.6
cut found two pyo3 advisories in the PyPI/Pyodide wheels by accident. This
script is the one gate for all of them, used by `release_check.sh security`
(CI, every lockfile, also nightly) and `publish_crates.sh` (the workspace
lockfile, before crates.io).

What it checks, in order:
  1. the policy file itself: every exception names known lockfiles, carries a
     reason and a review date no more than a quarter ahead, and has not
     expired; deny.toml's advisory ignores equal the exceptions naming ".";
  2. (full runs) every tracked Cargo.lock / package-lock.json is listed, and
     every tracked Cargo.toml belongs to a listed Cargo project;
  3. (full runs) each committed Cargo.lock agrees with the versions of the
     path crates it records (a lockfile cargo would rewrite is not the one
     being built);
  4. `cargo audit --json` per Cargo project and `npm audit --omit=dev --json`
     per npm package: every finding must be covered by an exception naming its
     id and that lockfile; (full runs) every exception must still match a
     finding, or it is stale.

Usage:
    python3 scripts/audit_lockfiles.py              # everything (CI)
    python3 scripts/audit_lockfiles.py --only .     # one project (publish gate)
    python3 scripts/audit_lockfiles.py --list       # show what would be audited

Needs cargo-audit (and npm for the npm packages) on PATH, and network access to
fetch the advisory database. RETE_AUDIT_TODAY=YYYY-MM-DD overrides the date the
review deadlines are measured against (for tests).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field

ROOT = pathlib.Path(__file__).resolve().parents[1]
POLICY = ROOT / "lockfile-audit.toml"
DENY = ROOT / "deny.toml"

# A renewal is a re-review, not a snooze: the deadline may sit at most this far
# ahead of the day it is checked.
MAX_REVIEW_HORIZON = dt.timedelta(days=92)

# cargo-audit's informational kinds. All of them fail unless excepted; a yanked
# crate has no advisory id and can never be excepted.
CARGO_WARNING_KINDS = ("unmaintained", "unsound", "notice", "yanked")


@dataclass(frozen=True)
class Finding:
    id: str
    package: str
    kind: str
    title: str


@dataclass
class Policy:
    cargo: dict[str, dict]
    npm: dict[str, dict]
    exceptions: list[dict]
    errors: list[str] = field(default_factory=list)


def load_policy(path: pathlib.Path = POLICY) -> Policy:
    """Parse and validate the policy file's shape. Problems land in `policy.errors`."""
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    errors: list[str] = []
    cargo: dict[str, dict] = {}
    npm: dict[str, dict] = {}
    for table, into in (("cargo", cargo), ("npm", npm)):
        for entry in data.get(table, []):
            key = entry.get("path")
            if not key:
                errors.append(f"[[{table}]] entry without a path: {entry}")
                continue
            if key in cargo or key in npm:
                errors.append(f"{key!r} is listed twice")
            if not (entry.get("ships") or entry.get("skip")):
                errors.append(f"[[{table}]] {key!r} needs `ships` (what it ships) or `skip` (why not)")
            if entry.get("lockfile", "committed") not in ("committed", "generated"):
                errors.append(f"[[{table}]] {key!r}: lockfile must be committed or generated")
            into[key] = entry
    known = set(cargo) | set(npm)
    exceptions = data.get("exception", [])
    seen: set[tuple[str, str]] = set()
    for exc in exceptions:
        ident = exc.get("id", "<no id>")
        for required in ("id", "lockfiles", "reason", "review_by"):
            if not exc.get(required):
                errors.append(f"exception {ident}: missing `{required}`")
        if not (exc.get("crate") or exc.get("package")):
            errors.append(f"exception {ident}: missing `crate` (Cargo) or `package` (npm)")
        lockfiles = exc.get("lockfiles") or []
        if not isinstance(lockfiles, list):
            errors.append(f"exception {ident}: `lockfiles` must be a list of project paths")
            lockfiles = []
        for lock in lockfiles:
            if lock not in known:
                errors.append(f"exception {ident}: unknown lockfile project {lock!r}")
            elif (ident, lock) in seen:
                errors.append(f"exception {ident}: {lock!r} is excepted twice")
            seen.add((ident, lock))
        review_by = exc.get("review_by")
        if review_by is not None and not isinstance(review_by, dt.date):
            errors.append(f"exception {ident}: review_by must be a TOML date (YYYY-MM-DD)")
    return Policy(cargo=cargo, npm=npm, exceptions=exceptions, errors=errors)


def check_deadlines(policy: Policy, today: dt.date, selected: set[str]) -> list[str]:
    """Expired or too-distant review dates, for exceptions touching `selected`.

    Scoped so that the crates.io gate (`--only .`) is not held up by, say, an
    expired desktop-app exception; the full CI run sees every one.
    """
    errors = []
    for exc in policy.exceptions:
        review_by = exc.get("review_by")
        if not isinstance(review_by, dt.date) or not selected & set(exc.get("lockfiles") or []):
            continue
        ident = exc.get("id", "<no id>")
        if review_by < today:
            errors.append(
                f"exception {ident} expired on {review_by}: check whether a fix has "
                "shipped, then remove the exception or renew it (new date, and a new "
                "reason if anything changed)"
            )
        elif review_by - today > MAX_REVIEW_HORIZON:
            errors.append(
                f"exception {ident}: review_by {review_by} is more than "
                f"{MAX_REVIEW_HORIZON.days} days away; renewals are at most a quarter apart"
            )
    return errors


def excepted_ids(policy: Policy, lockfile: str) -> set[str]:
    return {e["id"] for e in policy.exceptions if lockfile in (e.get("lockfiles") or [])}


def check_deny_toml(policy: Policy, deny_path: pathlib.Path = DENY) -> list[str]:
    """deny.toml (cargo deny, workspace only) must except exactly what "." does."""
    deny = tomllib.loads(deny_path.read_text(encoding="utf-8"))
    ignores = set()
    for item in deny.get("advisories", {}).get("ignore", []):
        ignores.add(item["id"] if isinstance(item, dict) else item)
    ours = excepted_ids(policy, ".")
    errors = []
    if ignores - ours:
        errors.append(f"deny.toml ignores {sorted(ignores - ours)} with no exception for '.' in lockfile-audit.toml")
    if ours - ignores:
        errors.append(f"lockfile-audit.toml excepts {sorted(ours - ignores)} for '.' but deny.toml does not ignore them")
    return errors


def evaluate(lockfile: str, findings: list[Finding], policy: Policy, check_stale: bool) -> list[str]:
    """The policy decision for one lockfile: unexcepted findings, stale exceptions."""
    allowed = excepted_ids(policy, lockfile)
    errors = []
    for f in findings:
        if f.kind == "yanked":
            errors.append(f"{lockfile}: {f.package} is yanked; update the lockfile (yanked crates cannot be excepted)")
        elif f.id not in allowed:
            errors.append(f"{lockfile}: {f.id} {f.package} ({f.kind}): {f.title}")
    if check_stale:
        found = {f.id for f in findings}
        for ident in sorted(allowed - found):
            errors.append(f"{lockfile}: exception {ident} matches nothing any more; delete it from lockfile-audit.toml")
    return errors


# --------------------------------------------------------------------------
# Collectors
# --------------------------------------------------------------------------


def parse_cargo_audit(report: dict) -> list[Finding]:
    findings = []
    for vuln in report.get("vulnerabilities", {}).get("list", []):
        adv, pkg = vuln["advisory"], vuln["package"]
        findings.append(Finding(adv["id"], f"{pkg['name']} {pkg['version']}", "vulnerability", adv.get("title", "")))
    for kind, warnings in (report.get("warnings") or {}).items():
        for warning in warnings:
            adv, pkg = warning.get("advisory") or {}, warning["package"]
            findings.append(
                Finding(
                    adv.get("id", f"{kind}:{pkg['name']}@{pkg['version']}"),
                    f"{pkg['name']} {pkg['version']}",
                    warning.get("kind", kind),
                    adv.get("title", kind),
                )
            )
    return findings


GHSA = re.compile(r"GHSA(-[0-9a-z]{4}){3}")


def parse_npm_audit(report: dict) -> list[Finding]:
    if "error" in report:
        raise RuntimeError(f"npm audit failed: {report['error']}")
    findings = {}
    for name, vuln in (report.get("vulnerabilities") or {}).items():
        for via in vuln.get("via", []):
            if not isinstance(via, dict):
                continue  # a transitive pointer; the advisory is listed on the named package
            match = GHSA.search(via.get("url", ""))
            ident = match.group(0) if match else f"npm:{via.get('source')}"
            findings[(ident, name)] = Finding(
                ident, f"{via.get('name', name)} {via.get('range', '')}".strip(), via.get("severity", "?"), via.get("title", "")
            )
    return list(findings.values())


class Runner:
    def __init__(self) -> None:
        self.fetched = False

    def cargo_audit(self, lockfile: pathlib.Path) -> list[Finding]:
        cmd = ["cargo", "audit", "--json", "--file", str(lockfile)]
        if self.fetched:
            cmd.insert(2, "--no-fetch")
        proc = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT)
        self.fetched = True
        try:
            report = json.loads(proc.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(f"cargo audit produced no report for {lockfile}:\n{proc.stderr.strip()}") from None
        return parse_cargo_audit(report)

    def npm_audit(self, directory: pathlib.Path) -> list[Finding]:
        proc = subprocess.run(
            ["npm", "audit", "--omit=dev", "--json"], capture_output=True, text=True, cwd=directory
        )
        try:
            report = json.loads(proc.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(f"npm audit produced no report in {directory}:\n{proc.stderr.strip()}") from None
        return parse_npm_audit(report)


# --------------------------------------------------------------------------
# Repository checks
# --------------------------------------------------------------------------


def tracked(patterns: list[str]) -> list[str] | None:
    try:
        out = subprocess.run(
            ["git", "ls-files", "-z", "--", *patterns], capture_output=True, text=True, cwd=ROOT, check=True
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    return [p for p in out.split("\0") if p]


def project_of(path: str) -> str:
    parent = str(pathlib.PurePosixPath(path).parent)
    return "." if parent in ("", ".") else parent


def project_metadata(project: str) -> tuple[set[str], dict[str, str]]:
    """The manifests cargo counts as part of this project (repo-relative), and
    the name -> version of each, from `cargo metadata --no-deps` (offline, no
    resolution, workspace inheritance already applied)."""
    manifest = ROOT / project / "Cargo.toml"
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(manifest)],
        capture_output=True, text=True, cwd=ROOT,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"cargo metadata failed for {manifest}:\n{proc.stderr.strip()}")
    meta = json.loads(proc.stdout)
    members = set(meta["workspace_members"])
    # The project's own manifest: a virtual workspace root is no package.
    manifests = {pathlib.PurePosixPath(project, "Cargo.toml").as_posix()}
    versions = {}
    for pkg in meta["packages"]:
        if pkg["id"] in members:
            manifests.add(pathlib.Path(pkg["manifest_path"]).resolve().relative_to(ROOT).as_posix())
            versions[pkg["name"]] = pkg["version"]
    return manifests, versions


def check_coverage(policy: Policy, manifests_by_project: dict[str, set[str]]) -> list[str]:
    locks = tracked(["*Cargo.lock", "*package-lock.json"])
    manifests = tracked(["*Cargo.toml"])
    if locks is None or manifests is None:
        message = "git is unavailable here, so the lockfile-coverage check did not run"
        if os.environ.get("CI"):
            return [message]
        print(f"warning: {message}", file=sys.stderr)
        return []
    errors = []
    for lock in locks:
        project = project_of(lock)
        table = policy.cargo if lock.endswith("Cargo.lock") else policy.npm
        if project not in table:
            errors.append(f"{lock} is not listed in lockfile-audit.toml (audit it, or skip it with a reason)")
    for project, entry in policy.cargo.items():
        committed = f"{project}/Cargo.lock".removeprefix("./")
        if entry.get("lockfile", "committed") == "committed" and committed not in locks:
            errors.append(f"{committed} is listed as committed but is not tracked")
    for project in policy.npm:
        if f"{project}/package-lock.json" not in locks:
            errors.append(f"{project}/package-lock.json is listed but is not tracked")
    covered = set().union(*manifests_by_project.values())
    for manifest in manifests:
        if manifest not in covered:
            errors.append(f"{manifest} belongs to no Cargo project listed in lockfile-audit.toml")
    return errors


def check_lock_freshness(project: str, versions: dict[str, str]) -> list[str]:
    """Path crates (no `source`) recorded at a version their manifest no longer
    has: cargo would rewrite this lockfile on the next build, so auditing it
    audits something nobody builds. (fuzz/Cargo.lock sat at rete-core 0.3.2,
    with ruzstd 0.7.3, through four releases.)"""
    lock = tomllib.loads((ROOT / project / "Cargo.lock").read_text(encoding="utf-8"))
    errors = []
    for pkg in lock.get("package", []):
        if "source" in pkg:
            continue
        expected = versions.get(pkg["name"])
        if expected is not None and expected != pkg["version"]:
            errors.append(
                f"{project}/Cargo.lock records {pkg['name']} {pkg['version']} but its manifest says "
                f"{expected}: the lockfile is stale (refresh it with `cargo update -p {pkg['name']}` there)"
            )
    return errors


# --------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--only", action="append", metavar="PATH", help="audit only this project path (repeatable)")
    parser.add_argument("--list", action="store_true", help="print the audited projects and exit")
    args = parser.parse_args(argv)

    override = os.environ.get("RETE_AUDIT_TODAY")
    today = dt.date.fromisoformat(override) if override else dt.date.today()
    policy = load_policy()
    errors = list(policy.errors) + check_deny_toml(policy)

    full = not args.only
    selected = set(args.only or []) or (set(policy.cargo) | set(policy.npm))
    unknown = selected - set(policy.cargo) - set(policy.npm)
    if unknown:
        parser.error(f"not in lockfile-audit.toml: {sorted(unknown)}")
    errors += check_deadlines(policy, today, selected)

    if args.list:
        for kind, table in (("cargo", policy.cargo), ("npm", policy.npm)):
            for path, entry in table.items():
                if path in selected:
                    state = f"skip: {entry['skip']}" if entry.get("skip") else entry.get("lockfile", "committed")
                    print(f"{kind:5} {path:28} {state}")
        return 0

    # Full runs only: which manifests exist, and whether each committed lockfile
    # agrees with them. (The workspace lockfile is also held to `--locked` by
    # every cargo invocation in CI and by `cargo package --locked` at publish.)
    versions: dict[str, str] = {}
    if full:
        manifests_by_project = {}
        for path in policy.cargo:
            try:
                manifests_by_project[path], project_versions = project_metadata(path)
            except RuntimeError as exc:
                errors.append(str(exc))
                continue
            versions.update(project_versions)
        errors += check_coverage(policy, manifests_by_project)

    runner = Runner()
    report_rows = []
    for path, entry in policy.cargo.items():
        if path not in selected or entry.get("skip"):
            continue
        lockfile = ROOT / path / "Cargo.lock"
        generated = entry.get("lockfile") == "generated" and not lockfile.exists()
        try:
            if generated:
                subprocess.run(
                    ["cargo", "generate-lockfile", "--manifest-path", str(ROOT / path / "Cargo.toml")],
                    check=True, cwd=ROOT, capture_output=True, text=True,
                )
            elif full:
                errors += check_lock_freshness(path, versions)
            findings = runner.cargo_audit(lockfile)
        except (RuntimeError, subprocess.CalledProcessError) as exc:
            detail = getattr(exc, "stderr", "") or ""
            errors.append(f"{path}: {exc} {detail}".strip())
            continue
        finally:
            if generated:
                lockfile.unlink(missing_ok=True)
        errors += evaluate(path, findings, policy, check_stale=full)
        report_rows.append((f"cargo {path}", findings))

    for path, entry in policy.npm.items():
        if path not in selected or entry.get("skip"):
            continue
        try:
            findings = runner.npm_audit(ROOT / path)
        except RuntimeError as exc:
            errors.append(f"{path}: {exc}")
            continue
        errors += evaluate(path, findings, policy, check_stale=full)
        report_rows.append((f"npm   {path}", findings))

    for name, findings in report_rows:
        allowed = excepted_ids(policy, name.split(maxsplit=1)[1])
        excepted = sorted({f.id for f in findings if f.id in allowed})
        other = sorted({f.id for f in findings if f.id not in allowed})
        line = f"{name:36} findings={len({f.id for f in findings})}"
        if excepted:
            line += f"  excepted={','.join(excepted)}"
        if other:
            line += f"  FAILING={','.join(other)}"
        print(line)

    if errors:
        print(f"\nlockfile audit: {len(errors)} problem(s)", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print("lockfile audit: clean (every finding is a documented, unexpired exception)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
