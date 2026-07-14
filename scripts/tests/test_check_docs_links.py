import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "check_docs_links.py"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_docs_links", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class LinkCheckerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def write(self, relative, body):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")

    def test_url_encoded_anchors_and_fragments_resolve(self):
        self.write(
            "index.html",
            '<a id="local"></a><a href="#local">local</a>'
            '<a href="target.html#property%20paths">target</a>',
        )
        self.write("target.html", '<h2 id="property paths">Paths</h2>')
        self.assertEqual(load_checker().check_docs(self.root), [])

    def test_duplicate_ids_are_reported(self):
        self.write("index.html", '<h2 id="same">One</h2><h3 id="same">Two</h3>')
        self.assertEqual(
            load_checker().check_docs(self.root),
            ["index.html -> duplicate anchor #same"],
        )

    def test_duplicate_heading_slugs_get_the_runtime_suffix(self):
        self.write(
            "index.html",
            '<h2>Same heading</h2><h3>Same heading</h3>'
            '<a href="#same-heading">one</a><a href="#same-heading-2">two</a>',
        )
        self.assertEqual(load_checker().check_docs(self.root), [])

    def test_directory_links_resolve_to_index_html(self):
        self.write("index.html", '<a href="guide/">Guide</a>')
        self.write("guide/index.html", "<h1>Guide</h1>")
        self.assertEqual(load_checker().check_docs(self.root), [])

    def test_missing_files_anchors_and_outside_root_are_reported(self):
        self.write(
            "index.html",
            '<a href="missing.html">missing</a>'
            '<a href="target.html#absent">anchor</a>'
            '<a href="../private.html">outside</a>'
            '<a href="https://example.com/x#y">external</a>'
            '<a href="mailto:test@example.com">mail</a>',
        )
        self.write("target.html", '<h1 id="present">Target</h1>')
        self.assertEqual(
            load_checker().check_docs(self.root),
            [
                "index.html -> ../private.html",
                "index.html -> missing.html",
                "index.html -> target.html#absent",
            ],
        )

    def test_playground_state_fragments_are_not_document_anchors(self):
        self.write(
            "index.html",
            '<a href="playground.html#dataset=worldcup2026&query=1">Open</a>',
        )
        self.write("playground.html", "<main>Playground</main>")
        self.assertEqual(load_checker().check_docs(self.root), [])


if __name__ == "__main__":
    unittest.main()
