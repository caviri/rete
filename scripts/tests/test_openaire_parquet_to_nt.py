"""scripts/openaire/parquet_to_nt.py — every IRI it emits must parse, and every
statement it does not emit must be counted.

Run: python3 -m unittest scripts/tests/test_openaire_parquet_to_nt.py
(pyarrow is not needed: the IRI rules are tested without reading Parquet.)
"""
import importlib.util
import io
import sys
import types
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "openaire" / "parquet_to_nt.py"


def load():
    for name in ("pyarrow", "pyarrow.parquet"):
        sys.modules.setdefault(name, types.ModuleType(name))
    spec = importlib.util.spec_from_file_location("parquet_to_nt", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


conv = load()


def judge(value):
    audit = conv.IriAudit()
    return conv.data_iri(value, audit, "f"), audit


class ShapeTests(unittest.TestCase):
    """The four shapes measured on the refused 2021 shards, one by one."""

    def test_empty_is_dropped_and_counted(self):
        for v in ("", "   "):
            iri, audit = judge(v)
            self.assertIsNone(iri)
            self.assertEqual(audit.shapes["empty"], 1)
            self.assertEqual(audit.dropped["f"], 1)

    def test_bare_host_is_dropped_never_given_a_scheme(self):
        for v in ("www.jorl.net", "ujp.bitp.kiev.ua", "www.pubs.iscience.in/journal/x"):
            iri, audit = judge(v)
            self.assertIsNone(iri)
            self.assertEqual(audit.shapes["bare host"], 1, v)

    def test_broken_scheme_punctuation_is_repaired(self):
        iri, audit = judge("http//www.uvs.edu")
        self.assertEqual(iri, "http://www.uvs.edu")
        self.assertEqual(audit.rescued["f"], 1)
        self.assertEqual(sum(audit.dropped.values()), 0)

    def test_internal_id_is_dropped(self):
        for v in ("odesi", "intR", "None"):
            iri, audit = judge(v)
            self.assertIsNone(iri)
            self.assertEqual(audit.shapes["not a URL"], 1, v)

    def test_a_repair_that_leaves_a_bad_authority_is_dropped(self):
        # `http//:host` repairs to `http://:host`: empty host, port "host".
        iri, audit = judge("http//:hecon.uni-corvinus.hu")
        self.assertIsNone(iri)
        self.assertEqual(audit.shapes["invalid authority"], 1)
        self.assertEqual(sum(audit.rescued.values()), 0)

    def test_ipv6_without_brackets_is_dropped(self):
        iri, audit = judge("https://::1")
        self.assertIsNone(iri)
        self.assertEqual(audit.shapes["invalid authority"], 1)

    def test_report_has_one_parsable_shapes_line(self):
        audit = conv.IriAudit()
        for v in ("", "www.x.org", "http//x.org", "odesi", "https://ok.org/"):
            conv.data_iri(v, audit, "f")
        buf = io.StringIO()
        audit.report(buf)
        line = [l for l in buf.getvalue().splitlines() if l.startswith("IRI audit shapes:")]
        self.assertEqual(line, ["IRI audit shapes: repaired=1 bare_host=1 empty=1 not_a_URL=1"])


class EscapeTests(unittest.TestCase):
    def test_valid_iris_are_untouched(self):
        for v in ("https://doi.org/10.1000/abc#frag", "https://bücher.example/ü",
                  "https://x.org/%41", "https://[::1]:8080/p", "http:///x"):
            self.assertEqual(judge(v)[0], v)

    def test_repairable_defects_are_escaped(self):
        self.assertEqual(judge("https://ok.org/a b[1]#x#y")[0], "https://ok.org/a%20b%5B1%5D#x%23y")
        self.assertEqual(judge("https://x.org/100%")[0], "https://x.org/100%25")
        # (inside the value: str.strip() treats U+0085 as whitespace at the ends)
        self.assertEqual(judge("https://x.org/\u0085x")[0], "https://x.org/%C2%85x")

    def test_minted_ids_come_out_valid(self):
        # DOIs carry [ ] # % -- the minted IRI must survive `rete build --strict`.
        self.assertEqual(conv.ienc("10.1002/(SICI)[x]#a#b%zz"),
                         "10.1002/(SICI)%5Bx%5D#a%23b%25zz")
        self.assertEqual(conv.ienc("10.1000/plain"), "10.1000/plain")
        self.assertEqual(conv.ienc("50/doi_::a b"), "50/doi_::a%20b")


class ParserAgreementTests(unittest.TestCase):
    """When pyoxigraph is installed: what the converter keeps, the gate accepts."""

    def test_kept_values_parse_and_dropped_ones_do_not(self):
        try:
            import pyoxigraph
        except ImportError:
            self.skipTest("pyoxigraph not installed")
        values = ["https://ok.org/a b[1]#x#y", "http//www.uvs.edu", "https://x.org/\u0085x",
                  "http:///x", "https://::1", "http//:hecon.uni-corvinus.hu"]

        def parses(iri):
            line = f"<http://s> <http://p> <{iri}> .\n".encode("utf-8")
            try:
                list(pyoxigraph.parse(line, format=pyoxigraph.RdfFormat.N_TRIPLES))
                return True
            except SyntaxError:
                return False

        for v in values:
            iri, _ = judge(v)
            if iri is not None:
                self.assertTrue(parses(iri), iri)
            else:
                raw = v if "://" in v else v.replace("//", "://", 1)
                self.assertFalse(parses(raw), raw)


if __name__ == "__main__":
    unittest.main()
