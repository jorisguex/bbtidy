import re
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = ROOT / "examples"
sys.path.insert(0, str(ROOT / "scripts"))
import check_workflows  # noqa: E402


def current_package_version():
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    release = re.search(r'^version = "([^"]+)"$', cargo, flags=re.MULTILINE)
    if release is None:
        raise AssertionError("Cargo package version is missing")
    return re.sub(
        r"-(alpha|beta|rc)\.(\d+)$",
        lambda prerelease: {
            "alpha": "a",
            "beta": "b",
            "rc": "rc",
        }[prerelease.group(1)]
        + prerelease.group(2),
        release.group(1),
    )


class StarterAssetTests(unittest.TestCase):
    def test_minimal_configuration_is_exactly_the_pilot_policy(self):
        configuration = (EXAMPLES / "bbtidy.toml").read_text(encoding="utf-8")

        self.assertEqual(
            configuration,
            """[lint]
profile = "recommended"
fail_on = "never"

[paths]
exclude = ["vendor/**"]
""",
        )
        for excluded_setting in (
            "[semantic]",
            "[safety]",
            "[lint.severity]",
            "enable =",
            "disable =",
        ):
            self.assertNotIn(excluded_setting, configuration)

    def test_generic_ci_is_pinned_and_platform_neutral(self):
        package_version = current_package_version()
        commands = (EXAMPLES / "generic-ci.txt").read_text(encoding="utf-8")

        self.assertEqual(
            commands.splitlines(),
            [
                f'python -m pip install --pre "bbtidy=={package_version}"',
                "bbtidy --version",
                "bbtidy format --check meta-my-layer/",
                "bbtidy check --profile recommended meta-my-layer/",
            ],
        )
        for shell_specific_token in ("set -", "${{", "$GITHUB_", "sudo "):
            self.assertNotIn(shell_specific_token, commands)
        for advanced_or_writing_flag in (
            "--write",
            "--fix",
            "--workspace",
            "--semantic",
        ):
            self.assertNotIn(advanced_or_writing_flag, commands)

    def test_github_actions_example_is_complete_safe_and_immutable(self):
        path = EXAMPLES / "github-actions.yml"
        workflow = path.read_text(encoding="utf-8")
        package_version = current_package_version()

        self.assertEqual(check_workflows.validate_starter_workflow(path), [])
        self.assertIn(
            'permissions:\n  contents: read\n\njobs:',
            workflow,
        )
        self.assertNotIn("security-events: write", workflow)
        self.assertIn(f'bbtidy=={package_version}', workflow)
        self.assertIn("run: bbtidy --version", workflow)

        format_step = workflow.index("run: bbtidy format --check meta-my-layer/")
        lint_step = workflow.index(
            "bbtidy check --profile recommended --output sarif meta-my-layer/"
        )
        saved_status = workflow.index("lint_status=$?", lint_step)
        exposed_status = workflow.index(
            'echo "status=$lint_status" >> "$GITHUB_OUTPUT"', saved_status
        )
        upload_step = workflow.index("- name: Upload SARIF", exposed_status)
        enforce_step = workflow.index("- name: Enforce lint result", upload_step)
        self.assertLess(format_step, lint_step)
        self.assertLess(lint_step, saved_status)
        self.assertLess(saved_status, exposed_status)
        self.assertLess(exposed_status, upload_step)
        self.assertLess(upload_step, enforce_step)

        self.assertIn("actions/upload-artifact@", workflow)
        self.assertIn("if-no-files-found: error", workflow)
        self.assertIn(
            "if: always() && (steps.bbtidy_lint.outputs.status == '0' || "
            "steps.bbtidy_lint.outputs.status == '1')",
            workflow,
        )
        self.assertIn(
            "BBTIDY_EXIT_STATUS: ${{ steps.bbtidy_lint.outputs.status }}",
            workflow,
        )
        self.assertIn('run: exit "$BBTIDY_EXIT_STATUS"', workflow)
        self.assertNotRegex(workflow, r"--(?:write|fix)(?:\s|$)")
        self.assertNotIn("--workspace", workflow)
        self.assertNotIn("check --semantic", workflow)

    def test_pre_commit_example_uses_only_local_system_hooks(self):
        hooks = (EXAMPLES / "pre-commit-config.yaml").read_text(encoding="utf-8")
        package_version = current_package_version()

        self.assertIn(f"bbtidy=={package_version}", hooks)
        self.assertEqual(hooks.count("repo: local"), 1)
        self.assertEqual(hooks.count("language: system"), 2)
        self.assertIn("entry: bbtidy format --check", hooks)
        self.assertIn("entry: bbtidy check --profile recommended", hooks)
        self.assertNotIn("rev:", hooks)
        self.assertNotRegex(hooks, r"--(?:write|fix)(?:\s|$)")
        self.assertNotIn("--workspace", hooks)
        self.assertNotIn("check --semantic", hooks)

    def test_existing_repository_migration_has_all_seven_stages_in_order(self):
        guide = (EXAMPLES / "existing-repository.md").read_text(encoding="utf-8")
        headings = re.findall(r"^## (.+)$", guide, flags=re.MULTILINE)

        self.assertIn(f"bbtidy=={current_package_version()}", guide)
        self.assertEqual(
            headings,
            [
                "1. Preview formatting",
                "2. Land formatting as a dedicated review",
                "3. Enable formatting CI",
                "4. Start lint in report-only mode",
                "5. Write and review a baseline",
                "6. Begin failing on new findings",
                "7. Reduce the baseline over time",
            ],
        )
        ordered_markers = [
            "bbtidy format --diff",
            "bbtidy format --write",
            "## 3. Enable formatting CI",
            "--fail-on never",
            "--write-baseline",
            "--baseline .bbtidy-baseline.json",
            "--refresh-baseline .bbtidy-baseline.json",
        ]
        positions = [guide.index(marker) for marker in ordered_markers]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("Do not refresh the baseline automatically in CI.", guide)
        self.assertNotIn("--workspace", guide)
        self.assertNotIn("check --semantic", guide)
        self.assertNotIn("bbtidy semantic", guide)

    def test_starter_index_links_every_copyable_asset(self):
        index = (EXAMPLES / "README.md").read_text(encoding="utf-8")
        assets = (
            "bbtidy.toml",
            "generic-ci.txt",
            "github-actions.yml",
            "pre-commit-config.yaml",
            "existing-repository.md",
        )

        for asset in assets:
            with self.subTest(asset=asset):
                self.assertIn(f"]({asset})", index)
                self.assertTrue((EXAMPLES / asset).is_file())


if __name__ == "__main__":
    unittest.main()
