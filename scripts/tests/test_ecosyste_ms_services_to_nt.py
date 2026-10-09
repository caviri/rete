"""scripts/ecosyste_ms/services_to_nt.py — every IRI it emits must parse.

Run: python3 -m unittest scripts/tests/test_ecosyste_ms_services_to_nt.py
"""
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "ecosyste_ms" / "services_to_nt.py"


def load():
    spec = importlib.util.spec_from_file_location("services_to_nt", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


conv = load()


class AuthorityTests(unittest.TestCase):
    def test_the_published_defect_is_rejected_on_its_port(self):
        # The value that blocked the dump: IPv6 loopback, no brackets.
        self.assertEqual(conv.authority_defect("::1"), "port")

    def test_valid_authorities_pass(self):
        for a in ("github.com", "example.org:8080", "user:pw@host", "[::1]",
                  "[2001:db8::7]:443", "127.0.0.1", "xn--bcher-kva.example",
                  "bücher.example", "a%20b.example", "", ":80", "[v1.fe80::a]"):
            with self.subTest(authority=a):
                self.assertIsNone(conv.authority_defect(a))

    def test_invalid_authorities_fail(self):
        for a, why in (("host:port", "port"), ("[::1", "unclosed IP-literal"),
                       ("[nope]", "IP-literal"), ("[fe80::1%25eth0]", "IP-literal"),
                       ("[::1]x", "text after IP-literal"), ("ho^st", "host"),
                       ("a b", "host"), ("u[x@host", "userinfo")):
            with self.subTest(authority=a):
                self.assertEqual(conv.authority_defect(a), why)


class EscapeTests(unittest.TestCase):
    def test_repairable_defects_are_escaped_not_dropped(self):
        cases = {
            "http://www.digitalmunition.com/DMA[2005-0131a].txt":
                "http://www.digitalmunition.com/DMA%5B2005-0131a%5D.txt",
            "https://matrix.to/#/#room:matrix.org":
                "https://matrix.to/#/%23room:matrix.org",
            "https://x.org/a b": "https://x.org/a%20b",
            "https://x.org/100%": "https://x.org/100%25",
            "https://x.org/%41": "https://x.org/%41",
            "https://[::1]:8080/p": "https://[::1]:8080/p",
            "https://bücher.example/ü": "https://bücher.example/ü",
        }
        for raw, want in cases.items():
            with self.subTest(raw=raw):
                audit = conv.IriAudit()
                self.assertEqual(conv.data_iri(raw, audit, "f"), f"<{want}>")
                self.assertEqual(sum(audit.dropped.values()), 0)

    def test_unrepairable_values_are_dropped_and_counted_never_rewritten(self):
        audit = conv.IriAudit()
        self.assertIsNone(conv.data_iri("https://::1", audit, "sponsor.website"))
        self.assertEqual(audit.dropped["sponsor.website"], 1)
        self.assertEqual(audit.reasons["sponsor.website"]["port"], 1)
        self.assertEqual(audit.samples["sponsor.website"], ["https://::1"])

    def test_non_http_values_are_skipped_apart_from_drops(self):
        audit = conv.IriAudit()
        for v in ("www.example.org", "mailto:a@b.c", "HTTP://upper.case"):
            self.assertIsNone(conv.data_iri(v, audit, "f"))
        self.assertEqual(audit.skipped["f"], 3)
        self.assertEqual(sum(audit.dropped.values()), 0)

    def test_absent_values_are_not_counted(self):
        audit = conv.IriAudit()
        for v in (None, "", "   ", 7):
            self.assertIsNone(conv.data_iri(v, audit, "f"))
        self.assertEqual(audit.seen, 0)


class ConvertTests(unittest.TestCase):
    def fixture(self, root: Path):
        def put(rel, rows):
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(json.dumps(rows), encoding="utf-8")

        put("sponsors/accounts.json", [
            {"login": "steamfoundry", "html_url": "https://github.com/steamfoundry",
             "data": {"kind": "user", "website": "https://::1"}},
            {"login": "ok", "data": {"website": "https://ok.example/"}},
        ])
        put("advisories/advisories.json", [
            {"uuid": "A1", "title": "t",
             "references": ["http://x.org/DMA[1].txt", "not a url"]},
        ])
        for rel in ("opencollective/collectives.json", "opencollective/projects.json",
                    "awesome/lists.json", "awesome/topics.json", "ost/projects.json"):
            put(rel, [])

    def test_convert_drops_exactly_the_invalid_statement(self):
        with tempfile.TemporaryDirectory() as d:
            self.fixture(Path(d))
            out, audit = io.BytesIO(), conv.IriAudit()
            n = conv.convert(d, out, audit)
        text = out.getvalue().decode("utf-8")
        self.assertNotIn("::1", text)
        self.assertIn("<https://ok.example/>", text)
        self.assertIn("<http://x.org/DMA%5B1%5D.txt>", text)
        self.assertEqual(n, len(text.splitlines()))
        self.assertEqual(sum(audit.dropped.values()), 1)
        self.assertEqual(audit.skipped["advisory.references"], 1)
        try:
            import pyoxigraph
        except ImportError:
            return
        # The referee the publication gate uses: the whole output must parse.
        list(pyoxigraph.parse(text.encode("utf-8"), format=pyoxigraph.RdfFormat.N_TRIPLES))


if __name__ == "__main__":
    unittest.main()
