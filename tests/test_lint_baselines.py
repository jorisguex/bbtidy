import copy
import json
import tempfile
import unittest
from pathlib import Path

from scripts.check_upstream_corpus import (
    CompatibilityError,
    compare_lint_baseline as compare_harness_lint_baseline,
    load_manifest,
)
from scripts.lint_quality import (
    LintBaselineError,
    NormalizationContext,
    baseline_for_update,
    baseline_from_summary,
    canonical_baseline_bytes,
    compare_lint_baseline,
    normalize_lint_report,
    review_policy_failures,
    review_summary,
    summarize_findings,
    validate_lint_baseline,
)


def manifest_fixture(corpus_id="example"):
    return {
        "schema": 1,
        "id": corpus_id,
        "tier": "supported",
        "yocto_version": "1.0",
        "bitbake_version": "1.0",
        "repositories": [
            {
                "name": "poky",
                "revision": "1" * 40,
                "url": "https://example.invalid/poky.git",
                "sparse_paths": ["meta/"],
            }
        ],
        "layers": [
            {
                "name": "openembedded-core",
                "repository": "poky",
                "path": "meta",
                "minimum_files": 1,
            }
        ],
        "syntax_metrics": {
            "minimum_structured_nodes": 0,
            "maximum_unknown_nodes": 0,
        },
        "bitbake": {
            "init_repository": "poky",
            "template": "meta/conf/templates/default",
            "target": "example",
            "additional_layers": [],
        },
    }


def diagnostic(path, rule_id="BBT001", severity="warning", message="finding"):
    return {
        "path": str(path),
        "line": 1,
        "column": 1,
        "end_line": 1,
        "end_column": 2,
        "range": {"start_byte": 0, "end_byte": 1},
        "rule_id": rule_id,
        "severity": severity,
        "message": message,
        "help": None,
        "fixable": False,
        "fixes": [],
    }


class BaselineFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.repository = self.root / "formatted" / "poky"
        self.metadata = self.repository / "meta" / "example.bb"
        self.metadata.parent.mkdir(parents=True)
        self.metadata.write_text("SUMMARY = 'example'\n", encoding="utf-8")
        self.context = NormalizationContext(
            repository_roots=(("poky", self.repository),),
            path_base=self.root,
        )
        self.manifest = manifest_fixture()

    def tearDown(self):
        self.temporary.cleanup()

    def summary(self, diagnostics=None):
        findings = normalize_lint_report(
            {
                "version": 1,
                "diagnostics": diagnostics
                if diagnostics is not None
                else [diagnostic(self.metadata)],
            },
            self.context,
        )
        summary = summarize_findings(findings)
        summary["corpus_id"] = self.manifest["id"]
        return summary

    def baseline(self, summary=None):
        return baseline_from_summary(self.manifest, summary or self.summary())


