"""Tests for scripts/github_release.py against a fake `gh` on PATH.

The fake keeps the repository's releases in a JSON file and implements the
three `gh api` calls the script makes (paginated list, create, update), with
GitHub's real quirk that matters here: drafts appear in the list but carry no
published tag binding, so only a list-and-match lookup can find them.

Run: python3 -m unittest scripts/tests/test_github_release.py
"""

import json
import os
import pathlib
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "github_release.py"

FAKE_GH = textwrap.dedent(
    r'''
    #!/usr/bin/env python3
    import json, os, re, sys
    path = os.environ["FAKE_GH_STATE"]
    state = json.load(open(path))
    args = sys.argv[1:]
    state["calls"].append(args)
    assert args[0] == "api", args
    args = args[1:]
    method = "GET"
    if "--method" in args:
        method = args[args.index("--method") + 1]
    endpoint = [a for a in args if a.startswith("repos/")][0]
    body = json.load(sys.stdin) if "--input" in args else None
    assert endpoint.startswith("repos/" + state["repo"] + "/releases"), endpoint
    out = None
    if method == "GET":
        state["lists"] += 1
        # A release another workflow creates "later" shows up after N lists.
        for pending in list(state["pending"]):
            if state["lists"] > pending["after_lists"]:
                state["releases"].append(pending["release"])
                state["pending"].remove(pending)
        # Newest first, two per page, printed back to back like --paginate.
        rel = sorted(state["releases"], key=lambda r: -r["id"])
        out = "".join(json.dumps(rel[i:i + 2]) for i in range(0, max(len(rel), 1), 2))
    elif method == "POST":
        if state.get("fail_create"):
            sys.stderr.write("HTTP 422: already_exists\n"); sys.exit(1)
        state["next_id"] += 1
        new = dict(body, id=state["next_id"], assets=[])
        state["releases"].append(new)
        out = json.dumps(new)
    elif method == "PATCH":
        rid = int(endpoint.rsplit("/", 1)[1])
        match = [r for r in state["releases"] if r["id"] == rid]
        if not match:
            sys.stderr.write("HTTP 404\n"); sys.exit(1)
        match[0].update(body)
        if body.get("make_latest") == "true":
            for r in state["releases"]:
                r["latest"] = r["id"] == rid
        out = json.dumps(match[0])
    json.dump(state, open(path, "w"))
    sys.stdout.write(out)
    '''
).lstrip()

CHANGELOG = textwrap.dedent(
    """\
    # Changelog

    ## [Unreleased]

    - not yet

    ## [0.3.5] - 2026-10-20

    ### Fixed

    - Release workflows create one release per tag.

    ## [0.3.4] - 2026-10-11

    - older
    """
)


class Harness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        gh = self.dir / "bin" / "gh"
        gh.parent.mkdir()
        gh.write_text(FAKE_GH)
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.state_path = self.dir / "state.json"
        self.set_state([])
        (self.dir / "CHANGELOG.md").write_text(CHANGELOG)
        (self.dir / "notes.md").write_text("notes\n")

    def tearDown(self):
        self.tmp.cleanup()

    def set_state(self, releases, pending=(), **extra):
        state = {
            "repo": "caviri/rete",
            "releases": list(releases),
            "pending": list(pending),
            "calls": [],
            "lists": 0,
            "next_id": max([r["id"] for r in releases] + [100]),
        }
        state.update(extra)
        self.state_path.write_text(json.dumps(state))

    def state(self):
        return json.loads(self.state_path.read_text())

    def run_script(self, *args, ok=True):
        env = dict(os.environ)
        env["PATH"] = f"{self.dir / 'bin'}{os.pathsep}{env['PATH']}"
        env["FAKE_GH_STATE"] = str(self.state_path)
        env["GH_REPO"] = "caviri/rete"
        env["GITHUB_OUTPUT"] = str(self.dir / "github_output")
        env.pop("GITHUB_REPOSITORY", None)
        proc = subprocess.run(
            [sys.executable, str(SCRIPT), *args], capture_output=True, text=True, env=env
        )
        if ok:
            self.assertEqual(proc.returncode, 0, proc.stderr)
        else:
            self.assertNotEqual(proc.returncode, 0, proc.stdout)
        return proc

    def releases_for(self, tag):
        return [r for r in self.state()["releases"] if r["tag_name"] == tag]


def rel(id, tag, draft=False, prerelease=False, name=None):
    return {"id": id, "tag_name": tag, "draft": draft, "prerelease": prerelease, "name": name or tag}


class ClassifyTests(Harness):
    def test_zero_x_final_is_not_a_prerelease(self):
        for version in ["0.3.4", "0.1.0", "1.0.0", "0.3.4+build.7"]:
            self.assertIn("prerelease=false", self.run_script("classify", version).stdout, version)

    def test_semver_prereleases(self):
        for version in ["0.3.5-rc.1", "1.0.0-alpha", "1.0.0-beta.2", "0.4.0-rc7"]:
            self.assertIn("prerelease=true", self.run_script("classify", version).stdout, version)

    def test_garbage_version_fails(self):
        self.run_script("classify", "v0.3.4", ok=False)


