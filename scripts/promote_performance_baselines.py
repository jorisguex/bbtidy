#!/usr/bin/env python3
"""Promote a reviewed, complete performance campaign into a baseline manifest."""

from __future__ import annotations

import argparse
import difflib
import json
import math
import statistics
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Mapping

try:
    from scripts.check_performance_budget import BudgetError, load_budgets
    from scripts.performance_baselines import (
        BaselineError,
        KIND,
        SCHEMA,
        canonical_baseline_bytes,
        load_baselines,
        reference_key,
        required_workloads_from_budgets,
        statistic_for,
    )
    from scripts.performance_schema import PerformanceSchemaError, load_evidence
except ModuleNotFoundError:  # direct script execution
    from check_performance_budget import BudgetError, load_budgets  # type: ignore
    from performance_baselines import (  # type: ignore
        BaselineError,
        KIND,
        SCHEMA,
        canonical_baseline_bytes,
        load_baselines,
        reference_key,
        required_workloads_from_budgets,
        statistic_for,
    )
    from performance_schema import PerformanceSchemaError, load_evidence  # type: ignore


METRICS = ("wall_ms", "peak_rss_bytes")
REFERENCE_RUNNER_CONTRACTS = {
    "github-ubuntu-24.04-x86_64": ("ubuntu24", "x86_64"),
    "github-ubuntu-22.04-x86_64": ("ubuntu22", "x86_64"),
}


def _records(path: Path) -> list[dict]:
    evidence = load_evidence(path)
    return evidence["records"] if evidence.get("kind") == "bbtidy-performance-suite" else [evidence]


def _p90(values: Iterable[float]) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    index = min(len(ordered) - 1, max(0, math.ceil(len(ordered) * 0.9) - 1))
    return ordered[index]


def _group_key(record: Mapping[str, Any]) -> tuple[str, str, str, str]:
    return (
        record["workload"],
        record["mode"],
        record["runner"]["class"],
        record["corpus"]["revision_digest"],
    )


def _all_sample_values(record: Mapping[str, Any], metric: str) -> list[float]:
    return [float(sample["result"][metric]) for sample in record["samples"]]


def _validate_sample_contract(record: Mapping[str, Any], source: Path) -> None:
    workload = record["workload"]
    count = record["sample_count"]
    minimum = 1
    if workload.startswith("synthetic."):
        minimum = 7
        if sum(float(sample["result"]["wall_ms"]) for sample in record["samples"]) < 1000:
            raise BaselineError(f"reference run {source} does not accumulate one second of synthetic samples")
    elif workload.endswith("-offline"):
        minimum = 5
    elif workload.endswith("-bitbake-warm"):
        minimum = 5
    if count < minimum:
        raise BaselineError(
            f"reference run {source} has {count} samples for {workload}; at least {minimum} are required"
        )


def _validate_reference_runner(record: Mapping[str, Any], source: Path) -> None:
    runner = record.get("runner")
    if not isinstance(runner, Mapping):
        raise BaselineError(f"reference run {source} has no runner metadata")
    contract = REFERENCE_RUNNER_CONTRACTS.get(runner.get("class"))
    if contract is None:
        return
    image_prefix, architecture = contract
    image_os = runner.get("image_os")
    image_version = runner.get("image_version")
    if not isinstance(image_os, str) or image_prefix not in image_os.lower():
        raise BaselineError(
            f"reference run {source} does not identify the expected hosted image: {image_os!r}"
        )
    if not isinstance(image_version, str) or not image_version.strip():
        raise BaselineError(f"reference run {source} is missing ImageVersion")
    if not isinstance(runner.get("os"), str) or "linux" not in runner["os"].lower():
        raise BaselineError(f"reference run {source} is not a Linux measurement")
    if runner.get("architecture") != architecture:
        raise BaselineError(
            f"reference run {source} has architecture {runner.get('architecture')!r}; expected {architecture}"
        )
    for field in ("cpu", "kernel", "rust"):
        if not isinstance(runner.get(field), str) or not runner[field].strip():
            raise BaselineError(f"reference run {source} is missing runner metadata: {field}")
    for field in ("logical_cores", "memory_bytes"):
        value = runner.get(field)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or value <= 0:
            raise BaselineError(f"reference run {source} has invalid runner metadata: {field}")


