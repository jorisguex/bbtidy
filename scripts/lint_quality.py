"""Pure deterministic normalization for ``bbtidy check --output json``.

This module deliberately has no subprocess, filesystem mutation, network, or
baseline-policy responsibilities.  Callers provide the parsed JSON report and
the repository roots that define the corpus path identity.
"""

from dataclasses import dataclass
from copy import deepcopy
import hashlib
import json
import math
import os
from pathlib import Path
import re
from typing import Any, Iterable, Mapping, Optional, Sequence, Tuple


FINGERPRINT_VERSION = 1
REPORT_VERSION = 1
SEVERITIES = ("info", "warning", "error")
KNOWN_RULE_IDS = tuple("BBT{:03d}".format(number) for number in range(1, 39))
BASELINE_SCHEMA = 1
REVIEW_SCHEMA = 2
QUALITY_REPORT_VERSION = 1
PILOT_THRESHOLDS = {
    "essential_false_positive_rate": 0.01,
    "recommended_false_positive_rate": 0.05,
    "recommended_unclear_rate": 0.05,
    "recommended_actionable_rate": 0.70,
    "new_config_minutes": 15,
    "operational_failures_mistaken_for_lint": 0,
    "unsafe_edits": 0,
}
BASELINE_REVIEW_STATUSES = frozenset(
    {"unreviewed", "reviewed", "accepted-known-limitations", "not-applicable"}
)
REVIEWED_RULE_STATUSES = frozenset({"reviewed", "accepted-known-limitations"})
BASELINE_TOP_LEVEL_KEYS = frozenset(
    {"schema", "corpus", "lint_contract", "measurement", "review"}
)
BASELINE_CORPUS_KEYS = frozenset({"id", "repositories", "layers"})
BASELINE_REPOSITORY_KEYS = frozenset({"name", "revision"})
BASELINE_LAYER_KEYS = frozenset({"name", "repository", "path"})
BASELINE_CONTRACT_KEYS = frozenset(
    {"report_version", "fingerprint_version", "source_state", "configuration", "scope"}
)
BASELINE_MEASUREMENT_KEYS = frozenset(
    {"total_findings", "findings_sha256", "files_with_findings", "severity_counts", "rules"}
)
BASELINE_RULE_MEASUREMENT_KEYS = frozenset(
    {"count", "files", "findings_sha256", "severity_counts"}
)
BASELINE_REVIEW_KEYS = frozenset({"status", "rules"})
BASELINE_REVIEW_V2_KEYS = frozenset({"schema", "status", "rules"})
BASELINE_REVIEW_RULE_KEYS = frozenset(
    {
        "status",
        "sample_size",
        "true_positive",
        "false_positive",
        "unclear",
        "notes",
        "repositories",
        "file_types",
        "diagnostic_shapes",
        "correctness",
        "actionability",
        "sample_fingerprints",
    }
)
BASELINE_REVIEW_RULE_LEGACY_KEYS = frozenset(
    {
        "status",
        "sample_size",
        "true_positive",
        "false_positive",
        "unclear",
        "notes",
        "repositories",
        "file_types",
        "diagnostic_shapes",
    }
)
REVISION_PATTERN = re.compile(r"^[0-9a-f]{40}$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")


class LintNormalizationError(RuntimeError):
    """A lint report cannot be converted to a canonical finding."""


class LintBaselineError(RuntimeError):
    """A lint-quality baseline is malformed or incompatible."""


@dataclass(frozen=True)
class NormalizationContext:
    """Explicit path and schema context required by normalization.

    ``repository_roots`` is an iterable of ``(repository_name, root)`` pairs.
    ``path_base`` is used only for relative diagnostic paths; callers should
    normally provide it explicitly so relative input cannot depend on the
    process working directory.
    """

    repository_roots: Tuple[Tuple[str, Path], ...]
    path_base: Optional[Path] = None
    known_rule_ids: Tuple[str, ...] = KNOWN_RULE_IDS
    volatile_roots: Tuple[Tuple[Path, str], ...] = ()

    def __post_init__(self) -> None:
        roots = []
        names = set()
        for name, root in self.repository_roots:
            name = str(name)
            if not name or name in names:
                raise ValueError("repository roots must have unique non-empty names")
            names.add(name)
            roots.append((name, Path(root)))
        if not roots:
            raise ValueError("at least one repository root is required")
        base = None if self.path_base is None else Path(self.path_base)
        volatile = tuple((Path(root), str(marker)) for root, marker in self.volatile_roots)
        if base is None and (
            any(not root.is_absolute() for _, root in roots)
            or any(not root.is_absolute() for root, _ in volatile)
        ):
            raise ValueError("relative repository roots require an explicit path_base")
        object.__setattr__(self, "repository_roots", tuple(roots))
        object.__setattr__(self, "path_base", base)
        object.__setattr__(self, "known_rule_ids", tuple(self.known_rule_ids))
        object.__setattr__(self, "volatile_roots", volatile)


def canonical_json_bytes(value: Any) -> bytes:
    """Serialize a value in the one format used by all fingerprints."""

    try:
        return json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise LintNormalizationError(
            "value cannot be canonically serialized: {}".format(error)
        ) from error


def _context_from(value: Any) -> NormalizationContext:
    if isinstance(value, NormalizationContext):
        return value
    if not isinstance(value, Mapping):
        raise TypeError("normalization context must be NormalizationContext or mapping")
    roots = value.get("repository_roots", value.get("repositories"))
    if isinstance(roots, Mapping):
        roots = list(roots.items())
    if roots is None:
        raise TypeError("normalization context is missing repository roots")
    normalized_roots = []
    for item in roots:
        if isinstance(item, Mapping):
            normalized_roots.append((item["name"], item["root"]))
        else:
            normalized_roots.append(tuple(item))
    return NormalizationContext(
        repository_roots=tuple(normalized_roots),
        path_base=value.get("path_base"),
        known_rule_ids=tuple(value.get("known_rule_ids", KNOWN_RULE_IDS)),
        volatile_roots=tuple(value.get("volatile_roots", ())),
    )


def _error(index: int, diagnostic: Any, message: str) -> LintNormalizationError:
    if isinstance(diagnostic, Mapping):
        rule_id = diagnostic.get("rule_id", "<missing>")
        path = diagnostic.get("path", "<missing>")
    else:
        rule_id = "<missing>"
        path = "<missing>"
    return LintNormalizationError(
        "diagnostic {} (rule_id={!r}, path={!r}): {}".format(
            index, rule_id, path, message
        )
    )


def _required(diagnostic: Mapping[str, Any], field: str, index: int) -> Any:
    if field not in diagnostic:
        raise _error(index, diagnostic, "missing required field {!r}".format(field))
    return diagnostic[field]


def _string(value: Any, field: str, index: int, diagnostic: Mapping[str, Any]) -> str:
    if not isinstance(value, str):
        raise _error(index, diagnostic, "field {!r} must be a string".format(field))
    return value


def _integer(
    value: Any,
    field: str,
    index: int,
    diagnostic: Mapping[str, Any],
    minimum: int = 0,
) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise _error(
            index,
            diagnostic,
            "field {!r} must be an integer >= {}".format(field, minimum),
        )
    return value