class BaselineSchemaTests(BaselineFixture):
    def test_generated_schema_separates_measurement_and_review(self):
        baseline = self.baseline()
        validate_lint_baseline(baseline, self.manifest)
        self.assertEqual(
            set(baseline), {"schema", "corpus", "lint_contract", "measurement", "review"}
        )
        self.assertEqual(baseline["lint_contract"]["source_state"], "formatted")
        self.assertEqual(baseline["measurement"]["rules"]["BBT001"]["count"], 1)
        self.assertEqual(baseline["review"]["status"], "unreviewed")
        self.assertEqual(baseline["review"]["rules"]["BBT001"]["status"], "unreviewed")
        self.assertNotIn(str(self.root), canonical_baseline_bytes(baseline).decode("utf-8"))

    def test_strict_schema_rejects_unknown_top_level_and_bad_schema(self):
        baseline = self.baseline()
        for mutated in (dict(baseline, extra=True), dict(baseline, schema=2)):
            with self.subTest(mutated=mutated), self.assertRaises(LintBaselineError):
                validate_lint_baseline(mutated, self.manifest)

    def test_measurement_invariants_reject_boolean_negative_and_missing_severity(self):
        baseline = self.baseline()
        cases = []
        boolean_count = copy.deepcopy(baseline)
        boolean_count["measurement"]["total_findings"] = True
        cases.append(boolean_count)
        negative = copy.deepcopy(baseline)
        negative["measurement"]["rules"]["BBT001"]["count"] = -1
        cases.append(negative)
        missing_severity = copy.deepcopy(baseline)
        del missing_severity["measurement"]["severity_counts"]["error"]
        cases.append(missing_severity)
        for case in cases:
            with self.subTest(case=case), self.assertRaises(LintBaselineError):
                validate_lint_baseline(case, self.manifest)

    def test_review_invariants_reject_missing_records_unknown_status_and_bad_totals(self):
        baseline = self.baseline()
        missing = copy.deepcopy(baseline)
        del missing["review"]["rules"]["BBT001"]
        with self.assertRaisesRegex(LintBaselineError, "review record"):
            validate_lint_baseline(missing, self.manifest)

        unknown = copy.deepcopy(baseline)
        unknown["review"]["rules"]["BBT001"]["status"] = "approved"
        with self.assertRaisesRegex(LintBaselineError, "unknown status"):
            validate_lint_baseline(unknown, self.manifest)

        inconsistent = copy.deepcopy(baseline)
        inconsistent["review"]["rules"]["BBT001"]["sample_size"] = 1
        inconsistent["review"]["rules"]["BBT001"]["true_positive"] = 0
        with self.assertRaisesRegex(LintBaselineError, "classifications"):
            validate_lint_baseline(inconsistent, self.manifest)

    def test_corpus_identity_rejects_revision_layer_and_cross_corpus_changes(self):
        baseline = self.baseline()
        altered_revision = copy.deepcopy(baseline)
        altered_revision["corpus"]["repositories"][0]["revision"] = "2" * 40
        with self.assertRaises(LintBaselineError):
            validate_lint_baseline(altered_revision, self.manifest)

        altered_layer = copy.deepcopy(self.manifest)
        altered_layer["layers"][0]["path"] = "other"
        with self.assertRaises(LintBaselineError):
            validate_lint_baseline(baseline, altered_layer)

    def test_corpus_identity_rejects_duplicate_repository_and_layer_names(self):
        baseline = self.baseline()
        duplicate_repository = copy.deepcopy(baseline)
        duplicate_repository["corpus"]["repositories"].append(
            copy.deepcopy(duplicate_repository["corpus"]["repositories"][0])
        )
        with self.assertRaises(LintBaselineError):
            validate_lint_baseline(duplicate_repository)

        duplicate_layer = copy.deepcopy(baseline)
        duplicate_layer["corpus"]["layers"].append(
            copy.deepcopy(duplicate_layer["corpus"]["layers"][0])
        )
        with self.assertRaises(LintBaselineError):
            validate_lint_baseline(duplicate_layer)

    def test_deterministic_serialization_preserves_unicode_and_one_newline(self):
        baseline = self.baseline()
        baseline["review"]["rules"]["BBT001"]["notes"] = "Révisé — пример"
        first = canonical_baseline_bytes(baseline)
        second = canonical_baseline_bytes(json.loads(first.decode("utf-8")))
        self.assertEqual(first, second)
        self.assertTrue(first.endswith(b"\n"))
        self.assertFalse(first.endswith(b"\n\n"))
        self.assertIn("Révisé".encode("utf-8"), first)


