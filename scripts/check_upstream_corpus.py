#!/usr/bin/env python3
"""Check bbtidy against versioned OpenEmbedded-Core compatibility corpora."""

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = (
    PROJECT_ROOT / "tests" / "upstream-corpora" / "yocto-5.0-scarthgap.json"
)
METADATA_EXTENSIONS = {".bb", ".bbappend", ".bbclass", ".conf", ".inc"}
FUNCTION_START = re.compile(r"^[^ \t#\r\n].*\(\s*\)\s*\{\s*(?:#.*)?(?:\r?\n)?$")
PYTHON_DEF_START = re.compile(r"^def\s+[A-Za-z_][A-Za-z0-9_]*\s*\(.*\)\s*:")
REVISION = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
LINT_REPORT_VERSION = 1
LINT_BASELINE_SCHEMA = 1
LINT_SEVERITIES = ("info", "warning", "error")
LINT_RULE_IDS = tuple("BBT{:03d}".format(number) for number in range(1, 38))
LINT_BASELINE_DIRECTORY = "lint-baselines"
BASELINE_METRIC_FIELDS = (
    "files",
    "structured_nodes",
    "total_nodes",
    "trivia_nodes",
    "unknown_bytes",
    "unknown_nodes",
    "version",
)
SEMANTIC_VARIABLE = re.compile(
    r"^(?:export\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_:+.\-${}]*)=(?P<value>.*)$"
)


class CompatibilityError(RuntimeError):
    """A reproducible upstream compatibility check failed."""


def workflow_command_value(value):
    return str(value).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def format_idempotence_command(bbtidy, inputs):
    return [bbtidy, "format", "--check"] + inputs


def lint_command(bbtidy, inputs):
    return [bbtidy, "check", "--output", "json", "--fail-on", "never"] + inputs


def report_error(error):
    message = "error: {}".format(error)
    print(message, file=sys.stderr)
    if os.environ.get("GITHUB_ACTIONS") == "true":
        print(
            "::error title=Upstream compatibility failed::{}".format(
                workflow_command_value(message)
            )
        )


def report_warning(message):
    print("warning: {}".format(message), file=sys.stderr)
    if os.environ.get("GITHUB_ACTIONS") == "true":
        print(
            "::warning title=Upstream compatibility warning::{}".format(
                workflow_command_value(message)
            )
        )


def canonical_json(value):
    """Serialize JSON values in the format used for reproducible fingerprints."""
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def json_digest(value):
    return hashlib.sha256(canonical_json(value)).hexdigest()


def _required_field(value, field, kind=None):
    if not isinstance(value, dict) or field not in value:
        raise CompatibilityError(
            "lint report diagnostic is missing required field {!r}".format(field)
        )
    result = value[field]
    if kind is not None and not isinstance(result, kind):
        raise CompatibilityError(
            "lint report field {!r} has the wrong type".format(field)
        )
    return result


def _lint_integer(value, field, minimum=0):
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise CompatibilityError(
            "lint report field {!r} must be an integer >= {}".format(field, minimum)
        )
    return value


def normalize_lint_path(path, corpus_roots):
    if not isinstance(path, str) or not path:
        raise CompatibilityError("lint diagnostic path must be a non-empty string")
    candidate = Path(path)
    if not candidate.is_absolute():
        candidate = Path.cwd() / candidate
    candidate = candidate.resolve(strict=False)
    roots = sorted(
        ((repository, root.resolve(strict=False)) for repository, root in corpus_roots),
        key=lambda item: len(str(item[1])),
        reverse=True,
    )
    for repository, root in roots:
        try:
            relative = candidate.relative_to(root)
        except ValueError:
            continue
        if relative == Path(".") or any(part in {"", ".", ".."} for part in relative.parts):
            break
        return repository, relative.as_posix()
    raise CompatibilityError(
        "lint diagnostic path cannot be normalized into the corpus: {}".format(path)
    )


def _normalize_lint_fix(fix, diagnostic_index, fix_index):
    if not isinstance(fix, dict):
        raise CompatibilityError(
            "lint diagnostic {} fix {} is not an object".format(
                diagnostic_index, fix_index
            )
        )
    start_byte = _lint_integer(
        _required_field(fix, "start_byte"), "fix.start_byte"
    )
    end_byte = _lint_integer(_required_field(fix, "end_byte"), "fix.end_byte")
    if end_byte < start_byte:
        raise CompatibilityError("lint fix end_byte precedes start_byte")
    replacement = _required_field(fix, "replacement", str)
    message = _required_field(fix, "message", str)
    return {
        "start_byte": start_byte,
        "end_byte": end_byte,
        "replacement": replacement,
        "message": message,
    }


def normalize_lint_report(report_text, corpus_id, corpus_roots):
    """Validate bbtidy's JSON report and return canonical, machine-independent findings."""
    try:
        report = json.loads(report_text)
    except (TypeError, UnicodeError, json.JSONDecodeError) as error:
        raise CompatibilityError("bbtidy returned malformed lint JSON: {}".format(error)) from error
    if not isinstance(report, dict):
        raise CompatibilityError("bbtidy lint report must be a JSON object")
    if isinstance(report.get("version"), bool) or report.get("version") != LINT_REPORT_VERSION:
        raise CompatibilityError(
            "unsupported bbtidy lint report version: {}".format(report.get("version"))
        )
    diagnostics = report.get("diagnostics")
    if not isinstance(diagnostics, list):
        raise CompatibilityError("bbtidy lint report diagnostics must be a list")

    findings = []
    for diagnostic_index, diagnostic in enumerate(diagnostics):
        if not isinstance(diagnostic, dict):
            raise CompatibilityError(
                "lint diagnostic {} is not an object".format(diagnostic_index)
            )
        path = _required_field(diagnostic, "path", str)
        repository, relative_path = normalize_lint_path(path, corpus_roots)
        rule_id = _required_field(diagnostic, "rule_id", str)
        if rule_id not in LINT_RULE_IDS:
            raise CompatibilityError("lint report contains unknown rule ID: {}".format(rule_id))
        severity = _required_field(diagnostic, "severity", str)
        if severity not in LINT_SEVERITIES:
            raise CompatibilityError("lint report contains unknown severity: {}".format(severity))
        line = _lint_integer(_required_field(diagnostic, "line"), "line", 1)
        column = _lint_integer(_required_field(diagnostic, "column"), "column", 1)
        end_line = _lint_integer(_required_field(diagnostic, "end_line"), "end_line", 1)
        end_column = _lint_integer(
            _required_field(diagnostic, "end_column"), "end_column", 1
        )
        if (end_line, end_column) < (line, column):
            raise CompatibilityError("lint diagnostic end range precedes start range")
        message = _required_field(diagnostic, "message", str)
        fixable = _required_field(diagnostic, "fixable")
        if not isinstance(fixable, bool):
            raise CompatibilityError("lint report field 'fixable' must be a boolean")
        fixes = _required_field(diagnostic, "fixes")
        if not isinstance(fixes, list):
            raise CompatibilityError("lint report field 'fixes' must be a list")
        normalized_fixes = [
            _normalize_lint_fix(fix, diagnostic_index, fix_index)
            for fix_index, fix in enumerate(fixes)
        ]
        normalized_fixes.sort(
            key=lambda fix: (
                fix["start_byte"],
                fix["end_byte"],
                fix["message"],
                fix["replacement"],
            )
        )
        finding = {
            "corpus_id": corpus_id,
            "repository": repository,
            "path": relative_path,
            "rule_id": rule_id,
            "severity": severity,
            "range": {
                "start_line": line,
                "start_column": column,
                "end_line": end_line,
                "end_column": end_column,
            },
            "message": message,
            "fixable": fixable,
            "fixes": normalized_fixes,
        }
        if "help" in diagnostic:
            help_text = diagnostic["help"]
            if help_text is not None and not isinstance(help_text, str):
                raise CompatibilityError("lint report field 'help' must be a string or null")
            if help_text is not None:
                finding["help"] = help_text
        findings.append(finding)

    findings.sort(
        key=lambda finding: (
            finding["repository"],
            finding["path"],
            finding["range"]["start_line"],
            finding["range"]["start_column"],
            finding["range"]["end_line"],
            finding["range"]["end_column"],
            finding["rule_id"],
            finding["message"],
            canonical_json(finding),
        )
    )
    return findings


