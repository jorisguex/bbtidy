import argparse
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.check_upstream_corpus import (
    CompatibilityError,
    baseline_update_allowed,
    build_lint_baseline,
    compare_lint_baseline,
    compare_semantic_probes,
    discover_metadata_files,
    format_idempotence_command,
    load_manifest,
    normalize_lint_report,
    lint_command,
    normalize_semantic_values,
    opaque_regions,
    parse_semantic_values,
    summarize_lint_findings,
    validate_lint_baseline,
    write_lint_evidence,
    verify_syntax_metrics,
    verify_tree_preservation,
    workflow_command_value,
)


class ManifestTests(unittest.TestCase):
    def test_checked_in_manifest_is_valid(self):
        root = Path(__file__).resolve().parents[1]
        manifests = sorted((root / "tests" / "upstream-corpora").glob("*.json"))

        self.assertEqual(len(manifests), 4)
        loaded = [load_manifest(path) for path in manifests]
        self.assertEqual(
            {manifest["id"] for manifest in loaded},
            {
                "community-master",
                "yocto-5.0-scarthgap",
                "yocto-6.0-wrynose",
                "yocto-master",
            },
        )
        self.assertEqual(
            {
                (manifest["yocto_version"], manifest["bitbake_version"])
                for manifest in loaded
                if manifest["tier"] == "supported"
            },
            {("5.0", "2.8"), ("6.0", "2.18")},
        )
        community = next(
            manifest for manifest in loaded if manifest["id"] == "community-master"
        )
        self.assertEqual(community["tier"], "development")
        self.assertEqual(len(community["layers"]), 3)
        self.assertIn("_baseline_metrics", community)
        self.assertEqual(community["_baseline_metrics"]["source"]["files"], 667)
        for manifest in loaded:
            if manifest["tier"] == "supported":
                self.assertEqual(
                    [probe["name"] for probe in manifest["bitbake"]["semantic_probes"]],
                    ["core-image-minimal-metadata"],
                )

    def test_manifest_rejects_floating_revisions(self):
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / "manifest.json"
            manifest.write_text(
                """
{
  "schema": 1,
  "id": "example",
  "tier": "supported",
  "yocto_version": "1",
  "bitbake_version": "1",
  "repositories": [{
    "name": "example",
    "url": "https://example.com/repository.git",
    "revision": "main",
    "sparse_paths": ["meta"]
  }],
  "layers": [{
    "name": "example",
    "repository": "example",
    "path": "meta",
    "minimum_files": 1
  }],
  "syntax_metrics": {
    "minimum_structured_nodes": 1,
    "maximum_unknown_nodes": 0
  },
  "bitbake": {
    "init_repository": "example",
    "template": "meta/conf/templates/default",
    "target": "example",
    "additional_layers": []
  }
}
""",
                encoding="utf-8",
            )

            with self.assertRaises(CompatibilityError):
                load_manifest(manifest)

    def test_development_manifest_accepts_an_explicit_branch_ref(self):
        root = Path(__file__).resolve().parents[1]
        manifest = load_manifest(root / "tests" / "upstream-corpora/yocto-master.json")

        self.assertEqual(manifest["tier"], "development")
        self.assertTrue(
            all(repository["ref"].startswith("refs/heads/")
                for repository in manifest["repositories"])
        )


class SyntaxMetricsTests(unittest.TestCase):
    def test_rejects_structural_coverage_regressions(self):
        source = {
            "version": 1,
            "files": 2,
            "structured_nodes": 10,
            "unknown_nodes": 1,
        }
        formatted = {
            "version": 1,
            "files": 2,
            "structured_nodes": 9,
            "unknown_nodes": 2,
        }

        with self.assertRaises(CompatibilityError):
            verify_syntax_metrics(
                source,
                formatted,
                {"minimum_structured_nodes": 10, "maximum_unknown_nodes": 1},
                2,
            )

    def test_rejects_checked_in_baseline_metric_changes(self):
        source = {
            "version": 1,
            "files": 2,
            "structured_nodes": 10,
            "total_nodes": 12,
            "trivia_nodes": 2,
            "unknown_bytes": 0,
            "unknown_nodes": 0,
        }
        formatted = dict(source)
        baseline = {"source": dict(source), "formatted": dict(formatted)}
        formatted["total_nodes"] = 13

        with self.assertRaisesRegex(CompatibilityError, "baseline"):
            verify_syntax_metrics(
                source,
                formatted,
                {"minimum_structured_nodes": 1, "maximum_unknown_nodes": 1},
                2,
                baseline,
            )


