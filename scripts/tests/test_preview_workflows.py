import re
import unittest
from pathlib import Path


ROOT = Path(__file__).parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def job(source: str, name: str, next_name: str | None = None) -> str:
    start = source.index(f"  {name}:\n")
    end = source.index(f"  {next_name}:\n", start) if next_name else len(source)
    return source[start:end]


class PreviewCiPolicyTests(unittest.TestCase):
    def test_ci_builds_same_repository_pr_head_without_secrets(self):
        ci = read(".github/workflows/ci.yml")
        preview = job(ci, "preview")
        self.assertIn(
            "github.event.pull_request.head.repo.full_name == 'caviri/rete'", preview
        )
        self.assertIn("permissions:\n      contents: read", preview)
        self.assertIn("ref: ${{ github.event.pull_request.head.sha }}", preview)
        self.assertIn("playground-preview-${{ github.event.pull_request.head.sha }}", preview)
        self.assertIn("retention-days: 1", preview)
        self.assertNotIn("secrets.", preview)
        for name in (
            "playground.html",
            "rete_wasm_async.js",
            "rete_wasm_async.wasm",
            "coi-serviceworker.js",
            "wasm-build.json",
        ):
            self.assertIn(name, preview)


class TrustedPublishPolicyTests(unittest.TestCase):
    def test_publish_uses_successful_workflow_run_and_exact_artifact(self):
        publish = read(".github/workflows/preview-publish.yml")
        self.assertIn("workflow_run:", publish)
        self.assertIn("workflows: [CI]", publish)
        self.assertIn("types: [completed]", publish)
        self.assertIn("github.event.workflow_run.conclusion == 'success'", publish)
        self.assertIn("github.event.workflow_run.event == 'pull_request'", publish)
        self.assertIn("playground-preview-${{ needs.resolve.outputs.head_sha }}", publish)
        self.assertIn("run-id: ${{ github.event.workflow_run.id }}", publish)
        self.assertNotIn("pull_request_target", publish)

    def test_privileged_jobs_use_default_branch_code_and_scoped_environment(self):
        publish = read(".github/workflows/preview-publish.yml")
        for name in ("publish", "prune"):
            block = job(publish, name, "smoke" if name == "publish" else None)
            self.assertIn("environment: playground-preview", block)
            self.assertIn("ref: ${{ github.event.repository.default_branch }}", block)
            self.assertIn("PREVIEW_BUCKET: ${{ secrets.PREVIEW_BUCKET }}", block)
            self.assertNotRegex(block, r"ref:\s*\$\{\{[^\n]*(?:pull_request|head)[^\n]*")

        smoke = job(publish, "smoke", "prune")
        self.assertNotIn("environment:", smoke)
        self.assertNotIn("secrets.", smoke)
        self.assertNotIn("PREVIEW_", smoke)
        self.assertIn("node checks/check_deployed.mjs", smoke)

    def test_trusted_jobs_never_execute_downloaded_artifact_as_local_code(self):
        publish = read(".github/workflows/preview-publish.yml")
        publish_job = job(publish, "publish", "smoke")
        self.assertIn("python3 scripts/preview_store.py upload", publish_job)
        self.assertNotRegex(publish_job, r"(?:node|python3?|bash|sh)\s+[^\n]*playground-preview")
        self.assertNotIn("npm ", publish_job)


class CleanupPolicyTests(unittest.TestCase):
    def test_close_cleanup_is_default_branch_only_and_immediate(self):
        cleanup = read(".github/workflows/preview-cleanup.yml")
        self.assertIn("pull_request_target:", cleanup)
        self.assertIn("types: [closed]", cleanup)
        self.assertIn("environment: playground-preview", cleanup)
        self.assertIn("ref: ${{ github.event.repository.default_branch }}", cleanup)
        self.assertIn('cleanup --pr "$PR_NUMBER"', cleanup)
        self.assertNotRegex(cleanup, r"ref:\s*\$\{\{[^\n]*head[^\n]*")
        self.assertNotIn("actions/download-artifact", cleanup)


if __name__ == "__main__":
    unittest.main()
