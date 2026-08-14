"""Corpus-bound performance baseline manifests.

Budgets describe policy (relative and absolute allowances).  This module
stores only reviewed reference measurements and their identities so a value
captured for one corpus revision cannot silently be reused for another.
"""

from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path
from typing import Any, Mapping


SCHEMA = 1
KIND = "bbtidy-performance-baselines"
DEFAULT_STATISTICS = {
    "wall_ms": "median",
    "peak_rss_bytes": "p90",
    "user_cpu_ms": "median",
    "system_cpu_ms": "median",
    "read_bytes": "median",
    "written_bytes": "median",
}


class BaselineError(ValueError):
    """A baseline manifest is malformed or does not match evidence."""


def reference_key(
    runner_class: str,
    workload: str,
    mode: str,
    corpus_digest: str,
    metric: str,
    statistic: str,
) -> str:
    return "|".join(
        (runner_class, workload, mode, corpus_digest, metric, statistic)
    )


def _nonnegative_number(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise BaselineError(f"{label} must be numeric")
    if not math.isfinite(float(value)) or value < 0:
        raise BaselineError(f"{label} must be finite and non-negative")
    return float(value)


def _entry_key(entry: Mapping[str, Any], runner_class: str) -> str:
    fields = ("workload", "mode", "corpus_digest", "metric", "statistic")
    if any(not isinstance(entry.get(field), str) or not entry[field] for field in fields):
        raise BaselineError("baseline reference identity fields are required")
    return reference_key(
        runner_class,
        entry["workload"],
        entry["mode"],
        entry["corpus_digest"],
        entry["metric"],
        entry["statistic"],
    )


def validate_baselines(value: Mapping[str, Any]) -> dict:
    if not isinstance(value, Mapping):
        raise BaselineError("baseline manifest must be an object")
    if value.get("schema") != SCHEMA or value.get("kind") != KIND:
        raise BaselineError("unsupported performance baseline schema or kind")
    runner = value.get("runner")
    if not isinstance(runner, Mapping) or not isinstance(runner.get("class"), str) or not runner["class"]:
        raise BaselineError("baseline runner.class is required")
    references = value.get("references")
    if not isinstance(references, Mapping):
        raise BaselineError("baseline references must be an object")
    checked = dict(value)
    checked_references = {}
    for key, raw in references.items():
        if not isinstance(key, str) or not isinstance(raw, Mapping):
            raise BaselineError("baseline references must map string keys to objects")
        entry = dict(raw)
        derived_key = _entry_key(entry, runner["class"])
        if key != derived_key:
            raise BaselineError(f"baseline reference key does not match its identity: {key}")
        if entry["statistic"] not in {"median", "p90", "min", "max"}:
            raise BaselineError(f"unsupported baseline statistic: {entry['statistic']}")
        if not isinstance(entry.get("corpus_id"), str) or not entry["corpus_id"]:
            raise BaselineError(f"baseline {key} must declare corpus_id")
        if not isinstance(entry.get("source_commit"), str) or not entry["source_commit"]:
            raise BaselineError(f"baseline {key} must declare source_commit")
        entry["value"] = _nonnegative_number(entry.get("value"), f"baseline {key}.value")
        if "sample_count" in entry:
            if isinstance(entry["sample_count"], bool) or not isinstance(entry["sample_count"], int) or entry["sample_count"] < 1:
                raise BaselineError(f"baseline {key}.sample_count must be a positive integer")
        checked_references[key] = entry
    checked["references"] = checked_references
    required = value.get("required_workloads", [])
    if not isinstance(required, list) or any(not isinstance(item, str) or not item for item in required):
        raise BaselineError("baseline required_workloads must be a list of strings")
    if len(set(required)) != len(required):
        raise BaselineError("baseline required_workloads contains duplicates")
    checked["required_workloads"] = required
    return checked


def load_baselines(path: Path) -> dict:
    try:
        value = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise BaselineError(f"could not read performance baselines {path}: {error}") from error
    return validate_baselines(value)


def canonical_baseline_bytes(value: Mapping[str, Any]) -> bytes:
    checked = validate_baselines(value)
    return (json.dumps(checked, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")


def statistic_for(metric: str, rule: Mapping[str, Any] | None = None) -> str:
    requested = rule.get("statistic") if isinstance(rule, Mapping) else None
    statistic = requested or DEFAULT_STATISTICS.get(metric, "median")
    if statistic not in {"median", "p90", "min", "max"}:
        raise BaselineError(f"unsupported statistic for {metric}: {statistic}")
    return statistic


def baseline_for(
    manifest: Mapping[str, Any],
    record: Mapping[str, Any],
    metric: str,
    statistic: str,
) -> Mapping[str, Any] | None:
    checked = validate_baselines(manifest)
    runner_class = record.get("runner", {}).get("class")
    if runner_class != checked["runner"]["class"]:
        raise BaselineError(
            f"baseline runner class mismatch: evidence={runner_class!r}, baseline={checked['runner']['class']!r}"
        )
    corpus_digest = record.get("corpus", {}).get("revision_digest")
    if not isinstance(corpus_digest, str) or not corpus_digest:
        raise BaselineError("performance evidence must declare a corpus revision digest")
    key = reference_key(
        runner_class,
        record.get("workload", ""),
        record.get("mode", ""),
        corpus_digest,
        metric,
        statistic,
    )
    return checked["references"].get(key)


def digest(value: Mapping[str, Any]) -> str:
    return hashlib.sha256(canonical_baseline_bytes(value)).hexdigest()


def required_workloads_from_budgets(budget: Mapping[str, Any]) -> list[str]:
    workloads = []
    for workload, rules in budget.get("workloads", {}).items():
        if not isinstance(rules, Mapping):
            continue
        if any(
            metric in rules and isinstance(rules[metric], Mapping)
            for metric in ("wall_ms", "peak_rss_bytes")
        ):
            workloads.append(workload)
    return sorted(workloads)


def missing_required_workloads(
    manifest: Mapping[str, Any], budget: Mapping[str, Any]
) -> list[str]:
    checked = validate_baselines(manifest)
    required = set(required_workloads_from_budgets(budget))
    present = {
        entry["workload"]
        for entry in checked["references"].values()
        if isinstance(entry, Mapping)
    }
    return sorted(required - present)
