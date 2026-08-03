import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))
import check_workflows  # noqa: E402


class WorkflowPinTests(unittest.TestCase):
    def test_repository_workflows_use_immutable_action_pins(self):
        errors = check_workflows.validate_workflow_directory(
            PROJECT_ROOT / ".github" / "workflows"
        )
        self.assertEqual(errors, [])

    def test_rejects_tag_and_short_sha_references(self):
        with tempfile.TemporaryDirectory() as temporary:
            workflow = Path(temporary) / "security.yml"
            workflow.write_text(
                "steps:\n"
                "  - uses: actions/checkout@v6\n"
                "  - uses: actions/setup-python@abcdef\n",
                encoding="utf-8",
            )

            errors = check_workflows.validate_workflow(workflow)

        self.assertEqual(len(errors), 2)
        self.assertIn("security.yml:2", errors[0])
        self.assertIn("40-character lowercase commit SHA", errors[0])
        self.assertIn("security.yml:3", errors[1])

    def test_accepts_local_actions_and_digest_pinned_container_actions(self):
        with tempfile.TemporaryDirectory() as temporary:
            workflow = Path(temporary) / "security.yml"
            workflow.write_text(
                "steps:\n"
                "  - uses: ./.github/actions/check\n"
                "  - uses: docker://example.invalid/tool@sha256:"
                + "a" * 64
                + "\n",
                encoding="utf-8",
            )

            errors = check_workflows.validate_workflow(workflow)

        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