def _lexical_absolute(path: Path, base: Optional[Path]) -> Path:
    if not path.is_absolute() and base is None:
        raise ValueError("relative paths require an explicit path_base")
    value = path if path.is_absolute() else base / path
    return Path(os.path.abspath(os.path.normpath(str(value))))


def _relative_to(path: Path, root: Path) -> Optional[Path]:
    try:
        return path.relative_to(root)
    except ValueError:
        return None


def _repository_path(
    raw_path: str, context: NormalizationContext, index: int, diagnostic: Mapping[str, Any]
) -> Tuple[str, str]:
    try:
        candidate = _lexical_absolute(Path(raw_path), context.path_base)
    except ValueError as error:
        raise _error(index, diagnostic, str(error)) from error
    candidate_real = Path(os.path.realpath(str(candidate)))
    matches = []
    for name, supplied_root in context.repository_roots:
        root = _lexical_absolute(supplied_root, context.path_base)
        root_real = Path(os.path.realpath(str(root)))
        relative_real = _relative_to(candidate_real, root_real)
        if relative_real is None or str(relative_real) in ("", "."):
            continue
        matches.append((len(str(root_real)), name, root, relative_real))

    if not matches:
        raise _error(
            index,
            diagnostic,
            "source path cannot be normalized into a known repository root",
        )
    longest = max(item[0] for item in matches)
    selected = [item for item in matches if item[0] == longest]
    if len({item[1] for item in selected}) != 1:
        raise _error(index, diagnostic, "source path matches ambiguous repository roots")
    _, repository, root, _ = selected[0]
    relative = _relative_to(candidate, root)
    if relative is None or str(relative) in ("", ".") or any(
        part in ("", ".", "..") for part in relative.parts
    ):
        raise _error(index, diagnostic, "source path has an invalid repository-relative path")
    return repository, "/".join(relative.parts)


def _prefix_variants(root: Path, base: Optional[Path]) -> Tuple[str, ...]:
    absolute = _lexical_absolute(root, base)
    values = {
        str(absolute),
        absolute.as_posix(),
        absolute.as_posix().replace("/", "\\"),
        str(Path(os.path.realpath(str(absolute)))),
        Path(os.path.realpath(str(absolute))).as_posix(),
    }
    values.add(Path(os.path.realpath(str(absolute))).as_posix().replace("/", "\\"))
    return tuple(value for value in values if value)


def _volatile_prefixes(context: NormalizationContext) -> Tuple[Tuple[str, str], ...]:
    prefixes = []
    for repository, root in context.repository_roots:
        for prefix in _prefix_variants(root, context.path_base):
            prefixes.append((prefix, "$CORPUS/{}".format(repository)))
    for root, marker in context.volatile_roots:
        for prefix in _prefix_variants(root, context.path_base):
            prefixes.append((prefix, marker))
    # Longest first prevents a short nested/prefix-colliding root from winning.
    return tuple(sorted(set(prefixes), key=lambda item: (-len(item[0]), item[0])))


def _replace_known_prefixes(value: Optional[str], prefixes: Sequence[Tuple[str, str]]) -> Optional[str]:
    if value is None:
        return None
    if not prefixes:
        return value
    pattern = re.compile(
        "|".join(
            "{}(?=$|[/\\\\])".format(re.escape(prefix))
            for prefix, _ in prefixes
        )
    )
    replacements = {prefix: marker for prefix, marker in prefixes}

    def replace(match: re.Match) -> str:
        return replacements[match.group(0)]

    return pattern.sub(replace, value)


def _normalize_fix(
    fix: Any,
    index: int,
    fix_index: int,
    diagnostic: Mapping[str, Any],
    prefixes: Sequence[Tuple[str, str]],
) -> dict:
    if not isinstance(fix, Mapping):
        raise _error(index, diagnostic, "fix {} must be an object".format(fix_index))
    start = _integer(_required(fix, "start_byte", index), "fix.start_byte", index, diagnostic)
    end = _integer(_required(fix, "end_byte", index), "fix.end_byte", index, diagnostic)
    if end < start:
        raise _error(index, diagnostic, "fix {} has an inverted byte range".format(fix_index))
    replacement = _string(_required(fix, "replacement", index), "fix.replacement", index, diagnostic)
    message = _string(_required(fix, "message", index), "fix.message", index, diagnostic)
    return {
        "start_byte": start,
        "end_byte": end,
        "replacement": replacement,
        "message": _replace_known_prefixes(message, prefixes),
    }


def normalize_diagnostic(
    diagnostic: Any,
    context: Any,
    diagnostic_index: int = 0,
) -> dict:
    """Validate and normalize one parsed diagnostic."""

    normalized_context = _context_from(context)
    if not isinstance(diagnostic, Mapping):
        raise _error(diagnostic_index, diagnostic, "diagnostic must be an object")

    path = _string(_required(diagnostic, "path", diagnostic_index), "path", diagnostic_index, diagnostic)
    repository, relative_path = _repository_path(
        path, normalized_context, diagnostic_index, diagnostic
    )
    rule_id = _string(
        _required(diagnostic, "rule_id", diagnostic_index),
        "rule_id",
        diagnostic_index,
        diagnostic,
    )
    if rule_id not in normalized_context.known_rule_ids:
        raise _error(diagnostic_index, diagnostic, "unknown rule ID {!r}".format(rule_id))
    severity = _string(
        _required(diagnostic, "severity", diagnostic_index),
        "severity",
        diagnostic_index,
        diagnostic,
    ).lower()
    if severity not in SEVERITIES:
        raise _error(diagnostic_index, diagnostic, "unsupported severity {!r}".format(severity))

    line = _integer(_required(diagnostic, "line", diagnostic_index), "line", diagnostic_index, diagnostic, 1)
    column = _integer(_required(diagnostic, "column", diagnostic_index), "column", diagnostic_index, diagnostic, 1)
    end_line = _integer(_required(diagnostic, "end_line", diagnostic_index), "end_line", diagnostic_index, diagnostic, 1)
    end_column = _integer(
        _required(diagnostic, "end_column", diagnostic_index),
        "end_column",
        diagnostic_index,
        diagnostic,
        1,
    )
    if (end_line, end_column) < (line, column):
        raise _error(diagnostic_index, diagnostic, "diagnostic range is inverted")

    raw_range = _required(diagnostic, "range", diagnostic_index)
    if not isinstance(raw_range, Mapping):
        raise _error(diagnostic_index, diagnostic, "field 'range' must be an object")
    start_byte = _integer(
        _required(raw_range, "start_byte", diagnostic_index),
        "range.start_byte",
        diagnostic_index,
        diagnostic,
    )
    end_byte = _integer(
        _required(raw_range, "end_byte", diagnostic_index),
        "range.end_byte",
        diagnostic_index,
        diagnostic,
    )
    if end_byte < start_byte:
        raise _error(diagnostic_index, diagnostic, "diagnostic byte range is inverted")

    message = _string(
        _required(diagnostic, "message", diagnostic_index),
        "message",
        diagnostic_index,
        diagnostic,
    )
    raw_help = diagnostic.get("help")
    if raw_help is not None and not isinstance(raw_help, str):
        raise _error(diagnostic_index, diagnostic, "field 'help' must be a string or null")
    fixable = _required(diagnostic, "fixable", diagnostic_index)
    if not isinstance(fixable, bool):
        raise _error(diagnostic_index, diagnostic, "field 'fixable' must be a boolean")
    raw_fixes = _required(diagnostic, "fixes", diagnostic_index)
    if not isinstance(raw_fixes, list):
        raise _error(diagnostic_index, diagnostic, "field 'fixes' must be an array")
    prefixes = _volatile_prefixes(normalized_context)
    fixes = [
        _normalize_fix(fix, diagnostic_index, fix_index, diagnostic, prefixes)
        for fix_index, fix in enumerate(raw_fixes)
    ]
    fixes.sort(key=lambda fix: (fix["start_byte"], fix["end_byte"], fix["replacement"], fix["message"]))
    if fixable != bool(fixes):
        raise _error(
            diagnostic_index,
            diagnostic,
            "field 'fixable' is inconsistent with the fixes array",
        )

    return {
        "fingerprint_version": FINGERPRINT_VERSION,
        "source": {"repository": repository, "path": relative_path},
        "rule_id": rule_id,
        "severity": severity,
        "range": {
            "start": {"line": line, "column": column, "byte": start_byte},
            "end": {"line": end_line, "column": end_column, "byte": end_byte},
        },
        "message": _replace_known_prefixes(message, prefixes),
        "help": _replace_known_prefixes(raw_help, prefixes),
        "fixable": fixable,
        "fixes": fixes,
    }


