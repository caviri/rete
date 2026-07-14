import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = (
    Path(__file__).parents[2]
    / "skills"
    / "rete-publish"
    / "scripts"
    / "upload_r2.py"
)


def load_uploader():
    spec = importlib.util.spec_from_file_location("upload_r2", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class UploadPlanTests(unittest.TestCase):
    def test_single_file_defaults_to_dataset_folder(self):
        uploader = load_uploader()
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "foo.rete"
            source.write_bytes(b"RETE")
            self.assertEqual(
                uploader.upload_plan(source, None),
                [(source, "foo/foo.rete")],
            )

    def test_explicit_key_is_preserved(self):
        uploader = load_uploader()
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "graph.rete"
            source.write_bytes(b"RETE")
            self.assertEqual(
                uploader.upload_plan(source, "foo/foo.rete"),
                [(source, "foo/foo.rete")],
            )

    def test_directory_upload_is_recursive_and_sorted(self):
        uploader = load_uploader()
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "companions"
            (source / "parquet").mkdir(parents=True)
            (source / "z.sqlite").write_bytes(b"z")
            (source / "parquet" / "a.parquet").write_bytes(b"a")
            self.assertEqual(
                uploader.upload_plan(source, "foo"),
                [
                    (source / "parquet" / "a.parquet", "foo/parquet/a.parquet"),
                    (source / "z.sqlite", "foo/z.sqlite"),
                ],
            )

    def test_parent_segments_and_empty_keys_are_rejected(self):
        uploader = load_uploader()
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "foo.rete"
            source.write_bytes(b"RETE")
            for key in ("", "/", "../foo.rete", "foo/../bar.rete"):
                with self.subTest(key=key), self.assertRaises(ValueError):
                    uploader.upload_plan(source, key)


if __name__ == "__main__":
    unittest.main()
