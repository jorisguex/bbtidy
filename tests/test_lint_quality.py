import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from scripts.lint_quality import (
    FINGERPRINT_VERSION,
    KNOWN_RULE_IDS,
    LintNormalizationError,
    NormalizationContext,
    canonical_json_bytes,
    digest_findings,
    digest_rule_findings,
    finding_sort_key,
    normalize_diagnostic,
    normalize_lint_report,
    summarize_findings,
)


def diagnostic(source_path, **overrides):
    value = {
        "path": str(source_path),
        "line": 2,
        "column": 3,
        "end_line": 2,
        "end_column": 9,
        "range": {"start_byte": 10, "end_byte": 16},
        "rule_id": "BBT001",
        "severity": "warning",
        "message": "trailing whitespace",
        "help": None,
        "fixable": False,
        "fixes": [],
    }
    value.update(overrides)
    return value


class LintQualityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.formatted = root / "formatted"
        self.poky = self.formatted / "poky"
        self.extra = self.formatted / "meta-extra"
        (self.poky / "meta" / "recipes" / "example").mkdir(parents=True)
        (self.extra / "meta" / "recipes" / "example").mkdir(parents=True)
        self.example = self.poky / "meta" / "recipes" / "example" / "example.bb"
        self.example.write_text("SUMMARY = 'x'\n", encoding="utf-8")
        self.extra_example = self.extra / "meta" / "recipes" / "example" / "example.bb"
        self.extra_example.write_text("SUMMARY = 'x'\n", encoding="utf-8")
        self.context = NormalizationContext(
            repository_roots=(("poky", self.poky), ("meta-extra", self.extra)),
            path_base=root,
        )

    def tearDown(self):
        self.temporary.cleanup()

    def report(self, diagnostics):
        return {"version": 1, "diagnostics": diagnostics}

    def normalize(self, diagnostics, context=None):
        return normalize_lint_report(self.report(diagnostics), context or self.context)

    def test_minimal_report_emits_fixed_canonical_keys(self):
        finding = self.normalize([diagnostic(self.example)])[0]
        self.assertEqual(
            set(finding),
            {
                "fingerprint_version",
                "source",
                "rule_id",
                "severity",
                "range",
                "message",
                "help",
                "fixable",
                "fixes",
            },
        )
        self.assertEqual(finding["fingerprint_version"], 1)
        self.assertEqual(finding["source"]["path"], "meta/recipes/example/example.bb")
        self.assertIsNone(finding["help"])

    def test_schema_and_required_fields_are_strict(self):
        for version in (2, 1.0, True):
            with self.subTest(version=version), self.assertRaisesRegex(
                LintNormalizationError, "version"
            ):
                normalize_lint_report({"version": version, "diagnostics": []}, self.context)
        with self.assertRaisesRegex(LintNormalizationError, "diagnostics"):
            normalize_lint_report({"version": 1}, self.context)
        with self.assertRaisesRegex(LintNormalizationError, "diagnostic 0"):
            normalize_lint_report({"version": 1, "diagnostics": [None]}, self.context)
        for field in ("rule_id", "severity", "range", "message", "fixable", "fixes"):
            value = diagnostic(self.example)
            value.pop(field)
            with self.subTest(field=field), self.assertRaisesRegex(
                LintNormalizationError, "diagnostic 0"
            ):
                self.normalize([value])

    def test_invalid_values_and_fix_consistency_are_rejected(self):
        cases = [
            ("unknown rule", {"rule_id": "BBT999"}),
            ("severity", {"severity": "fatal"}),
            ("line", {"line": True}),
            ("position", {"line": 0}),
            ("range", {"range": {"start_byte": 8, "end_byte": 4}}),
            ("inverted", {"line": 3, "end_line": 2}),
            ("help", {"help": 4}),
            ("fixable", {"fixable": "yes"}),
            ("fixes", {"fixes": {}}),
            ("inconsistent", {"fixable": True, "fixes": []}),
            (
                "malformed fix",
                {"fixable": True, "fixes": [{"start_byte": 1}]},
            ),
        ]
        for label, overrides in cases:
            with self.subTest(label=label), self.assertRaises(LintNormalizationError):
                self.normalize([diagnostic(self.example, **overrides)])

    def test_paths_are_repository_relative_and_longest_root_wins(self):
        finding = self.normalize(
            [diagnostic(self.extra_example, path=str(self.extra_example / ".." / "example.bb"))]
        )[0]
        self.assertEqual(finding["source"]["repository"], "meta-extra")
        self.assertEqual(finding["source"]["path"], "meta/recipes/example/example.bb")
        self.assertNotIn(str(self.formatted), json.dumps(finding))

    def test_prefix_collision_does_not_map_meta_to_meta_extra(self):
        finding = self.normalize([diagnostic(self.extra_example)])[0]
        self.assertEqual(finding["source"]["repository"], "meta-extra")

    def test_ambiguous_and_outside_paths_fail_with_context(self):
        ambiguous = NormalizationContext(
            repository_roots=(("one", self.poky), ("two", self.poky)),
            path_base=self.formatted.parent,
        )
        with self.assertRaisesRegex(LintNormalizationError, "diagnostic 0"):
            self.normalize([diagnostic(self.example)], ambiguous)
        outside = self.formatted.parent / "outside.bb"
        with self.assertRaisesRegex(LintNormalizationError, "outside"):
            self.normalize([diagnostic(outside)])

        absolute_context_without_base = NormalizationContext(
            repository_roots=(("poky", self.poky),)
        )
        with self.assertRaisesRegex(LintNormalizationError, "path_base"):
            self.normalize([diagnostic("meta/example.bb")], absolute_context_without_base)

    def test_symlink_to_outside_root_is_not_followed_into_the_corpus(self):
        outside = self.formatted.parent / "outside.bb"
        outside.write_text("x\n", encoding="utf-8")
        link = self.poky / "meta" / "recipes" / "example" / "link.bb"
        try:
            link.symlink_to(outside)
        except (NotImplementedError, OSError):
            self.skipTest("symbolic links are unavailable")
        with self.assertRaises(LintNormalizationError):
            self.normalize([diagnostic(link)])

    def test_spaces_unicode_and_posix_separator_are_stable(self):
        unicode_path = self.poky / "meta" / "recipes" / "example" / "space é.bb"
        unicode_path.write_text("x\n", encoding="utf-8")
        finding = self.normalize([diagnostic(unicode_path)])[0]
        self.assertEqual(finding["source"]["path"], "meta/recipes/example/space é.bb")

    def test_different_temporary_roots_produce_identical_findings(self):
        with tempfile.TemporaryDirectory() as other_temporary:
            other_root = Path(other_temporary) / "formatted" / "poky"
            (other_root / "meta" / "recipes" / "example").mkdir(parents=True)
            other_file = other_root / "meta" / "recipes" / "example" / "example.bb"
            other_file.write_text("SUMMARY = 'x'\n", encoding="utf-8")
            other_context = NormalizationContext(
                repository_roots=(("poky", other_root),),
                path_base=other_root.parent.parent,
            )
            self.assertEqual(
                self.normalize([diagnostic(self.example)]),
                self.normalize([diagnostic(other_file)], other_context),
            )

    def test_known_roots_in_text_are_replaced_but_unrelated_text_is_exact(self):
        message = "see {} and /tmp/not-a-corpus/path".format(self.example)
        finding = self.normalize(
            [
                diagnostic(
                    self.example,
                    message=message,
                    help="{}uffix".format(self.example),
                    fixable=True,
                    fixes=[
                        {
                            "start_byte": 10,
                            "end_byte": 11,
                            "replacement": "line\ntext",
                            "message": "fix {}".format(self.example),
                        }
                    ],
                )
            ]
        )[0]
        self.assertIn("$CORPUS/poky/meta/recipes/example/example.bb", finding["message"])
        self.assertIn("/tmp/not-a-corpus/path", finding["message"])
        self.assertEqual(finding["help"], "$CORPUS/poky/meta/recipes/example/example.bbuffix")
        self.assertEqual(finding["fixes"][0]["replacement"], "line\ntext")

    def test_fix_order_and_duplicate_findings_are_preserved(self):
        fixes = [
            {"start_byte": 20, "end_byte": 22, "replacement": "z", "message": "z"},
            {"start_byte": 10, "end_byte": 12, "replacement": "a", "message": "a"},
        ]
        finding = self.normalize(
            [diagnostic(self.example, fixable=True, fixes=fixes)]
        )[0]
        self.assertEqual([fix["start_byte"] for fix in finding["fixes"]], [10, 20])
        duplicate = self.normalize([diagnostic(self.example), diagnostic(self.example)])
        self.assertEqual(len(duplicate), 2)
        self.assertEqual(digest_findings(duplicate), digest_findings(list(reversed(duplicate))))

    def test_shuffled_diagnostics_and_equal_position_tiebreakers_are_stable(self):
        first = [
            diagnostic(self.example, rule_id="BBT002", message="b"),
            diagnostic(self.example, rule_id="BBT001", message="a"),
            diagnostic(self.example, rule_id="BBT001", message="z"),
            diagnostic(self.example, rule_id="BBT001", message="a", help="details"),
        ]
        second = list(reversed(first))
        normalized_first = self.normalize(first)
        normalized_second = self.normalize(second)
        self.assertEqual(normalized_first, normalized_second)
        self.assertEqual(digest_findings(normalized_first), digest_findings(normalized_second))
        self.assertEqual(
            [finding["message"] for finding in normalized_first], ["a", "a", "z", "b"]
        )

    def test_canonical_json_is_key_order_independent_unicode_safe_and_compact(self):
        first = {"z": "é", "a": [1, 2]}
        second = {"a": [1, 2], "z": "é"}
        self.assertEqual(canonical_json_bytes(first), canonical_json_bytes(second))
        self.assertEqual(
            canonical_json_bytes(first), '{"a":[1,2],"z":"é"}'.encode("utf-8")
        )
        self.assertNotIn(b" ", canonical_json_bytes(first))
        with self.assertRaises(LintNormalizationError):
            canonical_json_bytes(float("nan"))

    def test_domain_separated_digest_vectors_and_versioning(self):
        finding = self.normalize([diagnostic(self.example)])[0]
        whole = digest_findings([finding])
        per_rule = digest_rule_findings("BBT001", [finding])
        self.assertEqual(
            whole, "7f01f3eed20bc06bea9d060cf5cecda4a0fd73158c913e087d2f4f3246ba8a48"
        )
        self.assertEqual(
            per_rule, "39d133eaa841c74c7eb4c4797abea361b5518ad8631e615b6c38f44ac7e5838c"
        )
        self.assertNotEqual(whole, per_rule)
        payload = {
            "kind": "bbtidy-lint-findings",
            "fingerprint_version": FINGERPRINT_VERSION,
            "findings": [finding],
        }
        self.assertEqual(whole, hashlib.sha256(canonical_json_bytes(payload)).hexdigest())
        changed = dict(finding)
        changed["fingerprint_version"] = 2
        self.assertNotEqual(whole, digest_findings([changed]))

    def test_summary_is_derived_only_from_findings_and_includes_clean_rules(self):
        finding = self.normalize([diagnostic(self.example)])[0]
        summary = summarize_findings([finding])
        self.assertEqual(summary["total_findings"], 1)
        self.assertEqual(summary["files_with_findings"], 1)
        self.assertEqual(summary["severity_counts"], {"info": 0, "warning": 1, "error": 0})
        self.assertEqual(list(summary["rules"]), list(KNOWN_RULE_IDS))
        self.assertEqual(summary["rules"]["BBT001"]["count"], 1)
        self.assertEqual(summary["rules"]["BBT001"]["files"], 1)
        self.assertEqual(summary["rules"]["BBT002"]["count"], 0)

    def test_summary_counts_distinct_files_and_duplicate_findings(self):
        findings = self.normalize(
            [
                diagnostic(self.example),
                diagnostic(self.example),
                diagnostic(self.extra_example),
            ]
        )
        summary = summarize_findings(findings)
        self.assertEqual(summary["total_findings"], 3)
        self.assertEqual(summary["files_with_findings"], 2)
        self.assertEqual(summary["rules"]["BBT001"]["count"], 3)
        self.assertEqual(summary["rules"]["BBT001"]["files"], 2)


if __name__ == "__main__":
    unittest.main()
