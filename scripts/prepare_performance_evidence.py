#!/usr/bin/env python3
"""Consolidate validated performance records into release evidence."""

from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path

try:
    from scripts.check_performance_budget import BudgetError, compare_record, load_budgets
    from scripts.performance_schema import PerformanceSchemaError, load_evidence
except ModuleNotFoundError:  # direct script execution
    from check_performance_budget import BudgetError, compare_record, load_budgets  # type: ignore
    from performance_schema import PerformanceSchemaError, load_evidence  # type: ignore


def _version_matches(value: object, expected: str) -> bool:
    if not isinstance(value, str):
        return False
    return value.strip() in {expected, f"bbtidy {expected}"} or value.strip().endswith(
        f" {expected}"
    )


def consolidate(
    output: Path,
    budget_path: Path,
    records: list[Path],
    source_commit: str,
    version: str,
    runner_class: str,
) -> dict:
    budget = load_budgets(budget_path)
    if budget["runner_class"] != runner_class:
        raise ValueError("runner class does not match the checked-in budget policy")
    if not records:
        raise ValueError("at least one performance record is required")
    output.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(budget_path, output / "budgets.json")
    reports = []
    for source in records:
        evidence = load_evidence(source)
        record_list = evidence["records"] if evidence.get("kind") == "bbtidy-performance-suite" else [evidence]
        relative = source.name
        destination = output / relative
        shutil.copyfile(source, destination)
        for record in record_list:
            if record["runner"]["class"] != runner_class:
                raise ValueError(f"{source} uses runner class {record['runner']['class']!r}")
            if record["runner"].get("source_commit") not in {None, source_commit}:
                raise ValueError(f"{source} was measured from a different source commit")
            if record.get("commit") != source_commit:
                raise ValueError(f"{source} has the wrong source commit")
            if not _version_matches(record.get("version"), version):
                raise ValueError(f"{source} has the wrong bbtidy version")
            if record["summary"]["status"] != "success":
                raise ValueError(f"{source} contains an unsuccessful performance sample")
            comparison = compare_record(record, budget)
            reports.append(
                {"path": relative, "workload": record["workload"], "comparison": comparison}
            )
            if comparison["failures"]:
                raise ValueError(f"{source} exceeded a blocking performance budget")

    reports.sort(key=lambda report: (report["path"], report["workload"]))
    manifest = {
        "schema": 1,
        "kind": "bbtidy-performance-release",
        "source_commit": source_commit,
        "version": version,
        "runner_class": runner_class,
        "records": sorted({report["path"] for report in reports}),
    }
    summary = {
        "schema": 1,
        "status": "passed",
        "source_commit": source_commit,
        "version": version,
        "records": reports,
    }
    (output / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (output / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--budget", type=Path, required=True)
    parser.add_argument("--record", type=Path, action="append", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--runner-class", required=True)
    args = parser.parse_args()
    try:
        consolidate(
            args.output,
            args.budget,
            args.record,
            args.source_commit,
            args.version,
            args.runner_class,
        )
    except (BudgetError, PerformanceSchemaError, OSError, ValueError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