def _review_report(records_by_run: list[dict], budget: Mapping[str, Any]) -> dict:
    report = {}
    for key, records in sorted(records_by_run.items()):
        workload, mode, runner_class, corpus_digest = key
        rules = budget.get("workloads", {}).get(workload, {})
        metrics = {}
        for metric in METRICS:
            statistic = statistic_for(metric, rules.get(metric) if isinstance(rules, Mapping) else None)
            run_values = [float(record["summary"][metric]) for record in records]
            all_values = [value for record in records for value in _all_sample_values(record, metric)]
            mean = statistics.mean(all_values) if all_values else 0.0
            coefficient_of_variation = (
                statistics.pstdev(all_values) / mean if len(all_values) > 1 and mean else 0.0
            )
            rule = rules.get(metric, {}) if isinstance(rules, Mapping) else {}
            suggested = statistics.median(run_values)
            metrics[metric] = {
                "statistic": statistic,
                "run_values": run_values,
                "median_of_run_medians": suggested,
                "overall_p90": _p90(all_values),
                "min": min(all_values) if all_values else 0,
                "max": max(all_values) if all_values else 0,
                "coefficient_of_variation": coefficient_of_variation,
                "suggested_baseline": suggested,
                "policy": {
                    "max_ratio": rule.get("max_ratio"),
                    "min_absolute_regression": rule.get("min_absolute_regression"),
                    "absolute_allowance": (
                        suggested + rule["min_absolute_regression"]
                        if isinstance(rule, Mapping) and rule.get("min_absolute_regression") is not None
                        else None
                    ),
                },
            }
        report["|".join(key)] = {
            "workload": workload,
            "mode": mode,
            "runner_class": runner_class,
            "runner": dict(records[0]["runner"]),
            "corpus_digest": corpus_digest,
            "corpus_id": records[0]["corpus"]["id"],
            "file_count": records[0]["corpus"].get("files"),
            "metrics": metrics,
        }
    return report


def _new_manifest(first: Mapping[str, Any], required: list[str]) -> dict:
    return {
        "schema": SCHEMA,
        "kind": KIND,
        "runner": dict(first["runner"]),
        "required_workloads": required,
        "references": {},
        "history": [],
    }


