"""Pure deterministic normalization for ``bbtidy check --output json``.

This module deliberately has no subprocess, filesystem mutation, network, or
baseline-policy responsibilities.  Callers provide the parsed JSON report and
the repository roots that define the corpus path identity.
"""

from dataclasses import dataclass
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


class LintNormalizationError(RuntimeError):
    """A lint report cannot be converted to a canonical finding."""


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
