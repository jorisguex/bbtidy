import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

from scripts.benchmark_performance import run_command
from scripts.check_performance_budget import (
    BudgetError,
    compare_candidate_to_baseline,
    compare_record,
    load_budgets,
    update_budget,
)
from scripts.prepare_performance_evidence import consolidate
from scripts.performance_schema import (
    PerformanceSchemaError,
    aggregate_results,
    load_evidence,
    validate_record,
)


def result(wall_ms=100, status="success"):
    return {
        "status": status,
        "wall_ms": wall_ms,
        "user_cpu_ms": 10,
        "system_cpu_ms": 2,
        "peak_rss_bytes": 1024,
        "read_bytes": 3,
        "written_bytes": 4,
    }


def record(workload="synthetic-scaling", wall_ms=100, runner_class="test-runner"):
    samples = [{"result": result(wall_ms), "bbtidy": {"files_discovered": 3}}]
    summary = aggregate_results(samples)
    summary["bbtidy"] = {"files_discovered": 3}
    return {
        "schema": 1,
        "kind": "bbtidy-performance",
        "workload": workload,
        "mode": "offline",
        "commit": "a" * 40,
        "version": "test",
        "runner": {"class": runner_class},
        "corpus": {"id": "test", "revision_digest": "a" * 64},
        "samples": samples,
        "summary": summary,
    }


class PerformanceTests(unittest.TestCase):
    def test_schema_aggregates_repetitions_and_rejects_bad_status(self):
        aggregate = aggregate_results([{"result": result(100)}, {"result": result(200)}])
        self.assertEqual(aggregate["wall_ms"], 150)
        self.assertEqual(aggregate["sample_ranges"]["wall_ms"]["max"], 200)
        checked = validate_record(record())
        self.assertEqual(checked["sample_count"], 1)
        with self.assertRaises(PerformanceSchemaError):
            validate_record({**record(), "samples": [{"result": result(status="bogus")}], "summary": aggregate})

    def test_budget_requires_both_relative_and_absolute_regressions(self):
        budget = {
            "schema": 1,
            "runner_class": "test-runner",
            "policy": {"relative_and_absolute_required": True},
            "workloads": {
                "synthetic-scaling": {
                    "wall_ms": {
                        "baseline": 100,
                        "max_ratio": 1.15,
                        "min_absolute_regression": 10,
                        "blocking": True,
                    }
                }
            },
        }
        self.assertEqual(compare_record(record(wall_ms=115), budget)["status"], "matched")
        comparison = compare_record(record(wall_ms=130), budget)
        self.assertEqual(comparison["status"], "failed")
        self.assertEqual(len(comparison["failures"]), 1)
        with self.assertRaises(BudgetError):
            compare_record(record(runner_class="other"), budget)

    def test_structural_budget_is_blocking(self):
        budget = {
            "schema": 1,
            "runner_class": "test-runner",
            "policy": {"relative_and_absolute_required": True},
            "workloads": {
                "synthetic-scaling": {
                    "structural": {"bbtidy.files_discovered": {"max": 2}}
                }
            },
        }
        comparison = compare_record(record(), budget)
        self.assertEqual(comparison["status"], "failed")

    def test_candidate_comparison_requires_same_corpus_and_runner(self):
        budget = {
            "schema": 1,
            "runner_class": "test-runner",
            "policy": {"relative_and_absolute_required": True},
            "workloads": {
                "synthetic-scaling": {
                    "wall_ms": {
                        "baseline": None,
                        "max_ratio": 1.15,
                        "min_absolute_regression": 10,
                        "blocking": True,
                    }
                }
            },
        }
        candidate = record(wall_ms=110)
        baseline = record(wall_ms=100)
        self.assertEqual(
            compare_candidate_to_baseline(candidate, baseline, budget)["status"],
            "matched",
        )
        different = record()
        different["corpus"]["revision_digest"] = "b" * 64
        with self.assertRaises(BudgetError):
            compare_candidate_to_baseline(candidate, different, budget)

    def test_budget_update_requires_reason_and_preserves_other_workloads(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "budgets.json"
            value = {
                "schema": 1,
                "runner_class": "test-runner",
                "policy": {"relative_and_absolute_required": True},
                "workloads": {
                    "synthetic-scaling": {"wall_ms": {"baseline": None}},
                    "unrelated": {
                        "wall_ms": {
                            "baseline": 7,
                            "max_ratio": 1.15,
                            "min_absolute_regression": 0,
                            "blocking": False,
                        }
                    },
                },
            }
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaises(BudgetError):
                update_budget(path, record(), "")
            os.environ["CI"] = "true"
            try:
                with self.assertRaises(BudgetError):
                    update_budget(path, record(), "reference sample")
            finally:
                os.environ.pop("CI", None)
            update_budget(path, record(), "reference sample")
            updated = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(updated["workloads"]["unrelated"]["wall_ms"]["baseline"], 7)
            self.assertEqual(updated["workloads"]["synthetic-scaling"]["wall_ms"]["baseline"], 100)
            self.assertEqual(updated["history"][0]["reason"], "reference sample")

    def test_process_wrapper_classifies_timeout_and_captures_output(self):
        measured = run_command(
            [sys.executable, "-c", "import sys, time; print('ok'); sys.stderr.write('err'); time.sleep(1)"],
            timeout_seconds=0.02,
        )
        self.assertEqual(measured["status"], "timed-out")
        self.assertGreaterEqual(measured["stdout_bytes"], 0)
        self.assertGreaterEqual(measured["stderr_bytes"], 0)
        self.assertGreaterEqual(measured["peak_rss_bytes"], 0)

    def test_checked_in_budgets_have_structural_limits(self):
        budgets = load_budgets(Path("tests/performance/budgets.json"))
        self.assertTrue(budgets["policy"]["relative_and_absolute_required"])
        for workload in budgets["workloads"].values():
            for metric, rule in workload.items():
                if metric in {"notes", "structural"} or not isinstance(rule, dict):
                    continue
                if "max_ratio" not in rule:
                    self.assertTrue(
                        "max" in rule
                        or "baseline" in rule
                        or "allowed" in rule
                        or "equals" in rule
                    )

    def test_suite_evidence_is_validated(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "suite.json"
            path.write_text(
                json.dumps(
                    {
                        "schema": 1,
                        "kind": "bbtidy-performance-suite",
                        "runner": {"class": "test-runner"},
                        "records": [record()],
                    }
                ),
                encoding="utf-8",
            )
            evidence = load_evidence(path)
            self.assertEqual(len(evidence["records"]), 1)

    def test_release_performance_evidence_is_consolidated(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record_path = root / "record.json"
            record_path.write_text(json.dumps(record()), encoding="utf-8")
            budget_path = root / "budgets.json"
            budget_path.write_text(
                json.dumps(
                    {
                        "schema": 1,
                        "runner_class": "test-runner",
                        "policy": {"relative_and_absolute_required": True},
                        "workloads": {
                            "synthetic-scaling": {
                                "wall_ms": {
                                    "baseline": None,
                                    "max_ratio": 1.15,
                                    "min_absolute_regression": 50,
                                    "blocking": False,
                                },
                                "structural": {
                                    "bbtidy.files_discovered": {"max": 10}
                                },
                            }
                        },
                    }
                ),
                encoding="utf-8",
            )
            output = root / "release-performance"
            consolidate(output, budget_path, [record_path], "a" * 40, "test", "test-runner")
            self.assertEqual(json.loads((output / "summary.json").read_text())["status"], "passed")


if __name__ == "__main__":
    unittest.main()
