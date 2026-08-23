import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "check_dataset_catalog.py"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_dataset_catalog", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def stable_header(
    content_hash=bytes.fromhex("00112233445566778899aabbccddeeff"), version=5
):
    header = bytearray(1024)
    header[0:4] = b"RETE"
    header[4] = version
    header[8:24] = content_hash
    return bytes(header)


class ProbeValidationTests(unittest.TestCase):
    def test_stable_ranged_object_with_browser_cors_passes(self):
        checker = load_checker()
        result = checker.validate_probe(
            url="https://data.graphplaza.com/demo/demo.rete",
            final_url="https://data.graphplaza.com/demo/demo.rete",
            status=206,
            headers={
                "Accept-Ranges": "bytes",
                "Content-Range": "bytes 0-1023/4096",
                "Content-Length": "1024",
                "Access-Control-Allow-Origin": "*",
                "Access-Control-Expose-Headers": (
                    "Content-Range, Content-Length, Accept-Ranges, ETag"
                ),
            },
            body=stable_header(),
        )
        self.assertEqual(result["errors"], [])
        self.assertEqual(result["formatVersion"], 5)
        self.assertEqual(result["size"], 4096)
        self.assertEqual(result["contentHash"], "00112233445566778899aabbccddeeff")

    def test_paired_generation_is_a_readable_catalog_object(self):
        checker = load_checker()
        result = checker.validate_probe(
            url="https://data.graphplaza.com/demo/demo.rete",
            final_url="https://data.graphplaza.com/demo/demo.rete",
            status=206,
            headers={
                "Accept-Ranges": "bytes",
                "Content-Range": "bytes 0-1023/4096",
                "Content-Length": "1024",
                "Access-Control-Allow-Origin": "*",
                "Access-Control-Expose-Headers": "Content-Range",
            },
            body=stable_header(version=6),
        )
        self.assertEqual(result["errors"], [])
        self.assertEqual(result["formatVersion"], 6)

    def test_redirect_status_cors_version_and_length_are_gated(self):
        checker = load_checker()
        body = bytearray(stable_header())
        body[4] = 4
        result = checker.validate_probe(
            url="https://data.graphplaza.com/demo/demo.rete",
            final_url="https://cdn.example/demo.rete",
            status=200,
            headers={
                "Content-Range": "bytes 0-1023/999",
                "Content-Length": "1000",
                "Access-Control-Expose-Headers": "ETag",
            },
            body=bytes(body[:1000]),
        )
        joined = "\n".join(result["errors"])
        self.assertIn("redirected", joined)
        self.assertIn("expected HTTP 206", joined)
        self.assertIn("Accept-Ranges", joined)
        self.assertIn("Access-Control-Allow-Origin", joined)
        self.assertIn("expose Content-Range", joined)
        self.assertIn("1024-byte range", joined)
        self.assertIn("format byte 5 or 6", joined)
        self.assertIn("Content-Range total 999 is smaller", joined)

    def test_lock_mismatches_are_reported(self):
        checker = load_checker()
        result = checker.validate_probe(
            url="https://data.graphplaza.com/demo/demo.rete",
            final_url="https://data.graphplaza.com/demo/demo.rete",
            status=206,
            headers={
                "Accept-Ranges": "bytes",
                "Content-Range": "bytes 0-1023/4096",
                "Content-Length": "1024",
                "Access-Control-Allow-Origin": "*",
                "Access-Control-Expose-Headers": "Content-Range",
            },
            body=stable_header(),
            expected={
                "formatVersion": 5,
                "contentHash": "ffffffffffffffffffffffffffffffff",
                "size": 8192,
            },
        )
        self.assertEqual(
            result["errors"],
            [
                "content hash does not match datasets.lock.json",
                "size does not match datasets.lock.json (4096 != 8192)",
            ],
        )


class CatalogTargetTests(unittest.TestCase):
    def test_derived_explicit_and_sharded_urls_are_collected(self):
        checker = load_checker()
        catalog = {
            "remoteBase": "https://data.graphplaza.com",
            "datasets": [
                {"key": "embedded"},
                {"key": "explicit", "url": "https://elsewhere.test/x.rete"},
                {
                    "key": "sharded",
                    "shards": [
                        "https://data.graphplaza.com/sharded/part-1.rete",
                        "https://data.graphplaza.com/sharded/part-2.rete",
                    ],
                },
            ],
        }
        self.assertEqual(
            checker.catalog_targets(catalog),
            [
                {
                    "key": "embedded",
                    "dataset": "embedded",
                    "textIndex": False,
                    "url": "https://data.graphplaza.com/embedded/embedded.rete",
                },
                {
                    "key": "explicit",
                    "dataset": "explicit",
                    "textIndex": False,
                    "url": "https://elsewhere.test/x.rete",
                },
                {
                    "key": "sharded#1",
                    "dataset": "sharded",
                    "textIndex": False,
                    "url": "https://data.graphplaza.com/sharded/part-1.rete",
                },
                {
                    "key": "sharded#2",
                    "dataset": "sharded",
                    "textIndex": False,
                    "url": "https://data.graphplaza.com/sharded/part-2.rete",
                },
            ],
        )


if __name__ == "__main__":
    unittest.main()
