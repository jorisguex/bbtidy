"""Pure deterministic normalization for ``bbtidy check --output json``.

This module deliberately has no subprocess, filesystem mutation, network, or
baseline-policy responsibilities.  Callers provide the parsed JSON report and
the repository roots that define the corpus path identity.
"""

from dataclasses import dataclass
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import re
from typing import Any, Iterable, Mapping, Optional, Sequence, Tuple


FINGERPRINT_VERSION = 1
REPORT_VERSION = 1
SEVERITIES = ("info", "warning", "error")
KNOWN_RULE_IDS = tuple("BBT{:03d}".format(number) for number in range(1, 38))
BASELINE_SCHEMA = 1
BASELINE_REVIEW_STATUSES = frozenset(
    {"unreviewed", "reviewed", "accepted-known-limitations", "not-applicable"}
)
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
BASELINE_REVIEW_RULE_KEYS = frozenset(
    {"status", "sample_size", "true_positive", "false_positive", "unclear", "notes"}
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
    }


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


def _validate_review_rule(rule: Any, count: int, rule_id: str) -> None:
    _baseline_exact_keys(rule, BASELINE_REVIEW_RULE_KEYS, "review rule {}".format(rule_id))
    status = rule["status"]
    if status not in BASELINE_REVIEW_STATUSES:
        raise LintBaselineError("review rule {} has an unknown status".format(rule_id))
    sample_size = _baseline_integer(rule["sample_size"], "review rule {} sample_size".format(rule_id))
    true_positive = _baseline_integer(rule["true_positive"], "review rule {} true_positive".format(rule_id))
    false_positive = _baseline_integer(rule["false_positive"], "review rule {} false_positive".format(rule_id))
    unclear = _baseline_integer(rule["unclear"], "review rule {} unclear".format(rule_id))
    if true_positive + false_positive + unclear != sample_size:
        raise LintBaselineError("review rule {} classifications do not total sample_size".format(rule_id))
    if sample_size > count:
        raise LintBaselineError("review rule {} samples more findings than measured".format(rule_id))
    if not isinstance(rule["notes"], str):
        raise LintBaselineError("review rule {} notes must be a string".format(rule_id))
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
        _validate_review_rule(review["rules"][rule_id], rule["count"], rule_id)
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
    return result
