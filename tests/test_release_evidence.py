import hashlib
import json
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import verify_release_evidence as evidence
from scripts.lint_quality import KNOWN_RULE_IDS, summarize_findings


class ReleaseEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.evidence_root = self.root / "evidence"
        self.evidence_root.mkdir()
        self.source_commit = "a" * 40
        self.version = "1.2.3"
        self.manifest = {
            "schema": 1,
            "id": "supported-example",
            "tier": "supported",
            "yocto_version": "5.0",
            "bitbake_version": "2.8",
            "repositories": [
                {"name": "poky", "revision": "b" * 40},
            ],
            "layers": [],
            "lint_quality": {"baseline": "lint-baselines/example.json"},
            "bitbake": {"semantic_probes": []},
        }
        self.manifest_path = self.root / "manifest.json"
        self.manifest_path.write_text(json.dumps(self.manifest), encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def _write_bundle(self, name="bundle"):
        bundle = self.evidence_root / name
        (bundle / "metrics").mkdir(parents=True)
        (bundle / "lint").mkdir()
        (bundle / "logs").mkdir()
        (bundle / "manifest.json").write_text(
            json.dumps(self.manifest), encoding="utf-8"
        )
        findings = {
            "schema": 1,
            "fingerprint_version": 1,
            "corpus_id": self.manifest["id"],
            "findings": [],
        }
        derived = summarize_findings([], KNOWN_RULE_IDS)
        lint_summary = dict(derived)
        lint_summary["corpus_id"] = self.manifest["id"]
        baseline_rules = {
            rule_id: {
                field: rule[field]
                for field in ("count", "files", "findings_sha256", "severity_counts")
            }
            for rule_id, rule in lint_summary["rules"].items()
        }
        baseline = {
            "measurement": {
                field: lint_summary[field]
                for field in (
                    "total_findings",
                    "findings_sha256",
                    "files_with_findings",
                    "severity_counts",
                )
            }
        }
        baseline["measurement"]["rules"] = baseline_rules
        (bundle / "lint" / "findings.json").write_text(json.dumps(findings), encoding="utf-8")
        (bundle / "lint" / "summary.json").write_text(json.dumps(lint_summary), encoding="utf-8")
        (bundle / "lint" / "baseline-comparison.json").write_text(
            json.dumps({"status": "matched", "blocking_failures": [], "review_failures": []}),
            encoding="utf-8",
        )
        metrics = {
            "version": 1,
            "files": 1,
            "structured_nodes": 1,
            "total_nodes": 1,
            "trivia_nodes": 0,
            "unknown_bytes": 0,
            "unknown_nodes": 0,
        }
        for label in ("source", "formatted"):
            (bundle / "metrics" / (label + ".json")).write_text(
                json.dumps(metrics), encoding="utf-8"
            )
        (bundle / "logs" / "original-parse.log").write_text("parse\n", encoding="utf-8")
        (bundle / "logs" / "formatted-parse.log").write_text("parse\n", encoding="utf-8")
        (bundle / "logs" / "check.log").write_text("check\n", encoding="utf-8")
        (bundle / "commands.json").write_text(
            json.dumps([{"exit_code": 0, "log": "logs/check.log"}]), encoding="utf-8"
        )
        summary = {
            "schema": 1,
            "status": "passed",
            "corpus": {
                "id": self.manifest["id"],
                "tier": "supported",
                "yocto_version": "5.0",
                "bitbake_version": "2.8",
            },
            "bbtidy": {"version": "bbtidy " + self.version, "source_revision": self.source_commit},
            "repositories": {
                "poky": {
                    "expected_revision": "b" * 40,
                    "resolved_revision": "b" * 40,
                }
            },
            "results": {
                "metadata_files": 1,
                "files_changed_on_first_format": 0,
                "opaque_regions_preserved": 0,
                "excluded_payload_files_unchanged": 0,
                "source_metrics": metrics,
                "formatted_metrics": metrics,
                "bitbake_differential_parse": "passed",
                "lint_quality": {
                    "baseline_comparison": {
                        "status": "matched",
                        "blocking_failures": [],
                    }
                },
            },
        }
        (bundle / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
        return bundle, baseline

    def _validate(self, bundle, baseline, source_commit=None, version=None):
        with patch.object(evidence, "load_manifest", return_value=self.manifest), patch.object(
            evidence, "load_lint_baseline", return_value=baseline
        ):
            return evidence.validate_evidence_bundle(
                bundle,
                self.manifest_path,
                source_commit or self.source_commit,
                version or self.version,
            )

    def test_missing_corpus_evidence_fails(self):
        with self.assertRaisesRegex(evidence.EvidenceError, "missing corpus"):
            evidence.verify_release_evidence(
                self.evidence_root, [self.manifest_path], self.source_commit, self.version
            )

    def test_wrong_source_commit_and_version_fail(self):
        bundle, baseline = self._write_bundle()
        with self.assertRaisesRegex(evidence.EvidenceError, "source commit"):
            self._validate(bundle, baseline, source_commit="c" * 40)
        with self.assertRaisesRegex(evidence.EvidenceError, "version"):
            self._validate(bundle, baseline, version="9.9.9")

    def test_missing_parse_logs_fails(self):
        bundle, baseline = self._write_bundle()
        (bundle / "logs" / "formatted-parse.log").unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "parse.log"):
            self._validate(bundle, baseline)

    def test_baseline_fingerprint_mismatch_fails(self):
        bundle, baseline = self._write_bundle()
        baseline["measurement"]["findings_sha256"] = "d" * 64
        with self.assertRaisesRegex(evidence.EvidenceError, "fingerprint"):
            self._validate(bundle, baseline)

    def test_duplicate_corpus_evidence_fails(self):
        first, baseline = self._write_bundle("first")
        second, _ = self._write_bundle("second")
        # Keep the second fixture complete; duplicate detection happens first.
        with self.assertRaisesRegex(evidence.EvidenceError, "duplicate evidence"):
            evidence.verify_release_evidence(
                self.evidence_root, [self.manifest_path], self.source_commit, self.version
            )

    def test_failed_summary_and_unsafe_archive_members_fail(self):
        bundle, baseline = self._write_bundle()
        summary_path = bundle / "summary.json"
        summary = json.loads(summary_path.read_text(encoding="utf-8"))
        summary["status"] = "failed"
        summary_path.write_text(json.dumps(summary), encoding="utf-8")
        with self.assertRaisesRegex(evidence.EvidenceError, "not passed"):
            self._validate(bundle, baseline)
        for name in ("../escape", "/absolute", "a\\b"):
            with self.subTest(name=name), self.assertRaises(evidence.EvidenceError):
                evidence.validate_archive_members([name])

    def test_successful_evidence_is_archived_with_checksum(self):
        bundle, baseline = self._write_bundle()
        archive = self.root / "release-evidence.tar.gz"
        checksums = self.root / "release-evidence.sha256"
        with patch.object(evidence, "load_manifest", return_value=self.manifest), patch.object(
            evidence, "load_lint_baseline", return_value=baseline
        ):
            result = evidence.verify_release_evidence(
                self.evidence_root, [self.manifest_path], self.source_commit, self.version, archive, checksums
            )
        self.assertEqual(result["status"], "passed")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertIn(digest, checksums.read_text(encoding="utf-8"))
        with tarfile.open(archive, "r:gz") as tar:
            names = tar.getnames()
        self.assertIn("evidence-index.json", names)
        self.assertTrue(all(not name.startswith("/") and ".." not in name.split("/") for name in names))


if __name__ == "__main__":
    unittest.main()
