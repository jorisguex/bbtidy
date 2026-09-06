#!/usr/bin/env python3
"""Populate blocking budgets from audited, repeated native runner evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import statistics
from pathlib import Path

try:
    from scripts.check_performance_budget import BudgetError, load_budgets, workload_rules
    from scripts.performance_schema import PerformanceSchemaError, aggregate_results, load_evidence
except ModuleNotFoundError:
    from check_performance_budget import BudgetError, load_budgets, workload_rules
    from performance_schema import PerformanceSchemaError, aggregate_results, load_evidence


def populate(budget_path: Path, manifest_path: Path, reason: str) -> dict:
    if not reason.strip():
        raise BudgetError("baseline updates require a non-empty --reason")
    if os.environ.get("CI", "").lower() == "true" and os.environ.get("BBTIDY_ALLOW_PERFORMANCE_UPDATE") != "1":
        raise BudgetError("performance budget updates are disabled in CI")
    budget = load_budgets(budget_path)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema") != 1 or not manifest.get("evidence"):
        raise BudgetError("reference manifest must declare schema 1 and evidence")
    grouped = {}
    for entry in manifest["evidence"]:
        path = (manifest_path.parent / entry["path"]).resolve()
        if not path.is_relative_to(manifest_path.parent.resolve()):
            raise BudgetError("reference evidence must be inside the manifest directory")
        if hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
            raise BudgetError(f"reference checksum mismatch: {entry['path']}")
        if not entry["run_url"].startswith("https://github.com/jorisguex/bbtidy/actions/runs/"):
            raise BudgetError("reference must identify a bbtidy GitHub Actions run")
        evidence = load_evidence(path)
        records = evidence["records"] if evidence["kind"] == "bbtidy-performance-suite" else [evidence]
        for record in records:
            runner = record["runner"]
            if (
                runner["class"] != budget["runner_class"]
                or not runner.get("os", "").startswith("Linux-")
                or runner.get("architecture") != "x86_64"
                or record["commit"] != entry["source_commit"]
                or runner.get("source_commit") != record["commit"]
            ):
                raise BudgetError("reference runner or source identity mismatch")
            digest = record["corpus"].get("revision_digest")
            if not isinstance(digest, str) or len(digest) != 64:
                raise BudgetError("reference corpus digest is required")
            aggregate = aggregate_results(record["samples"])
            if aggregate["status"] != "success" or any(
                record["summary"].get(key) != value
                for key, value in aggregate.items()
            ):
                raise BudgetError("reference summary must match successful raw samples")
            grouped.setdefault(record["workload"], []).append((entry, record))

    changes = {}
    for workload, references in sorted(grouped.items()):
        first = references[0][1]
        runs = [entry["run_url"] for entry, _ in references]
        if len(set(runs)) != len(runs) or len(runs) < 2:
            raise BudgetError(f"{workload} requires at least two distinct reference runs")
        if any(
            record["mode"] != first["mode"] or record["corpus"] != first["corpus"]
            for _, record in references
        ):
            raise BudgetError(f"{workload} reference modes or corpora differ")
        rules = workload_rules(workload, budget)
        changes[workload] = {}
        for metric in ("wall_ms", "peak_rss_bytes"):
            if metric not in rules:
                raise BudgetError(f"{workload} needs an explicit {metric} policy")
            baseline = statistics.median(record["summary"][metric] for _, record in references)
            if baseline <= 0:
                raise BudgetError(f"{workload}.{metric} reference must be positive")
            changes[workload][metric] = {"before": rules[metric].get("baseline"), "after": baseline}
            rules[metric].update(baseline=baseline, blocking=True)
        rules["reference"] = {
            "mode": first["mode"],
            "corpus": first["corpus"],
            "aggregation": "median of per-run medians",
            "run_urls": sorted(runs),
            "commits": sorted({record["commit"] for _, record in references}),
            "sample_count": sum(len(record["samples"]) for _, record in references),
            "reason": reason,
        }
        budget["workloads"][workload] = rules

    required = set(budget["policy"].get("required_baselines", [])) | grouped.keys()
    budget["policy"]["required_baselines"] = sorted(required)
    budget["policy"]["timing_is_advisory_until_populated"] = True
    # No output is changed until every reference has passed validation.
    budget_path.write_text(json.dumps(budget, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return changes


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--budgets", type=Path, required=True)
    parser.add_argument("--references", type=Path, required=True)
    parser.add_argument("--reason", required=True)
    args = parser.parse_args()
    try:
        changes = populate(args.budgets, args.references, args.reason)
    except (BudgetError, PerformanceSchemaError, OSError, ValueError, KeyError) as error:
        parser.error(str(error))
    print(json.dumps(changes, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