class NotesTests(Harness):
    def test_notes_are_the_changelog_section(self):
        out = self.dir / "body.md"
        self.run_script("notes", "0.3.5", "--changelog", str(self.dir / "CHANGELOG.md"), "--output", str(out))
        body = out.read_text()
        self.assertTrue(body.startswith("### Fixed\n\n- Release workflows create one release per tag."))
        self.assertNotIn("older", body)
        self.assertNotIn("not yet", body)
        self.assertIn("blob/v0.3.5/CHANGELOG.md", body)

    def test_missing_section_fails_loudly(self):
        proc = self.run_script("notes", "0.3.6", "--changelog", str(self.dir / "CHANGELOG.md"), ok=False)
        self.assertIn("no '## [0.3.6]' section", proc.stderr)

    def test_empty_section_fails(self):
        (self.dir / "CHANGELOG.md").write_text("## [0.3.6] - x\n\n## [0.3.5] - y\n\n- a\n")
        self.run_script("notes", "0.3.6", "--changelog", str(self.dir / "CHANGELOG.md"), ok=False)

    def test_real_changelog_has_the_current_version(self):
        root = SCRIPT.parents[1]
        cargo = (root / "Cargo.toml").read_text()
        import re

        version = re.search(r'(?ms)^\[workspace\.package\].*?^version = "([^"]+)"', cargo).group(1)
        self.run_script("notes", version)


class EnsureTests(Harness):
    def test_creates_a_draft_when_nothing_exists(self):
        self.run_script("ensure", "v0.3.5", "--name", "Rete 0.3.5", "--notes-file", str(self.dir / "notes.md"))
        [created] = self.releases_for("v0.3.5")
        self.assertTrue(created["draft"])
        self.assertFalse(created["prerelease"])
        self.assertEqual(created["body"], "notes\n")
        self.assertIn(f"release_id={created['id']}", (self.dir / "github_output").read_text())

    def test_rc_tag_drafts_as_prerelease(self):
        self.run_script("ensure", "v0.4.0-rc.1", "--name", "x", "--notes-file", str(self.dir / "notes.md"))
        self.assertTrue(self.releases_for("v0.4.0-rc.1")[0]["prerelease"])

    def test_rerun_reuses_a_draft_it_cannot_see_by_tag(self):
        self.set_state([rel(7, "v0.3.5", draft=True)])
        proc = self.run_script("ensure", "v0.3.5", "--name", "x", "--notes-file", str(self.dir / "notes.md"))
        self.assertIn("release_id=7", proc.stdout)
        self.assertEqual(len(self.releases_for("v0.3.5")), 1)
        self.assertFalse([c for c in self.state()["calls"] if "POST" in c])

    def test_rerun_reuses_a_published_release_and_leaves_it_published(self):
        self.set_state([rel(7, "v0.3.5")])
        self.run_script("ensure", "v0.3.5", "--name", "x", "--notes-file", str(self.dir / "notes.md"))
        self.assertFalse(self.releases_for("v0.3.5")[0]["draft"])

    def test_finds_a_release_on_a_later_page(self):
        self.set_state([rel(i, f"py-v0.3.{i}") for i in range(1, 6)] + [rel(0, "v0.3.5", draft=True)])
        self.assertIn("release_id=0", self.run_script("find", "v0.3.5").stdout)

    def test_duplicate_releases_are_an_error_not_a_guess(self):
        # The exact v0.3.4 state before it was fixed by hand.
        self.set_state([
            rel(1, "v0.3.4", draft=True, prerelease=True, name="Rete File Explorer v0.3.4"),
            rel(2, "v0.3.4", prerelease=True, name="Rete 0.3.4"),
        ])
        proc = self.run_script("ensure", "v0.3.4", "--name", "x", "--notes-file", str(self.dir / "notes.md"), ok=False)
        self.assertIn("2 releases point at v0.3.4", proc.stderr)

    def test_explorer_tag_honours_explicit_flags(self):
        self.run_script(
            "ensure", "explorer-v0.3.5", "--name", "Rete File Explorer explorer-v0.3.5",
            "--notes-file", str(self.dir / "notes.md"), "--prerelease", "true", "--no-draft",
        )
        [created] = self.releases_for("explorer-v0.3.5")
        self.assertFalse(created["draft"])
        self.assertTrue(created["prerelease"])

    def test_find_reports_absence_with_exit_3(self):
        self.assertEqual(self.run_script("find", "v9.9.9", ok=False).returncode, 3)