class SemanticProbeTests(unittest.TestCase):
    def test_parses_and_normalizes_selected_bitbake_variables(self):
        values = parse_semantic_values(
            'PN="core-image-minimal"\n'
            'export DEPENDS=" a b "\n'
            'SRC_URI="/tmp/build/downloads/source.tar.gz"\n',
            ["PN", "DEPENDS", "SRC_URI", "MISSING"],
        )

        self.assertEqual(values["PN"], '"core-image-minimal"')
        self.assertEqual(values["MISSING"], None)
        self.assertEqual(
            normalize_semantic_values(values, [Path("/tmp/build")])["SRC_URI"],
            '"<CORPUS>/downloads/source.tar.gz"',
        )

    def test_rejects_semantic_differences(self):
        source = {
            "probe": {"values": {"PN": '"example"', "PV": '"1.0"'}}
        }
        formatted = {
            "probe": {"values": {"PN": '"changed"', "PV": '"1.0"'}}
        }

        with self.assertRaisesRegex(CompatibilityError, "changed variables: PN"):
            compare_semantic_probes(source, formatted)


class TreeVerificationTests(unittest.TestCase):
    def test_allows_only_expected_metadata_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "source"
            formatted = Path(temporary) / "formatted"
            (source / "repo" / "meta").mkdir(parents=True)
            (formatted / "repo" / "meta").mkdir(parents=True)
            (source / "repo" / "meta" / "example.bb").write_text(
                'SUMMARY = "before"\n', encoding="utf-8"
            )
            (formatted / "repo" / "meta" / "example.bb").write_text(
                'SUMMARY = "after"\n', encoding="utf-8"
            )
            (source / "repo" / "README").write_text("payload\n", encoding="utf-8")
            (formatted / "repo" / "README").write_text("payload\n", encoding="utf-8")

            self.assertEqual(
                verify_tree_preservation(
                    source, formatted, {"repo/meta/example.bb"}
                ),
                ["repo/meta/example.bb"],
            )

            (formatted / "repo" / "README").write_text(
                "changed\n", encoding="utf-8"
            )
            with self.assertRaisesRegex(CompatibilityError, "allowlist"):
                verify_tree_preservation(source, formatted, {"repo/meta/example.bb"})


class DiscoveryTests(unittest.TestCase):
    def test_discovers_metadata_without_recipe_payloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            layer = Path(temporary)
            files = [
                layer / "conf" / "layer.conf",
                layer / "conf" / "distro" / "example.conf",
                layer / "recipes-example" / "example" / "example.bb",
                layer / "recipes-example" / "example" / "example.inc",
                layer / "recipes-example" / "example" / "files" / "template.inc",
                layer / "recipes-example" / "example" / "example" / "runtime.conf",
            ]
            for path in files:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('VALUE = "example"\n', encoding="utf-8")

            discovered = {
                path.relative_to(layer) for path in discover_metadata_files(layer)
            }

            self.assertEqual(
                discovered,
                {
                    Path("conf/layer.conf"),
                    Path("conf/distro/example.conf"),
                    Path("recipes-example/example/example.bb"),
                    Path("recipes-example/example/example.inc"),
                },
            )


class OpaqueRegionTests(unittest.TestCase):
    def test_extracts_functions_and_top_level_python_blocks(self):
        source = (
            "python __anonymous() {\n"
            '    script = """echo "\n'
            "${VALUE}\n"
            '"""\n'
            "}\n"
            "\n"
            "def helper(d):\n"
            "    return d.getVar('VALUE')\n"
            "\n"
            'SUMMARY = "Example"\n'
        )

        self.assertEqual(
            opaque_regions(source),
            [
                (
                    "python __anonymous() {\n"
                    '    script = """echo "\n'
                    "${VALUE}\n"
                    '"""\n'
                    "}\n"
                ),
                "def helper(d):\n    return d.getVar('VALUE')\n\n",
            ],
        )