def lint_finding_digest(finding):
    return json_digest(finding)


def _default_lint_review(status="unreviewed"):
    return {
        "status": status,
        "sample_size": 0,
        "true_positive": 0,
        "false_positive": 0,
        "unclear": 0,
        "notes": "",
    }


def _review_for_finding_digest(rule_id, count, findings_sha256, baseline):
    if not baseline:
        return _default_lint_review()
    entry = baseline.get("rules", {}).get(rule_id)
    if not entry or entry.get("count") != count or entry.get("findings_sha256") != findings_sha256:
        return _default_lint_review()
    review = entry.get("review")
    return dict(review) if isinstance(review, dict) else _default_lint_review()


def summarize_lint_findings(corpus_id, findings, baseline=None):
    by_rule = {}
    for finding in findings:
        by_rule.setdefault(finding["rule_id"], []).append(finding)
    rules = {}
    for rule_id in sorted(by_rule):
        rule_findings = by_rule[rule_id]
        finding_digests = sorted(lint_finding_digest(finding) for finding in rule_findings)
        digest = json_digest(rule_findings)
        review = _review_for_finding_digest(rule_id, len(rule_findings), digest, baseline)
        rules[rule_id] = {
            "count": len(rule_findings),
            "findings_sha256": digest,
            "finding_digests": finding_digests,
            "review": review,
        }
    severity_counts = {severity: 0 for severity in LINT_SEVERITIES}
    for finding in findings:
        severity_counts[finding["severity"]] += 1
    files = sorted(
        "{}/{}".format(finding["repository"], finding["path"])
        for finding in findings
    )
    files = sorted(set(files))
    reviewed = [
        rule
        for rule in rules.values()
        if rule["review"].get("status") == "reviewed"
    ]
    unreviewed = [
        rule
        for rule in rules.values()
        if rule["review"].get("status") != "reviewed"
    ]
    sampled = [
        rule["review"]
        for rule in rules.values()
        if rule["review"].get("status") == "reviewed"
    ]
    return {
        "schema": 1,
        "corpus_id": corpus_id,
        "total_findings": len(findings),
        "findings_sha256": json_digest(findings),
        "severity_counts": severity_counts,
        "rules": rules,
        "files_with_findings": files,
        "rules_with_findings": sorted(rules),
        "reviewed_rule_count": len(reviewed),
        "unreviewed_rule_count": len(unreviewed),
        "true_positive_sample_total": sum(item.get("true_positive", 0) for item in sampled),
        "false_positive_sample_total": sum(item.get("false_positive", 0) for item in sampled),
        "unclear_sample_total": sum(item.get("unclear", 0) for item in sampled),
    }


def validate_lint_review(review, count, rule_id):
    if not isinstance(review, dict):
        raise CompatibilityError("lint baseline rule {} has no review object".format(rule_id))
    status = review.get("status")
    if status not in {"reviewed", "unreviewed", "not-applicable"}:
        raise CompatibilityError("lint baseline rule {} has an invalid review status".format(rule_id))
    values = {}
    for field in ("sample_size", "true_positive", "false_positive", "unclear"):
        values[field] = _lint_integer(review.get(field), "review.{}".format(field))
    if values["true_positive"] + values["false_positive"] + values["unclear"] != values["sample_size"]:
        raise CompatibilityError("lint baseline rule {} has inconsistent review counts".format(rule_id))
    if values["sample_size"] > count:
        raise CompatibilityError("lint baseline rule {} samples more findings than it contains".format(rule_id))
    if not isinstance(review.get("notes"), str):
        raise CompatibilityError("lint baseline rule {} review notes must be a string".format(rule_id))
    if values["false_positive"] and (
        not isinstance(review.get("false_positive_decision"), str)
        or not review.get("false_positive_decision")
        or not review.get("notes")
    ):
        raise CompatibilityError(
            "lint baseline rule {} false positives require an explicit remediation decision".format(
                rule_id
            )
        )
    if status == "not-applicable" and count:
        raise CompatibilityError("lint baseline rule {} cannot be not-applicable with findings".format(rule_id))
    return review


