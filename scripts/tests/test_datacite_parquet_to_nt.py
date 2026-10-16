"""scripts/datacite/parquet_to_nt.py — faithful values, valid IRIs, every drop counted.

The inputs are real values from the DataCite Parquet audit
(dev/datacite-parquet-audit/REPORT.md, out/07*.json, out/11_bad_quads.nt).
Validity is decided by pyoxigraph's IRI parser (oxiri, as `rete build --strict`
and Oxigraph use), never by the converter's own escaping rules.

Run: python3 -m unittest scripts/tests/test_datacite_parquet_to_nt.py
(pyoxigraph required; pyarrow is not: no Parquet is read here.)
"""
import importlib.util
import json
import os
import re
import sys
import types
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "datacite" / "parquet_to_nt.py"
BAD_QUADS = Path(os.environ.get(
    "DC_BAD_QUADS", Path(__file__).parents[2] / "dev/datacite-parquet-audit/out/11_bad_quads.nt"))


def load():
    for name in ("pyarrow", "pyarrow.parquet"):
        sys.modules.setdefault(name, types.ModuleType(name))
    spec = importlib.util.spec_from_file_location("dc_parquet_to_nt", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


conv = load()
import pyoxigraph  # noqa: E402  (required: the parser is the judge)

DOI = "https://doi.org/"


def parses(iri):
    try:
        pyoxigraph.NamedNode(iri)
        return True
    except ValueError:
        return False


def ctx():
    t = conv.Tally()
    return conv.Ctx("test", t), t


def counts(t, rule=None):
    out = {}
    for k, v in t.c.items():
        _, r, what = k.split("|", 2)
        if rule is None or r == rule:
            out[what] = out.get(what, 0) + v
    return out


def parse_nt(lines):
    data = "".join(lines).encode("utf-8")
    return list(pyoxigraph.parse(data, format=pyoxigraph.RdfFormat.N_TRIPLES))


class DoiNormaliserR1(unittest.TestCase):
    def test_clean_doi_is_untouched(self):
        self.assertEqual(conv.norm_doi("10.5281/zenodo.712319"), ("10.5281/zenodo.712319", "clean"))
        self.assertEqual(conv.norm_doi("10.1021/ACS.JNATPROD.9B00744"),
                         ("10.1021/acs.jnatprod.9b00744", "clean"))  # DOIs are case-insensitive
        self.assertEqual(conv.norm_doi(" 10.1021/acs.jnatprod.9b00744 "),
                         ("10.1021/acs.jnatprod.9b00744", "trimmed"))  # counted

    def test_url_and_scheme_wrappers_are_stripped(self):
        cases = {
            "https://doi.org/10.1088/1741-4326/aa66db": "10.1088/1741-4326/aa66db",
            "http://dx.doi.org/10.1101/010991": "10.1101/010991",
            "doi.org/10.25667/ethnographiques/2017-35": "10.25667/ethnographiques/2017-35",
            "doi:10.11588/data/9zqikf/zqml6d": "10.11588/data/9zqikf/zqml6d",
            "doi: 10.1063/1.4946871": "10.1063/1.4946871",
            "doi://10.5285/f8f45574-ee63-4dcb-bdf6-8c291bb65793": "10.5285/f8f45574-ee63-4dcb-bdf6-8c291bb65793",
            " https://doi.org/10.1785/0120020020": "10.1785/0120020020",
            "https://doi.org//10.1029/2011JB008968": "10.1029/2011jb008968",
            "\n\t\t10.2210/pdb4qkj/pdb\n\t": "10.2210/pdb4qkj/pdb",
            "http://dx.doi.org/10.13039/501100000780": "10.13039/501100000780",
            "https://dx.doi.org/10.1038%2fs41597-019-0106-6": "10.1038/s41597-019-0106-6",
        }
        for raw, want in cases.items():
            got, how = conv.norm_doi(raw)
            self.assertEqual(got, want, raw)
            self.assertNotEqual(how, "clean", raw)

    def test_schemeless_link_wrappers(self):
        self.assertEqual(conv.norm_doi("%2010.1039/c8sc03649g")[0], "10.1039/c8sc03649g")
        self.assertEqual(conv.norm_doi("%7C%5D10.1037/dev0001533")[0], "10.1037/dev0001533")
        self.assertEqual(conv.norm_doi("10%20.1128/mbio.02878-19")[0], "10.1128/mbio.02878-19")
        self.assertEqual(conv.norm_doi("doi/10.2766/40788")[0], "10.2766/40788")
        self.assertEqual(conv.norm_doi("org/10.46887/2016-008")[0], "10.46887/2016-008")

    def test_no_doi_is_not_invented(self):
        for raw in ("conn028-hahnemann-1", "https://doi.org/0.1088/1741-4326/ab1234", "not found",
                    "TBD", "unknown", "10.20367", "1007/s10584-012-0513-6",
                    "0.1109/hpcc/smartcity/dss.2019.00279", "journal.pone.0199499", ""):
            self.assertIsNone(conv.norm_doi(raw)[0], raw)
            self.assertIsNone(conv.norm_doi(raw, embedded=True)[0], raw)

    def test_embedded_doi_only_when_exactly_one_and_delimited(self):
        self.assertEqual(conv.norm_doi("10.1049/iet-opt.2018.5028 ,", embedded=True),
                         ("10.1049/iet-opt.2018.5028", "embedded"))
        self.assertEqual(conv.norm_doi("doi:10.7289/v5mp5171 [dataset]", embedded=True)[0],
                         "10.7289/v5mp5171")
        self.assertEqual(conv.norm_doi("10.5281/zenodo.17445469 (DOI)", embedded=True)[0],
                         "10.5281/zenodo.17445469")
        self.assertEqual(conv.norm_doi("https://raid.org/10.71821/418be95a", embedded=True)[0],
                         "10.71821/418be95a")
        # a DOI broken by a space, or followed by more words: where it ends is a guess
        for raw in ("10.3969/j.issn.1000 1565.2017.02.005",
                    "10.1016/j.procir.2025.02.004 get rights and content",
                    "10.20382/jocg.v2i1a3ff. ffinria-00595823"):
            self.assertIsNone(conv.norm_doi(raw, embedded=True)[0], raw)
        # spaces around the prefix separator are not part of any DOI
        self.assertEqual(conv.norm_doi("10.13822 / j.cnki.hxsj.2017.06.018")[0],
                         "10.13822/j.cnki.hxsj.2017.06.018")
        # glued to other digits: which DOI was meant is a guess -> dropped
        for raw in ("10.510.5281/zenodo.15089", "110.25810/n3qx-9x31", "10.110.1080/00222933.2012.681314"):
            self.assertIsNone(conv.norm_doi(raw, embedded=True)[0], raw)
        two = ("10.5281/zenodo.17763643 (isCitedBy)  # AR4274 characterization "
               "https://doi.org/10.5281/zenodo.17795900 (isCitedBy)")
        self.assertEqual(conv.norm_doi(two, embedded=True), (None, "several_dois"))
        # without `embedded` (funders, record DOIs) nothing is extracted
        self.assertIsNone(conv.norm_doi("10.1049/iet-opt.2018.5028 ,")[0])


class IriEncodingR2(unittest.TestCase):
    def test_sici_brackets_are_encoded_and_parse(self):
        iri = conv.iri_escape(DOI + "10.1636/0161-8202(2002)030[0307:tfgafe]2.0.co;2")
        self.assertEqual(iri, DOI + "10.1636/0161-8202(2002)030%5B0307:tfgafe%5D2.0.co;2")
        self.assertTrue(parses(iri))

    def test_repairable_classes(self):
        cases = {
            DOI + "10.6092/2240-7715/2014.2.%p": DOI + "10.6092/2240-7715/2014.2.%25p",
            DOI + "10.15203/atem_%x": DOI + "10.15203/atem_%25x",
            DOI + "10.1/a#b#c": DOI + "10.1/a#b%23c",
            DOI + "10.1175/1520-0485(2004)034<2027:tiorbm>2.0.co;2":
                DOI + "10.1175/1520-0485(2004)034%3C2027:tiorbm%3E2.0.co;2",
            DOI + "10.1161/â\u0080\u008bcirc": DOI + "10.1161/â%C2%80%C2%8Bcirc",
            DOI + "10.1/x�y": DOI + "10.1/x%EF%BF%BDy",
            DOI + "10.1/xy": DOI + "10.1/x%EF%81%A1y",
            DOI + "10.1/x￾y": DOI + "10.1/x%EF%BF%BEy",
        }
        for raw, want in cases.items():
            got = conv.iri_escape(raw)
            self.assertEqual(got, want, raw)
            self.assertTrue(parses(got), got)

    def test_valid_iris_do_not_move(self):
        for v in (DOI + "10.5281/zenodo.712319", DOI + "10.1/café", DOI + "10.1/a%20b",
                  "https://ror.org/02b5d8509", "https://orcid.org/0000-0002-1825-0097",
                  DOI + "10.1/a#frag"):
            self.assertEqual(conv.iri_escape(v), v)
            self.assertTrue(parses(v))

    @unittest.skipUnless(BAD_QUADS.exists(), "audit fixture 11_bad_quads.nt not present")
    def test_every_iri_of_the_published_bad_quads(self):
        """Each IRI the old build stored invalid: escaping repairs it, unless it has no scheme."""
        tok = re.compile(r"<([^>]*)>")
        bad, fixed, schemeless = set(), 0, set()
        with open(BAD_QUADS, encoding="utf-8") as fh:
            for line in fh:
                for iri in tok.findall(line.split('"', 1)[0]):
                    if not parses(iri):
                        bad.add(iri)
        for iri in bad:
            if not conv._SCHEME_RE.match(iri):
                schemeless.add(iri)
                continue
            self.assertTrue(parses(conv.iri_escape(iri)), iri)
            fixed += 1
        self.assertGreater(fixed, 10000)
        # the 40 scheme-less link values (as subjects of reified relations they DO have a scheme)
        self.assertLessEqual(len(schemeless), 40)


class LinkEndpointsR5R6(unittest.TestCase):
    SCHEMELESS = {  # the 40 distinct values of the audit (§5.2), value -> rescued DOI or None
        "%7C%5D10.1037/dev0001533": "10.1037/dev0001533",
        "%2010.1039/c8sc03649g": "10.1039/c8sc03649g",
        "%2010.15802/stp2019/159489": "10.15802/stp2019/159489",
        "%2010.6084/m9.figshare.24361180": "10.6084/m9.figshare.24361180",
        "%2010.1007/s11192-022-04510-7": "10.1007/s11192-022-04510-7",
        "%2010.11587/nnldtl": "10.11587/nnldtl",
        "%2010.1038/s41598-021-04728-0.": "10.1038/s41598-021-04728-0.",
        "%2010.3390/v16060880": "10.3390/v16060880",
        "%2010.1371/journal.pntd.0011601": "10.1371/journal.pntd.0011601",
        "%2010.25678/000033": "10.25678/000033",
        "%2010.1039/c8nr08595a": "10.1039/c8nr08595a",
        "%2010.1055/s-0042-1747674": "10.1055/s-0042-1747674",
        "%2010.5281/zenodo.10333295": "10.5281/zenodo.10333295",
        "%2010.1371/journal.pone.0205526": "10.1371/journal.pone.0205526",
        "%2010.1109/tgrs.2020.3008655": "10.1109/tgrs.2020.3008655",
        "%2010.3390/environments11060111": "10.3390/environments11060111",
        "%2010.3390/vetsci11060278": "10.3390/vetsci11060278",
        "10%20.1128/mbio.02878-19": "10.1128/mbio.02878-19",
        "doi/10.2766/9691": "10.2766/9691",
        "doi/10.2766/40788": "10.2766/40788",
        "org/10.46887/2016-008": "10.46887/2016-008",
        # heuristic-only: a DOI could be guessed, so it is not
        "1007/s10584-012-0513-6": None, "0.1109/hpcc/smartcity/dss.2019.00279": None,
        "journal.pone.0199499": None, "ejt.2021.746.1319": None, "na23oar4590364": None,
        # junk
        "doesnotexistyet": None, "doesnotexistyets": None, "d589xn": None, "bs2r3k": None,
        "bfwrwj": None, "bfbtwf": None, "d4qcrc": None, "bjmrwh": None, "rda00070": None,
        "b926dx": None, "dhs5g4": None, "rda00068": None, "cdkghx": None, "na": None,
    }

    def test_the_forty_schemeless_values(self):
        self.assertEqual(len(self.SCHEMELESS), 40)
        rescued = dropped = 0
        for raw, want in self.SCHEMELESS.items():
            x, t = ctx()
            iri = conv.pid_iri(raw, x, "obj")
            if want is None:
                self.assertIsNone(iri, raw)
                dropped += 1
            else:
                self.assertEqual(iri, DOI + want, raw)
                self.assertTrue(parses(iri))
                rescued += 1
            self.assertEqual(sum(t.c.values()) >= 1, True)
        self.assertEqual((rescued, dropped), (21, 19))

    def test_dois_orcids_handles_urls(self):
        x, _ = ctx()
        self.assertEqual(conv.pid_iri("10.5281/zenodo.4633398", x, "subj"), DOI + "10.5281/zenodo.4633398")
        self.assertEqual(conv.pid_iri("https://doi.org/10.5281/ZENODO.1", x, "obj"), DOI + "10.5281/zenodo.1")
        self.assertEqual(conv.pid_iri("http://orcid.org/0000-0002-8250-6253", x, "obj"),
                         "https://orcid.org/0000-0002-8250-6253")
        self.assertEqual(conv.pid_iri("https://orcid.org/0000-0001-7854-580x", x, "obj"),
                         "https://orcid.org/0000-0001-7854-580X")
        self.assertEqual(conv.pid_iri("hdl:10013/epic.12345", x, "obj"),
                         "https://hdl.handle.net/10013/epic.12345")
        self.assertEqual(conv.pid_iri("https://ror.org/02tyer376", x, "obj"), "https://ror.org/02tyer376")
        self.assertEqual(conv.pid_iri("oai:zbmath.org:6800000", x, "obj"), "oai:zbmath.org:6800000")
        sici = conv.pid_iri("10.1130/0016-7606(1970)81[1625:sbszod]2.0.co;2", x, "obj")
        self.assertEqual(sici, DOI + "10.1130/0016-7606(1970)81%5B1625:sbszod%5D2.0.co;2")

    def test_unhyphenated_orcid_is_the_same_id(self):
        x, t = ctx()
        self.assertEqual(conv.pid_iri("https://orcid.org/0000000218250097", x, "obj"),
                         "https://orcid.org/0000-0002-1825-0097")
        self.assertEqual(counts(t, "R6"), {"rescued.obj.orcid_unhyphenated": 1})
        self.assertEqual(conv.find_orcid("https://orcid.org/0000000328880321")[0], "0000-0003-2888-0321")

    def test_orcid_failing_check_is_not_minted(self):
        x, t = ctx()
        # a URL the source states is kept verbatim (not canonicalised) ...
        self.assertEqual(conv.pid_iri("https://orcid.org/0000-0000-0000-0000", x, "obj"),
                         "https://orcid.org/0000-0000-0000-0000")
        # ... a bare id we would have to turn into an IRI ourselves is dropped
        self.assertIsNone(conv.pid_iri("0000-0002-1825-0023", x, "obj"))
        c = counts(t, "R6")
        self.assertEqual(c["obj.orcid_url_failed_check.kept_verbatim"], 1)
        self.assertEqual(c["dropped.obj.bare_orcid_failed_check"], 1)

    def test_url_the_parser_rejects_is_dropped_and_counted(self):
        x, t = ctx()
        self.assertIsNone(conv.pid_iri("https://::1/x", x, "obj"))
        self.assertEqual(counts(t, "R5"), {"dropped.link_obj.invalid_iri": 1})


class Checksums(unittest.TestCase):
    def test_orcid_mod11_2(self):
        self.assertTrue(conv.orcid_ok("0000-0002-1825-0097"))
        self.assertTrue(conv.orcid_ok("0000-0001-5109-3700"))
        self.assertTrue(conv.orcid_ok("0000-0002-1694-233X"))
        for bad in ("0000-0000-0000-0000", "0000-0002-1825-0023", "0000-00002-1825-009",
                    "0000-0002-1694-233x"):
            self.assertFalse(conv.orcid_ok(bad), bad)

    def test_ror(self):
        self.assertTrue(conv.ror_ok("02b5d8509"))
        self.assertTrue(conv.ror_ok("03yrm5c26"))
        self.assertFalse(conv.ror_ok("02b5d8508"))
        self.assertIsNone(conv.ror_from("https://ror.org/Hungarian%20Research%20Network"))
        # a malformed ror.org URL still names one id; minting waits for ror_ok
        self.assertEqual(conv.ror_from("https://ror.org00012xz55/"), "00012xz55")
        self.assertEqual(conv.ror_from("https://https://ror.org/017zqws13"), "017zqws13")
        self.assertEqual(conv.ror_from("https://ror.org/00q7mnz48/ROR"), "00q7mnz48")
        self.assertEqual(conv.ror_from("01s5ya894", typed_ror=True), "01s5ya894")
        self.assertIsNone(conv.ror_from("01s5ya894"))


def record(**kw):
    r = {c: None for c in conv.META_COLS}
    r.update(doi="10.17638/00008533", resource_type_general="Dataset")
    r.update(kw)
    return r


def run_meta(r, snapshot="2025"):
    x, t = ctx()
    out = []
    conv.emit_metadata(out, r, snapshot, x)
    triples = parse_nt(out)  # the whole record must parse with a strict parser
    return out, triples, t


class FundersR3R4(unittest.TestCase):
    def funders(self, refs, **kw):
        out, triples, t = run_meta(record(funding_references_json=json.dumps(refs), **kw))
        fb = [str(tr.object.value) for tr in triples if tr.predicate.value.endswith("#fundedBy")]
        return fb, triples, t, out

    def test_dx_doi_crossref_funder_joins_crossref(self):
        fb, *_ = self.funders([{"funderName": "European Commission",
                                "funderIdentifier": "http://dx.doi.org/10.13039/501100000780",
                                "funderIdentifierType": "Crossref Funder ID"}])
        self.assertEqual(fb, [DOI + "10.13039/501100000780"])

    def test_bare_crossref_funder_number(self):
        fb, *_ = self.funders([{"funderName": "NSF", "funderIdentifier": "100000001",
                                "funderIdentifierType": "Crossref Funder ID"}])
        self.assertEqual(fb, [DOI + "10.13039/100000001"])
        # a 9-digit value that is not a Funder Registry number is not prefixed
        fb, *_ = self.funders([{"funderName": "Y", "funderIdentifier": "https://doi.org/501100003407",
                                "funderIdentifierType": "Crossref Funder ID"}])
        self.assertEqual(fb, [DOI + "10.13039/501100003407"])
        fb, triples, t, _ = self.funders([{"funderName": "X", "funderIdentifier": "034900433",
                                           "funderIdentifierType": "Crossref Funder ID"}])
        self.assertIn("/funder/", fb[0])

    def test_ror_bare_and_url(self):
        fb, *_ = self.funders([{"funderName": "A", "funderIdentifier": "01s5ya894", "funderIdentifierType": "ROR"},
                               {"funderName": "B", "funderIdentifier": "https://ror.org/02b5d8509"}])
        self.assertEqual(fb, ["https://ror.org/01s5ya894", "https://ror.org/02b5d8509"])

    def test_ufffd_bracket_hash_long_names_mint_valid_hashed_iris(self):
        names = ["Funda�c~ao para a Ci^encia e",
                 "Consejo Nacional de Ciencia y Tecnolog��a (CONACYT)",
                 "JAO.ALMA [Cycle 4]",
                 "Montana Wheat and Barley Committee, NSF ##1552976",
                 "USDOE Office of Energy Efficiency and Renewable Energy (EERE), " * 4]
        fb, triples, _, _ = self.funders([{"funderName": n} for n in names])
        self.assertEqual(len(fb), len(names))
        for iri, n in zip(fb, names):
            self.assertTrue(parses(iri), iri)
            self.assertEqual(iri, DOI + "10.17638/00008533/funder/" + conv.name_key(n))
        got = {str(tr.object.value) for tr in triples if tr.predicate.value.endswith("#agentName")}
        self.assertEqual(got, set(names))  # names kept verbatim, never truncated

    def test_placeholder_and_unusable(self):
        fb, triples, t, _ = self.funders([
            {"funderName": "#PLACEHOLDER_PARENT_METADATA_VALUE#"},
            {"funderName": "", "funderIdentifier": ""},
            {"funderName": "#PLACEHOLDER_PARENT_METADATA_VALUE#",
             "funderIdentifier": "https://doi.org/10.13039/501100000780"}])
        self.assertEqual(fb, [DOI + "10.13039/501100000780"])
        self.assertFalse(any("PLACEHOLDER" in str(tr.object.value) for tr in triples))
        self.assertEqual(counts(t, "R4")["dropped.funder_without_name_or_usable_id"], 2)

    def test_grid_and_isni_are_kept_as_text(self):
        fb, triples, t, _ = self.funders([
            {"funderName": "Some Lab", "funderIdentifier": "grid.512915.b", "funderIdentifierType": "GRID"},
            {"funderName": "Other", "funderIdentifier": "\n\t\t0000 0001 2162 673X", "funderIdentifierType": "ISNI"}])
        raw = sorted(str(tr.object.value) for tr in triples if tr.predicate.value.endswith("#rawIdentifier"))
        self.assertEqual(raw, ["\n\t\t0000 0001 2162 673X", "grid.512915.b"])


class RecordFields(unittest.TestCase):
    def test_snapshot_provenance_and_types(self):
        out, triples, t = run_meta(record(resource_type_general="ComputationalNotebook"), "2023")
        po = {(tr.predicate.value, str(tr.object.value)) for tr in triples}
        self.assertIn((conv.D + "fromSnapshot", conv.D + "snapshot-2023"), po)
        self.assertIn((conv.RDF_TYPE, conv.D + "ComputationalNotebook"), po)

    def test_unknown_resource_type_general_is_kept_as_text(self):
        _, triples, _ = run_meta(record(resource_type_general="Film"))
        po = {(tr.predicate.value, str(tr.object.value)) for tr in triples}
        self.assertIn((conv.RDF_TYPE, conv.D + "Resource"), po)
        self.assertIn((conv.D + "resourceTypeGeneralText", "Film"), po)

    def test_publication_year_r11(self):
        for py, typed in ((2019, True), (1000, True), (2027, True), (0, False), (9999, False),
                          (2030, False), (850, False)):
            _, triples, _ = run_meta(record(publication_year=py))
            props = {tr.predicate.value: tr.object for tr in triples}
            if typed:
                self.assertEqual(props[conv.D + "publicationYear"].datatype.value, conv.XSD + "gYear")
                self.assertNotIn(conv.D + "publicationYearText", props)
            else:
                self.assertEqual(props[conv.D + "publicationYearText"].value, str(py))
                self.assertNotIn(conv.D + "publicationYear", props)

    def test_landing_page_r12(self):
        _, triples, t = run_meta(record(url='{"handle"=>"0.NA/10.2905"}'))
        lp = [tr.object for tr in triples if tr.predicate.value.endswith("#landingPage")]
        self.assertEqual(lp[0].datatype.value, conv.XSD + "string")
        _, triples, _ = run_meta(record(url="https://example.org/a"))
        lp = [tr.object for tr in triples if tr.predicate.value.endswith("#landingPage")]
        self.assertEqual(lp[0].datatype.value, conv.XSD + "anyURI")

    def test_literals_are_faithful_r13(self):
        title = "﻿Café � \u0082Ð¡  tail \x01 "
        _, triples, _ = run_meta(record(title=title))
        got = [tr.object.value for tr in triples if tr.predicate.value.endswith("#title")]
        self.assertEqual(got, [title])

    def test_record_doi_with_stray_percent(self):
        _, triples, _ = run_meta(record(doi="10.6092/2240-7715/2014.2.%p"))
        self.assertEqual(triples[0].subject.value, DOI + "10.6092/2240-7715/2014.2.%25p")

    def test_creators_orcid_and_ror_r6_r14(self):
        creators = [
            {"name": "Good", "nameIdentifiers": [{"nameIdentifier": "https://orcid.org/0000-0002-1825-0097",
                                                  "nameIdentifierScheme": "ORCID"}],
             "affiliation": [{"name": "U", "affiliationIdentifier": "https://ror.org/02b5d8509",
                              "affiliationIdentifierScheme": "ROR"}]},
            {"name": "Zeros", "nameIdentifiers": [{"nameIdentifier": "0000-0000-0000-0000",
                                                   "nameIdentifierScheme": "ORCID"}],
             "affiliation": [{"name": "HRN", "affiliationIdentifier":
                              "https://ror.org/Hungarian%20Research%20Network"}]},
            {"name": "Malformed", "nameIdentifiers": [{"nameIdentifier": "0000-00002-1825-0097",
                                                       "nameIdentifierScheme": "ORCID"}],
             "affiliation": "a plain string"},
        ]
        _, triples, t = run_meta(record(creators_json=json.dumps(creators)))
        agents = [str(tr.object.value) for tr in triples if tr.predicate.value.endswith("#creator")]
        self.assertEqual(agents[0], "https://orcid.org/0000-0002-1825-0097")
        self.assertTrue(agents[1].endswith("/creator/2/agent"))
        self.assertTrue(agents[2].endswith("/creator/3/agent"))
        raw = sorted(str(tr.object.value) for tr in triples if tr.predicate.value.endswith("#rawIdentifier"))
        self.assertEqual(raw, ["0000-0000-0000-0000", "0000-00002-1825-0097",
                               "https://ror.org/Hungarian%20Research%20Network"])
        ror = [str(tr.object.value) for tr in triples if tr.predicate.value.endswith("#rorId")]
        self.assertEqual(ror, ["https://ror.org/02b5d8509"])

    def test_related_identifiers_r1_r7_r8(self):
        rels = [
            {"relatedIdentifier": "https://doi.org/10.3133/SIR20195095", "relatedIdentifierType": "DOI",
             "relationType": "IsDocumentedBy"},
            {"relatedIdentifier": "10.1206/0003-0082(2006)3526[1:ansopt]2.0.co;2",
             "relatedIdentifierType": "DOI", "relationType": "IsDerivedFrom"},
            {"relatedIdentifier": "conn028-hahnemann-1", "relatedIdentifierType": "DOI", "relationType": "Cites"},
            {"relatedIdentifier": "10.1234/x", "relatedIdentifierType": "DOI", "relationType": "HasTranslation"},
            {"relatedIdentifier": "10.1234/v", "relatedIdentifierType": "DOI", "relationType": "IsVariantFormOf"},
            {"relatedIdentifier": "https://x.org", "relatedIdentifierType": "URL", "relationType": "Cites"},
        ]
        _, triples, t = run_meta(record(related_identifiers_json=json.dumps(rels)))
        got = sorted((tr.predicate.value[len(conv.D):], str(tr.object.value)) for tr in triples
                     if tr.predicate.value[len(conv.D):] in conv.REL_PROP.values())
        self.assertEqual(got, [
            ("hasTranslation", DOI + "10.1234/x"),
            ("isDerivedFrom", DOI + "10.1206/0003-0082(2006)3526%5B1:ansopt%5D2.0.co;2"),
            ("isDocumentedBy", DOI + "10.3133/sir20195095"),
            ("isVariantFormOf", DOI + "10.1234/v"),
        ])
        self.assertEqual(counts(t, "R8"), {"dropped.no_doi": 1})


class RelationVocabularyR7(unittest.TestCase):
    def test_all_38_datacite_46_types_have_their_own_property(self):
        self.assertEqual(len(conv.DATACITE_46_RELATIONS), 38)
        self.assertEqual(len(set(conv.DATACITE_46_RELATIONS)), 38)
        for term in conv.DATACITE_46_RELATIONS:
            prop = conv.REL_PROP[conv.relkey(term)]
            self.assertEqual(prop, term[0].lower() + term[1:])
            kebab = re.sub(r"(?<!^)([A-Z])", r"-\1", term).lower()
            self.assertEqual(conv.REL_PROP[conv.relkey(kebab)], prop, kebab)

    def test_properties_and_concepts_are_declared_in_the_ontology(self):
        ttl = (SCRIPT.parent / "datacite.ttl").read_text(encoding="utf-8")
        for prop in set(conv.REL_PROP.values()) | {"fromSnapshot", "rawIdentifier", "rawIdentifierScheme",
                                                   "publicationYearText", "resourceTypeGeneralText"}:
            self.assertRegex(ttl, rf"(?m)^dcite:{prop} a owl:", prop)
        for cls in set(conv.TYPE_CLASS.values()):
            self.assertRegex(ttl, rf"(?m)^dcite:{cls} +a owl:Class", cls)
        for rt in conv.DATACITE_46_RELATIONS + ["is-review-of", "is-authored-at", "is-funded-by"]:
            self.assertIn(f"dcite:{conv.rel_concept(rt)} a dcite:RelationType", ttl, rt)

    def test_concept_names(self):
        self.assertEqual(conv.rel_concept("is-supplement-to"), "rel-isSupplementTo")
        self.assertEqual(conv.rel_concept("IsSupplementTo"), "rel-isSupplementTo")
        self.assertEqual(conv.rel_concept("references"), "rel-references")


class LinkEvents(unittest.TestCase):
    def link(self, **kw):
        r = {"subj_id": "10.1002/2013jb010164", "obj_id": "10.1130/0016-7606(1970)81[1625:sbszod]2.0.co;2",
             "relation_type": "references", "source_id": "crossref",
             "occurred_at": "2013-11-01T00:00:00.000Z"}
        r.update(kw)
        x, t = ctx()
        out = []
        conv.emit_link(out, r, x)
        return parse_nt(out), t

    def test_reified_event_with_bracket_doi_parses(self):
        triples, _ = self.link()
        self.assertEqual(len(triples), 7)
        rel = [tr for tr in triples if tr.predicate.value.endswith("#relationType")][0]
        self.assertEqual(rel.object.value, conv.D + "rel-references")

    def test_sentinel_occurred_at_r11(self):
        triples, t = self.link(occurred_at="0000-01-01T00:00:00.000Z")
        self.assertFalse(any(tr.predicate.value.endswith("#occurredAt") for tr in triples))
        self.assertEqual(counts(t, "R11"), {"dropped.occurred_at_sentinel": 1})

    def test_relation_types(self):
        triples, _ = self.link(relation_type="has-metadata", obj_id="https://www.plate-archive.org/x")
        self.assertTrue(any(tr.predicate.value == conv.D + "hasMetadata" for tr in triples))
        triples, _ = self.link(relation_type="is-funded-by", obj_id="10.13039/501100001809")
        self.assertTrue(any(tr.predicate.value == conv.D + "fundedBy" for tr in triples))
        triples, t = self.link(relation_type="is-review-of")
        self.assertTrue(any(tr.predicate.value == conv.D + "isRelatedTo" for tr in triples))
        rel = [tr.object.value for tr in triples if tr.predicate.value.endswith("#relationType")]
        self.assertEqual(rel, [conv.D + "rel-isReviewOf"])

    def test_dropped_endpoint_drops_the_whole_event_and_counts_it(self):
        triples, t = self.link(obj_id="doesnotexistyet")
        self.assertEqual(triples, [])
        self.assertEqual(counts(t, "links"), {"dropped.event": 1})


class Literals(unittest.TestCase):
    def test_lit_escapes_without_removing(self):
        self.assertEqual(conv.lit('a"b\\c\nd\x01e'), 'a\\"b\\\\c\\nd\\u0001e')


if __name__ == "__main__":
    unittest.main()
