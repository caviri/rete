import importlib.util
import os
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "promote_r2_format.py"


def load_promoter():
    spec = importlib.util.spec_from_file_location("promote_r2_format", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PromotionPlanTests(unittest.TestCase):
    def test_crlf_env_file_is_loaded_without_mutating_values(self):
        promoter = load_promoter()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ".env"
            path.write_bytes(
                b"S3_API_ENDPOINT=https://r2.example\r\n"
                b"ACCESS_KEY_ID=abc\r\nSECRET_ACCESS_KEY=def\r\n"
            )
            previous = {
                name: os.environ.get(name)
                for name in ("S3_API_ENDPOINT", "ACCESS_KEY_ID", "SECRET_ACCESS_KEY")
            }
            try:
                promoter.load_env_file(path)
                self.assertEqual(os.environ["S3_API_ENDPOINT"], "https://r2.example")
                self.assertEqual(os.environ["ACCESS_KEY_ID"], "abc")
                self.assertEqual(os.environ["SECRET_ACCESS_KEY"], "def")
            finally:
                for name, value in previous.items():
                    if value is None:
                        os.environ.pop(name, None)
                    else:
                        os.environ[name] = value

    def test_r2_url_maps_to_bucket_key(self):
        promoter = load_promoter()
        self.assertEqual(
            promoter.object_key(
                "https://data.graphplaza.com/worldcup2026/worldcup2026.rete"
            ),
            "worldcup2026/worldcup2026.rete",
        )
        with self.assertRaisesRegex(ValueError, "not an R2 catalog URL"):
            promoter.object_key("https://zenodo.org/record/file.rete")

    def test_multipart_ranges_cover_the_object_without_gaps(self):
        promoter = load_promoter()
        part = 64 * 1024 * 1024
        self.assertEqual(promoter.part_ranges(10, part), [(1, 0, 9)])
        self.assertEqual(promoter.part_ranges(part, part), [(1, 0, part - 1)])
        self.assertEqual(
            promoter.part_ranges(part + 1, part),
            [(1, 0, part - 1), (2, part, part)],
        )
        ranges = promoter.part_ranges(part * 2 + 17, part)
        self.assertEqual(ranges[-1], (3, part * 2, part * 2 + 16))


class HeaderPromotionTests(unittest.TestCase):
    def test_only_the_format_byte_changes(self):
        promoter = load_promoter()
        header = bytearray(1024)
        header[0:4] = b"RETE"
        header[4] = 4
        header[6:8] = (1024).to_bytes(2, "little")
        header[8:24] = bytes.fromhex("00112233445566778899aabbccddeeff")
        promoted = promoter.promote_header(bytes(header))
        self.assertEqual(promoted[4], 5)
        self.assertEqual(promoted[:4], b"RETE")
        self.assertEqual(promoted[5:], bytes(header[5:]))

    def test_wrong_magic_layout_or_source_version_is_rejected(self):
        promoter = load_promoter()
        valid = bytearray(1024)
        valid[0:4] = b"RETE"
        valid[4] = 4
        valid[6:8] = (1024).to_bytes(2, "little")
        for offset, value, message in [
            (0, ord("X"), "magic"),
            (4, 3, "format byte 4"),
            (7, 0, "1024-byte header"),
        ]:
            broken = bytearray(valid)
            broken[offset] = value
            with self.assertRaisesRegex(ValueError, message):
                promoter.promote_header(bytes(broken))


if __name__ == "__main__":
    unittest.main()
