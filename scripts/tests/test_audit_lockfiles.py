"""Tests for the policy logic of scripts/audit_lockfiles.py.

The audit itself shells out to cargo-audit and npm, which need the advisory
database and the network; these tests cover the decisions made on their
reports — what fails, what an exception covers, when an exception is stale or
expired — plus the real policy file's own consistency.

Run: python3 -m unittest scripts/tests/test_audit_lockfiles.py
"""

import datetime as dt
import pathlib
import sys
import tempfile
import textwrap
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))

import audit_lockfiles as audit  # noqa: E402

POLICY = textwrap.dedent(
    """
    [[cargo]]
    path = "."
    ships = "crates"

    [[cargo]]
    path = "clients/python"
    ships = "wheels"

    [[npm]]
    path = "clients/js"
    ships = "npm"

    [[npm]]
    path = "tests/gate"
    skip = "test only"

    [[exception]]
    id = "RUSTSEC-0000-0001"
    crate = "quick-xml"
    lockfiles = [".", "clients/python"]
    reason = "blocked upstream"
    review_by = 2027-01-21
    """
)


def write(directory, name, text):
    path = pathlib.Path(directory) / name
    path.write_text(text, encoding="utf-8")
    return path


def finding(ident, kind="vulnerability", package="quick-xml 0.37.5"):
    return audit.Finding(ident, package, kind, "title")


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def load(self, text):
        return audit.load_policy(write(self.tmp.name, "policy.toml", text))

    def test_valid_policy_has_no_errors(self):
        policy = self.load(POLICY)
        self.assertEqual(policy.errors, [])
        self.assertEqual(audit.excepted_ids(policy, "."), {"RUSTSEC-0000-0001"})
        self.assertEqual(audit.excepted_ids(policy, "clients/js"), set())

    def test_exception_needs_reason_date_and_known_lockfiles(self):
        policy = self.load(
            POLICY
            + textwrap.dedent(
                """
                [[exception]]
                id = "RUSTSEC-0000-0002"
                crate = "x"
                lockfiles = ["*"]
                """
            )
        )
        joined = "\n".join(policy.errors)
        self.assertIn("missing `reason`", joined)
        self.assertIn("missing `review_by`", joined)
        self.assertIn("unknown lockfile project '*'", joined)

    def test_project_needs_ships_or_skip(self):
        policy = self.load('[[cargo]]\npath = "fuzz"\n')
        self.assertIn("needs `ships`", policy.errors[0])

    def test_deadlines(self):
        policy = self.load(POLICY)
        on_time = dt.date(2026, 10, 21)
        self.assertEqual(audit.check_deadlines(policy, on_time, {"."}), [])
        expired = audit.check_deadlines(policy, dt.date(2027, 1, 22), {"."})
        self.assertIn("expired on 2027-01-21", expired[0])
        too_far = audit.check_deadlines(policy, dt.date(2026, 6, 1), {"."})
        self.assertIn("at most a quarter apart", too_far[0])
        # An exception for other projects does not hold up a scoped run.
        self.assertEqual(audit.check_deadlines(policy, dt.date(2027, 1, 22), {"clients/js"}), [])

    def test_deny_toml_must_match_the_root_exceptions(self):
        policy = self.load(POLICY)
        same = write(self.tmp.name, "deny.toml", '[advisories]\nignore = [{ id = "RUSTSEC-0000-0001", reason = "r" }]\n')
        self.assertEqual(audit.check_deny_toml(policy, same), [])
        extra = write(self.tmp.name, "deny2.toml", '[advisories]\nignore = ["RUSTSEC-0000-0001", "RUSTSEC-9999-9999"]\n')
        self.assertIn("RUSTSEC-9999-9999", audit.check_deny_toml(policy, extra)[0])
        missing = write(self.tmp.name, "deny3.toml", "[advisories]\nignore = []\n")
        self.assertIn("deny.toml does not ignore", audit.check_deny_toml(policy, missing)[0])


class EvaluateTests(unittest.TestCase):
    def setUp(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.policy = audit.load_policy(write(tmp, "policy.toml", POLICY))

    def test_excepted_finding_passes(self):
        self.assertEqual(audit.evaluate(".", [finding("RUSTSEC-0000-0001")], self.policy, check_stale=True), [])

    def test_exception_is_per_lockfile(self):
        errors = audit.evaluate("clients/js", [finding("RUSTSEC-0000-0001")], self.policy, check_stale=True)
        self.assertEqual(len(errors), 1)
        self.assertIn("clients/js: RUSTSEC-0000-0001", errors[0])

    def test_unexcepted_finding_of_any_kind_fails(self):
        for kind in ("vulnerability", "unmaintained", "unsound", "notice"):
            errors = audit.evaluate(".", [finding("RUSTSEC-0000-0001"), finding("RUSTSEC-0000-0009", kind)], self.policy, True)
            self.assertEqual(len(errors), 1, kind)
            self.assertIn(f"({kind})", errors[0])

    def test_yanked_cannot_be_excepted(self):
        errors = audit.evaluate(".", [finding("RUSTSEC-0000-0001", kind="yanked")], self.policy, check_stale=False)
        self.assertIn("yanked", errors[0])

    def test_stale_exception_fails_only_when_asked(self):
        self.assertEqual(audit.evaluate(".", [], self.policy, check_stale=False), [])
        errors = audit.evaluate(".", [], self.policy, check_stale=True)
        self.assertIn("matches nothing any more", errors[0])


class ParserTests(unittest.TestCase):
    def test_cargo_audit_report(self):
        report = {
            "vulnerabilities": {
                "list": [
                    {
                        "advisory": {"id": "RUSTSEC-2025-0020", "title": "overflow"},
                        "package": {"name": "pyo3", "version": "0.23.5"},
                    }
                ]
            },
            "warnings": {
                "unmaintained": [
                    {
                        "kind": "unmaintained",
                        "advisory": {"id": "RUSTSEC-2024-0436", "title": "paste"},
                        "package": {"name": "paste", "version": "1.0.15"},
                    }
                ],
                "yanked": [{"kind": "yanked", "advisory": None, "package": {"name": "foo", "version": "1.0.0"}}],
            },
        }
        found = audit.parse_cargo_audit(report)
        self.assertEqual(
            [(f.id, f.kind) for f in found],
            [("RUSTSEC-2025-0020", "vulnerability"), ("RUSTSEC-2024-0436", "unmaintained"), ("yanked:foo@1.0.0", "yanked")],
        )

    def test_npm_audit_report(self):
        report = {
            "vulnerabilities": {
                "qs": {
                    "via": [
                        {
                            "source": 1,
                            "name": "qs",
                            "title": "DoS",
                            "url": "https://github.com/advisories/GHSA-4mjr-xmp4-gh2g",
                            "severity": "moderate",
                            "range": "<6.16.0",
                        }
                    ]
                },
                "express": {"via": ["qs"]},
            }
        }
        found = audit.parse_npm_audit(report)
        self.assertEqual([(f.id, f.kind) for f in found], [("GHSA-4mjr-xmp4-gh2g", "moderate")])
        with self.assertRaises(RuntimeError):
            audit.parse_npm_audit({"error": {"code": "ENOLOCK"}})


class RealPolicyTests(unittest.TestCase):
    """The committed policy file parses and agrees with deny.toml."""

    def test_committed_policy(self):
        policy = audit.load_policy()
        self.assertEqual(policy.errors, [])
        self.assertEqual(audit.check_deny_toml(policy), [])
        for exc in policy.exceptions:
            self.assertNotIn("*", exc["lockfiles"])


if __name__ == "__main__":
    unittest.main()