def validate_lint_baseline(baseline, corpus_id):
    if (
        not isinstance(baseline, dict)
        or isinstance(baseline.get("schema"), bool)
        or baseline.get("schema") != LINT_BASELINE_SCHEMA
    ):
        raise CompatibilityError("lint baseline must use schema 1")
    if baseline.get("corpus_id") != corpus_id:
        raise CompatibilityError("lint baseline corpus ID does not match manifest")
    total = _lint_integer(baseline.get("total_findings"), "total_findings")
    digest = baseline.get("findings_sha256")
    if not isinstance(digest, str) or not SHA256.fullmatch(digest):
        raise CompatibilityError("lint baseline has an invalid complete findings digest")
    severity_counts = baseline.get("severity_counts")
    if not isinstance(severity_counts, dict) or set(severity_counts) != set(LINT_SEVERITIES):
        raise CompatibilityError("lint baseline has invalid severity counts")
    if sum(_lint_integer(severity_counts.get(severity), "severity_counts.{}".format(severity)) for severity in LINT_SEVERITIES) != total:
        raise CompatibilityError("lint baseline severity counts do not total the findings count")
    rules = baseline.get("rules")
    if not isinstance(rules, dict):
        raise CompatibilityError("lint baseline rules must be an object")
    total_rule_findings = 0
    for rule_id, rule in rules.items():
        if rule_id not in LINT_RULE_IDS or not isinstance(rule, dict):
            raise CompatibilityError("lint baseline contains unknown rule ID: {}".format(rule_id))
        count = _lint_integer(rule.get("count"), "rules.{}.count".format(rule_id))
        rule_digest = rule.get("findings_sha256")
        if not isinstance(rule_digest, str) or not SHA256.fullmatch(rule_digest):
            raise CompatibilityError("lint baseline rule {} has an invalid digest".format(rule_id))
        finding_digests = rule.get("finding_digests", [])
        if not isinstance(finding_digests, list) or any(
            not isinstance(item, str) or not SHA256.fullmatch(item) for item in finding_digests
        ):
            raise CompatibilityError("lint baseline rule {} has invalid finding digests".format(rule_id))
        if len(finding_digests) != count:
            raise CompatibilityError("lint baseline rule {} finding digest count differs".format(rule_id))
        validate_lint_review(rule.get("review"), count, rule_id)
        total_rule_findings += count
    if total_rule_findings != total:
        raise CompatibilityError("lint baseline rule counts do not total the findings count")
    return baseline


def lint_baseline_path(manifest_path, manifest):
    return manifest_path.parent / LINT_BASELINE_DIRECTORY / (manifest["id"] + ".json")


def load_lint_baseline(path, corpus_id):
    if not path.is_file():
        return None
    try:
        baseline = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise CompatibilityError("could not read lint baseline {}: {}".format(path, error)) from error
    return validate_lint_baseline(baseline, corpus_id)


def compare_lint_baseline(summary, baseline, tier, corpus_id, baseline_path):
    policy = "supported" if tier == "supported" else "pinned-community" if corpus_id == "community-master" else "moving-development"
    comparison = {
        "schema": 1,
        "corpus_id": corpus_id,
        "policy": policy,
        "baseline_path": "{}/{}.json".format(
            LINT_BASELINE_DIRECTORY, corpus_id
        ),
        "status": "missing" if baseline is None else "passed",
        "baseline_present": baseline is not None,
        "added_findings_by_rule": {},
        "removed_findings_by_rule": {},
        "count_changes": {},
        "digest_changes": {},
        "newly_active_rules": [],
        "newly_clean_rules": [],
        "review_status_failures": [],
        "blocking_failures": [],
    }
    if baseline is None:
        if tier == "supported" or corpus_id == "community-master":
            comparison["blocking_failures"].append("lint baseline is missing")
        return comparison

    previous_rules = baseline.get("rules", {})
    current_rules = summary["rules"]
    for rule_id in sorted(set(previous_rules) | set(current_rules)):
        previous = previous_rules.get(rule_id, {"count": 0, "finding_digests": [], "findings_sha256": json_digest([])})
        current = current_rules.get(rule_id, {"count": 0, "finding_digests": [], "findings_sha256": json_digest([])})
        if previous["count"] != current["count"]:
            comparison["count_changes"][rule_id] = {
                "baseline": previous["count"],
                "current": current["count"],
            }
        if previous["findings_sha256"] != current["findings_sha256"]:
            comparison["digest_changes"][rule_id] = {
                "baseline": previous["findings_sha256"],
                "current": current["findings_sha256"],
            }
        if previous["count"] == 0 and current["count"] > 0:
            comparison["newly_active_rules"].append(rule_id)
        if previous["count"] > 0 and current["count"] == 0:
            comparison["newly_clean_rules"].append(rule_id)
        previous_digests = previous.get("finding_digests")
        current_digests = current.get("finding_digests", [])
        if previous_digests:
            added = sorted(set(current_digests) - set(previous_digests))
            removed = sorted(set(previous_digests) - set(current_digests))
            if added:
                comparison["added_findings_by_rule"][rule_id] = added
            if removed:
                comparison["removed_findings_by_rule"][rule_id] = removed
        elif previous["findings_sha256"] != current["findings_sha256"]:
            comparison["added_findings_by_rule"][rule_id] = None
            comparison["removed_findings_by_rule"][rule_id] = None

    for rule_id, current in sorted(current_rules.items()):
        previous = previous_rules.get(rule_id)
        if previous is None:
            comparison["review_status_failures"].append(
                {"rule_id": rule_id, "reason": "active rule is absent from baseline"}
            )
            continue
        if previous.get("count") != current["count"] or previous.get("findings_sha256") != current["findings_sha256"]:
            comparison["review_status_failures"].append(
                {"rule_id": rule_id, "reason": "digest changed; explicit review is required"}
            )
            continue
        review = previous.get("review", {})
        if review.get("status") != "reviewed":
            comparison["review_status_failures"].append(
                {"rule_id": rule_id, "reason": "active rule is not reviewed"}
            )
        if review.get("false_positive", 0) and not review.get("false_positive_decision"):
            comparison["review_status_failures"].append(
                {"rule_id": rule_id, "reason": "false positives have no remediation decision"}
            )

    changed = any(
        (
            summary["total_findings"] != baseline["total_findings"],
            summary["findings_sha256"] != baseline["findings_sha256"],
            summary["severity_counts"] != baseline["severity_counts"],
            comparison["count_changes"],
            comparison["digest_changes"],
            comparison["review_status_failures"],
        )
    )
    if changed:
        comparison["status"] = "failed"
        comparison["blocking_failures"].append("lint findings differ from the checked-in baseline")
    return comparison


def build_lint_baseline(summary, previous=None):
    rules = {}
    for rule_id in LINT_RULE_IDS:
        current = summary["rules"].get(
            rule_id,
            {
                "count": 0,
                "findings_sha256": json_digest([]),
                "finding_digests": [],
            },
        )
        old = (previous or {}).get("rules", {}).get(rule_id)
        if old and old.get("count") == current["count"] and old.get("findings_sha256") == current["findings_sha256"]:
            review = dict(old.get("review", _default_lint_review("not-applicable")))
        else:
            review = _default_lint_review("not-applicable" if current["count"] == 0 else "unreviewed")
        rules[rule_id] = {
            "count": current["count"],
            "findings_sha256": current["findings_sha256"],
            "finding_digests": list(current.get("finding_digests", [])),
            "review": review,
        }
    return {
        "schema": LINT_BASELINE_SCHEMA,
        "corpus_id": summary["corpus_id"],
        "total_findings": summary["total_findings"],
        "findings_sha256": summary["findings_sha256"],
        "severity_counts": dict(summary["severity_counts"]),
        "rules": rules,
    }


