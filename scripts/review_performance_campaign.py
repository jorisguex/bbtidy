#!/usr/bin/env python3
"""Validate a reference campaign and emit its review report without promotion."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

try:
    from scripts.check_performance_budget import BudgetError, load_budgets
    from scripts.performance_baselines import required_workloads_from_budgets
    from scripts.performance_schema import PerformanceSchemaError, load_evidence
    from scripts.promote_performance_baselines import (
        _group_key,
        _review_report,
        _validate_reference_runner,
        _validate_sample_contract,
    )
except ModuleNotFoundError:  # direct script execution
    from check_performance_budget import BudgetError, load_budgets  # type: ignore
    from performance_baselines import required_workloads_from_budgets  # type: ignore
    from performance_schema import PerformanceSchemaError, load_evidence  # type: ignore
    from promote_performance_baselines import (  # type: ignore
        _group_key,
        _review_report,
        _validate_reference_runner,
        _validate_sample_contract,
    )


def review(budget_path: Path, evidence_paths: list[Path]) -> dict:
    if len(evidence_paths) < 3:
        raise ValueError("at least three reference evidence files are required")
    budget = load_budgets(budget_path)
    grouped_records = {}
    for path in evidence_paths:
        evidence = load_evidence(path)
        records = evidence["records"] if evidence.get("kind") == "bbtidy-performance-suite" else [evidence]
        grouped = {}
        for record in records:
            if record["summary"]["status"] != "success":
                raise ValueError(f"{path} contains an unsuccessful record")
            _validate_sample_contract(record, path)
            _validate_reference_runner(record, path)
            key = _group_key(record)
            if key in grouped:
                raise ValueError(f"{path} contains duplicate workload {key[0]}")
            grouped[key] = record
            grouped_records.setdefault(key, []).append(record)
    if not grouped_records:
        raise ValueError("reference evidence contains no records")
    required = set(required_workloads_from_budgets(budget))
    missing_workloads = sorted(required - {key[0] for key in grouped_records})
    if missing_workloads:
        raise ValueError(
            "reference evidence is missing required workloads: " + ", ".join(missing_workloads)
        )
    for key, records in grouped_records.items():
        if len(records) != 3:
            raise ValueError(
                f"{key[0]} has {len(records)} compatible runs; exactly three are required"
            )
    records_by_run = {key: records for key, records in sorted(grouped_records.items())}
    report = _review_report(records_by_run, budget)
    for group_report in report.values():
        for metric_report in group_report["metrics"].values():
            values = metric_report["run_values"]
            if min(values) > 0 and max(values) / min(values) > 1.2:
                raise ValueError(
                    "independent reference medians differ by more than 20% for "
                    + group_report["workload"]
                )
    commits = {
        record.get("commit")
        for records in grouped_records.values()
        for record in records
    }
    runners = {
        record["runner"]["class"]
        for records in grouped_records.values()
        for record in records
    }
    if len(commits) != 1 or None in commits or "unknown" in commits:
        raise ValueError("reference evidence must use one known source commit")
    if len(runners) != 1:
        raise ValueError("reference evidence must use one runner class")
    return {
        "schema": 1,
        "kind": "bbtidy-performance-review",
        "source_commit": next(iter(commits)),
        "runner_class": next(iter(runners)),
        "runner": dict(next(iter(next(iter(grouped_records.values()))))["runner"]),
        "evidence": [str(path) for path in evidence_paths],
        "run_count": 3,
        "workload_count": len(report),
        "reports": report,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--budgets", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        value = review(args.budgets, args.evidence)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    except (BudgetError, PerformanceSchemaError, OSError, UnicodeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"wrote campaign review for {value['workload_count']} workloads to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
