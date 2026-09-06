import copy
import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from scripts.adoption_pilot import (
    ACTIONABILITY, COHORTS, evaluate, observation_form, review_form, review_packet,
)
from scripts.lint_quality import (
    LintBaselineError, PILOT_THRESHOLDS, _finding_digest,
    evaluate_pilot_thresholds, stratified_review_sample,
)


def finding(index=0, repository="poky"):
    return {
        "fingerprint_version": 1, "rule_id": "BBT005", "severity": "warning",
        "source": {"repository": repository, "path": f"meta/example{index}.bb"},
        "range": {"start": {"byte": 0, "line": 1, "column": 1}, "end": {"byte": 1, "line": 1, "column": 2}},
        "message": "missing license checksum", "help": None, "fixable": False, "fixes": [],
    }


class AdoptionPilotTests(unittest.TestCase):
    def setUp(self):
        self.packet = review_packet([finding()], "test-corpus")
        self.reviews = review_form(self.packet)
        self.fingerprint = next(iter(self.reviews["rules"]["BBT005"]["samples"]))
        self.observations = {**observation_form(self.packet), "sessions": [
            {"id": str(index), "cohort": cohort, "completed": True,
             "new_config_minutes": 10 if cohort == "new-config" else None,
             "operational_failures_mistaken_for_lint": 0, "unsafe_edits": 0}
            for index, cohort in enumerate(COHORTS)
        ]}

    def complete_review(self):
        rule = self.reviews["rules"]["BBT005"]
        rule["reviewer"] = "test fixture reviewer"
        rule["samples"][self.fingerprint].update(correctness="true_positive", actionability="should_fix")

    def test_threshold_direction_and_failed_evidence(self):
        metrics = {key: 0 for key in PILOT_THRESHOLDS}
        for actionability in (0.7, 0.8, 1):
            metrics["recommended_actionable_rate"] = actionability
            self.assertEqual(evaluate_pilot_thresholds({}, metrics)["status"], "pass")
        metrics["recommended_actionable_rate"] = 0.69
        self.assertEqual(evaluate_pilot_thresholds({}, metrics)["status"], "fail")
        self.assertEqual(evaluate_pilot_thresholds({}, {"unsafe_edits": 1})["status"], "fail")
        self.assertEqual(evaluate_pilot_thresholds({})["status"], "insufficient-evidence")
        for key, invalid in (("unsafe_edits", True), ("unsafe_edits", 0.5), ("new_config_minutes", -1), ("new_config_minutes", float("nan")), ("new_config_minutes", float("inf")), ("recommended_actionable_rate", 1.1)):
            with self.subTest(key=key, value=invalid), self.assertRaises(LintBaselineError):
                evaluate_pilot_thresholds({}, {key: invalid})

    def test_blank_human_forms_do_not_manufacture_success(self):
        result = evaluate(self.packet, self.reviews, observation_form(self.packet))
        self.assertEqual(result["status"], "insufficient-evidence")
        self.assertTrue(all(value["value"] is None for value in result["thresholds"].values()))
        self.assertEqual(result["review_coverage"]["BBT005"]["reviewed"], 0)

    def test_complete_review_and_observed_cohorts_are_required(self):
        self.complete_review()
        self.assertEqual(evaluate(self.packet, self.reviews, self.observations)["status"], "pass")
        self.observations["sessions"].pop(0)
        result = evaluate(self.packet, self.reviews, self.observations)
        self.assertEqual(result["status"], "insufficient-evidence")
        self.assertEqual(result["default_decision"], "retain-all-and-collect-pilot-evidence")

    def test_missing_observations_and_unsafe_incomplete_sessions(self):
        self.complete_review()
        self.observations["sessions"][0]["unsafe_edits"] = None
        self.assertEqual(evaluate(self.packet, self.reviews, self.observations)["status"], "insufficient-evidence")
        self.observations["sessions"][0].update(completed=False, unsafe_edits=1)
        self.assertEqual(evaluate(self.packet, self.reviews, self.observations)["status"], "fail")

    def test_invalid_session_observations_are_rejected(self):
        for field, value in (("id", ""), ("completed", "yes"), ("unsafe_edits", -1), ("cohort", "invented"), ("new_config_minutes", float("nan"))):
            observations = copy.deepcopy(self.observations)
            observations["sessions"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                evaluate(self.packet, self.reviews, observations)
        self.observations["sessions"].append(self.observations["sessions"][0])
        with self.assertRaises(ValueError):
            evaluate(self.packet, self.reviews, self.observations)

    def test_classifications_need_identity_and_remediation(self):
        self.complete_review()
        rule = self.reviews["rules"]["BBT005"]
        rule["reviewer"] = None
        with self.assertRaises(ValueError):
            evaluate(self.packet, self.reviews, self.observations)
        rule["reviewer"] = "test fixture reviewer"
        decision = rule["samples"][self.fingerprint]
        decision["correctness"] = "false_positive"
        with self.assertRaises(ValueError):
            evaluate(self.packet, self.reviews, self.observations)
        decision["notes"] = "Test fixture limitation and proposed remediation."
        self.assertEqual(evaluate(self.packet, self.reviews, self.observations)["status"], "fail")
        decision["correctness"] = "approved"
        with self.assertRaises(ValueError):
            evaluate(self.packet, self.reviews, self.observations)

    def test_stale_reviews_and_tampered_packets_are_rejected(self):
        for field in ("corpus_id", "findings_sha256"):
            reviews = copy.deepcopy(self.reviews)
            reviews[field] = "changed"
            with self.assertRaises(ValueError):
                evaluate(self.packet, reviews, self.observations)
        packet = copy.deepcopy(self.packet)
        packet["rules"]["BBT005"]["samples"][0]["finding"]["message"] = "changed"
        with self.assertRaises(ValueError):
            evaluate(packet, self.reviews, self.observations)
        self.reviews["rules"]["BBT005"]["samples"] = {}
        with self.assertRaises(ValueError):
            evaluate(self.packet, self.reviews, self.observations)

    def test_actionability_is_never_inferred_from_correctness(self):
        self.complete_review()
        decision = self.reviews["rules"]["BBT005"]["samples"][self.fingerprint]
        for actionability in ACTIONABILITY:
            decision["actionability"] = actionability
            result = evaluate(self.packet, self.reviews, self.observations)
            expected = "pass" if actionability in ("must_fix", "should_fix") else "fail"
            self.assertEqual(result["status"], expected)
        decision["actionability"] = None
        self.assertEqual(evaluate(self.packet, self.reviews, self.observations)["status"], "insufficient-evidence")

    def test_sampling_preserves_minor_repositories(self):
        findings = [finding(index, repository) for repository, count in (("a", 1000), ("b", 10), ("c", 10)) for index in range(count)]
        selected = set(stratified_review_sample(findings, 30))
        self.assertEqual(len(selected), 30)
        for repository in ("a", "b", "c"):
            self.assertGreaterEqual(sum(_finding_digest(value) in selected and value["source"]["repository"] == repository for value in findings), 3)
        self.assertEqual(stratified_review_sample(findings, 30), stratified_review_sample(reversed(findings), 30))

    def test_checked_in_packets_match_pinned_baselines_and_remain_unreviewed(self):
        root = Path("tests/upstream-corpora/lint-reviews")
        sources = json.loads((root / "sources.json").read_text())["sources"]
        for source in sources:
            packet_path = root / source["packet"]
            self.assertEqual(hashlib.sha256(packet_path.read_bytes()).hexdigest(), source["packet_sha256"])
            packet = json.loads(packet_path.read_text())
            baseline = json.loads((root.parent / "lint-baselines" / (source["corpus_id"] + ".json")).read_text())
            self.assertEqual(packet["findings_sha256"], baseline["measurement"]["findings_sha256"])
            for rule_id, rule in packet["rules"].items():
                self.assertEqual(rule["total_findings"], baseline["measurement"]["rules"][rule_id]["count"])
                self.assertEqual(len(rule["samples"]), rule["required_samples"])
            reviews = json.loads((packet_path.parent / "reviews.json").read_text())
            observations = json.loads((packet_path.parent / "observations.json").read_text())
            self.assertEqual(evaluate(packet, reviews, observations)["status"], "insufficient-evidence")

    def test_cli_prepares_once_and_reports_incomplete_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            findings = root / "findings.json"
            findings.write_text(json.dumps({"schema": 1, "fingerprint_version": 1, "corpus_id": "test", "findings": [finding()]}))
            output = root / "pilot"
            command = [sys.executable, "scripts/adoption_pilot.py", "prepare", "--findings", str(findings), "--output", str(output)]
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 0)
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 2)
            command = [sys.executable, "scripts/adoption_pilot.py", "evaluate", "--packet", str(output / "packet.json"), "--reviews", str(output / "reviews.json"), "--observations", str(output / "observations.json"), "--output", str(output / "result.json")]
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 3)
            self.assertEqual(json.loads((output / "result.json").read_text())["status"], "insufficient-evidence")


if __name__ == "__main__":
    unittest.main()
