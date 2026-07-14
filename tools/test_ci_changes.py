import importlib.util
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


def load_module(name: str, relative_path: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative_path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


changes = load_module("ci_changes", "scripts/ci/changes.py")
workflow_runs = load_module("verify_workflow_run", "scripts/ci/verify-workflow-run.py")


class CiChangesTests(unittest.TestCase):
    def test_unrelated_documentation_selects_no_expensive_lane(self):
        result = changes.classify_paths(["docs/ci.md"])
        self.assertFalse(any(result.values()))

    def test_representative_markdown_change_refreshes_generated_contract(self):
        result = changes.classify_paths(["docs/development.md", "README.md"])
        self.assertEqual(
            {lane for lane, selected in result.items() if selected},
            {"generated"},
        )

    def test_pi_change_is_isolated(self):
        result = changes.classify_paths(["pi-mark/extensions/pi-mark.ts"])
        self.assertEqual(
            {lane for lane, selected in result.items() if selected},
            {"pi"},
        )

    def test_syntax_change_selects_transitive_lanes(self):
        result = changes.classify_paths(["crates/mark-syntax/src/lib.rs"])
        self.assertEqual(
            {lane for lane, selected in result.items() if selected},
            {"rust", "syntax", "generated", "performance"},
        )

    def test_workflow_change_runs_every_lane(self):
        result = changes.classify_paths([".github/workflows/quality.yml"])
        self.assertTrue(all(result.values()))

    def test_cargo_change_runs_rust_and_catalog_contracts(self):
        result = changes.classify_paths(["Cargo.toml"])
        self.assertTrue(result["rust"])
        self.assertTrue(result["syntax"])
        self.assertTrue(result["generated"])
        self.assertTrue(result["performance"])


class WorkflowRunTests(unittest.TestCase):
    def test_requires_exact_sha_branch_event_and_success(self):
        payload = {
            "workflow_runs": [
                {
                    "head_sha": "good",
                    "head_branch": "main",
                    "event": "push",
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": "https://example.test/good",
                },
                {
                    "head_sha": "good",
                    "head_branch": "main",
                    "event": "pull_request",
                    "status": "completed",
                    "conclusion": "success",
                },
                {
                    "head_sha": "bad",
                    "head_branch": "main",
                    "event": "push",
                    "status": "completed",
                    "conclusion": "failure",
                },
            ]
        }
        matches = workflow_runs.successful_runs(
            payload, sha="good", branch="main", event="push"
        )
        self.assertEqual(len(matches), 1)
        self.assertEqual(matches[0]["html_url"], "https://example.test/good")

    def test_rejects_missing_run_list(self):
        with self.assertRaises(ValueError):
            workflow_runs.successful_runs({}, sha="x", branch="main", event="push")


if __name__ == "__main__":
    unittest.main()
