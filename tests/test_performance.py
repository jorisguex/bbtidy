import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

from scripts.benchmark_performance import measure_cli, run_command
from scripts.check_performance_budget import (
    BudgetError,
    compare_candidate_to_baseline,
    compare_record,
    load_budgets,
    update_budget,
)
from scripts.prepare_performance_evidence import consolidate
from scripts.review_performance_campaign import review as review_campaign
from scripts.performance_schema import (
    PerformanceSchemaError,
    aggregate_results,
    load_evidence,
    validate_record,
)
from scripts.performance_baselines import BaselineError, canonical_baseline_bytes, reference_key
from scripts.promote_performance_baselines import _validate_reference_runner


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
    def test_wall_uses_median_and_rss_uses_p90(self):
        samples = []
        for wall_ms, rss in zip(range(1, 11), range(10, 20)):
            measured = result(wall_ms)
            measured["peak_rss_bytes"] = rss
            samples.append({"result": measured})
        aggregate = aggregate_results(samples)
        self.assertEqual(aggregate["wall_ms"], 5.5)
        self.assertEqual(aggregate["peak_rss_bytes"], 18)
        self.assertEqual(aggregate["statistics"]["peak_rss_bytes"], "p90")

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
                        "min_absolute_regression": 50,
                        "blocking": True,
                    }
                }
            },
        }
        self.assertEqual(compare_record(record(wall_ms=115), budget)["status"], "matched")
        self.assertEqual(compare_record(record(wall_ms=114), budget)["status"], "matched")
        self.assertEqual(compare_record(record(wall_ms=120), budget)["status"], "matched")
        comparison = compare_record(record(wall_ms=160), budget)
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
        if sys.platform == "linux":
            self.assertGreater(measured["peak_rss_bytes"], 0)

    @unittest.skipUnless(sys.platform == "linux", "the process-tree RSS contract is Linux-specific")
    def test_sequential_rss_samples_do_not_inherit_a_previous_peak(self):
        large = run_command(
            [
                sys.executable,
                "-c",
                "import time; value=bytearray(96 * 1024 * 1024); value[0]=1; time.sleep(.2)",
            ]
        )
        small = run_command(
            [sys.executable, "-c", "import time; time.sleep(.2)"]
        )
        self.assertGreater(large["peak_rss_bytes"], small["peak_rss_bytes"] * 2)

    def test_format_write_restores_the_fixture_for_every_sample(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "layer"
            root.mkdir()
            fixture = root / "recipe.bb"
            original = 'SUMMARY = "fixture"\n'
            fixture.write_text(original, encoding="utf-8")
            fake = Path(directory) / "fake-bbtidy.py"
            fake.write_text(
                "#!/usr/bin/env python3\n"
                "import json, pathlib, sys\n"
                "args = sys.argv[1:]\n"
                "root = pathlib.Path(args[-1])\n"
                "if 'syntax-stats' in args:\n"
                "    print(json.dumps({'files': 1, 'total_nodes': 1, 'structured_nodes': 1, 'trivia_nodes': 0, 'unknown_nodes': 0, 'unknown_bytes': 0}))\n"
                "elif '--write' in args:\n"
                "    path = next(root.rglob('*.bb'))\n"
                "    path.write_text(path.read_text() + '# changed\\n')\n",
                encoding="utf-8",
            )
            fake.chmod(0o755)
            measured = measure_cli(fake, root, "format-write", "offline", 3)
            self.assertEqual(len(measured["samples"]), 3)
            self.assertEqual(fixture.read_text(encoding="utf-8"), original)

    def test_strict_baseline_comparison_rejects_a_missing_or_stale_corpus(self):
        candidate = record(workload="synthetic.recipe-1k.json")
        budget = {
            "schema": 1,
            "runner_class": "test-runner",
            "policy": {"relative_and_absolute_required": True},
            "workloads": {
                "synthetic.recipe-1k.json": {
                    "wall_ms": {
                        "max_ratio": 1.15,
                        "min_absolute_regression": 10,
                        "statistic": "median",
                        "blocking": True,
                    }
                }
            },
        }
        baselines = {
            "schema": 1,
            "kind": "bbtidy-performance-baselines",
            "runner": {"class": "test-runner"},
            "required_workloads": [],
            "references": {},
        }
        with self.assertRaises(BudgetError):
            compare_record(candidate, budget, baselines, strict_baseline=True)
        baselines["references"][reference_key(
            "test-runner", "synthetic.recipe-1k.json", "offline", "b" * 64, "wall_ms", "median"
        )] = {
            "workload": "synthetic.recipe-1k.json",
            "mode": "offline",
            "corpus_id": "test",
            "corpus_digest": "b" * 64,
            "metric": "wall_ms",
            "statistic": "median",
            "value": 100,
            "source_commit": "a" * 40,
        }
        with self.assertRaises(BudgetError):
            compare_record(candidate, budget, baselines, strict_baseline=True)

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

    def test_release_consolidation_copies_and_uses_a_corpus_bound_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record_path = root / "record.json"
            record_path.write_text(json.dumps(record()), encoding="utf-8")
            budget_path = root / "budgets.json"
            budget = {
                "schema": 1,
                "runner_class": "test-runner",
                "policy": {"relative_and_absolute_required": True},
                "workloads": {
                    "synthetic-scaling": {
                        "wall_ms": {
                            "max_ratio": 1.15,
                            "min_absolute_regression": 50,
                            "statistic": "median",
                            "blocking": True,
                        }
                    }
                },
            }
            budget_path.write_text(json.dumps(budget), encoding="utf-8")
            baseline = {
                "schema": 1,
                "kind": "bbtidy-performance-baselines",
                "runner": {"class": "test-runner"},
                "required_workloads": ["synthetic-scaling"],
                "references": {},
            }
            key = reference_key(
                "test-runner", "synthetic-scaling", "offline", "a" * 64, "wall_ms", "median"
            )
            baseline["references"][key] = {
                "workload": "synthetic-scaling",
                "mode": "offline",
                "corpus_id": "test",
                "corpus_digest": "a" * 64,
                "metric": "wall_ms",
                "statistic": "median",
                "value": 100,
                "source_commit": "a" * 40,
            }
            baseline_path = root / "baselines.json"
            baseline_path.write_bytes(canonical_baseline_bytes(baseline))
            output = root / "release-performance"
            consolidate(
                output,
                budget_path,
                [record_path],
                "a" * 40,
                "test",
                "test-runner",
                baseline_path,
            )
            self.assertTrue((output / "baselines.json").is_file())
            self.assertEqual(json.loads((output / "summary.json").read_text())["status"], "passed")

    def test_campaign_review_requires_three_runs_and_reports_variance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = []
            for index in range(1, 4):
                path = root / f"run-{index}.json"
                path.write_text(json.dumps(record()), encoding="utf-8")
                evidence.append(path)
            budget = {
                "schema": 1,
                "runner_class": "test-runner",
                "policy": {"relative_and_absolute_required": True},
                "workloads": {
                    "synthetic-scaling": {
                        "wall_ms": {"max_ratio": 1.15, "min_absolute_regression": 10, "statistic": "median", "blocking": True},
                        "peak_rss_bytes": {"max_ratio": 1.15, "min_absolute_regression": 10, "statistic": "p90", "blocking": True},
                    }
                },
            }
            budget_path = root / "budgets.json"
            budget_path.write_text(json.dumps(budget), encoding="utf-8")
            report = review_campaign(budget_path, evidence)
            self.assertEqual(report["run_count"], 3)
            self.assertEqual(report["workload_count"], 1)
            self.assertIn("coefficient_of_variation", next(iter(report["reports"].values()))["metrics"]["wall_ms"])

    def test_campaign_review_rejects_missing_required_workloads(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = []
            for index in range(1, 4):
                path = root / f"run-{index}.json"
                path.write_text(json.dumps(record()), encoding="utf-8")
                evidence.append(path)
            budget_path = root / "budgets.json"
            budget_path.write_text(
                json.dumps(
                    {
                        "schema": 1,
                        "runner_class": "test-runner",
                        "policy": {"relative_and_absolute_required": True},
                        "workloads": {
                            "synthetic-scaling": {"wall_ms": {"max_ratio": 1.15, "min_absolute_regression": 10}},
                            "missing-workload": {"wall_ms": {"max_ratio": 1.15, "min_absolute_regression": 10}},
                        },
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaises(ValueError):
                review_campaign(budget_path, evidence)

    def test_ubuntu_reference_promotion_rejects_untruthful_runner_metadata(self):
        candidate = record(workload="synthetic.recipe-1k.json")
        candidate["runner"] = {"class": "github-ubuntu-24.04-x86_64"}
        with self.assertRaises(BaselineError):
            _validate_reference_runner(candidate, Path("evidence.json"))


if __name__ == "__main__":
    unittest.main()
