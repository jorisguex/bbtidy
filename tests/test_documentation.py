import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def read_document(relative_path):
    return (ROOT / relative_path).read_text(encoding="utf-8")


class DocumentationTests(unittest.TestCase):
    def test_adoption_contract_freezes_phase_one_product_decisions(self):
        guide = read_document("docs/beta-user-guide.md")

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

    def test_readme_opens_with_the_task_first_entry_path(self):
        readme = read_document("README.md")
        headings = re.findall(r"^## (.+)$", readme, flags=re.MULTILINE)
        self.assertEqual(
            headings[:6],
            [
                "What bbtidy does",
                "Current release status and supported Yocto versions",
                "Verified installation command",
                "Five-minute read-only trial",
                "Two CI commands",
                "Production and advanced documentation",
            ],
        )

        trial = readme.split("## Five-minute read-only trial", 1)[1].split(
            "\n## ", 1
        )[0]
        for first_screen_answer in (
            "What will this change?",
            "Will it write files?",
            "Will it invoke BitBake?",
            "Which command should I run first?",
            "What does exit code `1` mean?",
            "bbtidy format --diff meta-my-layer/",
            "bbtidy check --profile recommended --fail-on never meta-my-layer/",
        ):
            self.assertIn(first_screen_answer, trial)

        ci = readme.split("## Two CI commands", 1)[1].split("\n## ", 1)[0]
        self.assertIn("bbtidy format --check meta-my-layer/", ci)
        self.assertIn("bbtidy check --profile recommended meta-my-layer/", ci)

    def test_getting_started_is_one_linear_ten_step_workflow(self):
        getting_started = read_document("docs/getting-started.md")
        headings = re.findall(r"^## (.+)$", getting_started, flags=re.MULTILINE)
        numbered_headings = [
            heading
            for heading in headings
            if re.match(r"\d+\. ", heading)
        ]
        self.assertEqual(
            numbered_headings,
            [
                "1. Install a pinned version",
                "2. Verify the executable",
                "3. Preview formatting",
                "4. Run report-only recommended linting",
                "5. Create a minimal configuration",
                "6. Handle existing findings with a baseline",
                "7. Enable formatting CI",
                "8. Enable lint CI",
                "9. Optionally enable workspace linting",
                "10. Apply writes only in a clean branch",
            ],
        )
        self.assertEqual(headings[:10], numbered_headings)

        write_step = getting_started.index("## 10. Apply writes only in a clean branch")
        self.assertGreater(getting_started.index("bbtidy format --write"), write_step)
        self.assertGreater(
            getting_started.index("bbtidy check --profile recommended --fix"),
            write_step,
        )

    def test_progressive_enforcement_and_baseline_commands_are_exact(self):
        exact_baseline_commands = """bbtidy check \\
  --profile recommended \\
  --write-baseline .bbtidy-baseline.json \\
  meta-my-layer/

bbtidy check \\
  --profile recommended \\
  --baseline .bbtidy-baseline.json \\
  meta-my-layer/"""

        for relative_path in (
            "docs/getting-started.md",
            "docs/lint-rules.md",
        ):
            document = read_document(relative_path)
            with self.subTest(document=relative_path):
                self.assertIn("Observe", document)
                self.assertIn("Baseline", document)
                self.assertIn("Enforce", document)
                self.assertIn("--fail-on never", document)
                self.assertIn("--fail-on warning", document)
                self.assertIn(exact_baseline_commands, document)

    def test_focused_references_cover_the_required_topics(self):
        configuration = read_document("docs/configuration.md")
        lint = read_document("docs/lint-rules.md")
        bitbake = read_document("docs/bitbake-integration.md")
        ci = read_document("docs/ci-integration.md")

        for section in (
            "[format]",
            "[semantic]",
            "[lint]",
            "[lint.severity]",
            "[paths]",
            "[safety]",
            "[bitbake]",
        ):
            self.assertIn(section, configuration)
        for key in (
            "max_top_level_blank_lines",
            "metadata_list_layout",
            "build_dir",
            "bitbake",
            "full",
            "graph",
            "dry_run",
            "inventory",
            "packages",
            "profile",
            "enable",
            "disable",
            "fail_on",
            "baseline",
            "exclude",
            "max_files",
            "max_bytes",
            "command_timeout_seconds",
            "total_timeout_seconds",
            "max_stdout_bytes",
            "max_stderr_bytes",
            "max_commands",
            "max_recipe_queries",
        ):
            self.assertIn(key, configuration)
        self.assertIn("precedence", configuration.lower())

        for rule_number in range(1, 39):
            self.assertIn(f"`BBT{rule_number:03}`", lint)
        for topic in (
            "## Profiles and failure policy",
            "## Inline suppressions",
            "## Baselines",
            "## Fixes",
            "LIC_FILES_CHKSUM",
            "LAYERSERIES_COMPAT",
        ):
            self.assertIn(topic, lint)

        for mode in (
            "check PATH...",
            "check --workspace BUILD_DIR",
            "check --semantic ... PATH...",
            "`semantic`",
        ):
            self.assertIn(mode, bitbake)

        for topic in (
            "## Generic CI",
            "## GitHub Actions",
            "## SARIF",
            "## Pre-commit",
            "--profile recommended",
        ):
            self.assertIn(topic, ci)

    def test_user_facing_documents_collectively_cover_advanced_workflows(self):
        documents = "\n".join(
            read_document(relative_path)
            for relative_path in (
                "README.md",
                "docs/getting-started.md",
                "docs/configuration.md",
                "docs/lint-rules.md",
                "docs/bitbake-integration.md",
                "docs/ci-integration.md",
                "docs/beta-user-guide.md",
            )
        )

        for required_text in (
            "bbtidy --version",
            "bbtidy format --check",
            "bbtidy format --diff",
            "bbtidy format --write",
            "--show-fixes",
            "--fix",
            "check --semantic",
            "--variable",
            "check --workspace",
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
            self.assertIn(required_text, documents)

    def test_install_examples_pin_the_current_cargo_release(self):
        cargo = read_document("Cargo.toml")
        match = re.search(r'^version = "([^"]+)"$', cargo, flags=re.MULTILINE)
        self.assertIsNotNone(match)
        release_version = match.group(1)
        package_version = re.sub(
            r"-(alpha|beta|rc)\.(\d+)$",
            lambda prerelease: {
                "alpha": "a",
                "beta": "b",
                "rc": "rc",
            }[prerelease.group(1)]
            + prerelease.group(2),
            release_version,
        )

        adoption_documents = [ROOT / "README.md", *sorted((ROOT / "docs").glob("*.md"))]
        install_examples = []
        for path in adoption_documents:
            for line_number, line in enumerate(
                path.read_text(encoding="utf-8").splitlines(), start=1
            ):
                if "bbtidy" in line and (
                    "pip install" in line or "pipx install" in line
                ):
                    install_examples.append((path, line_number, line))

        self.assertTrue(install_examples)
        for path, line_number, line in install_examples:
            with self.subTest(path=path, line=line_number):
                self.assertIn(f"bbtidy=={package_version}", line)

        readme = read_document("README.md")
        self.assertIn(release_version, readme)

    def test_pilot_lint_examples_keep_the_recommended_profile_explicit(self):
        support_contract = read_document("docs/beta-support-contract.md")
        execution_guide = read_document("docs/bitbake-execution.md")

        self.assertIn("bbtidy check --profile recommended", support_contract)
        self.assertIn(
            "bbtidy check --workspace /path/to/build --bitbake /path/to/bitbake \\\n  --profile recommended --output json",
            execution_guide,
        )

    def test_all_documented_relative_links_exist(self):
        documents = [ROOT / "README.md", *sorted((ROOT / "docs").glob("*.md"))]
        for path in documents:
            source = path.read_text(encoding="utf-8")
            for target in re.findall(r"\[[^]]*\]\(([^)]+)\)", source):
                if target.startswith(("http://", "https://", "mailto:")):
                    continue
                relative_target = target.split("#", 1)[0]
                if not relative_target:
                    continue
                resolved = path.parent / relative_target
                with self.subTest(document=path, target=target):
                    self.assertTrue(resolved.exists(), resolved)

    def test_beta_support_contract_remains_the_authoritative_reference(self):
        readme = read_document("README.md")
        contract = read_document("docs/beta-support-contract.md")
        guide = read_document("docs/beta-user-guide.md")

        self.assertIn("[beta support contract](docs/beta-support-contract.md)", readme)
        self.assertIn("authoritative", guide)
        self.assertIn("beta user guide", contract)


if __name__ == "__main__":
    unittest.main()
