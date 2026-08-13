"""Versioned performance evidence helpers.

Performance records intentionally live outside the lint and compatibility
fingerprints. Timing and memory are runner-dependent; structural counters and
corpus identity are retained so a result can still be audited.
"""

from __future__ import annotations

import json
import math
import statistics
from pathlib import Path
from typing import Any, Iterable, Mapping


SCHEMA = 1
KIND = "bbtidy-performance"
RESULT_FIELDS = (
    "status",
    "wall_ms",
    "user_cpu_ms",
    "system_cpu_ms",
    "peak_rss_bytes",
    "read_bytes",
    "written_bytes",
)


class PerformanceSchemaError(ValueError):
    """A performance record is malformed or unsafe to compare."""


def _number(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise PerformanceSchemaError(f"{label} must be numeric")
    if not math.isfinite(float(value)) or value < 0:
        raise PerformanceSchemaError(f"{label} must be finite and non-negative")
    return float(value)


def _nonnegative_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise PerformanceSchemaError(f"{label} must be a non-negative integer")
    return value


def validate_result(result: Mapping[str, Any], label: str = "result") -> dict:
    if not isinstance(result, Mapping):
        raise PerformanceSchemaError(f"{label} must be an object")
    status = result.get("status")
    if status not in {"success", "failed", "cancelled", "timed-out", "limit-terminated"}:
        raise PerformanceSchemaError(f"{label}.status is unsupported")
    checked = dict(result)
    checked["status"] = status
    for field in RESULT_FIELDS[1:]:
        checked[field] = _number(result.get(field), f"{label}.{field}")
    for field in ("stdout_bytes", "stderr_bytes"):
        if field in result:
            checked[field] = _nonnegative_int(result[field], f"{label}.{field}")
    if "exit_code" in result and result["exit_code"] is not None:
        if isinstance(result["exit_code"], bool) or not isinstance(result["exit_code"], int):
            raise PerformanceSchemaError(f"{label}.exit_code must be an integer")
        checked["exit_code"] = result["exit_code"]
    return checked


def _aggregate_numeric(samples: Iterable[Mapping[str, Any]], field: str) -> dict:
    values = [float(sample[field]) for sample in samples]
    if not values:
        return {"median": 0, "p90": 0, "min": 0, "max": 0}
    ordered = sorted(values)
    p90_index = min(len(ordered) - 1, max(0, math.ceil(len(ordered) * 0.9) - 1))
    return {
        "median": statistics.median(values),
        "p90": ordered[p90_index],
        "min": ordered[0],
        "max": ordered[-1],
    }


def aggregate_results(samples: Iterable[Mapping[str, Any]]) -> dict:
    checked = [
        validate_result(sample.get("result", sample), "sample.result")
        for sample in samples
    ]
    statuses = {sample["status"] for sample in checked}
    status = "success" if statuses == {"success"} else "failed"
    return {
        "status": status,
        "wall_ms": _aggregate_numeric(checked, "wall_ms")["median"],
        "user_cpu_ms": _aggregate_numeric(checked, "user_cpu_ms")["median"],
        "system_cpu_ms": _aggregate_numeric(checked, "system_cpu_ms")["median"],
        "peak_rss_bytes": _aggregate_numeric(checked, "peak_rss_bytes")["median"],
        "read_bytes": _aggregate_numeric(checked, "read_bytes")["median"],
        "written_bytes": _aggregate_numeric(checked, "written_bytes")["median"],
        "sample_ranges": {
            field: _aggregate_numeric(checked, field)
            for field in RESULT_FIELDS[1:]
        },
    }


def validate_record(record: Mapping[str, Any]) -> dict:
    if not isinstance(record, Mapping):
        raise PerformanceSchemaError("performance record must be an object")
    if record.get("schema") != SCHEMA or record.get("kind") != KIND:
        raise PerformanceSchemaError("unsupported performance schema or kind")
    for field in ("workload", "mode", "commit", "version"):
        if not isinstance(record.get(field), str) or not record[field]:
            raise PerformanceSchemaError(f"performance {field} is required")
    runner = record["runner"]
    if not isinstance(runner, Mapping) or not isinstance(runner.get("class"), str):
        raise PerformanceSchemaError("performance runner.class is required")
    corpus = record.get("corpus")
    if not isinstance(corpus, Mapping) or not isinstance(corpus.get("id"), str) or not corpus["id"]:
        raise PerformanceSchemaError("performance corpus.id is required")
    samples = record.get("samples")
    if not isinstance(samples, list) or not samples:
        raise PerformanceSchemaError("performance samples must be a non-empty array")
    for index, sample in enumerate(samples):
        if not isinstance(sample, Mapping):
            raise PerformanceSchemaError(f"sample {index} must be an object")
        validate_result(sample.get("result", {}), f"sample {index}.result")
    summary = validate_result(record.get("summary", {}), "summary")
    if "sample_ranges" not in record.get("summary", {}):
        raise PerformanceSchemaError("summary.sample_ranges is required")
    ranges = record["summary"]["sample_ranges"]
    if not isinstance(ranges, Mapping):
        raise PerformanceSchemaError("summary.sample_ranges must be an object")
    for field in RESULT_FIELDS[1:]:
        value = ranges.get(field)
        if not isinstance(value, Mapping):
            raise PerformanceSchemaError(f"summary.sample_ranges.{field} must be an object")
        for statistic in ("median", "p90", "min", "max"):
            _number(value.get(statistic), f"summary.sample_ranges.{field}.{statistic}")
    checked = dict(record)
    checked["summary"] = summary
    checked["sample_count"] = len(samples)
    return checked


def load_record(path: Path) -> dict:
    try:
        value = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise PerformanceSchemaError(f"could not read performance record {path}: {error}") from error
    return validate_record(value)


def validate_suite(suite: Mapping[str, Any]) -> dict:
    if not isinstance(suite, Mapping):
        raise PerformanceSchemaError("performance suite must be an object")
    if suite.get("schema") != SCHEMA or suite.get("kind") != "bbtidy-performance-suite":
        raise PerformanceSchemaError("unsupported performance suite schema or kind")
    records = suite.get("records")
    if not isinstance(records, list) or not records:
        raise PerformanceSchemaError("performance suite records must be a non-empty array")
    checked = dict(suite)
    checked["records"] = [validate_record(record) for record in records]
    runner = suite.get("runner")
    if not isinstance(runner, Mapping) or not isinstance(runner.get("class"), str):
        raise PerformanceSchemaError("performance suite runner.class is required")
    for index, record in enumerate(checked["records"]):
        if record["runner"]["class"] != runner["class"]:
            raise PerformanceSchemaError(f"suite record {index} has a different runner class")
    return checked


def load_evidence(path: Path) -> dict:
    try:
        value = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise PerformanceSchemaError(f"could not read performance evidence {path}: {error}") from error
    if isinstance(value, Mapping) and value.get("kind") == "bbtidy-performance-suite":
        return validate_suite(value)
    return validate_record(value)


def write_record(path: Path, record: Mapping[str, Any]) -> None:
    checked = validate_record(record)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(checked, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