def write_lint_evidence(evidence_dir, findings, summary, comparison):
    write_json(
        evidence_dir / "lint" / "findings.json",
        {"schema": 1, "corpus_id": summary["corpus_id"], "findings": findings},
    )
    write_json(evidence_dir / "lint" / "summary.json", summary)
    write_json(evidence_dir / "lint" / "baseline-comparison.json", comparison)


def load_manifest(path):
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if manifest.get("schema") != 1:
        raise CompatibilityError("upstream corpus manifest must use schema 1")

    corpus_id = manifest.get("id")
    tier = manifest.get("tier")
    if not corpus_id or tier not in {"supported", "development"}:
        raise CompatibilityError(
            "upstream corpus manifest must identify a supported or development tier"
        )
    if not manifest.get("yocto_version") or not manifest.get("bitbake_version"):
        raise CompatibilityError(
            "upstream corpus manifest must declare Yocto and BitBake versions"
        )

    repositories = manifest.get("repositories", [])
    layers = manifest.get("layers", [])
    if not repositories or not layers:
        raise CompatibilityError(
            "upstream corpus manifest has no repositories or layers"
        )

    repository_names = set()
    for repository in repositories:
        name = repository.get("name")
        revision = repository.get("revision", "")
        tracking_ref = repository.get("ref", "")
        if not name or name in repository_names:
            raise CompatibilityError("repository names must be present and unique")
        if tier == "supported" and not REVISION.fullmatch(revision):
            raise CompatibilityError(
                "repository {} does not use a full commit revision".format(name)
            )
        if tier == "development" and not (
            REVISION.fullmatch(revision)
            or (
                tracking_ref.startswith("refs/heads/")
                and len(tracking_ref) > len("refs/heads/")
            )
        ):
            raise CompatibilityError(
                "development repository {} has no full revision or branch ref".format(
                    name
                )
            )
        if not repository.get("url") or not repository.get("sparse_paths"):
            raise CompatibilityError(
                "repository {} has no URL or sparse paths".format(name)
            )
        repository_names.add(name)

    layer_names = set()
    for layer in layers:
        name = layer.get("name")
        if not name or name in layer_names:
            raise CompatibilityError("layer names must be present and unique")
        if layer.get("repository") not in repository_names:
            raise CompatibilityError(
                "layer {} references an unknown repository".format(name)
            )
        if not layer.get("path") or not isinstance(layer.get("minimum_files"), int):
            raise CompatibilityError(
                "layer {} has no path or minimum file count".format(name)
            )
        layer_names.add(name)

    bitbake = manifest.get("bitbake", {})
    init_repository = bitbake.get("init_repository", bitbake.get("repository"))
    if (
        init_repository not in repository_names
        or not bitbake.get("template")
        or not bitbake.get("target")
        or not isinstance(bitbake.get("additional_layers"), list)
    ):
        raise CompatibilityError("upstream corpus has an invalid BitBake configuration")

    semantic_probes = bitbake.get("semantic_probes", [])
    if not isinstance(semantic_probes, list):
        raise CompatibilityError("upstream corpus semantic probes must be a list")
    probe_names = set()
    for probe in semantic_probes:
        if not isinstance(probe, dict):
            raise CompatibilityError("upstream corpus has an invalid semantic probe")
        name = probe.get("name")
        target = probe.get("target", bitbake.get("target"))
        variables = probe.get("variables")
        if (
            not isinstance(name, str)
            or not name
            or name in probe_names
            or not isinstance(target, str)
            or not target
            or not isinstance(variables, list)
            or not variables
            or any(not isinstance(variable, str) or not variable for variable in variables)
            or len(set(variables)) != len(variables)
        ):
            raise CompatibilityError("upstream corpus has an invalid semantic probe")
        probe_names.add(name)

    syntax_metrics = manifest.get("syntax_metrics", {})
    if not all(
        isinstance(syntax_metrics.get(field), int)
        and syntax_metrics[field] >= 0
        for field in ("minimum_structured_nodes", "maximum_unknown_nodes")
    ):
        raise CompatibilityError("upstream corpus has invalid syntax metric thresholds")

    baseline_reference = syntax_metrics.get("baseline_metrics")
    if baseline_reference is not None:
        if not isinstance(baseline_reference, str) or not baseline_reference:
            raise CompatibilityError("upstream corpus has an invalid baseline metrics path")
        manifest_directory = path.resolve().parent
        baseline_path = (manifest_directory / baseline_reference).resolve()
        try:
            baseline_path.relative_to(manifest_directory)
        except ValueError as error:
            raise CompatibilityError(
                "upstream corpus baseline metrics must be inside the manifest directory"
            ) from error
        try:
            baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as error:
            raise CompatibilityError(
                "could not read upstream corpus baseline metrics {}: {}".format(
                    baseline_path, error
                )
            ) from error
        if (
            baseline.get("schema") != 1
            or baseline.get("corpus_id") != corpus_id
            or not isinstance(baseline.get("source"), dict)
            or not isinstance(baseline.get("formatted"), dict)
        ):
            raise CompatibilityError(
                "upstream corpus baseline metrics have an invalid schema"
            )
        for label in ("source", "formatted"):
            metrics = baseline[label]
            if not all(
                isinstance(metrics.get(field), int) and metrics[field] >= 0
                for field in BASELINE_METRIC_FIELDS
            ):
                raise CompatibilityError(
                    "upstream corpus {} baseline metrics are invalid".format(label)
                )
        manifest["_baseline_metrics"] = baseline

    return manifest


def run(command, cwd=None, environment=None, accepted=(0,)):
    return run_recorded(command, None, None, cwd, environment, accepted)


def run_recorded(
    command,
    label,
    records,
    cwd=None,
    environment=None,
    accepted=(0,),
    log_path=None,
):
    started = time.monotonic()
    result = subprocess.run(
        [str(argument) for argument in command],
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
    )
    duration = time.monotonic() - started
    if log_path is not None:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        command_line = shlex.join(str(argument) for argument in command)
        log_path.write_text(
            "$ {}\n\n{}{}".format(
                command_line,
                result.stdout,
                "\n[stderr]\n{}".format(result.stderr) if result.stderr else "",
            ),
            encoding="utf-8",
        )
    if records is not None:
        records.append(
            {
                "label": label or "command",
                "command": [str(argument) for argument in command],
                "cwd": str(cwd) if cwd is not None else None,
                "exit_code": result.returncode,
                "duration_seconds": round(duration, 6),
                "log": str(log_path) if log_path is not None else None,
            }
        )
    if result.returncode not in accepted:
        output = "\n".join(
            part.strip() for part in [result.stdout, result.stderr] if part.strip()
        )
        raise CompatibilityError(
            "command failed with exit code {}: {}\n{}".format(
                result.returncode,
                " ".join(str(argument) for argument in command),
                output,
            ).rstrip()
        )
    return result


