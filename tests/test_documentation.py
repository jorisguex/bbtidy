import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class DocumentationTests(unittest.TestCase):
    def test_adoption_contract_freezes_phase_one_product_decisions(self):
        guide = (ROOT / "docs" / "beta-user-guide.md").read_text(encoding="utf-8")

        heading = "## Adoption contract"
        self.assertIn(heading, guide)
        self.assertLess(guide.index(heading), guide.index("## Before you adopt it"))
        contract = guide.split(heading, 1)[1].split("\n## ", 1)[0]

        for decision in (
            "`format` and offline `check` are the primary interface.",
            "`check --workspace` is the preferred authoritative production check.",
            "`check --semantic` is an optional target-specific overlay.",
            "`semantic` is an inspection and reporting tool, not part of basic adoption.",
            "The quickstart is entirely read-only",
            "Every CI example must install or select an exact bbtidy version",
            "documentation and CI examples use `--profile recommended` explicitly.",
            "does not add lint rules, parser features, editor integration, or an",
            "initialization command.",
        ):
            self.assertIn(decision, contract)

        for workflow in (
            "| Quick start | First evaluation and ordinary CI | `format` and offline `check` | No |",
            "| Production | Complete build-aware linting | `check --workspace BUILD_DIR` | Yes |",
            "| Advanced | Target-specific linting and metadata inspection | `check --semantic` and `semantic` | Yes |",
        ):
            self.assertIn(workflow, contract)

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
            "bbtidy format --check",
            "bbtidy format --diff",
            "bbtidy format --write",
            "bbtidy check --show-fixes",
            "bbtidy check --fix",
            "bbtidy check --semantic",
            "--variable",
            "bbtidy check --workspace",
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