class BaselineComparisonTests(BaselineFixture):
    def test_exact_match(self):
        result = compare_lint_baseline(self.baseline(), self.summary(), self.manifest)
        self.assertEqual(result["status"], "matched")
        self.assertFalse(result["digest_changed"])

    def test_count_neutral_fingerprint_change_is_changed(self):
        baseline = self.baseline()
        current = self.summary([diagnostic(self.metadata, message="different")])
        result = compare_lint_baseline(baseline, current, self.manifest)
        self.assertEqual(result["status"], "changed")
        self.assertEqual(result["total_delta"], 0)
        self.assertTrue(result["digest_changed"])
        self.assertIn("BBT001", result["rules_changed"])

    def test_severity_and_per_rule_transitions_are_reported(self):
        baseline = self.baseline()
        current = self.summary(
            [
                diagnostic(self.metadata, rule_id="BBT002", severity="error"),
                diagnostic(self.metadata, rule_id="BBT001", severity="error"),
            ]
        )
        result = compare_lint_baseline(baseline, current, self.manifest)
        self.assertEqual(result["status"], "changed")
        self.assertEqual(result["rules_added"], ["BBT002"])
        self.assertEqual(result["severity_changes"]["error"]["current"], 2)
        self.assertEqual(list(result["rules_changed"]), ["BBT001", "BBT002"])

        removed = compare_lint_baseline(
            baseline,
            self.summary([diagnostic(self.metadata, rule_id="BBT002")]),
            self.manifest,
        )
        self.assertEqual(removed["rules_added"], ["BBT002"])
        self.assertEqual(removed["rules_removed"], ["BBT001"])

    def test_invalid_and_missing_baselines_are_distinct_from_changed(self):
        missing = compare_lint_baseline(None, self.summary(), self.manifest)
        self.assertEqual(missing["status"], "invalid")
        malformed = copy.deepcopy(self.baseline())
        malformed["measurement"]["findings_sha256"] = "not-a-digest"
        invalid = compare_lint_baseline(malformed, self.summary(), self.manifest)
        self.assertEqual(invalid["status"], "invalid")
        self.assertTrue(invalid["errors"])

    def test_contract_mismatch_is_reported_deterministically(self):
        baseline = self.baseline()
        baseline["lint_contract"]["configuration"] = "project-config"
        result = compare_lint_baseline(baseline, self.summary(), self.manifest)
        self.assertEqual(result["status"], "changed")
        self.assertEqual(result["contract_changes"][0]["field"], "lint_contract")

    def test_report_and_fingerprint_version_mismatches_are_invalid(self):
        for field in ("report_version", "fingerprint_version"):
            baseline = self.baseline()
            baseline["lint_contract"][field] = 99
            result = compare_lint_baseline(baseline, self.summary(), self.manifest)
            with self.subTest(field=field):
                self.assertEqual(result["status"], "invalid")

    def test_review_policy_requires_an_explicit_decision_for_active_rules(self):
        baseline = self.baseline()
        summary = self.summary()
        self.assertEqual(review_summary(summary, baseline)["unreviewed_rules"], 1)
        self.assertEqual(
            review_policy_failures(summary, baseline), ["active rule BBT001 is unreviewed"]
        )

        baseline["review"]["rules"]["BBT001"].update(
            {
                "status": "reviewed",
                "sample_size": 1,
                "true_positive": 1,
                "repositories": ["poky"],
                "file_types": [".bb"],
                "diagnostic_shapes": ["assignment"],
            }
        )
        self.assertEqual(review_policy_failures(summary, baseline), [])

    def test_false_positive_and_unclear_samples_need_remediation_notes(self):
        baseline = self.baseline()
        record = baseline["review"]["rules"]["BBT001"]
        record.update(
            {
                "status": "reviewed",
                "sample_size": 1,
                "true_positive": 0,
                "false_positive": 1,
                "repositories": ["poky"],
                "file_types": [".bb"],
                "diagnostic_shapes": ["assignment"],
            }
        )
        with self.assertRaisesRegex(LintBaselineError, "notes"):
            validate_lint_baseline(baseline, self.manifest)
        record["notes"] = "Known limitation is tracked for narrowing."
        validate_lint_baseline(baseline, self.manifest)
        self.assertEqual(review_policy_failures(self.summary(), baseline), [])

    def test_update_preserves_unchanged_review_and_resets_changed_rules(self):
        previous = self.baseline()
        previous["review"]["rules"]["BBT001"].update(
            {
                "status": "reviewed",
                "sample_size": 1,
                "true_positive": 1,
                "repositories": ["poky"],
                "file_types": [".bb"],
                "diagnostic_shapes": ["assignment"],
            }
        )
        validate_lint_baseline(previous, self.manifest)

        unchanged = baseline_for_update(self.manifest, self.summary(), previous)
        self.assertEqual(unchanged["review"]["rules"]["BBT001"]["status"], "reviewed")

        changed_summary = self.summary([diagnostic(self.metadata, message="changed")])
        changed = baseline_for_update(self.manifest, changed_summary, previous)
        self.assertEqual(changed["review"]["rules"]["BBT001"]["status"], "unreviewed")
        validate_lint_baseline(changed, self.manifest)

        legacy_previous = copy.deepcopy(previous)
        legacy_previous["review"].pop("schema")
        legacy_changed = baseline_for_update(
            self.manifest, changed_summary, legacy_previous
        )
        self.assertNotIn("schema", legacy_changed["review"])
        validate_lint_baseline(legacy_changed, self.manifest)

    def test_large_rule_populations_need_more_than_one_review_sample(self):
        baseline = self.baseline()
        measurement = baseline["measurement"]["rules"]["BBT001"]
        measurement["count"] = 100
        baseline["measurement"]["total_findings"] = 100
        baseline["measurement"]["severity_counts"] = {"info": 0, "warning": 100, "error": 0}
        baseline["measurement"]["findings_sha256"] = "a" * 64
        measurement["severity_counts"] = {"info": 0, "warning": 100, "error": 0}
        record = baseline["review"]["rules"]["BBT001"]
        record.update(
            {
                "status": "reviewed",
                "sample_size": 1,
                "true_positive": 1,
                "repositories": ["poky"],
                "file_types": [".bb"],
                "diagnostic_shapes": ["assignment"],
            }
        )
        with self.assertRaisesRegex(LintBaselineError, "at least 5"):
            validate_lint_baseline(baseline, self.manifest)

    def test_harness_comparison_exposes_review_failures_as_blocking(self):
        summary = self.summary()
        manifest = copy.deepcopy(self.manifest)
        manifest["lint_quality"] = {"baseline": "lint-baselines/example.json"}
        comparison = compare_harness_lint_baseline(
            summary,
            self.baseline(summary),
            manifest,
        )
        self.assertEqual(comparison["status"], "matched")
        self.assertIn("active rule BBT001 is unreviewed", comparison["blocking_failures"])


