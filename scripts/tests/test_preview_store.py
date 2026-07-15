import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).parents[1] / "preview_store.py"
SHA = "91ac238000000000000000000000000000000000"
OLD_SHA = "0123456789abcdef0123456789abcdef01234567"


def load_store():
    spec = importlib.util.spec_from_file_location("preview_store", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_artifact(root: Path, sha: str = SHA) -> None:
    (root / "playground.html").write_text(
        f'<script>window.RETE_BUILD = "{sha[:12]}";\n'
        "window.RETE_PREVIEW = null;</script>",
        encoding="utf-8",
    )
    (root / "rete_wasm_async.js").write_text("export default {};", encoding="utf-8")
    (root / "rete_wasm_async.wasm").write_bytes(b"\x00asm")
    (root / "coi-serviceworker.js").write_text("// worker", encoding="utf-8")
    (root / "wasm-build.json").write_text(
        json.dumps({"schemaVersion": 1, "gitCommit": sha}), encoding="utf-8"
    )


class FakeS3:
    def __init__(self, pages=None):
        self.pages = list(pages or [])
        self.uploads = []
        self.deletes = []
        self.list_requests = []

    def upload_file(self, filename, bucket, key, ExtraArgs):
        self.uploads.append((Path(filename).name, bucket, key, ExtraArgs))

    def list_objects_v2(self, **request):
        self.list_requests.append(request)
        if self.pages:
            return self.pages.pop(0)
        return {}

    def delete_objects(self, **request):
        self.deletes.append(request)


class PreviewPathTests(unittest.TestCase):
    def test_prefix_and_public_url_use_full_sha(self):
        store = load_store()
        self.assertEqual(store.object_prefix(72, SHA), f"pr-72/{SHA}/")
        self.assertEqual(
            store.preview_url(72, SHA),
            f"https://preview.graphplaza.com/pr-72/{SHA}/playground.html",
        )

    def test_invalid_pr_or_sha_is_rejected(self):
        store = load_store()
        for pr, sha in [(0, SHA), (-1, SHA), (72, "bad"), (72, SHA.upper())]:
            with self.subTest(pr=pr, sha=sha), self.assertRaisesRegex(
                ValueError, "40-character"
            ):
                store.object_prefix(pr, sha)


class ArtifactValidationTests(unittest.TestCase):
    def test_exact_artifact_is_accepted(self):
        store = load_store()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_artifact(root)
            files = store.validate_artifact(root, SHA)
            self.assertEqual({path.name for path in files}, store.ALLOWED)

    def test_missing_and_extra_files_are_rejected(self):
        store = load_store()
        for change in ("missing", "extra"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                write_artifact(root)
                if change == "missing":
                    (root / "rete_wasm_async.js").unlink()
                else:
                    (root / "surprise.js").write_text("bad", encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "exactly"):
                    store.validate_artifact(root, SHA)

    def test_symlink_is_rejected(self):
        store = load_store()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_artifact(root)
            target = root / "actual.js"
            target.write_text("bad", encoding="utf-8")
            link = root / "rete_wasm_async.js"
            link.unlink()
            try:
                link.symlink_to(target)
            except OSError as error:
                self.skipTest(f"symlinks unavailable: {error}")
            target.unlink()
            with self.assertRaisesRegex(ValueError, "symlink"):
                store.validate_artifact(root, SHA)

    def test_oversized_artifact_is_rejected(self):
        store = load_store()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_artifact(root)
            with mock.patch.object(store, "MAX_ARTIFACT_BYTES", 4):
                with self.assertRaisesRegex(ValueError, "64 MiB"):
                    store.validate_artifact(root, SHA)

    def test_build_stamp_and_manifest_must_match_full_head_sha(self):
        store = load_store()
        for mismatch in ("stamp", "manifest"):
            with self.subTest(mismatch=mismatch), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                write_artifact(root)
                if mismatch == "stamp":
                    page = root / "playground.html"
                    page.write_text(
                        page.read_text(encoding="utf-8").replace(SHA[:12], OLD_SHA[:12]),
                        encoding="utf-8",
                    )
                else:
                    manifest = root / "wasm-build.json"
                    manifest.write_text(
                        json.dumps({"schemaVersion": 1, "gitCommit": OLD_SHA}),
                        encoding="utf-8",
                    )
                with self.assertRaisesRegex(ValueError, "head SHA"):
                    store.validate_artifact(root, SHA)


class MetadataAndUploadTests(unittest.TestCase):
    def test_metadata_replaces_exactly_one_marker_and_uses_json(self):
        store = load_store()
        metadata = {
            "number": 72,
            "headSha": SHA,
            "baseSha": OLD_SHA,
            "title": '</script><script>alert("x")</script>',
        }
        rendered = store.inject_preview_metadata(
            "before\nwindow.RETE_PREVIEW = null;\nafter", metadata
        )
        self.assertNotIn("window.RETE_PREVIEW = null;", rendered)
        payload = rendered.split("window.RETE_PREVIEW = ", 1)[1].split(";", 1)[0]
        self.assertEqual(json.loads(payload), metadata)
        for html in ("no marker", "window.RETE_PREVIEW = null;" * 2):
            with self.subTest(html=html), self.assertRaisesRegex(ValueError, "exactly once"):
                store.inject_preview_metadata(html, metadata)

    def test_upload_stages_metadata_and_assigns_safe_cache_headers(self):
        store = load_store()
        client = FakeS3()
        metadata = {
            "number": 72,
            "headSha": SHA,
            "baseSha": OLD_SHA,
            "title": "Preview title",
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_artifact(root)
            url = store.upload_preview(client, "previews", root, metadata)
        self.assertEqual(url, store.preview_url(72, SHA))
        self.assertEqual(len(client.uploads), 5)
        uploads = {name: (key, args) for name, _bucket, key, args in client.uploads}
        for name, (key, _args) in uploads.items():
            self.assertEqual(key, store.object_prefix(72, SHA) + name)
        self.assertEqual(
            uploads["rete_wasm_async.js"][1]["CacheControl"],
            "public,max-age=31536000,immutable",
        )
        self.assertEqual(
            uploads["rete_wasm_async.wasm"][1]["CacheControl"],
            "public,max-age=31536000,immutable",
        )
        for name in ("playground.html", "wasm-build.json", "coi-serviceworker.js"):
            self.assertEqual(uploads[name][1]["CacheControl"], "no-store")


class CleanupTests(unittest.TestCase):
    def test_cleanup_preserves_keep_sha_and_batches_at_one_thousand(self):
        store = load_store()
        old_keys = [f"pr-72/{OLD_SHA}/file-{index}" for index in range(1001)]
        keep_keys = [f"pr-72/{SHA}/playground.html"]
        client = FakeS3(
            [
                {
                    "Contents": [{"Key": key} for key in old_keys[:600] + keep_keys],
                    "IsTruncated": True,
                    "NextContinuationToken": "next",
                },
                {"Contents": [{"Key": key} for key in old_keys[600:]]},
            ]
        )
        deleted = store.cleanup_preview(client, "previews", 72, keep_sha=SHA)
        self.assertEqual(deleted, 1001)
        self.assertEqual(client.list_requests[1]["ContinuationToken"], "next")
        deleted_keys = [
            item["Key"]
            for request in client.deletes
            for item in request["Delete"]["Objects"]
        ]
        self.assertEqual(deleted_keys, old_keys)
        self.assertNotIn(keep_keys[0], deleted_keys)
        self.assertTrue(all(len(request["Delete"]["Objects"]) <= 1000 for request in client.deletes))

    def test_empty_cleanup_is_idempotent(self):
        store = load_store()
        client = FakeS3([{}])
        self.assertEqual(store.cleanup_preview(client, "previews", 72), 0)
        self.assertEqual(client.deletes, [])


if __name__ == "__main__":
    unittest.main()