class WaitTests(Harness):
    def test_waits_until_the_owner_creates_it(self):
        self.set_state([], pending=[{"after_lists": 2, "release": rel(55, "v0.3.5", draft=True)}])
        proc = self.run_script("wait", "v0.3.5", "--timeout", "30", "--interval", "0.01")
        self.assertIn("release_id=55", proc.stdout)
        self.assertEqual(self.state()["lists"], 3)
        self.assertFalse([c for c in self.state()["calls"] if "POST" in c], "wait must never create")

    def test_times_out_without_creating(self):
        proc = self.run_script("wait", "v0.3.5", "--timeout", "0.05", "--interval", "0.01", ok=False)
        self.assertIn("no release for v0.3.5", proc.stderr)
        self.assertEqual(self.state()["releases"], [])


class PublishTests(Harness):
    def history(self):
        return [rel(1, "v0.3.0"), rel(2, "v0.3.4"), rel(3, "explorer-v9.0.0"), rel(4, "py-v9.0.0"), rel(5, "v1.0.0-rc.1", prerelease=True)]


    def test_final_release_becomes_latest(self):
        self.set_state(self.history() + [rel(9, "v0.3.5", draft=True)])
        self.run_script("publish", "v0.3.5", "--version", "0.3.5")
        [r] = self.releases_for("v0.3.5")
        self.assertEqual((r["draft"], r["prerelease"], r["make_latest"], r.get("latest")), (False, False, "true", True))

    def test_rc_is_prerelease_and_not_latest(self):
        self.set_state(self.history() + [rel(9, "v0.4.0-rc.1", draft=True)])
        self.run_script("publish", "v0.4.0-rc.1", "--version", "0.4.0-rc.1")
        [r] = self.releases_for("v0.4.0-rc.1")
        self.assertEqual((r["draft"], r["prerelease"], r["make_latest"]), (False, True, "false"))

    def test_backport_does_not_steal_latest(self):
        self.set_state(self.history() + [rel(9, "v0.2.9", draft=True)])
        self.run_script("publish", "v0.2.9", "--version", "0.2.9")
        self.assertEqual(self.releases_for("v0.2.9")[0]["make_latest"], "false")

    def test_republish_is_idempotent(self):
        self.set_state(self.history() + [rel(9, "v0.3.5", draft=True)])
        self.run_script("publish", "v0.3.5", "--version", "0.3.5")
        self.run_script("publish", "v0.3.5", "--version", "0.3.5")
        [r] = self.releases_for("v0.3.5")
        self.assertEqual((r["draft"], r["make_latest"]), (False, "true"))

    def test_publish_without_a_release_fails(self):
        self.run_script("publish", "v0.3.5", "--version", "0.3.5", ok=False)


class FullTagFlowTests(Harness):
    """The next `v*` tag end to end, in the order the two workflows run."""

    def test_v_tag_flow_and_reruns(self):
        notes = self.dir / "body.md"
        changelog = str(self.dir / "CHANGELOG.md")
        self.set_state([rel(1, "v0.3.4"), rel(2, "explorer-v0.3.0-rc7", draft=True, prerelease=True)])

        # release.yml / release job: notes + draft.
        self.run_script("notes", "0.3.5", "--changelog", changelog, "--output", str(notes))
        rid = self.run_script("ensure", "v0.3.5", "--name", "Rete 0.3.5", "--notes-file", str(notes)).stdout
        # desktop-release.yml / create-release job on a v* tag: lookup only.
        self.assertEqual(self.run_script("wait", "v0.3.5", "--timeout", "1").stdout, rid)
        # release.yml / assemble: publish.
        self.run_script("publish", "v0.3.5", "--version", "0.3.5")

        # Re-run every job of both workflows.
        self.assertEqual(self.run_script("ensure", "v0.3.5", "--name", "Rete 0.3.5", "--notes-file", str(notes)).stdout, rid)
        self.assertEqual(self.run_script("wait", "v0.3.5", "--timeout", "1").stdout, rid)
        self.run_script("publish", "v0.3.5", "--version", "0.3.5")

        [r] = self.releases_for("v0.3.5")
        self.assertEqual((r["name"], r["draft"], r["prerelease"], r["make_latest"]), ("Rete 0.3.5", False, False, "true"))
        self.assertIn("Release workflows create one release per tag.", r["body"])
        self.assertEqual(len([c for c in self.state()["calls"] if "POST" in c]), 1)

    def test_desktop_started_first_still_makes_one_release(self):
        # desktop-release.yml reaches its lookup before release.yml's draft exists.
        self.set_state([], pending=[{"after_lists": 1, "release": rel(77, "v0.3.5", draft=True)}])
        self.assertIn("release_id=77", self.run_script("wait", "v0.3.5", "--timeout", "5", "--interval", "0.01").stdout)
        self.run_script("ensure", "v0.3.5", "--name", "Rete 0.3.5", "--notes-file", str(self.dir / "notes.md"))
        self.assertEqual(len(self.releases_for("v0.3.5")), 1)


if __name__ == "__main__":
    unittest.main()