def promote(
    budget_path: Path,
    baseline_path: Path,
    evidence_paths: list[Path],
    evidence_ids: list[str],
    reason: str,
    source_commit: str | None = None,
    workloads: list[str] | None = None,
    report_path: Path | None = None,
) -> dict:
    if not reason.strip():
        raise BaselineError("--reason must be non-empty")
    if len(evidence_paths) < 3:
        raise BaselineError("at least three compatible reference runs are required")
    if len(evidence_ids) != len(evidence_paths) or len(set(evidence_ids)) != len(evidence_ids):
        raise BaselineError("each reference run must have one unique evidence artifact identifier")
    budget = load_budgets(budget_path)
    grouped_records: dict[tuple[str, str, str, str], list[dict]] = {}
    for path in evidence_paths:
        records = _records(path)
        grouped: dict[tuple[str, str, str, str], dict] = {}
        for record in records:
            if record["summary"]["status"] != "success":
                raise BaselineError(f"reference evidence {path} contains an unsuccessful sample")
            _validate_sample_contract(record, path)
            _validate_reference_runner(record, path)
            key = _group_key(record)
            if key in grouped:
                raise BaselineError(f"reference evidence {path} contains duplicate workload {key[0]}")
            grouped[key] = record
            if source_commit is not None and record.get("commit") != source_commit:
                raise BaselineError(f"reference evidence {path} has the wrong source commit")
        for key, record in grouped.items():
            grouped_records.setdefault(key, []).append(record)
    if not grouped_records:
        raise BaselineError("reference evidence contains no records")
    incomplete = {
        key[0]: len(records)
        for key, records in grouped_records.items()
        if len(records) != 3
    }
    if incomplete:
        details = ", ".join(f"{workload}={count}" for workload, count in sorted(incomplete.items()))
        raise BaselineError("each workload needs exactly three compatible reference runs: " + details)
    common = set(grouped_records)
    if workloads:
        requested = set(workloads)
        selected = {key for key in common if key[0] in requested}
        if {key[0] for key in selected} != requested:
            missing = sorted(requested - {key[0] for key in selected})
            raise BaselineError("requested workload is absent from every reference run: " + ", ".join(missing))
    else:
        required = set(required_workloads_from_budgets(budget))
        missing = sorted(required - {key[0] for key in common})
        if missing:
            raise BaselineError(
                "complete promotion is missing required workloads: " + ", ".join(missing)
            )
        selected = common
    if not selected:
        raise BaselineError("no workloads selected for promotion")

    first_record = grouped_records[sorted(selected)[0]][0]
    source_commits = {
        record.get("commit")
        for records in grouped_records.values()
        for record in records
    }
    if len(source_commits) != 1 or None in source_commits or "unknown" in source_commits:
        raise BaselineError("reference runs must use one known source commit")
    resolved_commit = next(iter(source_commits))
    if source_commit is not None and resolved_commit != source_commit:
        raise BaselineError("reference runs disagree with --source-commit")
    runner_classes = {key[2] for key in selected}
    if len(runner_classes) != 1:
        raise BaselineError("reference runs use more than one runner class")

    if baseline_path.is_file():
        manifest = load_baselines(baseline_path)
    else:
        manifest = _new_manifest(first_record, required_workloads_from_budgets(budget))
    if manifest["runner"]["class"] != first_record["runner"]["class"]:
        raise BaselineError("baseline manifest uses a different runner class")
    # The runner image/version and host metadata belong to the campaign that
    # most recently established the references.  Refresh them from the
    # verified Ubuntu run so a locally-created or older manifest cannot keep
    # advertising stale host identity after promotion.
    manifest["runner"] = dict(first_record["runner"])

    before = canonical_baseline_bytes(manifest).decode("utf-8")
    changed = []
    report = _review_report(
        {key: grouped_records[key] for key in selected}, budget
    )
    for group_report in report.values():
        for metric_report in group_report["metrics"].values():
            run_values = metric_report["run_values"]
            minimum = min(run_values)
            maximum = max(run_values)
            if minimum > 0 and maximum / minimum > 1.2:
                raise BaselineError(
                    "independent reference medians differ by more than 20% for "
                    + group_report["workload"]
                )
    for key in sorted(selected):
        workload, mode, runner_class, corpus_digest = key
        records = grouped_records[key]
        corpus_ids = {record["corpus"]["id"] for record in records}
        if len(corpus_ids) != 1:
            raise BaselineError(f"reference runs disagree on corpus id for {workload}")
        rules = budget.get("workloads", {}).get(workload, {})
        for metric in METRICS:
            statistic = statistic_for(metric, rules.get(metric) if isinstance(rules, Mapping) else None)
            values = [float(record["summary"][metric]) for record in records]
            entry = {
                "workload": workload,
                "mode": mode,
                "corpus_id": records[0]["corpus"]["id"],
                "corpus_digest": corpus_digest,
                "corpus_files": records[0]["corpus"].get("files"),
                "corpus_source_bytes": records[0]["corpus"].get("source_bytes"),
                "metric": metric,
                "statistic": statistic,
                "value": statistics.median(values),
                "sample_count": sum(record["sample_count"] for record in records),
                "source_commit": resolved_commit,
                "evidence": list(evidence_ids),
            }
            entry_key = reference_key(runner_class, workload, mode, corpus_digest, metric, statistic)
            old = manifest["references"].get(entry_key)
            manifest["references"][entry_key] = entry
            changed.append({"key": entry_key, "before": old, "after": entry})

    manifest.setdefault("history", []).append(
        {
            "reason": reason,
            "source_commit": resolved_commit,
            "evidence": list(evidence_ids),
            "workloads": sorted({key[0] for key in selected}),
            "references": [item["key"] for item in changed],
            "timestamp": datetime.now(timezone.utc).isoformat(),
        }
    )
    manifest["required_workloads"] = sorted(
        set(manifest.get("required_workloads", [])) | set(required_workloads_from_budgets(budget))
    )
    after = canonical_baseline_bytes(manifest).decode("utf-8")
    baseline_path.parent.mkdir(parents=True, exist_ok=True)
    baseline_path.write_bytes(after.encode("utf-8"))
    if report_path is not None:
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(json.dumps({"schema": 1, "reports": report}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    diff = "".join(
        difflib.unified_diff(
            before.splitlines(keepends=True),
            after.splitlines(keepends=True),
            fromfile=str(baseline_path) + " (before)",
            tofile=str(baseline_path) + " (after)",
        )
    )
    result = {
        "baseline": str(baseline_path),
        "source_commit": resolved_commit,
        "runner_class": first_record["runner"]["class"],
        "workloads": sorted({key[0] for key in selected}),
        "changes": changed,
        "review_report": report,
        "diff": diff,
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--budgets", type=Path, required=True)
    parser.add_argument("--baselines", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, action="append", required=True)
    parser.add_argument("--evidence-id", action="append")
    parser.add_argument("--reason", required=True)
    parser.add_argument("--source-commit")
    parser.add_argument("--workload", action="append")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args(argv)
    evidence_ids = args.evidence_id or [path.name for path in args.evidence]
    if len(evidence_ids) != len(args.evidence):
        parser.error("--evidence-id must be supplied once per --evidence, or omitted entirely")
    try:
        promote(
            args.budgets,
            args.baselines,
            args.evidence,
            evidence_ids,
            args.reason,
            args.source_commit,
            args.workload,
            args.report,
        )
    except (BaselineError, BudgetError, PerformanceSchemaError, OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