def finding_sort_key(finding: Mapping[str, Any]) -> tuple:
    """Return the total deterministic ordering key for a normalized finding."""

    source = finding["source"]
    start = finding["range"]["start"]
    end = finding["range"]["end"]
    return (
        source["repository"],
        source["path"],
        start["byte"],
        end["byte"],
        start["line"],
        start["column"],
        end["line"],
        end["column"],
        finding["rule_id"],
        finding["severity"],
        finding["message"],
        canonical_json_bytes(finding["help"]),
        canonical_json_bytes(finding["fixes"]),
        canonical_json_bytes(finding),
    )


def normalize_lint_report(report: Any, context: Any) -> list:
    """Validate a parsed report and return sorted canonical findings."""

    normalized_context = _context_from(context)
    if not isinstance(report, Mapping):
        raise LintNormalizationError("lint report must be a JSON object")
    version = report.get("version")
    if isinstance(version, bool) or not isinstance(version, int) or version != REPORT_VERSION:
        raise LintNormalizationError(
            "unsupported lint report version {!r}".format(version)
        )
    diagnostics = report.get("diagnostics")
    if not isinstance(diagnostics, list):
        raise LintNormalizationError("lint report field 'diagnostics' must be an array")
    findings = [
        normalize_diagnostic(diagnostic, normalized_context, index)
        for index, diagnostic in enumerate(diagnostics)
    ]
    return sorted(findings, key=finding_sort_key)


def _digest_payload(kind: str, findings: Iterable[Mapping[str, Any]], rule_id: Optional[str] = None) -> dict:
    payload = {
        "kind": kind,
        "fingerprint_version": FINGERPRINT_VERSION,
        "findings": list(findings),
    }
    if rule_id is not None:
        payload["rule_id"] = rule_id
    return payload


def _sha256(value: Any) -> str:
    return hashlib.sha256(canonical_json_bytes(value)).hexdigest()


def digest_findings(findings: Iterable[Mapping[str, Any]]) -> str:
    """Digest the sorted corpus findings with a domain-separated payload."""

    ordered = sorted(list(findings), key=finding_sort_key)
    return _sha256(_digest_payload("bbtidy-lint-findings", ordered))


def digest_rule_findings(rule_id: str, findings: Iterable[Mapping[str, Any]]) -> str:
    """Digest one rule's sorted findings with a rule-specific domain."""

    ordered = sorted(list(findings), key=finding_sort_key)
    return _sha256(
        _digest_payload("bbtidy-lint-rule-findings", ordered, rule_id=rule_id)
    )


def _finding_digest(finding: Mapping[str, Any]) -> str:
    return _sha256(_digest_payload("bbtidy-lint-finding", [finding]))


def summarize_findings(
    findings: Iterable[Mapping[str, Any]],
    known_rule_ids: Sequence[str] = KNOWN_RULE_IDS,
) -> dict:
    """Build a deterministic summary solely from normalized findings."""

    ordered = sorted(list(findings), key=finding_sort_key)
    by_rule = {rule_id: [] for rule_id in sorted(set(known_rule_ids))}
    for finding in ordered:
        rule_id = finding["rule_id"]
        by_rule.setdefault(rule_id, []).append(finding)

    rules = {}
    for rule_id in sorted(by_rule):
        rule_findings = by_rule[rule_id]
        files = {
            (finding["source"]["repository"], finding["source"]["path"])
            for finding in rule_findings
        }
        rules[rule_id] = {
            "count": len(rule_findings),
            "files": len(files),
            "findings_sha256": digest_rule_findings(rule_id, rule_findings),
            "finding_digests": [_finding_digest(finding) for finding in rule_findings],
            "severity_counts": {
                severity: sum(
                    finding["severity"] == severity for finding in rule_findings
                )
                for severity in SEVERITIES
            },
        }

    severity_counts = {severity: 0 for severity in SEVERITIES}
    for finding in ordered:
        severity_counts[finding["severity"]] += 1
    files_with_findings = {
        (finding["source"]["repository"], finding["source"]["path"])
        for finding in ordered
    }
    return {
        "schema": 1,
        "fingerprint_version": FINGERPRINT_VERSION,
        "total_findings": len(ordered),
        "findings_sha256": digest_findings(ordered),
        "files_with_findings": len(files_with_findings),
        "severity_counts": severity_counts,
        "rules": rules,
    }


PROFILE_RULES = {
    "essential": frozenset(
        {
            rule_id
            for rule_id in KNOWN_RULE_IDS
            if rule_id not in {"BBT003", "BBT011", "BBT012", "BBT013", "BBT016", "BBT020", "BBT021"}
        }
    ),
    "recommended": frozenset(
        rule_id
        for rule_id in KNOWN_RULE_IDS
        if rule_id not in {"BBT003", "BBT011", "BBT012", "BBT013", "BBT016", "BBT021"}
    ),
    "strict": frozenset(KNOWN_RULE_IDS),
    "all": frozenset(KNOWN_RULE_IDS),
}


def _diagnostic_shape(finding: Mapping[str, Any]) -> str:
    message = re.sub(r"\d+", "#", str(finding.get("message", "")).lower())
    message = re.sub(r"[^a-z0-9_]+", " ", message).strip()
    return "{}:{}".format(finding["rule_id"], message)


def _file_type(path: str) -> str:
    suffix = Path(path).suffix.lower()
    return suffix if suffix else "<no-extension>"


def _distribution(values: Iterable[str]) -> dict:
    counts = {}
    for value in values:
        counts[value] = counts.get(value, 0) + 1
    return {key: counts[key] for key in sorted(counts)}


