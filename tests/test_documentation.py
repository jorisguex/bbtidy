import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class DocumentationTests(unittest.TestCase):
    def test_user_facing_contract_documents_required_workflows(self):
        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        contract = (ROOT / "docs" / "beta-support-contract.md").read_text(
            encoding="utf-8"
        )
        guide = (ROOT / "docs" / "beta-user-guide.md").read_text(encoding="utf-8")

        self.assertIn("docs/beta-support-contract.md", readme)
        self.assertIn("docs/beta-user-guide.md", readme)
        self.assertIn("beta user guide", contract)
        for required_text in (
            "bbtidy --version",
            "bbtidy check",
            "bbtidy format --diff",
            "bbtidy format --write",
            "bbtidy lint --show-fixes",
            "bbtidy lint --fix",
            "bbtidy lint --semantic",
            "bbtidy lint --workspace",
            "bitbake --parse-only",
            "max_files",
            "max_bytes",
            "fail_on",
            "--fail-on",
            "BBTIDY_BITBAKE_BUILD_DIR",
            "BUILDDIR",
            "--project-dir",
            "BBT020",
            "BBT033",
            "BBT034",
            "BBT036",
            "BBT037",
            "LIC_FILES_CHKSUM",
            "LAYERSERIES_COMPAT",
            "installation method",
        ):
            self.assertIn(required_text, guide)

    def test_documented_relative_links_exist(self):
        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        self.assertIn("[beta support contract](docs/beta-support-contract.md)", readme)
        self.assertIn("[beta user guide](docs/beta-user-guide.md)", readme)
        self.assertTrue((ROOT / "docs" / "beta-support-contract.md").is_file())
        self.assertTrue((ROOT / "docs" / "beta-user-guide.md").is_file())


if __name__ == "__main__":
    unittest.main()