class ManifestAssociationTests(unittest.TestCase):
    def write_manifest(self, manifest, baseline=None):
        temporary = tempfile.TemporaryDirectory()
        root = Path(temporary.name)
        path = root / "manifest.json"
        if baseline is not None:
            reference = manifest["lint_quality"]["baseline"]
            baseline_path = root / reference
            baseline_path.parent.mkdir(parents=True)
            baseline_path.write_bytes(canonical_baseline_bytes(baseline))
        path.write_text(json.dumps(manifest), encoding="utf-8")
        return temporary, path

    def test_pinned_manifest_requires_safe_existing_explicit_reference(self):
        manifest = manifest_fixture()
        manifest["lint_quality"] = {"baseline": "lint-baselines/example.json"}
        baseline = baseline_from_summary(manifest, {"corpus_id": "example", "total_findings": 0, "findings_sha256": "0" * 64, "files_with_findings": 0, "severity_counts": {"info": 0, "warning": 0, "error": 0}, "rules": {}})
        temporary, path = self.write_manifest(manifest, baseline)
        try:
            loaded = load_manifest(path)
            self.assertEqual(loaded["lint_quality"]["baseline"], "lint-baselines/example.json")
        finally:
            temporary.cleanup()

    def test_absolute_traversal_and_missing_references_are_rejected(self):
        for reference in (
            "/tmp/example.json",
            "C:\\tmp\\example.json",
            "../example.json",
            "lint-baselines/missing.json",
        ):
            manifest = manifest_fixture()
            manifest["lint_quality"] = {"baseline": reference}
            temporary, path = self.write_manifest(manifest)
            try:
                with self.subTest(reference=reference), self.assertRaises(CompatibilityError):
                    load_manifest(path)
            finally:
                temporary.cleanup()

    def test_moving_manifest_remains_report_only_and_rejects_attachment(self):
        root = Path(__file__).resolve().parents[1] / "tests" / "upstream-corpora"
        moving = json.loads((root / "yocto-master.json").read_text(encoding="utf-8"))
        self.assertNotIn("lint_quality", moving)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "yocto-master.json"
            path.write_text(json.dumps(moving), encoding="utf-8")
            load_manifest(path)
            moving["lint_quality"] = {"baseline": "lint-baselines/master.json"}
            path.write_text(json.dumps(moving), encoding="utf-8")
            with self.assertRaises(CompatibilityError):
                load_manifest(path)


if __name__ == "__main__":
    unittest.main()