def stratified_review_sample(
    findings: Iterable[Mapping[str, Any]], sample_size: int
) -> list:
    """Select deterministic fingerprints across the important signal strata."""

    ordered = sorted(list(findings), key=lambda finding: (finding_sort_key(finding), _finding_digest(finding)))
    if sample_size <= 0 or not ordered:
        return []
    target = min(sample_size, len(ordered))
    selected = []
    selected_digests = set()

    def add(finding):
        digest = _finding_digest(finding)
        if digest in selected_digests or len(selected) >= target:
            return False
        selected.append(finding)
        selected_digests.add(digest)
        return True

    # Cover every repository, file type, and diagnostic shape first.
    for dimension in (
        lambda finding: finding["source"]["repository"],
        lambda finding: _file_type(finding["source"]["path"]),
        _diagnostic_shape,
    ):
        groups = {}
        for finding in ordered:
            groups.setdefault(dimension(finding), []).append(finding)
        for key in sorted(groups):
            add(groups[key][0])

    # Reserve at least ten percent per repository where the target permits it.
    repositories = {}
    for finding in ordered:
        repositories.setdefault(finding["source"]["repository"], []).append(finding)
    for repository in sorted(repositories):
        quota = max(1, math.ceil(len(repositories[repository]) * 0.10))
        for finding in repositories[repository][:quota]:
            if len(selected) >= target:
                break
            add(finding)

    # Fill the remaining budget in digest order, which naturally retains
    # long-tail diagnostic shapes rather than over-sampling one common form.
    for finding in ordered:
        if len(selected) >= target:
            break
        add(finding)
    return [_finding_digest(finding) for finding in selected]


def _review_classification(record: Optional[Mapping[str, Any]]) -> dict:
    if not isinstance(record, Mapping):
        return {
            "correctness": {"true_positive": 0, "false_positive": 0, "unclear": 0},
            "actionability": {
                "must_fix": 0,
                "should_fix": 0,
                "context_dependent": 0,
                "policy_only": 0,
                "not_actionable": 0,
            },
        }
    correctness = record.get("correctness")
    if not isinstance(correctness, Mapping):
        correctness = {
            "true_positive": record.get("true_positive", 0),
            "false_positive": record.get("false_positive", 0),
            "unclear": record.get("unclear", 0),
        }
    elif sum(int(correctness.get(key, 0) or 0) for key in ("true_positive", "false_positive", "unclear")) == 0 and any(
        record.get(key, 0) for key in ("true_positive", "false_positive", "unclear")
    ):
        correctness = {
            "true_positive": record.get("true_positive", 0),
            "false_positive": record.get("false_positive", 0),
            "unclear": record.get("unclear", 0),
        }
    actionability = record.get("actionability")
    if not isinstance(actionability, Mapping):
        # Legacy true-positive reviews predate the actionability dimension;
        # retain their decision as should-fix for compatibility while new v2
        # reviews must record the five explicit actionability classes.
        actionability = {"should_fix": correctness.get("true_positive", 0)}
    elif sum(int(actionability.get(key, 0) or 0) for key in ("must_fix", "should_fix", "context_dependent", "policy_only", "not_actionable")) == 0 and sum(
        int(correctness.get(key, 0) or 0) for key in ("true_positive", "false_positive", "unclear")
    ):
        actionability = {"should_fix": correctness.get("true_positive", 0)}
    return {
        "correctness": {
            key: int(correctness.get(key, 0) or 0)
            for key in ("true_positive", "false_positive", "unclear")
        },
        "actionability": {
            key: int(actionability.get(key, 0) or 0)
            for key in (
                "must_fix",
                "should_fix",
                "context_dependent",
                "policy_only",
                "not_actionable",
            )
        },
    }


def quality_report(
    findings: Iterable[Mapping[str, Any]],
    total_files: Optional[int] = None,
    reviews: Optional[Mapping[str, Any]] = None,
    runtime_seconds: Optional[Mapping[str, float]] = None,
    known_rule_ids: Sequence[str] = KNOWN_RULE_IDS,
) -> dict:
    """Build per-rule signal, review, and runtime measurements.

    The report deliberately keeps static findings separate from authoritative
    BitBake diagnostics so a high-volume static rule cannot look trustworthy
    merely because a resolver emitted a related message.
    """

    ordered = sorted(list(findings), key=finding_sort_key)
    denominator = total_files if total_files is not None else len(
        {(f["source"]["repository"], f["source"]["path"]) for f in ordered}
    )
    denominator = max(int(denominator), 1)
    review_rules = {}
    if isinstance(reviews, Mapping):
        raw_rules = reviews.get("rules", reviews)
        if isinstance(raw_rules, Mapping):
            review_rules = raw_rules
    by_rule = {rule_id: [] for rule_id in sorted(set(known_rule_ids))}
    for finding in ordered:
        by_rule.setdefault(finding["rule_id"], []).append(finding)
    rules = {}
    for rule_id, rule_findings in sorted(by_rule.items()):
        repositories = [f["source"]["repository"] for f in rule_findings]
        file_types = [_file_type(f["source"]["path"]) for f in rule_findings]
        shapes = [_diagnostic_shape(f) for f in rule_findings]
        authoritative = [
            f
            for f in rule_findings
            if f["rule_id"] == "BBT019"
            or str(f.get("message", "")).startswith("BitBake:")
        ]
        static = [f for f in rule_findings if f not in authoritative]
        review = _review_classification(review_rules.get(rule_id))
        review_target = minimum_review_samples(len(rule_findings))
        rules[rule_id] = {
            "total": len(rule_findings),
            "files": len({(f["source"]["repository"], f["source"]["path"]) for f in rule_findings}),
            "density_per_1000_files": round(len(rule_findings) * 1000 / denominator, 6),
            "repositories": _distribution(repositories),
            "file_types": _distribution(file_types),
            "diagnostic_shapes": _distribution(shapes),
            "origin": {"static": len(static), "authoritative": len(authoritative)},
            "fixability": {
                "fixable": sum(bool(f.get("fixable")) for f in rule_findings),
                "not_fixable": sum(not bool(f.get("fixable")) for f in rule_findings),
            },
            "review": review,
            "review_sampling": {
                "target": review_target,
                "sample_size": min(review_target, len(rule_findings)),
                "sample_fingerprints": stratified_review_sample(rule_findings, review_target),
                "repositories": sorted(set(repositories)),
                "file_types": sorted(set(file_types)),
                "diagnostic_shapes": sorted(set(shapes)),
            },
            "runtime_seconds": round(float((runtime_seconds or {}).get(rule_id, 0.0)), 6),
        }
    profile_totals = {}
    for profile, profile_rule_ids in PROFILE_RULES.items():
        selected = [f for f in ordered if f["rule_id"] in profile_rule_ids]
        profile_totals[profile] = {
            "rules": sorted(profile_rule_ids),
            "total_findings": len(selected),
            "files_with_findings": len({(f["source"]["repository"], f["source"]["path"]) for f in selected}),
            "findings_sha256": digest_findings(selected),
        }
    report = {
        "version": QUALITY_REPORT_VERSION,
        "fingerprint_version": FINGERPRINT_VERSION,
        "total_files": int(total_files) if total_files is not None else denominator,
        "total_findings": len(ordered),
        "profiles": profile_totals,
        "rules": rules,
    }
    report["pilot_evidence"] = evaluate_pilot_thresholds(report)
    return report


