import json
import copy
import hashlib
import os
import sys
import tempfile
import unittest
from pathlib import Path

from scripts.benchmark_performance import measure_cli, run_command, runner_metadata
from scripts.check_performance_budget import (
    BudgetError,
    compare_candidate_to_baseline,
    compare_record,
    load_budgets,
    update_budget,
)
from scripts.prepare_performance_evidence import consolidate
from scripts.populate_performance_baselines import populate
from scripts.benchmark_performance import synthetic_cases
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
    def test_rss_does_not_inherit_an_earlier_child_peak(self):
        high = run_command([sys.executable, "-c", "data = bytearray(96 * 1024 * 1024)"])
        low = run_command([sys.executable, "-c", "pass"])
        self.assertEqual(high["status"], "success")
        self.assertEqual(low["status"], "success")
        self.assertGreater(high["peak_rss_bytes"] - low["peak_rss_bytes"], 64 * 1024 * 1024)

    def test_resource_capture_drains_large_output_and_records_compiler(self):
        measured = run_command([sys.executable, "-c", "import sys; sys.stdout.write('x' * 2000000); sys.stderr.write('y' * 1000000)"])
        self.assertEqual(measured["status"], "success")
        self.assertEqual(measured["stdout"], b"x" * 2000000)
        self.assertEqual(measured["stderr"], b"y" * 1000000)
        metadata = runner_metadata()
        self.assertTrue(metadata["rust"].startswith("rustc "))
        self.assertEqual(metadata["measurement_contract"], 2)

    def test_shell_fixture_is_one_mib_and_retains_its_closing_brace(self):
        source = dict(synthetic_cases())["shell-body-1m"]
        self.assertEqual(len(source.encode()), 1024 * 1024)
        self.assertTrue(source.startswith("do_compile() {\n"))
        self.assertTrue(source.endswith("}\n"))

    def test_format_repetitions_restore_input_and_verify_every_write(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            layer = root / "layer"
            layer.mkdir()
            source = layer / "example.bb"
            log = root / "writes.log"
            fake = root / "fake-bbtidy"
            fake.write_text(
                "#!/usr/bin/env python3\n"
                "import pathlib, sys\n"
                f"source = pathlib.Path({str(source)!r})\n"
                f"log = pathlib.Path({str(log)!r})\n"
                "if 'syntax-stats' in sys.argv:\n"
                "    print('{\"files\": 1}')\n"
                "elif '--write' in sys.argv:\n"
                "    with log.open('ab') as stream: stream.write(source.read_bytes())\n"
                "    source.write_bytes(b'A = \\\"a\\\"\\n')\n"
                "else:\n"
                "    sys.stdout.write('A = \\\"a\\\"\\n')\n"
            )
            fake.chmod(0o755)
            original = b'A="a"\n'
            source.write_bytes(original)
            measured = measure_cli(fake, layer, "format", "offline", 3)
            self.assertEqual(log.read_bytes(), original * 3)
            self.assertEqual(source.read_bytes(), b'A = "a"\n')
            self.assertEqual(len(measured["samples"]), 3)
            for sample in measured["samples"]:
                self.assertEqual(sample["result"]["status"], "success")
                self.assertEqual(sample["bbtidy"]["files_changed"], 1)
                self.assertEqual(sample["bbtidy"]["source_bytes"], len(original))
            with self.assertRaisesRegex(ValueError, "unformatted"):
                measure_cli(fake, layer, "format", "offline", 3)
            source.write_bytes(original)
            fake.write_text(fake.read_text().replace("source.write_bytes(b'A = \\\"a\\\"\\n')", "pass"))
            broken = measure_cli(fake, layer, "format", "offline", 3)
            self.assertEqual(broken["samples"][0]["result"]["status"], "failed")
            self.assertEqual(len(broken["samples"]), 1)

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

    def test_semantic_target_is_passed_to_bbtidy(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "build"
            (root / "conf").mkdir(parents=True)
            (root / "conf" / "local.conf").write_text(
                'MACHINE = "qemux86-64"\n', encoding="utf-8"
            )
            (root / "conf" / "bblayers.conf").write_text(
                'BBLAYERS = "/layer"\n', encoding="utf-8"
            )
            arguments = Path(directory) / "arguments.json"
            fake = Path(directory) / "fake-bbtidy.py"
            fake.write_text(
                "#!/usr/bin/env python3\n"
                "import json, os, pathlib, sys\n"
                "pathlib.Path(os.environ['ARGS_LOG']).write_text(json.dumps(sys.argv[1:]))\n"
                "if 'syntax-stats' in sys.argv:\n"
                "    print(json.dumps({'files': 2, 'total_nodes': 2, 'structured_nodes': 2, 'trivia_nodes': 0, 'unknown_nodes': 0, 'unknown_bytes': 0}))\n"
                "else:\n"
                "    print(json.dumps({'parse_succeeded': True}))\n",
                encoding="utf-8",
            )
            fake.chmod(0o755)
            os.environ["ARGS_LOG"] = str(arguments)
            try:
                measured = measure_cli(
                    fake,
                    root,
                    "semantic",
                    "cold",
                    1,
                    timeout_seconds=2,
                    bitbake_command=fake,
                    bitbake_target="core-image-minimal",
                )
            finally:
                os.environ.pop("ARGS_LOG", None)
            self.assertEqual(measured["samples"][0]["result"]["status"], "success")
            command = json.loads(arguments.read_text(encoding="utf-8"))
            self.assertIn("--target", command)
            self.assertEqual(command[command.index("--target") + 1], "core-image-minimal")

    def test_checked_in_budgets_have_structural_limits(self):
        budgets = load_budgets(Path("tests/performance/budgets.json"))
        self.assertTrue(budgets["policy"]["relative_and_absolute_required"])
        for workload in budgets["workloads"].values():
            for metric, rule in workload.items():
                if metric in {"notes", "structural", "reference"} or not isinstance(rule, dict):
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

    def test_checked_in_references_reproduce_complete_blocking_budgets(self):
        budget_path = Path("tests/performance/budgets.json")
        budget = load_budgets(budget_path)
        expected = {
            f"{name}-{operation}"
            for name, _ in synthetic_cases()
            for operation in ("format-check", "format", "json", "sarif")
        } | {"yocto-community-offline"} | {
            f"yocto-{version}-{operation}"
            for version in ("5.0", "6.0")
            for operation in ("offline", "bitbake-cold", "bitbake-warm", "semantic-full")
        }
        self.assertEqual(set(budget["policy"]["required_baselines"]), expected)
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "budgets.json"
            target.write_bytes(budget_path.read_bytes())
            from unittest.mock import patch
            with patch.dict(os.environ, {"BBTIDY_ALLOW_PERFORMANCE_UPDATE": "1"}):
                changes = populate(
                    target, Path("tests/performance/references/manifest.json"),
                    budget["workloads"]["recipe-1k-json"]["reference"]["reason"],
                )
            self.assertEqual(set(changes), expected)
            self.assertEqual(json.loads(target.read_text()), budget)

    def test_real_references_match_and_regressions_fail(self):
        budget = load_budgets(Path("tests/performance/budgets.json"))
        for path in Path("tests/performance/references").rglob("*.json"):
            if path.name == "manifest.json":
                continue
            evidence = load_evidence(path)
            for reference in evidence.get("records", [evidence]):
                with self.subTest(path=path, workload=reference["workload"]):
                    self.assertEqual(compare_record(reference, budget)["status"], "matched")
                    for metric in ("wall_ms", "peak_rss_bytes"):
                        changed = copy.deepcopy(reference)
                        rule = budget["workloads"][reference["workload"]][metric]
                        changed["summary"][metric] = max(
                            rule["baseline"] * rule["max_ratio"],
                            rule["baseline"] + rule["min_absolute_regression"],
                        ) + 1
                        self.assertEqual(compare_record(changed, budget)["status"], "failed")

    def test_populated_baseline_rejects_changed_identity_and_failed_samples(self):
        budget = load_budgets(Path("tests/performance/budgets.json"))
        reference = load_evidence(Path("tests/performance/references/33971299726/performance-json.json"))["records"][0]
        for field in ("mode", "corpus"):
            changed = copy.deepcopy(reference)
            changed[field] = "cold" if field == "mode" else {**changed[field], "revision_digest": "b" * 64}
            with self.assertRaises(BudgetError):
                compare_record(changed, budget)
        reference["samples"][0]["result"]["status"] = "failed"
        self.assertEqual(compare_record(reference, budget)["status"], "failed")

    def test_populated_bitbake_budget_retains_common_structural_checks(self):
        budget = load_budgets(Path("tests/performance/budgets.json"))
        reference = load_evidence(Path("tests/performance/references/33971300100/yocto-5.0-bitbake-warm.json"))
        reference["summary"]["bitbake"]["commands_failed"] = 1
        comparison = compare_record(reference, budget)
        self.assertTrue(any("commands_failed" in failure for failure in comparison["failures"]))

    def test_required_baseline_cannot_be_disabled_or_emptied(self):
        original = load_budgets(Path("tests/performance/budgets.json"))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "budgets.json"
            for change in ({"baseline": None}, {"baseline": float("nan")}, {"blocking": False}):
                budget = copy.deepcopy(original)
                budget["workloads"]["recipe-1k-json"]["wall_ms"].update(change)
                path.write_text(json.dumps(budget))
                with self.assertRaises(BudgetError):
                    load_budgets(path)

    def test_reference_update_rejects_untrusted_measurements_without_writing(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"BBTIDY_ALLOW_PERFORMANCE_UPDATE": "1"}):
            root = Path(directory)
            budget = {
                "schema": 1, "runner_class": "test-runner",
                "policy": {"relative_and_absolute_required": True},
                "workloads": {"synthetic-scaling": {
                    metric: {"baseline": None, "max_ratio": 1.15, "min_absolute_regression": 50}
                    for metric in ("wall_ms", "peak_rss_bytes")
                }},
            }
            budget_path = root / "budget.json"
            budget_path.write_text(json.dumps(budget))
            before = budget_path.read_bytes()
            original = record()
            original["runner"].update(os="Linux-test", architecture="x86_64", source_commit=original["commit"])
            for failure in ("checksum", "failed", "summary", "runner", "source", "corpus", "single-run"):
                entries = []
                for index in range(2):
                    sample = copy.deepcopy(original)
                    if index == 1:
                        if failure == "failed":
                            sample["samples"][0]["result"]["status"] = "failed"
                        elif failure == "summary":
                            sample["summary"]["wall_ms"] = 1
                        elif failure == "runner":
                            sample["runner"]["os"] = "Darwin-test"
                        elif failure == "source":
                            sample["commit"] = "b" * 40
                        elif failure == "corpus":
                            sample["corpus"]["revision_digest"] = "b" * 64
                    path = root / f"sample-{index}.json"
                    path.write_text(json.dumps(sample))
                    entries.append({
                        "path": path.name,
                        "sha256": "bad" if failure == "checksum" else hashlib.sha256(path.read_bytes()).hexdigest(),
                        "source_commit": original["commit"],
                        "run_url": f"https://github.com/jorisguex/bbtidy/actions/runs/{0 if failure == 'single-run' else index}",
                    })
                manifest = root / "manifest.json"
                manifest.write_text(json.dumps({"schema": 1, "evidence": entries}))
                with self.subTest(failure=failure), self.assertRaises(BudgetError):
                    populate(budget_path, manifest, "test invalid reference")
                self.assertEqual(budget_path.read_bytes(), before)

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
