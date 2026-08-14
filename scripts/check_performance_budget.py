#!/usr/bin/env python3
"""Compare performance evidence with explicit, runner-bound budgets."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any, Mapping

try:
    from scripts.performance_schema import PerformanceSchemaError, load_evidence, load_record
    from scripts.performance_baselines import BaselineError, baseline_for, load_baselines, statistic_for
except ModuleNotFoundError:  # direct script execution
    from performance_schema import PerformanceSchemaError, load_evidence, load_record  # type: ignore[no-redef]
    from performance_baselines import BaselineError, baseline_for, load_baselines, statistic_for  # type: ignore[no-redef]


class BudgetError(ValueError):
    """A budget file or comparison is invalid."""


def load_budgets(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise BudgetError(f"could not read budgets: {error}") from error
    if value.get("schema") != 1 or not isinstance(value.get("runner_class"), str):
        raise BudgetError("budgets must use schema 1 and declare runner_class")
    if not isinstance(value.get("workloads"), Mapping):
        raise BudgetError("budgets.workloads must be an object")
    global_structural = value.get("structural_defaults", {})
    if not isinstance(global_structural, Mapping):
        raise BudgetError("budgets.structural_defaults must be an object")
    for metric, rule in global_structural.items():
        if not isinstance(rule, Mapping):
            raise BudgetError(f"structural default for {metric} must be an object")
    policy = value.get("policy")
    if not isinstance(policy, Mapping) or policy.get("relative_and_absolute_required") is not True:
        raise BudgetError("budgets must require both relative and absolute thresholds")
    for workload, rules in value["workloads"].items():
        if not isinstance(rules, Mapping):
            raise BudgetError(f"budget for {workload} must be an object")
        for metric, rule in rules.items():
            if metric in {"notes"}:
                continue
            if metric == "structural":
                structural = rule
                if not isinstance(structural, Mapping):
                    raise BudgetError(f"structural budget for {workload} must be an object")
                for structural_metric, structural_rule in structural.items():
                    if not isinstance(structural_rule, Mapping):
                        raise BudgetError(f"structural budget for {workload}.{structural_metric} is invalid")
                    if (
                        structural_rule.get("max") is None
                        and structural_rule.get("baseline") is None
                        and structural_rule.get("allowed") is None
                        and structural_rule.get("equals") is None
                    ):
                        raise BudgetError(
                            f"structural budget for {workload}.{structural_metric} needs max or baseline"
                        )
                continue
            if not isinstance(rule, Mapping):
                raise BudgetError(f"budget for {workload}.{metric} must be an object")
            if ("max" in rule or "max_delta" in rule) and "max_ratio" not in rule:
                if (
                    rule.get("max") is None
                    and rule.get("baseline") is None
                    and rule.get("allowed") is None
                    and rule.get("equals") is None
                ):
                    raise BudgetError(
                        f"structural budget for {workload}.{metric} needs max or baseline"
                    )
                continue
            if "max_ratio" in rule:
                if "min_absolute_regression" not in rule:
                    raise BudgetError(
                        f"timing budget for {workload}.{metric} needs max_ratio and min_absolute_regression"
                    )
            if "statistic" in rule and rule["statistic"] not in {"median", "p90", "min", "max"}:
                raise BudgetError(f"budget for {workload}.{metric} has an unsupported statistic")
    return value


def _metric(result: Mapping[str, Any], name: str) -> float:
    value: Any = result
    for component in name.split("."):
        if not isinstance(value, Mapping) or component not in value:
            raise BudgetError(f"result is missing metric {name}")
        value = value[component]
    if isinstance(value, bool) or not isinstance(value, (int, float)) or value < 0:
        raise BudgetError(f"result metric {name} is invalid")
    return float(value)


def _value(result: Mapping[str, Any], name: str) -> Any:
    value: Any = result
    for component in name.split("."):
        if not isinstance(value, Mapping) or component not in value:
            raise BudgetError(f"result is missing metric {name}")
        value = value[component]
    return value


def compare_record(
    record: Mapping[str, Any],
    budget: Mapping[str, Any],
    baselines: Mapping[str, Any] | None = None,
    strict_baseline: bool = False,
) -> dict:
    runner_class = record.get("runner", {}).get("class")
    if runner_class != budget.get("runner_class"):
        raise BudgetError(
            f"runner class mismatch: evidence={runner_class!r}, budget={budget.get('runner_class')!r}"
        )
    workload = record.get("workload")
    workload_budget = budget.get("workloads", {}).get(workload)
    if not isinstance(workload_budget, Mapping):
        raise BudgetError(f"no budget exists for workload {workload!r}")
    current = record["summary"]
    failures = []
    advisory = []
    measurements = {}
    for metric, rule in workload_budget.items():
        if metric in {"structural", "notes"}:
            continue
        if not isinstance(rule, Mapping):
            raise BudgetError(f"budget for {workload}.{metric} must be an object")
        if ("max" in rule or "max_delta" in rule) and "max_ratio" not in rule:
            continue
        baseline = rule.get("baseline")
        baseline_reference = None
        statistic = statistic_for(metric, rule)
        if baselines is not None:
            try:
                baseline_reference = baseline_for(baselines, record, metric, statistic)
            except BaselineError as error:
                raise BudgetError(str(error)) from error
            if baseline_reference is not None:
                baseline = baseline_reference["value"]
            else:
                message = (
                    f"{workload}.{metric} has no reference for corpus "
                    f"{record.get('corpus', {}).get('revision_digest')!r} and statistic {statistic}"
                )
                if strict_baseline:
                    raise BudgetError(message)
                advisory.append(message)
        current_value = _metric(current, metric)
        measurements[metric] = {
            "current": current_value,
            "baseline": baseline,
            "statistic": statistic,
            "reference": baseline_reference,
        }
        if baseline is None:
            if baselines is None:
                advisory.append(f"{workload}.{metric} has no populated baseline")
            continue
        if isinstance(baseline, bool) or not isinstance(baseline, (int, float)) or baseline < 0:
            raise BudgetError(f"baseline for {workload}.{metric} is invalid")
        ratio = rule.get("max_ratio")
        absolute = rule.get("min_absolute_regression")
        if ratio is None or absolute is None:
            raise BudgetError(f"timing budget for {workload}.{metric} is missing relative or absolute threshold")
        if ratio < 0 or absolute < 0:
            raise BudgetError(f"timing budget for {workload}.{metric} has a negative threshold")
        relative_regression = current_value - baseline * ratio
        absolute_regression = current_value - baseline
        epsilon = max(1e-9, abs(baseline * ratio) * 1e-12)
        if relative_regression > epsilon and absolute_regression > absolute + epsilon:
            message = (
                f"{workload}.{metric}: {current_value:g} > {baseline:g} * {ratio:g} "
                f"and absolute regression {current_value - baseline:g} > {absolute:g}"
            )
            if rule.get("blocking", False):
                failures.append(message)
            else:
                advisory.append(message)

    structural_rules = dict(budget.get("structural_defaults", {}) or {})
    structural_rules.update(dict(workload_budget.get("structural", {}) or {}))
    structural_rules.update(
        {
            metric: rule
            for metric, rule in workload_budget.items()
            if metric not in {"structural", "notes"}
            and isinstance(rule, Mapping)
            and ("max" in rule or "max_delta" in rule)
            and "max_ratio" not in rule
        }
    )
    for metric, rule in structural_rules.items():
        if not isinstance(rule, Mapping):
            raise BudgetError(f"structural budget for {workload}.{metric} must be an object")
        current_value = _value(current, metric)
        allowed = rule.get("allowed")
        if allowed is not None and current_value not in allowed:
            failures.append(f"{workload}.{metric}: {current_value!r} is not an allowed structural value")
        if "equals" in rule and current_value != rule["equals"]:
            failures.append(
                f"{workload}.{metric}: {current_value!r} does not equal structural value {rule['equals']!r}"
            )
        if not isinstance(current_value, (int, float)) or isinstance(current_value, bool):
            continue
        maximum = rule.get("max")
        if maximum is not None and current_value > maximum:
            failures.append(f"{workload}.{metric}: {current_value:g} exceeds structural maximum {maximum:g}")
        baseline = rule.get("baseline")
        if baseline is not None:
            delta = current_value - baseline
            if delta > rule.get("max_delta", 0):
                failures.append(f"{workload}.{metric}: structural delta {delta:g} exceeds {rule.get('max_delta', 0):g}")
    return {
        "status": "failed" if failures else "advisory" if advisory else "matched",
        "workload": workload,
        "runner_class": runner_class,
        "failures": failures,
        "advisory": advisory,
        "measurements": measurements,
    }


def update_budget(budget_path: Path, record: Mapping[str, Any], reason: str) -> dict:
    if not reason.strip():
        raise BudgetError("--update requires a non-empty --reason")
    if os.environ.get("CI", "").lower() == "true" and os.environ.get("BBTIDY_ALLOW_PERFORMANCE_UPDATE") != "1":
        raise BudgetError("performance budget updates are disabled in CI")
    value = load_budgets(budget_path)
    if value["runner_class"] != record["runner"]["class"]:
        raise BudgetError("cannot update budgets from an incompatible runner class")
    workload = record["workload"]
    workload_budget = value["workloads"].setdefault(workload, {})
    before = {}
    for metric in ("wall_ms", "peak_rss_bytes", "user_cpu_ms", "system_cpu_ms", "read_bytes", "written_bytes"):
        current = record["summary"].get(metric)
        if current is None:
            continue
        rule = workload_budget.setdefault(metric, {"max_ratio": 1.15, "min_absolute_regression": 0, "blocking": False})
        before[metric] = rule.get("baseline")
        rule["baseline"] = current
    value.setdefault("history", []).append(
        {"workload": workload, "reason": reason, "before": before, "after": {metric: workload_budget[metric]["baseline"] for metric in before}}
    )
    budget_path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"workload": workload, "reason": reason, "before": before, "after": record["summary"]}, indent=2, sort_keys=True))
    return value


def compare_candidate_to_baseline(
    candidate: Mapping[str, Any], baseline: Mapping[str, Any], budget: Mapping[str, Any]
) -> dict:
    for field in ("workload", "mode"):
        if candidate.get(field) != baseline.get(field):
            raise BudgetError(f"candidate and baseline differ in {field}")
    if candidate.get("runner", {}).get("class") != baseline.get("runner", {}).get("class"):
        raise BudgetError("candidate and baseline use different runner classes")
    candidate_corpus = candidate.get("corpus", {})
    baseline_corpus = baseline.get("corpus", {})
    if candidate_corpus.get("revision_digest") != baseline_corpus.get("revision_digest"):
        raise BudgetError("candidate and baseline use different corpus revisions")
    workload = candidate["workload"]
    rules = budget.get("workloads", {}).get(workload)
    if not isinstance(rules, Mapping):
        raise BudgetError(f"no budget exists for workload {workload!r}")
    derived = json.loads(json.dumps(budget))
    derived_rules = derived["workloads"].setdefault(workload, dict(rules))
    for metric, rule in derived_rules.items():
        if not isinstance(rule, dict) or "max_ratio" not in rule:
            continue
        rule["baseline"] = _metric(baseline["summary"], metric)
    return compare_record(candidate, {**derived, "workloads": {workload: derived_rules}})


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--budgets", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--reason", default="")
    parser.add_argument("--baseline-evidence", type=Path)
    parser.add_argument("--baselines", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        budget = load_budgets(args.budgets)
        evidence = load_evidence(args.evidence)
        baselines = load_baselines(args.baselines) if args.baselines else None
        if args.update:
            raise BudgetError(
                "direct budget updates are disabled; use scripts/promote_performance_baselines.py"
            )
        if args.baseline_evidence:
            baseline_evidence = load_record(args.baseline_evidence)
            if evidence.get("kind") != "bbtidy-performance":
                raise BudgetError("--baseline-evidence requires one candidate record")
            comparison = compare_candidate_to_baseline(evidence, baseline_evidence, budget)
        elif evidence.get("kind") == "bbtidy-performance-suite":
            comparisons = [compare_record(record, budget, baselines) for record in evidence["records"]]
            comparison = {
                "status": (
                    "failed"
                    if any(item["failures"] for item in comparisons)
                    else "advisory"
                    if any(item["advisory"] for item in comparisons)
                    else "matched"
                ),
                "runner_class": evidence["runner"]["class"],
                "comparisons": comparisons,
                "failures": [message for item in comparisons for message in item["failures"]],
                "advisory": [message for item in comparisons for message in item["advisory"]],
            }
        else:
            comparison = compare_record(evidence, budget, baselines)
    except (BudgetError, BaselineError, PerformanceSchemaError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(comparison, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    for message in comparison["advisory"]:
        print(f"advisory: {message}", file=sys.stderr)
    for message in comparison["failures"]:
        print(f"error: {message}", file=sys.stderr)
    return 1 if comparison["failures"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