def checkout_repository(repository, destination):
    destination.mkdir(parents=True)
    run(["git", "init", "--quiet", destination])
    run(["git", "-C", destination, "remote", "add", "origin", repository["url"]])
    run(["git", "-C", destination, "sparse-checkout", "init", "--no-cone"])
    patterns = ["/{}".format(path) for path in repository["sparse_paths"]]
    target = repository.get("ref", repository.get("revision"))
    run(
        [
            "git",
            "-C",
            destination,
            "sparse-checkout",
            "set",
            "--no-cone",
        ]
        + patterns
    )
    run(
        [
            "git",
            "-C",
            destination,
            "fetch",
            "--depth",
            "1",
            "--filter=blob:none",
            "origin",
            target,
        ]
    )
    run(["git", "-C", destination, "checkout", "--quiet", "--detach", "FETCH_HEAD"])


def verify_repository(repository, path):
    if not path.is_dir():
        raise CompatibilityError(
            "repository {} is missing at {}".format(repository["name"], path)
        )
    revision = run(["git", "-C", path, "rev-parse", "HEAD"]).stdout.strip()
    expected = repository.get("revision")
    if expected and revision != expected:
        raise CompatibilityError(
            "repository {} is at {}; expected {}".format(
                repository["name"], revision, repository["revision"]
            )
        )
    return revision


def is_layer_configuration(path, layer_root):
    parent = path.parent
    while parent != layer_root.parent:
        if parent.name == "conf" and (parent / "layer.conf").is_file():
            return True
        if parent == layer_root:
            break
        parent = parent.parent
    return False


def discover_metadata_files(layer_root):
    files = []
    for path in layer_root.rglob("*"):
        if not path.is_file() or path.suffix not in METADATA_EXTENSIONS:
            continue
        relative = path.relative_to(layer_root)
        if "files" in relative.parts:
            continue
        if path.suffix == ".conf" and not is_layer_configuration(path, layer_root):
            continue
        files.append(path)
    return sorted(files)


def excluded_candidate_files(layer_root, metadata_files):
    metadata_files = set(metadata_files)
    return sorted(
        path
        for path in layer_root.rglob("*")
        if path.is_file()
        and path.suffix in METADATA_EXTENSIONS
        and path not in metadata_files
    )


def file_digest(path):
    return hashlib.sha256(path.read_bytes()).digest()


def opaque_regions(text):
    lines = text.splitlines(keepends=True)
    regions = []
    index = 0
    while index < len(lines):
        line = lines[index]
        if FUNCTION_START.match(line):
            start = index
            index += 1
            while index < len(lines) and lines[index].strip() != "}":
                index += 1
            if index == len(lines):
                raise CompatibilityError("opaque function has no closing brace")
            index += 1
            regions.append("".join(lines[start:index]))
            continue

        if PYTHON_DEF_START.match(line):
            start = index
            index += 1
            while index < len(lines):
                candidate = lines[index]
                if (
                    candidate.startswith((" ", "\t"))
                    or not candidate.strip()
                    or candidate.lstrip().startswith("#")
                ):
                    index += 1
                    continue
                break
            regions.append("".join(lines[start:index]))
            continue

        index += 1
    return regions


def copy_sources(source_root, formatted_root, repositories):
    for repository in repositories:
        source = source_root / repository["name"]
        destination = formatted_root / repository["name"]
        shutil.copytree(
            source,
            destination,
            symlinks=True,
            ignore=shutil.ignore_patterns(".git"),
        )


def tree_snapshot(root, top_level_names=None):
    snapshot = {}
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if ".git" in relative.parts:
            continue
        if top_level_names and relative.parts[0] not in top_level_names:
            continue
        key = relative.as_posix()
        if path.is_symlink():
            snapshot[key] = {"kind": "symlink", "target": os.readlink(path)}
        elif path.is_file():
            snapshot[key] = {"kind": "file", "digest": file_digest(path).hex()}
    return snapshot


def allowed_metadata_paths(layers, metadata):
    allowed = set()
    for layer in layers:
        prefix = Path(layer["repository"]) / layer["path"]
        for relative in metadata[layer["name"]]:
            allowed.add((prefix / relative).as_posix())
    return allowed


def verify_tree_preservation(
    source_root, formatted_root, allowed_changes, top_level_names=None
):
    source_snapshot = tree_snapshot(source_root, top_level_names)
    formatted_snapshot = tree_snapshot(formatted_root, top_level_names)
    source_paths = set(source_snapshot)
    formatted_paths = set(formatted_snapshot)
    missing = sorted(source_paths - formatted_paths)
    unexpected = sorted(formatted_paths - source_paths)
    if missing or unexpected:
        details = []
        if missing:
            details.append("missing " + ", ".join(missing[:5]))
        if unexpected:
            details.append("unexpected " + ", ".join(unexpected[:5]))
        raise CompatibilityError(
            "formatted repository tree differs from source: " + "; ".join(details)
        )

    changed = []
    for relative in sorted(source_paths):
        before = source_snapshot[relative]
        after = formatted_snapshot[relative]
        if before["kind"] != after["kind"]:
            raise CompatibilityError(
                "formatted repository changed file type for {}".format(relative)
            )
        if before != after:
            changed.append(relative)

    unexpected_changes = sorted(set(changed) - allowed_changes)
    if unexpected_changes:
        raise CompatibilityError(
            "formatter changed files outside the metadata allowlist: {}".format(
                ", ".join(unexpected_changes[:10])
            )
        )
    return changed


def layer_paths(root, layers):
    return [root / layer["repository"] / layer["path"] for layer in layers]