def quality_report_markdown(report: Mapping[str, Any]) -> str:
    """Render the quality report in a compact review-friendly form."""

    lines = [
        "# bbtidy lint quality report",
        "",
        "- Findings: {}".format(report.get("total_findings", 0)),
        "- Files: {}".format(report.get("total_files", 0)),
        "",
        "| Rule | Findings | Files | Density/1000 | Static | Authoritative | TP | FP | Unclear | Actionable |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for rule_id, rule in sorted(report.get("rules", {}).items()):
        review = rule.get("review", {})
        correctness = review.get("correctness", {})
        actionability = review.get("actionability", {})
        actionable = int(actionability.get("must_fix", 0)) + int(actionability.get("should_fix", 0))
        lines.append(
            "| {rule} | {total} | {files} | {density} | {static} | {authoritative} | {tp} | {fp} | {unclear} | {actionable} |".format(
                rule=rule_id,
                total=rule.get("total", 0),
                files=rule.get("files", 0),
                density=rule.get("density_per_1000_files", 0),
                static=rule.get("origin", {}).get("static", 0),
                authoritative=rule.get("origin", {}).get("authoritative", 0),
                tp=correctness.get("true_positive", 0),
                fp=correctness.get("false_positive", 0),
                unclear=correctness.get("unclear", 0),
                actionable=actionable,
            )
        )
    return "\n".join(lines) + "\n"


def evaluate_pilot_thresholds(
    report: Mapping[str, Any], pilot_metrics: Optional[Mapping[str, Any]] = None
) -> dict:
    """Evaluate the documented beta thresholds without inventing pilot data."""

    pilot_metrics = pilot_metrics or {}
    results = {}
    for key, limit in PILOT_THRESHOLDS.items():
        value = pilot_metrics.get(key)
        if value is None:
            results[key] = {"limit": limit, "value": None, "status": "not-run"}
        else:
            passing = value <= limit if "rate" not in key else value <= limit
            results[key] = {
                "limit": limit,
                "value": value,
                "status": "pass" if passing else "fail",
            }
    measured = [entry["status"] for entry in results.values() if entry["status"] != "not-run"]
    return {
        "thresholds": results,
        "status": "pass" if measured and all(status == "pass" for status in measured) and len(measured) == len(results) else "insufficient-evidence",
        "default_decision": "recommended-beta-candidate" if measured and all(status == "pass" for status in measured) and len(measured) == len(results) else "retain-all-and-collect-pilot-evidence",
    }


def review_summary(summary: Mapping[str, Any], baseline: Optional[Mapping[str, Any]]) -> dict:
    """Summarize human review coverage for the findings in ``summary``.

    Measurement and review remain separate: this function only joins the
    generated rule counts with review records when a baseline is available.
    Missing records are intentionally reported as unreviewed instead of being
    silently treated as clean.
    """

    rules = summary.get("rules", {})
    review_rules = {}
    if isinstance(baseline, Mapping):
        review = baseline.get("review")
        if isinstance(review, Mapping) and isinstance(review.get("rules"), Mapping):
            review_rules = review["rules"]

    active_rule_ids = sorted(
        rule_id
        for rule_id, rule in rules.items()
        if isinstance(rule, Mapping) and rule.get("count", 0) > 0
    )
    reviewed_rule_ids = []
    unreviewed_rule_ids = []
    true_positive = false_positive = unclear = 0
    must_fix = should_fix = context_dependent = policy_only = not_actionable = 0
    for rule_id in active_rule_ids:
        record = review_rules.get(rule_id)
        if not isinstance(record, Mapping):
            unreviewed_rule_ids.append(rule_id)
            continue
        status = record.get("status")
        if status in REVIEWED_RULE_STATUSES:
            reviewed_rule_ids.append(rule_id)
        else:
            unreviewed_rule_ids.append(rule_id)
        classifications = _review_classification(record)
        for field in ("true_positive", "false_positive", "unclear"):
            value = classifications["correctness"].get(field, 0)
            if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
                if field == "true_positive":
                    true_positive += value
                elif field == "false_positive":
                    false_positive += value
                else:
                    unclear += value
        actionability = classifications["actionability"]
        must_fix += actionability["must_fix"]
        should_fix += actionability["should_fix"]
        context_dependent += actionability["context_dependent"]
        policy_only += actionability["policy_only"]
        not_actionable += actionability["not_actionable"]

    return {
        "active_rules": len(active_rule_ids),
        "reviewed_rules": len(reviewed_rule_ids),
        "unreviewed_rules": len(unreviewed_rule_ids),
        "reviewed_rule_ids": reviewed_rule_ids,
        "unreviewed_rule_ids": unreviewed_rule_ids,
        "true_positive_samples": true_positive,
        "false_positive_samples": false_positive,
        "unclear_samples": unclear,
        "actionability_samples": {
            "must_fix": must_fix,
            "should_fix": should_fix,
            "context_dependent": context_dependent,
            "policy_only": policy_only,
            "not_actionable": not_actionable,
        },
    }


def review_policy_failures(
    summary: Mapping[str, Any], baseline: Optional[Mapping[str, Any]]
) -> list:
    """Return deterministic failures for active rules lacking review decisions."""

    if baseline is None:
        return ["lint-quality baseline is missing"]
    review = baseline.get("review")
    review_rules = review.get("rules", {}) if isinstance(review, Mapping) else {}
    tiered_sampling = isinstance(review, Mapping) and review.get("schema") == REVIEW_SCHEMA
    failures = []
    for rule_id in sorted(summary.get("rules", {})):
        measurement = summary["rules"][rule_id]
        if not isinstance(measurement, Mapping) or measurement.get("count", 0) == 0:
            continue
        record = review_rules.get(rule_id) if isinstance(review_rules, Mapping) else None
        if not isinstance(record, Mapping):
            failures.append("active rule {} has no review record".format(rule_id))
            continue
        status = record.get("status")
        if status not in REVIEWED_RULE_STATUSES:
            failures.append("active rule {} is {}".format(rule_id, status or "unreviewed"))
        if status in REVIEWED_RULE_STATUSES:
            count = measurement.get("count", 0)
            required_samples = minimum_review_samples(count) if tiered_sampling else min(5, count)
            if record.get("sample_size", 0) < required_samples:
                failures.append(
                    "active rule {} has only {} reviewed samples; at least 5 and {} required".format(
                        rule_id, record.get("sample_size", 0), required_samples
                    )
                )
            for field in ("repositories", "file_types", "diagnostic_shapes"):
                if not record.get(field):
                    failures.append(
                        "active rule {} has no {} review metadata".format(rule_id, field)
                    )
        classifications = _review_classification(record)
        false_positive = classifications["correctness"]["false_positive"]
        notes = record.get("notes", "")
        if not isinstance(notes, str):
            notes = ""
        if false_positive and not notes.strip():
            failures.append(
                "active rule {} has false-positive samples without remediation notes".format(
                    rule_id
                )
            )
        unclear = classifications["correctness"]["unclear"]
        false_positive = classifications["correctness"]["false_positive"]
        sample_size = record.get("sample_size", 0)
        review_is_v2 = isinstance(review, Mapping) and review.get("schema") == REVIEW_SCHEMA
        has_explicit_actionability = bool(record.get("sample_fingerprints")) or any(
            record.get("actionability", {}).get(key, 0)
            for key in ("must_fix", "should_fix", "context_dependent", "policy_only", "not_actionable")
        ) if isinstance(record.get("actionability"), Mapping) else False
        if sample_size and review_is_v2 and unclear / sample_size > 0.05:
            failures.append("active rule {} has more than 5% unclear samples".format(rule_id))
        actionable = classifications["actionability"]["must_fix"] + classifications["actionability"]["should_fix"]
        if sample_size and review_is_v2 and has_explicit_actionability and actionable / sample_size < 0.70:
            failures.append("active rule {} has fewer than 70% actionable samples".format(rule_id))
        if unclear and not notes.strip():
            failures.append(
                "active rule {} has unclear samples without review notes".format(rule_id)
            )
    return failures


def _baseline_exact_keys(value: Any, expected: Iterable[str], label: str) -> None:
    if not isinstance(value, Mapping) or set(value) != set(expected):
        raise LintBaselineError(
            "{} must contain exactly {}".format(label, ", ".join(sorted(expected)))
        )


def _baseline_integer(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise LintBaselineError("{} must be a non-negative integer".format(label))
    return value


def _baseline_digest(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA256_PATTERN.fullmatch(value):
        raise LintBaselineError("{} must be a lowercase SHA-256 digest".format(label))
    return value


def _baseline_severity_counts(value: Any, total: int, label: str) -> dict:
    if not isinstance(value, Mapping) or set(value) != set(SEVERITIES):
        raise LintBaselineError(
            "{} must contain exactly {}".format(label, ", ".join(SEVERITIES))
        )
    counts = {
        severity: _baseline_integer(value[severity], "{}.{}".format(label, severity))
        for severity in SEVERITIES
    }
    if sum(counts.values()) != total:
        raise LintBaselineError("{} must total {}".format(label, total))
    return counts


def _baseline_relative_path(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise LintBaselineError("{} must be a non-empty relative path".format(label))
    normalized = value.replace("\\", "/")
    if normalized.startswith("/") or re.match(r"^[A-Za-z]:/", normalized):
        raise LintBaselineError("{} must be a non-empty relative path".format(label))
    if normalized != "." and any(part in ("", ".", "..") for part in normalized.split("/")):
        raise LintBaselineError("{} must not contain traversal components".format(label))
    return normalized


def _manifest_corpus_identity(manifest: Mapping[str, Any]) -> dict:
    corpus_id = manifest.get("id")
    if not isinstance(corpus_id, str) or not corpus_id:
        raise LintBaselineError("manifest has no stable corpus ID")

    repositories = []
    seen_repositories = set()
    for repository in manifest.get("repositories", []):
        if not isinstance(repository, Mapping):
            raise LintBaselineError("manifest contains an invalid repository")
        name = repository.get("name")
        revision = repository.get("revision")
        if (
            not isinstance(name, str)
            or not name
            or name in seen_repositories
            or not isinstance(revision, str)
            or not REVISION_PATTERN.fullmatch(revision)
        ):
            raise LintBaselineError(
                "baseline corpus requires unique repositories with pinned revisions"
            )
        seen_repositories.add(name)
        repositories.append({"name": name, "revision": revision})

    layers = []
    seen_layers = set()
    for layer in manifest.get("layers", []):
        if not isinstance(layer, Mapping):
            raise LintBaselineError("manifest contains an invalid layer")
        name = layer.get("name")
        repository = layer.get("repository")
        path = layer.get("path")
        if (
            not isinstance(name, str)
            or not name
            or name in seen_layers
            or not isinstance(repository, str)
            or repository not in seen_repositories
        ):
            raise LintBaselineError("baseline corpus contains an invalid layer identity")
        path = _baseline_relative_path(path, "layer path")
        seen_layers.add(name)
        layers.append({"name": name, "repository": repository, "path": path})

    if not repositories or not layers:
        raise LintBaselineError("baseline corpus must contain repositories and layers")
    return {
        "id": corpus_id,
        "repositories": sorted(repositories, key=lambda item: item["name"]),
        "layers": sorted(
            layers,
            key=lambda item: (item["name"], item["repository"], item["path"]),
        ),
    }


def _baseline_contract() -> dict:
    return {
        "report_version": REPORT_VERSION,
        "fingerprint_version": FINGERPRINT_VERSION,
        "source_state": "formatted",
        "configuration": "none",
        "scope": "manifest-layers",
    }


def _default_review_rule(status: str = "unreviewed") -> dict:
    return {
        "status": status,
        "sample_size": 0,
        "true_positive": 0,
        "false_positive": 0,
        "unclear": 0,
        "notes": "",
        "repositories": [],
        "file_types": [],
        "diagnostic_shapes": [],
        "correctness": {"true_positive": 0, "false_positive": 0, "unclear": 0},
        "actionability": {
            "must_fix": 0,
            "should_fix": 0,
            "context_dependent": 0,
            "policy_only": 0,
            "not_actionable": 0,
        },
        "sample_fingerprints": [],
    }


def minimum_review_samples(count: int) -> int:
    """Require more than a token sample for large active rule populations."""

    if not count:
        return 0
    if count <= 5:
        return count
    if count <= 25:
        return 8
    if count <= 100:
        return 12
    if count <= 500:
        return 20
    return 30


def baseline_from_summary(
    manifest: Mapping[str, Any], summary: Mapping[str, Any], review: Optional[Mapping[str, Any]] = None
) -> dict:
    """Build schema-versioned baseline data from normalized measurements only."""

    corpus = _manifest_corpus_identity(manifest)
    if summary.get("corpus_id") not in (None, corpus["id"]):
        raise LintBaselineError("summary corpus ID does not match manifest")
    measurement_rules = {}
    for rule_id in sorted(summary.get("rules", {})):
        rule = summary["rules"][rule_id]
        measurement_rules[rule_id] = {
            "count": rule["count"],
            "files": rule["files"],
            "findings_sha256": rule["findings_sha256"],
            "severity_counts": dict(rule["severity_counts"]),
        }

    if review is None:
        review_value = {
            "schema": REVIEW_SCHEMA,
            "status": "unreviewed",
            "rules": {
                rule_id: _default_review_rule()
                for rule_id in sorted(measurement_rules)
            },
        }
    else:
        review_value = deepcopy(dict(review))

    return {
        "schema": BASELINE_SCHEMA,
        "corpus": corpus,
        "lint_contract": _baseline_contract(),
        "measurement": {
            "total_findings": summary["total_findings"],
            "findings_sha256": summary["findings_sha256"],
            "files_with_findings": summary["files_with_findings"],
            "severity_counts": dict(summary["severity_counts"]),
            "rules": measurement_rules,
        },
        "review": review_value,
    }


def baseline_for_update(
    manifest: Mapping[str, Any],
    summary: Mapping[str, Any],
    previous: Optional[Mapping[str, Any]] = None,
) -> dict:
    """Create an explicit-update baseline without inventing review decisions.

    Review metadata for an unchanged rule is retained when its measured
    fingerprint is identical. Any new or changed rule receives the generated
    ``unreviewed`` record, so a baseline update cannot silently bless a
    changed diagnostic population.
    """

    generated = baseline_from_summary(manifest, summary)
    if previous is None:
        return generated
    try:
        validate_lint_baseline(previous, manifest)
    except LintBaselineError:
        return generated

    previous_rules = previous["measurement"]["rules"]
    generated_rules = generated["measurement"]["rules"]
    previous_review_rules = previous["review"]["rules"]
    legacy_review = "schema" not in previous["review"]
    retained = {}
    for rule_id, generated_rule in generated_rules.items():
        previous_rule = previous_rules.get(rule_id)
        previous_review = previous_review_rules.get(rule_id)
        if (
            previous_rule == generated_rule
            and isinstance(previous_review, Mapping)
        ):
            retained[rule_id] = deepcopy(dict(previous_review))
        else:
            default_review = _default_review_rule()
            if legacy_review:
                default_review = {
                    key: default_review[key]
                    for key in BASELINE_REVIEW_RULE_LEGACY_KEYS
                }
            retained[rule_id] = default_review

    active_unreviewed = any(
        generated_rules[rule_id]["count"] > 0
        and retained[rule_id].get("status") not in REVIEWED_RULE_STATUSES
        for rule_id in generated_rules
    )
    previous_status = previous["review"].get("status", "unreviewed")
    generated["review"] = {
        **({} if legacy_review else {"schema": REVIEW_SCHEMA}),
        "status": "unreviewed" if active_unreviewed else previous_status,
        "rules": retained,
    }
    return generated


def _validate_review_rule(rule: Any, count: int, rule_id: str, tiered_sampling: bool = True) -> None:
    if "correctness" in rule or "actionability" in rule:
        _baseline_exact_keys(rule, BASELINE_REVIEW_RULE_KEYS, "review rule {}".format(rule_id))
    else:
        _baseline_exact_keys(rule, BASELINE_REVIEW_RULE_LEGACY_KEYS, "review rule {}".format(rule_id))
    status = rule["status"]
    if status not in BASELINE_REVIEW_STATUSES:
        raise LintBaselineError("review rule {} has an unknown status".format(rule_id))
    sample_size = _baseline_integer(rule["sample_size"], "review rule {} sample_size".format(rule_id))
    classifications = _review_classification(rule)
    true_positive = _baseline_integer(classifications["correctness"]["true_positive"], "review rule {} true_positive".format(rule_id))
    false_positive = _baseline_integer(classifications["correctness"]["false_positive"], "review rule {} false_positive".format(rule_id))
    unclear = _baseline_integer(classifications["correctness"]["unclear"], "review rule {} unclear".format(rule_id))
    if true_positive + false_positive + unclear != sample_size:
        raise LintBaselineError("review rule {} classifications do not total sample_size".format(rule_id))
    if sample_size > count:
        raise LintBaselineError("review rule {} samples more findings than measured".format(rule_id))
    if not isinstance(rule["notes"], str):
        raise LintBaselineError("review rule {} notes must be a string".format(rule_id))
    if "correctness" in rule:
        for field in ("correctness", "actionability"):
            values = rule[field]
            if not isinstance(values, Mapping):
                raise LintBaselineError("review rule {} {} must be an object".format(rule_id, field))
        for field in ("true_positive", "false_positive", "unclear"):
            _baseline_integer(rule["correctness"].get(field), "review rule {} correctness.{}".format(rule_id, field))
        for field in ("must_fix", "should_fix", "context_dependent", "policy_only", "not_actionable"):
            _baseline_integer(rule["actionability"].get(field), "review rule {} actionability.{}".format(rule_id, field))
        if not isinstance(rule["sample_fingerprints"], list) or any(
            not isinstance(value, str) or not value.strip() for value in rule["sample_fingerprints"]
        ) or len(set(rule["sample_fingerprints"])) != len(rule["sample_fingerprints"]):
            raise LintBaselineError("review rule {} sample_fingerprints must be unique strings".format(rule_id))
    for field in ("repositories", "file_types", "diagnostic_shapes"):
        values = rule[field]
        if (
            not isinstance(values, list)
            or any(not isinstance(value, str) or not value.strip() for value in values)
            or len(set(values)) != len(values)
        ):
            raise LintBaselineError(
                "review rule {} {} must be a list of unique strings".format(
                    rule_id, field
                )
            )
    if count and rule["status"] in REVIEWED_RULE_STATUSES:
        required_samples = minimum_review_samples(count) if tiered_sampling else min(5, count)
        if rule["sample_size"] < required_samples:
            raise LintBaselineError(
                "review rule {} needs at least 5 and {} reviewed samples for {} findings".format(
                    rule_id, required_samples, count
                )
            )
        if not rule["repositories"] or not rule["file_types"] or not rule["diagnostic_shapes"]:
            raise LintBaselineError(
                "review rule {} must record repositories, file types, and diagnostic shapes".format(
                    rule_id
                )
            )
    if (false_positive or unclear) and not rule["notes"].strip():
        raise LintBaselineError(
            "review rule {} false-positive or unclear samples require notes".format(
                rule_id
            )
        )
    if status == "unreviewed" and sample_size != 0:
        raise LintBaselineError("unreviewed rule {} must have zero samples".format(rule_id))
    if status in {"reviewed", "accepted-known-limitations"} and count and sample_size == 0 and not rule["notes"]:
        raise LintBaselineError("active rule {} needs a sample or explanatory notes".format(rule_id))


def validate_lint_baseline(
    baseline: Any, manifest: Optional[Mapping[str, Any]] = None
) -> dict:
    """Strictly validate a schema-versioned lint baseline."""

    _baseline_exact_keys(baseline, BASELINE_TOP_LEVEL_KEYS, "baseline")
    if isinstance(baseline["schema"], bool) or baseline["schema"] != BASELINE_SCHEMA:
        raise LintBaselineError("baseline must use schema 1")

    corpus = baseline["corpus"]
    _baseline_exact_keys(corpus, BASELINE_CORPUS_KEYS, "baseline corpus")
    if not isinstance(corpus["id"], str) or not corpus["id"]:
        raise LintBaselineError("baseline corpus ID must be a non-empty string")
    repository_names = set()
    for repository in corpus["repositories"]:
        _baseline_exact_keys(repository, BASELINE_REPOSITORY_KEYS, "baseline repository")
        name = repository["name"]
        if not isinstance(name, str) or not name or name in repository_names:
            raise LintBaselineError("baseline repository names must be unique")
        if not isinstance(repository["revision"], str) or not REVISION_PATTERN.fullmatch(repository["revision"]):
            raise LintBaselineError("baseline repository revisions must be full commit IDs")
        repository_names.add(name)
    if not corpus["repositories"]:
        raise LintBaselineError("baseline corpus must contain repositories")
    if corpus["repositories"] != sorted(corpus["repositories"], key=lambda item: item["name"]):
        raise LintBaselineError("baseline repositories must be in deterministic order")
    layer_names = set()
    for layer in corpus["layers"]:
        _baseline_exact_keys(layer, BASELINE_LAYER_KEYS, "baseline layer")
        name = layer["name"]
        if not isinstance(name, str) or not name or name in layer_names:
            raise LintBaselineError("baseline layer names must be unique")
        if layer["repository"] not in repository_names:
            raise LintBaselineError("baseline layer references an unknown repository")
        _baseline_relative_path(layer["path"], "baseline layer path")
        layer_names.add(name)
    if not corpus["layers"]:
        raise LintBaselineError("baseline corpus must contain layers")
    if corpus["layers"] != sorted(
        corpus["layers"],
        key=lambda item: (item["name"], item["repository"], item["path"]),
    ):
        raise LintBaselineError("baseline layers must be in deterministic order")
    if manifest is not None:
        try:
            expected_corpus = _manifest_corpus_identity(manifest)
        except LintBaselineError as error:
            raise LintBaselineError("manifest cannot provide a pinned baseline identity: {}".format(error)) from error
        if corpus != expected_corpus:
            raise LintBaselineError("baseline corpus identity does not match manifest")

    contract = baseline["lint_contract"]
    _baseline_exact_keys(contract, BASELINE_CONTRACT_KEYS, "lint contract")
    if isinstance(contract["report_version"], bool) or contract["report_version"] != REPORT_VERSION:
        raise LintBaselineError("baseline has an unsupported report version")
    if isinstance(contract["fingerprint_version"], bool) or contract["fingerprint_version"] != FINGERPRINT_VERSION:
        raise LintBaselineError("baseline has an unsupported fingerprint version")
    for field in ("source_state", "configuration", "scope"):
        if not isinstance(contract[field], str) or not contract[field]:
            raise LintBaselineError("lint contract {} must be a non-empty string".format(field))
    if contract["source_state"] not in {"original", "formatted"}:
        raise LintBaselineError("lint contract source_state is unsupported")

    measurement = baseline["measurement"]
    _baseline_exact_keys(measurement, BASELINE_MEASUREMENT_KEYS, "measurement")
    total = _baseline_integer(measurement["total_findings"], "measurement.total_findings")
    files = _baseline_integer(measurement["files_with_findings"], "measurement.files_with_findings")
    _baseline_digest(measurement["findings_sha256"], "measurement.findings_sha256")
    _baseline_severity_counts(measurement["severity_counts"], total, "measurement.severity_counts")
    if not isinstance(measurement["rules"], Mapping):
        raise LintBaselineError("measurement.rules must be an object")
    if list(measurement["rules"]) != sorted(measurement["rules"]):
        raise LintBaselineError("measurement rules must be in deterministic order")
    rule_total = 0
    for rule_id, rule in measurement["rules"].items():
        if rule_id not in KNOWN_RULE_IDS:
            raise LintBaselineError("measurement contains unknown rule {}".format(rule_id))
        _baseline_exact_keys(rule, BASELINE_RULE_MEASUREMENT_KEYS, "measurement rule {}".format(rule_id))
        count = _baseline_integer(rule["count"], "measurement rule {} count".format(rule_id))
        rule_files = _baseline_integer(rule["files"], "measurement rule {} files".format(rule_id))
        if rule_files > files:
            raise LintBaselineError("measurement rule {} has too many files".format(rule_id))
        _baseline_digest(rule["findings_sha256"], "measurement rule {} findings_sha256".format(rule_id))
        _baseline_severity_counts(rule["severity_counts"], count, "measurement rule {} severity_counts".format(rule_id))
        rule_total += count
    if rule_total != total:
        raise LintBaselineError("measurement rule counts do not total findings")

    review = baseline["review"]
    if isinstance(review, Mapping) and "schema" in review:
        _baseline_exact_keys(review, BASELINE_REVIEW_V2_KEYS, "review")
        if review["schema"] != REVIEW_SCHEMA:
            raise LintBaselineError("review must use schema 2")
    else:
        _baseline_exact_keys(review, BASELINE_REVIEW_KEYS, "review")
    if review["status"] not in BASELINE_REVIEW_STATUSES:
        raise LintBaselineError("baseline has an unknown review status")
    if not isinstance(review["rules"], Mapping):
        raise LintBaselineError("review.rules must be an object")
    if list(review["rules"]) != sorted(review["rules"]):
        raise LintBaselineError("review rules must be in deterministic order")
    for rule_id in review["rules"]:
        if rule_id not in measurement["rules"]:
            raise LintBaselineError("review refers to a nonexistent rule {}".format(rule_id))
    for rule_id, rule in measurement["rules"].items():
        if rule_id not in review["rules"]:
            if rule["count"]:
                raise LintBaselineError("active rule {} has no review record".format(rule_id))
            continue
        _validate_review_rule(
            review["rules"][rule_id],
            rule["count"],
            rule_id,
            tiered_sampling=isinstance(review, Mapping) and review.get("schema") == REVIEW_SCHEMA,
        )
    return baseline


def load_lint_baseline(path: Path, manifest: Mapping[str, Any]) -> dict:
    """Read and validate a baseline associated with a pinned manifest."""

    try:
        baseline = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise LintBaselineError("could not read lint baseline {}: {}".format(path, error)) from error
    return validate_lint_baseline(baseline, manifest)


def canonical_baseline_bytes(baseline: Mapping[str, Any]) -> bytes:
    """Serialize a baseline as deterministic, readable UTF-8 JSON."""

    validate_lint_baseline(baseline)
    return (
        json.dumps(
            baseline,
            ensure_ascii=False,
            sort_keys=True,
            indent=2,
            separators=(",", ": "),
        )
        + "\n"
    ).encode("utf-8")


def _comparison_result(corpus_id: Any) -> dict:
    return {
        "status": "invalid",
        "corpus_id": corpus_id,
        "total_delta": 0,
        "digest_changed": False,
        "severity_changes": {},
        "rules_added": [],
        "rules_removed": [],
        "rules_changed": {},
        "contract_changes": [],
        "review_failures": [],
    }


def compare_lint_baseline(
    baseline: Any, summary: Mapping[str, Any], manifest: Optional[Mapping[str, Any]] = None
) -> dict:
    """Compare current normalized measurements with a baseline deterministically."""

    corpus_id = summary.get("corpus_id")
    if manifest is not None:
        corpus_id = manifest.get("id", corpus_id)
    result = _comparison_result(corpus_id)
    if baseline is None:
        result["errors"] = ["baseline is missing"]
        return result
    try:
        validate_lint_baseline(baseline, manifest)
        current = baseline_from_summary(
            manifest or baseline["corpus"], summary
        )
    except LintBaselineError as error:
        result["errors"] = [str(error)]
        return result

    contract_fields = ("corpus", "lint_contract")
    for field in contract_fields:
        if baseline[field] != current[field]:
            result["contract_changes"].append(
                {
                    "field": field,
                    "baseline": baseline[field],
                    "current": current[field],
                }
            )

    previous_measurement = baseline["measurement"]
    current_measurement = current["measurement"]
    result["total_delta"] = current_measurement["total_findings"] - previous_measurement["total_findings"]
    result["digest_changed"] = (
        current_measurement["findings_sha256"] != previous_measurement["findings_sha256"]
    )
    for severity in SEVERITIES:
        old = previous_measurement["severity_counts"][severity]
        new = current_measurement["severity_counts"][severity]
        if old != new:
            result["severity_changes"][severity] = {"baseline": old, "current": new}

    previous_rules = previous_measurement["rules"]
    current_rules = current_measurement["rules"]
    active_previous = {rule_id for rule_id, rule in previous_rules.items() if rule["count"]}
    active_current = {rule_id for rule_id, rule in current_rules.items() if rule["count"]}
    result["rules_added"] = sorted(active_current - active_previous)
    result["rules_removed"] = sorted(active_previous - active_current)
    for rule_id in sorted(set(previous_rules) | set(current_rules)):
        old = previous_rules.get(rule_id)
        new = current_rules.get(rule_id)
        if old == new:
            continue
        result["rules_changed"][rule_id] = {
            "baseline": old,
            "current": new,
        }

    changed = bool(
        result["contract_changes"]
        or result["total_delta"]
        or result["digest_changed"]
        or result["severity_changes"]
        or result["rules_added"]
        or result["rules_removed"]
        or result["rules_changed"]
    )
    result["status"] = "changed" if changed else "matched"
    result["review_failures"] = review_policy_failures(summary, baseline)
    result["review"] = review_summary(summary, baseline)
    return result
