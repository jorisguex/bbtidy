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
        self.assertEqual(
            check_workflows.validate_runner_identity(
                PROJECT_ROOT / ".github" / "workflows"
            ),
            [],
        )

    def test_rejects_runner_class_that_disagrees_with_runs_on(self):
        with tempfile.TemporaryDirectory() as temporary:
            workflow = Path(temporary) / "performance.yml"
            workflow.write_text(
                "jobs:\n"
                "  measure:\n"
                "    runs-on: ubuntu-22.04\n"
                "    steps:\n"
                "      - run: python3 scripts/benchmark_performance.py --runner-class github-ubuntu-24.04-x86_64\n",
                encoding="utf-8",
            )
            errors = check_workflows.validate_runner_identity(Path(temporary))
        self.assertEqual(len(errors), 1)
        self.assertIn("disagrees", errors[0])

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

    def test_release_topology_has_one_tag_orchestrator_and_blocking_gates(self):
        self.assertEqual(
            check_workflows.validate_release_topology(
                PROJECT_ROOT / ".github" / "workflows"
            ),
            [],
        )

    def test_release_topology_rejects_a_second_tag_workflow(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            source = PROJECT_ROOT / ".github" / "workflows"
            for name in (
                "release.yml",
                "release-gate.yml",
                "publish-crates.yml",
                "publish-pypi.yml",
            ):
                (temporary / name).write_text(
                    (source / name).read_text(encoding="utf-8"), encoding="utf-8"
                )
            (temporary / "bypass.yml").write_text(
                "on:\n  push:\n    tags: [\"v*\"]\n", encoding="utf-8"
            )
            errors = check_workflows.validate_release_topology(temporary)
        self.assertTrue(any("exactly one workflow" in error for error in errors))

    def test_release_topology_requires_layer_scoped_supported_benchmarks(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            source = PROJECT_ROOT / ".github" / "workflows"
            for name in (
                "release.yml",
                "release-gate.yml",
                "publish-crates.yml",
                "publish-pypi.yml",
            ):
                text = (source / name).read_text(encoding="utf-8")
                if name == "release-gate.yml":
                    lines = []
                    for line in text.splitlines(keepends=True):
                        if "export BBTIDY_PERFORMANCE_SOURCE_ROOT" in line:
                            continue
                        lines.append(
                            line.replace(
                                '"$performance_root"',
                                "compatibility-workspace/formatted",
                            )
                        )
                    text = "".join(lines)
                (temporary / name).write_text(text, encoding="utf-8")

            errors = check_workflows.validate_release_topology(temporary)

        self.assertTrue(
            any("manifest-declared layers" in error for error in errors)
        )

    def test_release_topology_requires_deterministic_bitbake_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            source = PROJECT_ROOT / ".github" / "workflows"
            for name in (
                "release.yml",
                "release-gate.yml",
                "publish-crates.yml",
                "publish-pypi.yml",
            ):
                text = (source / name).read_text(encoding="utf-8")
                if name == "release-gate.yml":
                    text = text.replace(
                        '          build_dir="compatibility-workspace/build-original"\n',
                        "",
                    )
                (temporary / name).write_text(text, encoding="utf-8")

            errors = check_workflows.validate_release_topology(temporary)

        self.assertTrue(
            any("deterministic compatibility paths" in error for error in errors)
        )


if __name__ == "__main__":
    unittest.main()