def verify_layers(source_root, formatted_root, layers):
    metadata = {}
    excluded = {}
    total_files = 0

    for layer in layers:
        source_layer = source_root / layer["repository"] / layer["path"]
        formatted_layer = formatted_root / layer["repository"] / layer["path"]
        if not source_layer.is_dir() or not formatted_layer.is_dir():
            raise CompatibilityError("layer {} is missing".format(layer["name"]))

        source_files = discover_metadata_files(source_layer)
        if len(source_files) < layer["minimum_files"]:
            raise CompatibilityError(
                "layer {} contains {} metadata files; expected at least {}".format(
                    layer["name"], len(source_files), layer["minimum_files"]
                )
            )
        formatted_files = discover_metadata_files(formatted_layer)
        source_relative = {path.relative_to(source_layer) for path in source_files}
        formatted_relative = {
            path.relative_to(formatted_layer) for path in formatted_files
        }
        if source_relative != formatted_relative:
            raise CompatibilityError(
                "formatted layer {} has a different metadata file set".format(
                    layer["name"]
                )
            )
        total_files += len(source_files)
        metadata[layer["name"]] = [
            path.relative_to(source_layer) for path in source_files
        ]
        excluded[layer["name"]] = {
            path.relative_to(source_layer): file_digest(path)
            for path in excluded_candidate_files(source_layer, source_files)
        }
        print("  {}: {} metadata files".format(layer["name"], len(source_files)))

    return metadata, excluded, total_files


def verify_preservation(source_root, formatted_root, layers, metadata, excluded):
    opaque_count = 0
    excluded_count = 0
    for layer in layers:
        source_layer = source_root / layer["repository"] / layer["path"]
        formatted_layer = formatted_root / layer["repository"] / layer["path"]

        for relative in metadata[layer["name"]]:
            source = (source_layer / relative).read_text(encoding="utf-8")
            formatted = (formatted_layer / relative).read_text(encoding="utf-8")
            before = opaque_regions(source)
            after = opaque_regions(formatted)
            if before != after:
                raise CompatibilityError(
                    "formatter changed an opaque function or Python block in {}/{}".format(
                        layer["name"], relative
                    )
                )
            opaque_count += len(before)

        for relative, digest in excluded[layer["name"]].items():
            formatted_path = formatted_layer / relative
            if file_digest(formatted_path) != digest:
                raise CompatibilityError(
                    "formatter changed excluded recipe payload {}/{}".format(
                        layer["name"], relative
                    )
                )
            excluded_count += 1

    return opaque_count, excluded_count


def syntax_stats(bbtidy, inputs, label=None, records=None, log_path=None):
    result = run_recorded(
        [bbtidy, "syntax-stats"] + inputs,
        label,
        records,
        log_path=log_path,
    )
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise CompatibilityError(
            "bbtidy returned invalid syntax statistics: {}".format(error)
        ) from error


def parse_semantic_values(text, variables):
    values = {variable: None for variable in variables}
    for line in text.splitlines():
        match = SEMANTIC_VARIABLE.match(line)
        if match and match.group("name") in values:
            values[match.group("name")] = match.group("value")
    return values


def normalize_semantic_values(values, roots):
    normalized = {}
    replacements = sorted(
        ((str(root), "<CORPUS>") for root in roots if root),
        key=lambda item: len(item[0]),
        reverse=True,
    )
    for name, value in values.items():
        if value is None:
            normalized[name] = None
            continue
        for source, replacement in replacements:
            value = value.replace(source, replacement)
        normalized[name] = value
    return normalized


def compare_semantic_probes(source, formatted):
    if set(source) != set(formatted):
        raise CompatibilityError("semantic probe sets differ between source and formatted builds")
    for name in sorted(source):
        source_values = source[name]["values"]
        formatted_values = formatted[name]["values"]
        if source_values != formatted_values:
            differing = [
                variable
                for variable in sorted(source_values)
                if source_values.get(variable) != formatted_values.get(variable)
            ]
            raise CompatibilityError(
                "semantic probe {} changed variables: {}".format(
                    name, ", ".join(differing)
                )
            )


def run_bitbake_environment(
    root,
    build_root,
    configuration,
    target,
    label,
    records,
    log_path,
):
    checkout = root / configuration.get(
        "init_repository", configuration.get("repository")
    )
    script = """
set -e
checkout=$1
build=$2
export TEMPLATECONF=$3
target=$4
. "$checkout/oe-init-build-env" "$build" >/dev/null
bitbake -e "$target"
"""
    return run_recorded(
        [
            "bash",
            "-c",
            script,
            "bbtidy-upstream",
            checkout,
            build_root,
            checkout / configuration["template"],
            target,
        ],
        label,
        records,
        log_path=log_path,
    )


def run_semantic_probes(
    root, build_root, configuration, probes, roots, records, evidence_dir, label
):
    results = {}
    for probe in probes:
        safe_name = re.sub(r"[^A-Za-z0-9_.-]+", "_", probe["name"]).strip("_")
        result = run_bitbake_environment(
            root,
            build_root,
            configuration,
            probe.get("target", configuration["target"]),
            "{} semantic probe {}".format(label, probe["name"]),
            records,
            evidence_dir / "logs" / "{}-{}.log".format(label, safe_name),
        )
        values = parse_semantic_values(result.stdout, probe["variables"])
        results[probe["name"]] = {
            "target": probe.get("target", configuration["target"]),
            "variables": probe["variables"],
            "values": normalize_semantic_values(values, roots),
        }
    return results


def verify_syntax_metrics(
    source, formatted, thresholds, total_files, baseline_metrics=None
):
    for label, metrics in (("source", source), ("formatted", formatted)):
        if metrics.get("version") != 1 or metrics.get("files") != total_files:
            raise CompatibilityError(
                "{} syntax metrics did not cover all metadata files".format(label)
            )

    if formatted["structured_nodes"] < thresholds["minimum_structured_nodes"]:
        raise CompatibilityError(
            "formatted corpus has {} structured nodes; expected at least {}".format(
                formatted["structured_nodes"], thresholds["minimum_structured_nodes"]
            )
        )
    if formatted["unknown_nodes"] > thresholds["maximum_unknown_nodes"]:
        raise CompatibilityError(
            "formatted corpus has {} unknown nodes; expected at most {}".format(
                formatted["unknown_nodes"], thresholds["maximum_unknown_nodes"]
            )
        )
    if formatted["unknown_nodes"] > source["unknown_nodes"]:
        raise CompatibilityError(
            "formatting increased unknown nodes from {} to {}".format(
                source["unknown_nodes"], formatted["unknown_nodes"]
            )
        )
    if baseline_metrics is not None:
        for label, metrics in (("source", source), ("formatted", formatted)):
            expected = baseline_metrics[label]
            for field in BASELINE_METRIC_FIELDS:
                if metrics.get(field) != expected[field]:
                    raise CompatibilityError(
                        "{} syntax metric {} changed from {} to {}; update the "
                        "pinned baseline only with an explicit compatibility review".format(
                            label, field, expected[field], metrics.get(field)
                        )
                    )