class WorkflowCommandTests(unittest.TestCase):
    def test_escapes_annotation_control_characters(self):
        self.assertEqual(
            workflow_command_value("100%\r\nfailed"),
            "100%25%0D%0Afailed",
        )

    def test_uses_current_cli_commands_for_formatting_and_linting(self):
        bbtidy = Path("/tmp/bbtidy")
        inputs = [Path("/tmp/layer")]

        self.assertEqual(
            format_idempotence_command(bbtidy, inputs),
            [bbtidy, "format", "--check", *inputs],
        )
        self.assertEqual(
            lint_command(bbtidy, inputs),
            [bbtidy, "check", "--output", "json", "--fail-on", "never", *inputs],
        )


def lint_report(root, diagnostics):
    return json.dumps({"version": 1, "diagnostics": diagnostics})


def lint_diagnostic(path, rule_id="BBT001", message="trailing whitespace", line=1):
    return {
        "path": str(path),
        "line": line,
        "column": 4,
        "severity": "warning",
        "rule_id": rule_id,
        "message": message,
        "end_line": line,
        "end_column": 6,
        "range": {"start_byte": 3, "end_byte": 5},
        "help": None,
        "fixable": True,
        "fixes": [
            {
                "start_byte": 3,
                "end_byte": 5,
                "replacement": "",
                "message": "remove trailing whitespace",
            }
        ],
    }


class LintQualityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.formatted = self.root / "formatted"
        self.metadata = self.formatted / "poky" / "meta" / "example.bb"
        self.metadata.parent.mkdir(parents=True)
        self.metadata.write_text("SUMMARY = \"demo\"  \n", encoding="utf-8")
        self.roots = [("poky", self.formatted / "poky")]

    def tearDown(self):
        self.temporary.cleanup()

    def parse(self, diagnostics):
        return normalize_lint_report(
            lint_report(self.root, diagnostics), "example", self.roots
        )

    def test_valid_reports_are_normalized_and_temporary_roots_are_removed(self):
        findings = self.parse(
            [
                lint_diagnostic(self.metadata, line=2),
                lint_diagnostic(self.metadata, rule_id="BBT004", line=1),
            ]
        )
        self.assertEqual([finding["rule_id"] for finding in findings], ["BBT004", "BBT001"])
        self.assertEqual(findings[0]["source"]["repository"], "poky")
        self.assertEqual(findings[0]["source"]["path"], "meta/example.bb")
        self.assertNotIn(str(self.root), json.dumps(findings))

    def test_malformed_version_unknown_rule_and_missing_fields_are_rejected(self):
        with self.assertRaisesRegex(CompatibilityError, "malformed"):
            normalize_lint_report("not json", "example", self.roots)
        with self.assertRaisesRegex(CompatibilityError, "version"):
            normalize_lint_report(json.dumps({"version": 99, "diagnostics": []}), "example", self.roots)
        unknown = lint_diagnostic(self.metadata, rule_id="BBT999")
        with self.assertRaisesRegex(CompatibilityError, "unknown rule"):
            self.parse([unknown])
        outside = lint_diagnostic(self.root / "outside.bb")
        with self.assertRaisesRegex(CompatibilityError, "cannot be normalized"):
            self.parse([outside])
        missing = lint_diagnostic(self.metadata)
        del missing["message"]
        with self.assertRaisesRegex(CompatibilityError, "required field"):
            self.parse([missing])

    def test_path_normalization_and_hashing_are_deterministic(self):
        first = self.parse(
            [lint_diagnostic(self.metadata, line=2), lint_diagnostic(self.metadata)]
        )
        second = self.parse(
            [lint_diagnostic(self.metadata), lint_diagnostic(self.metadata, line=2)]
        )
        self.assertEqual(first, second)
        first_summary = summarize_lint_findings("example", first)
        second_summary = summarize_lint_findings("example", second)
        self.assertEqual(first_summary["findings_sha256"], second_summary["findings_sha256"])
        self.assertEqual(first_summary["rules"]["BBT001"]["findings_sha256"], second_summary["rules"]["BBT001"]["findings_sha256"])
        with tempfile.TemporaryDirectory() as other_temporary:
            other_root = Path(other_temporary)
            other_formatted = other_root / "formatted" / "poky"
            other_metadata = other_formatted / "meta" / "example.bb"
            other_metadata.parent.mkdir(parents=True)
            other_metadata.write_text("SUMMARY = \"demo\"  \n", encoding="utf-8")
            other = normalize_lint_report(
                lint_report(other_root, [lint_diagnostic(other_metadata)]),
                "example",
                [("poky", other_formatted)],
            )
            expected = self.parse([lint_diagnostic(self.metadata)])
            self.assertEqual(expected, other)

    def test_baseline_comparison_detects_counts_fingerprints_and_rule_transitions(self):
        original = self.parse([lint_diagnostic(self.metadata)])
        original_summary = summarize_lint_findings("example", original)
        baseline = build_lint_baseline(original_summary)
        baseline["rules"]["BBT001"]["review"] = {
            "status": "reviewed",
            "sample_size": 1,
            "true_positive": 1,
            "false_positive": 0,
            "unclear": 0,
            "notes": "Reviewed the whitespace finding.",
        }
        unchanged = compare_lint_baseline(
            original_summary, baseline, "supported", "example", self.root / "baseline.json"
        )
        self.assertEqual(unchanged["status"], "passed")

        changed = self.parse([lint_diagnostic(self.metadata, message="changed finding")])
        changed_summary = summarize_lint_findings("example", changed, baseline)
        changed_comparison = compare_lint_baseline(
            changed_summary, baseline, "supported", "example", self.root / "baseline.json"
        )
        self.assertIn("BBT001", changed_comparison["digest_changes"])
        self.assertTrue(changed_comparison["review_status_failures"])

        counted = self.parse([lint_diagnostic(self.metadata), lint_diagnostic(self.metadata, line=2)])
        counted_summary = summarize_lint_findings("example", counted, baseline)
        counted_comparison = compare_lint_baseline(
            counted_summary, baseline, "supported", "example", self.root / "baseline.json"
        )
        self.assertEqual(counted_comparison["count_changes"]["BBT001"]["current"], 2)

        old_with_two = build_lint_baseline(
            summarize_lint_findings(
                "example",
                self.parse(
                    [
                        lint_diagnostic(self.metadata),
                        lint_diagnostic(self.metadata, rule_id="BBT002"),
                    ]
                ),
            )
        )
        clean_summary = summarize_lint_findings("example", original)
        transitions = compare_lint_baseline(
            clean_summary, old_with_two, "supported", "example", self.root / "baseline.json"
        )
        self.assertIn("BBT002", transitions["newly_clean_rules"])

    def test_unreviewed_findings_fail_review_and_updates_reset_changed_rules(self):
        findings = self.parse([lint_diagnostic(self.metadata)])
        summary = summarize_lint_findings("example", findings)
        baseline = build_lint_baseline(summary)
        comparison = compare_lint_baseline(
            summary, baseline, "supported", "example", self.root / "baseline.json"
        )
        self.assertTrue(comparison["review_status_failures"])

        baseline["rules"]["BBT001"]["review"] = {
            "status": "reviewed",
            "sample_size": 1,
            "true_positive": 1,
            "false_positive": 0,
            "unclear": 0,
            "notes": "Reviewed.",
        }
        updated = build_lint_baseline(
            summarize_lint_findings(
                "example", self.parse([lint_diagnostic(self.metadata, message="new")])
            ),
            baseline,
        )
        self.assertEqual(updated["rules"]["BBT001"]["review"]["status"], "unreviewed")

    def test_missing_baseline_is_nonblocking_only_for_moving_development(self):
        summary = summarize_lint_findings("example", self.parse([lint_diagnostic(self.metadata)]))
        supported = compare_lint_baseline(
            summary, None, "supported", "example", self.root / "baseline.json"
        )
        community = compare_lint_baseline(
            summary, None, "development", "community-master", self.root / "baseline.json"
        )
        development = compare_lint_baseline(
            summary, None, "development", "yocto-master", self.root / "baseline.json"
        )
        self.assertTrue(supported["blocking_failures"])
        self.assertTrue(community["blocking_failures"])
        self.assertFalse(development["blocking_failures"])

    def test_generated_baseline_has_explicit_review_records_for_all_rules(self):
        summary = summarize_lint_findings("example", self.parse([lint_diagnostic(self.metadata)]))
        baseline = build_lint_baseline(summary)
        validate_lint_baseline(baseline, "example")
        self.assertEqual(len(baseline["rules"]), 37)
        self.assertEqual(baseline["rules"]["BBT001"]["review"]["status"], "unreviewed")
        self.assertEqual(baseline["rules"]["BBT002"]["review"]["status"], "not-applicable")

    def test_false_positive_samples_require_a_remediation_decision(self):
        summary = summarize_lint_findings("example", self.parse([lint_diagnostic(self.metadata)]))
        baseline = build_lint_baseline(summary)
        baseline["rules"]["BBT001"]["review"] = {
            "status": "reviewed",
            "sample_size": 1,
            "true_positive": 0,
            "false_positive": 1,
            "unclear": 0,
            "notes": "Known false positive.",
        }
        with self.assertRaisesRegex(CompatibilityError, "remediation decision"):
            validate_lint_baseline(baseline, "example")

    def test_ci_baseline_updates_require_an_intentionally_named_override(self):
        arguments = argparse.Namespace(
            update_lint_baseline=True,
            allow_ci_lint_baseline_update=False,
        )
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}, clear=False):
            with self.assertRaisesRegex(CompatibilityError, "disabled"):
                baseline_update_allowed(arguments)
        arguments.allow_ci_lint_baseline_update = True
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}, clear=False):
            baseline_update_allowed(arguments)

    def test_evidence_bundle_contains_machine_readable_lint_files(self):
        findings = self.parse([lint_diagnostic(self.metadata)])
        summary = summarize_lint_findings("example", findings)
        comparison = compare_lint_baseline(
            summary, None, "development", "example", self.root / "baseline.json"
        )
        evidence = self.root / "evidence"
        write_lint_evidence(evidence, findings, summary, comparison)
        self.assertEqual(
            {
                path.name for path in (evidence / "lint").iterdir()
            },
            {"findings.json", "summary.json", "baseline-comparison.json"},
        )
        stored = json.loads((evidence / "lint" / "findings.json").read_text(encoding="utf-8"))
        self.assertEqual(len(stored["findings"]), 1)

    def test_reordered_reports_write_identical_normalized_evidence(self):
        diagnostics = [
            lint_diagnostic(self.metadata, rule_id="BBT002", message="second"),
            lint_diagnostic(self.metadata, rule_id="BBT001", message="first"),
        ]
        first = self.parse(diagnostics)
        second = self.parse(list(reversed(diagnostics)))
        first_evidence = self.root / "first-evidence"
        second_evidence = self.root / "second-evidence"
        for evidence, findings in ((first_evidence, first), (second_evidence, second)):
            summary = summarize_lint_findings("example", findings)
            comparison = compare_lint_baseline(
                summary, None, "development", "example", self.root / "baseline.json"
            )
            write_lint_evidence(evidence, findings, summary, comparison)
        for name in ("findings.json", "summary.json"):
            self.assertEqual(
                (first_evidence / "lint" / name).read_bytes(),
                (second_evidence / "lint" / name).read_bytes(),
            )

    def test_old_text_line_counting_output_is_rejected(self):
        with self.assertRaisesRegex(CompatibilityError, "malformed"):
            normalize_lint_report(
                "/tmp/example.bb:1:1: warning[BBT001]: finding\n",
                "example",
                self.roots,
            )


if __name__ == "__main__":
    unittest.main()
