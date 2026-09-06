#!/usr/bin/env python3
"""Prepare fingerprint-bound lint reviews and evaluate recorded adoption pilots."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

try:
    from scripts.lint_quality import (
        LintBaselineError, PROFILE_RULES, _finding_digest, digest_findings,
        evaluate_pilot_thresholds, minimum_review_samples, quality_report,
    )
except ModuleNotFoundError:
    from lint_quality import (
        LintBaselineError, PROFILE_RULES, _finding_digest, digest_findings,
        evaluate_pilot_thresholds, minimum_review_samples, quality_report,
    )

CORRECTNESS = ("true_positive", "false_positive", "unclear")
ACTIONABILITY = ("must_fix", "should_fix", "context_dependent", "policy_only", "not_actionable")
COHORTS = ("quickstart", "existing-project", "new-config")


def review_packet(findings: list, corpus_id: str) -> dict:
    quality = quality_report(findings)
    by_digest = {_finding_digest(finding): finding for finding in findings}
    return {
        "schema": 1,
        "kind": "bbtidy-adoption-review",
        "corpus_id": corpus_id,
        "source_state": "formatted",
        "findings_sha256": digest_findings(findings),
        "profiles": {name: sorted(rules) for name, rules in PROFILE_RULES.items()},
        "rules": {
            rule_id: {
                "total_findings": rule["total"],
                "required_samples": rule["review_sampling"]["target"],
                "samples": [
                    {"fingerprint": fingerprint, "finding": by_digest[fingerprint]}
                    for fingerprint in rule["review_sampling"]["sample_fingerprints"]
                ],
            }
            for rule_id, rule in quality["rules"].items() if rule["total"]
        },
    }


def review_form(packet: dict) -> dict:
    return {
        "schema": 1,
        "corpus_id": packet["corpus_id"],
        "findings_sha256": packet["findings_sha256"],
        "rules": {
            rule_id: {
                "reviewer": None,
                "samples": {
                    sample["fingerprint"]: {"correctness": None, "actionability": None, "notes": ""}
                    for sample in rule["samples"]
                },
            }
            for rule_id, rule in packet["rules"].items()
        },
    }


def observation_form(packet: dict) -> dict:
    return {"schema": 1, "corpus_id": packet["corpus_id"], "findings_sha256": packet["findings_sha256"], "sessions": []}


def evaluate(packet: dict, reviews: dict, observations: dict) -> dict:
    if packet.get("schema") != 1 or packet.get("kind") != "bbtidy-adoption-review":
        raise ValueError("unsupported review packet")
    if packet.get("profiles") != {name: sorted(rules) for name, rules in PROFILE_RULES.items()}:
        raise ValueError("profile membership changed; prepare a fresh packet")
    if reviews.get("schema") != 1 or any(
        reviews.get(key) != packet.get(key) for key in ("corpus_id", "findings_sha256")
    ):
        raise ValueError("reviews do not match this corpus and finding digest")
    if set(reviews.get("rules", {})) != set(packet["rules"]):
        raise ValueError("review rules do not match the packet")
    coverage = {}
    counts = {profile: {key: 0 for key in (*CORRECTNESS, *ACTIONABILITY)} for profile in PROFILE_RULES}
    for rule_id, rule in packet["rules"].items():
        record = reviews["rules"][rule_id]
        expected = {sample["fingerprint"] for sample in rule["samples"]}
        target = minimum_review_samples(rule["total_findings"])
        if rule["required_samples"] != target or len(expected) != len(rule["samples"]):
            raise ValueError("invalid packet sample target or duplicate fingerprints")
        if set(record.get("samples", {})) != expected:
            raise ValueError("review sample fingerprints do not match the packet")
        complete = 0
        for sample in rule["samples"]:
            finding = sample["finding"]
            if _finding_digest(finding) != sample["fingerprint"] or finding["rule_id"] != rule_id:
                raise ValueError("packet finding does not match its fingerprint or rule")
            decision = record["samples"][sample["fingerprint"]]
            correctness, actionability = decision.get("correctness"), decision.get("actionability")
            if correctness not in (*CORRECTNESS, None) or actionability not in (*ACTIONABILITY, None):
                raise ValueError("unknown correctness or actionability classification")
            if correctness is None or actionability is None:
                continue
            if not isinstance(record.get("reviewer"), str) or not record["reviewer"].strip():
                raise ValueError("completed classifications require a human reviewer identifier")
            if correctness in ("false_positive", "unclear") and not str(decision.get("notes") or "").strip():
                raise ValueError("false-positive and unclear findings require remediation notes")
            complete += 1
            for profile, rules in PROFILE_RULES.items():
                if rule_id in rules:
                    counts[profile][correctness] += 1
                    counts[profile][actionability] += 1
        coverage[rule_id] = {"required": target, "reviewed": complete, "complete": complete >= target}

    metrics = {}
    for profile in ("essential", "recommended"):
        selected = [value for rule_id, value in coverage.items() if rule_id in PROFILE_RULES[profile]]
        denominator = sum(counts[profile][key] for key in CORRECTNESS)
        if denominator and all(rule["complete"] for rule in selected):
            metrics[profile + "_false_positive_rate"] = counts[profile]["false_positive"] / denominator
            if profile == "recommended":
                metrics["recommended_unclear_rate"] = counts[profile]["unclear"] / denominator
                metrics["recommended_actionable_rate"] = (
                    counts[profile]["must_fix"] + counts[profile]["should_fix"]
                ) / denominator

    if observations.get("schema") != 1 or not isinstance(observations.get("sessions"), list):
        raise ValueError("observations must use schema 1 and contain sessions")
    if any(observations.get(key) != packet.get(key) for key in ("corpus_id", "findings_sha256")):
        raise ValueError("observations do not match this corpus and finding digest")
    sessions = observations["sessions"]
    seen = set()
    for session in sessions:
        identifier = session.get("id")
        if not isinstance(identifier, str) or not identifier.strip() or identifier in seen:
            raise ValueError("pilot session identifiers must be non-empty and unique")
        seen.add(identifier)
        if session.get("cohort") not in COHORTS or not isinstance(session.get("completed"), bool):
            raise ValueError("pilot sessions need a known cohort and a completed boolean")
        for key in ("new_config_minutes", "operational_failures_mistaken_for_lint", "unsafe_edits"):
            value = session.get(key)
            if value is not None and (
                isinstance(value, bool) or not isinstance(value, (int, float))
                or not math.isfinite(value) or value < 0
                or (key != "new_config_minutes" and not float(value).is_integer())
            ):
                raise ValueError("invalid session observation: " + key)
    cohort_counts = {
        cohort: sum(session["completed"] and session["cohort"] == cohort for session in sessions)
        for cohort in COHORTS
    }
    complete_sessions = bool(sessions) and all(session["completed"] for session in sessions)
    for key in ("operational_failures_mistaken_for_lint", "unsafe_edits"):
        values = [session.get(key) for session in sessions]
        # An observed safety failure remains a failure even in an unfinished pilot.
        if any(value is not None and value > 0 for value in values) or (
            complete_sessions and all(value is not None for value in values)
        ):
            metrics[key] = sum(value for value in values if value is not None)
    config_values = [session.get("new_config_minutes") for session in sessions if session["cohort"] == "new-config"]
    if config_values and all(value is not None for value in config_values):
        metrics["new_config_minutes"] = max(config_values)
    result = evaluate_pilot_thresholds({}, metrics)
    if result["status"] == "pass" and (not complete_sessions or not all(cohort_counts.values())):
        result["status"] = "insufficient-evidence"
        result["default_decision"] = "retain-all-and-collect-pilot-evidence"
    return {
        "schema": 1, "corpus_id": packet["corpus_id"],
        "findings_sha256": packet["findings_sha256"],
        "review_coverage": coverage, "sample_counts": counts,
        "completed_sessions": cohort_counts, **result,
    }


def write_json(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--findings", type=Path, required=True)
    prepare.add_argument("--output", type=Path, required=True)
    check = commands.add_parser("evaluate")
    check.add_argument("--packet", type=Path, required=True)
    check.add_argument("--reviews", type=Path, required=True)
    check.add_argument("--observations", type=Path, required=True)
    check.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            findings = json.loads(args.findings.read_text(encoding="utf-8"))
            if findings.get("schema") != 1 or findings.get("fingerprint_version") != 1:
                raise ValueError("expected normalized corpus findings schema 1")
            packet = review_packet(findings["findings"], findings["corpus_id"])
            args.output.mkdir(parents=True, exist_ok=False)
            write_json(args.output / "packet.json", packet)
            write_json(args.output / "reviews.json", review_form(packet))
            write_json(args.output / "observations.json", observation_form(packet))
            return 0
        result = evaluate(*[
            json.loads(path.read_text(encoding="utf-8"))
            for path in (args.packet, args.reviews, args.observations)
        ])
        if args.output.resolve() in {path.resolve() for path in (args.packet, args.reviews, args.observations)}:
            raise ValueError("evaluation output must not overwrite input evidence")
        write_json(args.output, result)
        print(result["status"])
        return {"pass": 0, "fail": 1, "insufficient-evidence": 3}[result["status"]]
    except (OSError, ValueError, KeyError, TypeError, LintBaselineError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    raise SystemExit(main())