def run_bitbake_parse(
    root, build_root, configuration, label, records, evidence_dir
):
    init_repository = configuration.get(
        "init_repository", configuration.get("repository")
    )
    checkout = root / init_repository
    build_root.mkdir(parents=True)
    script = """
set -e
checkout=$1
build=$2
export TEMPLATECONF=$3
target=$4
shift 4
. "$checkout/oe-init-build-env" "$build" >/dev/null
for layer in "$@"; do
    bitbake-layers add-layer "$layer"
done
bitbake --parse-only "$target"
"""
    run_recorded(
        [
            "bash",
            "-c",
            script,
            "bbtidy-upstream",
            checkout,
            build_root,
            checkout / configuration["template"],
            configuration["target"],
        ]
        + [root / path for path in configuration["additional_layers"]],
        label,
        records,
        log_path=evidence_dir / "logs" / "{}-parse.log".format(label),
    )


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def project_revision():
    try:
        return run(["git", "-C", PROJECT_ROOT, "rev-parse", "HEAD"]).stdout.strip()
    except (CompatibilityError, OSError):
        return None


def write_evidence(
    evidence_dir,
    manifest_path,
    manifest,
    records,
    revisions,
    bbtidy_version,
    source_metrics,
    formatted_metrics,
    summary,
):
    evidence_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(manifest_path, evidence_dir / "manifest.json")
    write_json(evidence_dir / "metrics" / "source.json", source_metrics)
    write_json(evidence_dir / "metrics" / "formatted.json", formatted_metrics)

    commands = []
    for record in records:
        command = dict(record)
        if command["log"]:
            command["log"] = Path(command["log"]).resolve().relative_to(
                evidence_dir.resolve()
            ).as_posix()
        commands.append(command)
    write_json(evidence_dir / "commands.json", commands)
    write_json(
        evidence_dir / "summary.json",
        {
            "schema": 1,
            "status": "passed",
            "corpus": {
                "id": manifest["id"],
                "tier": manifest["tier"],
                "yocto_version": manifest["yocto_version"],
                "bitbake_version": manifest["bitbake_version"],
                "manifest": "manifest.json",
            },
            "bbtidy": {
                "version": bbtidy_version,
                "source_revision": project_revision(),
            },
            "repositories": revisions,
            "runner": {
                "platform": platform.platform(),
                "python": sys.version,
                "cwd": str(Path.cwd()),
                "environment": {
                    key: os.environ.get(key)
                    for key in ("LANG", "LC_ALL", "TZ")
                    if os.environ.get(key) is not None
                },
            },
            "results": summary,
        },
    )


def baseline_update_allowed(arguments):
    if not arguments.update_lint_baseline:
        return
    if os.environ.get("GITHUB_ACTIONS") == "true" and not (
        arguments.allow_ci_lint_baseline_update
        or os.environ.get("BBTIDY_ALLOW_CI_LINT_BASELINE_UPDATE") == "1"
    ):
        raise CompatibilityError(
            "lint baseline updates are disabled in GitHub Actions; pass "
            "--allow-ci-lint-baseline-update or set "
            "BBTIDY_ALLOW_CI_LINT_BASELINE_UPDATE=1 intentionally"
        )
    print(
        "WARNING: --update-lint-baseline will write findings as unreviewed; "
        "review every active rule before relying on this baseline",
        file=sys.stderr,
    )


def check_compatibility(arguments, workspace, evidence_dir):
    baseline_update_allowed(arguments)
    manifest = load_manifest(arguments.manifest)
    repositories = manifest["repositories"]
    layers = manifest["layers"]
    records = []
    evidence_dir.mkdir(parents=True, exist_ok=False)
    source_root = (
        arguments.source_root.resolve()
        if arguments.source_root
        else workspace / "sources"
    )

    if arguments.source_root:
        print("Using existing pinned upstream checkouts")
    else:
        source_root.mkdir(parents=True)
        print("Fetching upstream checkouts")
        for repository in repositories:
            target = repository.get("revision", repository.get("ref"))
            print("  {} @ {}".format(repository["name"], target))
            checkout_repository(repository, source_root / repository["name"])

    revisions = {}
    for repository in repositories:
        revision = verify_repository(repository, source_root / repository["name"])
        revisions[repository["name"]] = {
            "expected_revision": repository.get("revision"),
            "resolved_revision": revision,
        }
        if repository.get("ref"):
            print("  {} resolved to {}".format(repository["name"], revision))

    formatted_root = workspace / "formatted"
    copy_sources(source_root, formatted_root, repositories)

    print("Discovering real-world metadata")
    metadata, excluded, total_files = verify_layers(source_root, formatted_root, layers)

    inputs = layer_paths(formatted_root, layers)
    source_inputs = layer_paths(source_root, layers)
    version_result = run_recorded(
        [arguments.bbtidy, "--version"],
        "bbtidy version",
        records,
        log_path=evidence_dir / "logs" / "bbtidy-version.log",
    )
    bbtidy_version = version_result.stdout.strip()
    source_metrics = syntax_stats(
        arguments.bbtidy,
        source_inputs,
        "source syntax metrics",
        records,
        evidence_dir / "logs" / "source-syntax-stats.log",
    )
    print("Formatting {} metadata files".format(total_files))
    formatted = run_recorded(
        [arguments.bbtidy, "format", "--write"] + inputs,
        "format metadata",
        records,
        log_path=evidence_dir / "logs" / "format.log",
    )
    changed_files = sum(
        line.startswith("formatted: ") for line in formatted.stdout.splitlines()
    )

    run_recorded(
        format_idempotence_command(arguments.bbtidy, inputs),
        "format idempotence check",
        records,
        log_path=evidence_dir / "logs" / "idempotence.log",
    )
    formatted_metrics = syntax_stats(
        arguments.bbtidy,
        inputs,
        "formatted syntax metrics",
        records,
        evidence_dir / "logs" / "formatted-syntax-stats.log",
    )
    verify_syntax_metrics(
        source_metrics,
        formatted_metrics,
        manifest["syntax_metrics"],
        total_files,
        manifest.get("_baseline_metrics"),
    )
    linted = run_recorded(
        lint_command(arguments.bbtidy, inputs),
        "lint metadata",
        records,
        accepted=(0,),
        log_path=evidence_dir / "logs" / "lint.log",
    )
    baseline_path = lint_baseline_path(arguments.manifest, manifest)
    lint_baseline = load_lint_baseline(baseline_path, manifest["id"])
    lint_findings = normalize_lint_report(
        linted.stdout,
        manifest["id"],
        [(repository["name"], formatted_root / repository["name"]) for repository in repositories],
    )
    lint_summary = summarize_lint_findings(
        manifest["id"], lint_findings, lint_baseline
    )
    lint_comparison = compare_lint_baseline(
        lint_summary,
        lint_baseline,
        manifest["tier"],
        manifest["id"],
        baseline_path,
    )
    if arguments.update_lint_baseline:
        lint_comparison["status"] = "update-pending"
    write_lint_evidence(evidence_dir, lint_findings, lint_summary, lint_comparison)
    if (
        lint_comparison["blocking_failures"]
        and manifest["tier"] != "supported"
        and manifest["id"] != "community-master"
        and not arguments.update_lint_baseline
    ):
        report_warning(
            "non-blocking lint-quality regression: {}".format(
                "; ".join(lint_comparison["blocking_failures"])
            )
        )
    if (
        not arguments.update_lint_baseline
        and lint_comparison["blocking_failures"]
        and (manifest["tier"] == "supported" or manifest["id"] == "community-master")
    ):
        raise CompatibilityError(
            "lint quality baseline check failed: {}".format(
                "; ".join(lint_comparison["blocking_failures"])
            )
        )
    print(
        "  lint findings: {} ({} rules, {} reviewed)".format(
            lint_summary["total_findings"],
            len(lint_summary["rules"]),
            lint_summary["reviewed_rule_count"],
        )
    )

    opaque_count, excluded_count = verify_preservation(
        source_root, formatted_root, layers, metadata, excluded
    )
    changed_paths = verify_tree_preservation(
        source_root,
        formatted_root,
        allowed_metadata_paths(layers, metadata),
        [repository["name"] for repository in repositories],
    )
    if len(changed_paths) != changed_files:
        raise CompatibilityError(
            "formatter reported {} changed files but tree verification found {}".format(
                changed_files, len(changed_paths)
            )
        )

    parsed = False
    source_semantics = {}
    formatted_semantics = {}
    if not arguments.skip_bitbake:
        print("Parsing original layers with BitBake")
        original_build = workspace / "build-original"
        run_bitbake_parse(
            source_root,
            original_build,
            manifest["bitbake"],
            "original",
            records,
            evidence_dir,
        )
        print("Parsing formatted layers with BitBake")
        formatted_build = workspace / "build-formatted"
        run_bitbake_parse(
            formatted_root,
            formatted_build,
            manifest["bitbake"],
            "formatted",
            records,
            evidence_dir,
        )
        probes = manifest["bitbake"].get("semantic_probes", [])
        source_semantics = run_semantic_probes(
            source_root,
            original_build,
            manifest["bitbake"],
            probes,
            [source_root, original_build],
            records,
            evidence_dir,
            "original",
        )
        formatted_semantics = run_semantic_probes(
            formatted_root,
            formatted_build,
            manifest["bitbake"],
            probes,
            [formatted_root, formatted_build],
            records,
            evidence_dir,
            "formatted",
        )
        compare_semantic_probes(source_semantics, formatted_semantics)
        parsed = True

    if arguments.update_lint_baseline:
        write_json(baseline_path, build_lint_baseline(lint_summary, lint_baseline))
        print(
            "WARNING: wrote lint baseline {}; active rules remain unreviewed".format(
                baseline_path
            ),
            file=sys.stderr,
        )

    write_evidence(
        evidence_dir,
        arguments.manifest,
        manifest,
        records,
        revisions,
        bbtidy_version,
        source_metrics,
        formatted_metrics,
        {
            "metadata_files": total_files,
            "files_changed_on_first_format": changed_files,
            "changed_paths": changed_paths,
            "lint_diagnostics": lint_summary["total_findings"],
            "lint_quality": {
                "summary": lint_summary,
                "baseline_comparison": lint_comparison,
            },
            "source_metrics": source_metrics,
            "formatted_metrics": formatted_metrics,
            "opaque_regions_preserved": opaque_count,
            "excluded_payload_files_unchanged": excluded_count,
            "bitbake_differential_parse": "passed" if parsed else "skipped",
            "semantic_probes": {
                "original": source_semantics,
                "formatted": formatted_semantics,
            },
        },
    )

    print(
        "Upstream compatibility check passed: Yocto {} / BitBake {} ({})".format(
            manifest["yocto_version"], manifest["bitbake_version"], manifest["tier"]
        )
    )
    print("  metadata files: {}".format(total_files))
    print("  files changed on first format: {}".format(changed_files))
    print("  lint diagnostics: {}".format(lint_summary["total_findings"]))
    print("  lint findings digest: {}".format(lint_summary["findings_sha256"]))
    print("  structured CST nodes: {}".format(formatted_metrics["structured_nodes"]))
    print("  unknown CST nodes: {}".format(formatted_metrics["unknown_nodes"]))
    print("  unknown CST bytes: {}".format(formatted_metrics["unknown_bytes"]))
    print("  opaque regions preserved: {}".format(opaque_count))
    print("  excluded payload files unchanged: {}".format(excluded_count))
    print("  BitBake differential parse: {}".format("passed" if parsed else "skipped"))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--bbtidy",
        type=Path,
        default=PROJECT_ROOT / "target" / "release" / "bbtidy",
    )
    parser.add_argument(
        "--source-root",
        type=Path,
        help="directory containing already checked out pinned repositories",
    )
    parser.add_argument(
        "--workspace",
        type=Path,
        help="directory to create for retaining formatted sources and the build",
    )
    parser.add_argument(
        "--skip-bitbake",
        action="store_true",
        help="run formatter and lint checks without the Linux-only BitBake parse",
    )
    parser.add_argument(
        "--evidence-dir",
        type=Path,
        help="directory for the machine-readable verification evidence bundle",
    )
    parser.add_argument(
        "--update-lint-baseline",
        action="store_true",
        help="explicitly regenerate the corpus lint-quality baseline",
    )
    parser.add_argument(
        "--allow-ci-lint-baseline-update",
        action="store_true",
        help="explicitly permit --update-lint-baseline in GitHub Actions",
    )
    arguments = parser.parse_args()
    arguments.manifest = arguments.manifest.resolve()
    arguments.bbtidy = arguments.bbtidy.resolve()
    if arguments.evidence_dir:
        arguments.evidence_dir = arguments.evidence_dir.resolve()

    if not arguments.bbtidy.is_file():
        print(
            "error: bbtidy executable not found: {}".format(arguments.bbtidy),
            file=sys.stderr,
        )
        return 2

    try:
        if arguments.workspace:
            workspace = arguments.workspace.resolve()
            workspace.mkdir(parents=True, exist_ok=False)
            evidence_dir = arguments.evidence_dir or workspace / "evidence"
            check_compatibility(arguments, workspace, evidence_dir)
        else:
            with tempfile.TemporaryDirectory(prefix="bbtidy-upstream-") as temporary:
                workspace = Path(temporary)
                evidence_dir = arguments.evidence_dir or workspace / "evidence"
                check_compatibility(arguments, workspace, evidence_dir)
    except (CompatibilityError, OSError, UnicodeError) as error:
        report_error(error)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
