#!/usr/bin/env python3
"""Verify and consolidate the blocking compatibility evidence for a release.

The compatibility harness deliberately writes one self-contained directory per
corpus.  This verifier is the trust boundary between those debug artifacts and
the durable archive attached to a release: it checks identity, required files,
blocking results, lint fingerprints, and archive safety before creating output.
"""

import argparse
import gzip
import hashlib
import json
import re
import sys
import tarfile
from pathlib import Path, PurePosixPath

try:
    from scripts.check_upstream_corpus import load_manifest
    from scripts.lint_quality import (
        KNOWN_RULE_IDS,
        summarize_findings,
        load_lint_baseline,
    )
    from scripts.check_performance_budget import BudgetError, compare_record, load_budgets
    from scripts.performance_schema import PerformanceSchemaError, load_evidence
except ImportError:  # pragma: no cover - direct script execution
    from check_upstream_corpus import load_manifest  # type: ignore
    from lint_quality import (  # type: ignore
        KNOWN_RULE_IDS,
        summarize_findings,
        load_lint_baseline,
    )
    from check_performance_budget import BudgetError, compare_record, load_budgets  # type: ignore
    from performance_schema import PerformanceSchemaError, load_evidence  # type: ignore


PROJECT_ROOT = Path(__file__).resolve().parents[1]
REQUIRED_FILES = (
    "manifest.json",
    "summary.json",
    "commands.json",
    "metrics/source.json",
    "metrics/formatted.json",
    "lint/findings.json",
    "lint/summary.json",
    "lint/baseline-comparison.json",
)
REQUIRED_PARSE_LOGS = ("logs/original-parse.log", "logs/formatted-parse.log")


class EvidenceError(RuntimeError):
    """A release evidence bundle is incomplete, unsafe, or inconsistent."""


def _json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise EvidenceError("could not read JSON {}: {}".format(path, error)) from error


def _safe_relative(value, label):
    if (
        not isinstance(value, str)
        or not value
        or "\\" in value
        or "\x00" in value
        or re.match(r"^[A-Za-z]:/", value)
    ):
        raise EvidenceError("{} is not a safe relative path: {!r}".format(label, value))
    path = PurePosixPath(value)
    if path.is_absolute() or "." in path.parts or ".." in path.parts:
        raise EvidenceError("{} is not a safe relative path: {}".format(label, value))
    return path


def _required_file(bundle, relative, non_empty=True):
    relative_path = _safe_relative(relative, "evidence member")
    path = bundle.joinpath(*relative_path.parts)
    if not path.is_file() or path.is_symlink():
        raise EvidenceError("missing required evidence file: {}".format(relative))
    if non_empty and path.stat().st_size == 0:
        raise EvidenceError("required evidence file is empty: {}".format(relative))
    return path


def _manifest_identity(manifest):
    return {
        "schema": manifest.get("schema"),
        "id": manifest.get("id"),
        "tier": manifest.get("tier"),
        "yocto_version": manifest.get("yocto_version"),
        "bitbake_version": manifest.get("bitbake_version"),
        "repositories": [
            {"name": item.get("name"), "revision": item.get("revision")}
            for item in manifest.get("repositories", [])
        ],
    }


def _version_matches(recorded, expected):
    if not isinstance(recorded, str):
        return False
    value = recorded.strip()
    return value == expected or value == "bbtidy {}".format(expected) or value.endswith(
        " {}".format(expected)
    )


def _assert_repository_identity(summary, manifest):
    recorded = summary.get("repositories")
    if not isinstance(recorded, dict):
        raise EvidenceError("summary has no repository identity")
    expected = {
        repository["name"]: repository.get("revision")
        for repository in manifest["repositories"]
    }
    for name, revision in expected.items():
        item = recorded.get(name)
        if not isinstance(item, dict) or item.get("expected_revision") != revision:
            raise EvidenceError("repository identity mismatch for {}".format(name))
        if revision and item.get("resolved_revision") != revision:
            raise EvidenceError("repository {} resolved to the wrong revision".format(name))


def _assert_metrics(metrics, label):
    if not isinstance(metrics, dict) or metrics.get("version") != 1:
        raise EvidenceError("{} metrics have an invalid schema".format(label))
    for field in ("files", "structured_nodes", "total_nodes", "trivia_nodes", "unknown_bytes", "unknown_nodes"):
        if isinstance(metrics.get(field), bool) or not isinstance(metrics.get(field), int) or metrics[field] < 0:
            raise EvidenceError("{} metrics field {} is invalid".format(label, field))


def _assert_lint(bundle, manifest):
    findings = _json(_required_file(bundle, "lint/findings.json"))
    lint_summary = _json(_required_file(bundle, "lint/summary.json"))
    comparison = _json(_required_file(bundle, "lint/baseline-comparison.json"))
    if findings.get("schema") != 1 or findings.get("corpus_id") != manifest["id"]:
        raise EvidenceError("lint findings have the wrong corpus identity")
    if not isinstance(findings.get("findings"), list):
        raise EvidenceError("lint findings are not an array")
    if findings.get("fingerprint_version") != 1:
        raise EvidenceError("lint findings use an unsupported fingerprint version")
    if lint_summary.get("corpus_id") != manifest["id"]:
        raise EvidenceError("lint summary has the wrong corpus identity")
    derived = summarize_findings(findings["findings"], KNOWN_RULE_IDS)
    for field in ("total_findings", "findings_sha256", "files_with_findings", "severity_counts", "rules"):
        if lint_summary.get(field) != derived.get(field):
            raise EvidenceError("lint summary does not match lint findings: {}".format(field))

    blocking = manifest["tier"] in {"supported", "pinned-community"}
    if blocking:
        if comparison.get("status") != "matched":
            raise EvidenceError("lint baseline comparison did not match")
        if comparison.get("blocking_failures"):
            raise EvidenceError("lint baseline comparison has blocking failures")
        if comparison.get("review_failures"):
            raise EvidenceError("lint baseline review has failures")
        lint_quality = manifest.get("lint_quality") or {}
        baseline_path = PROJECT_ROOT / "tests" / "upstream-corpora" / lint_quality["baseline"]
        baseline = load_lint_baseline(baseline_path, manifest)
        measurement = baseline["measurement"]
        current_rules = {
            rule_id: {
                field: rule[field]
                for field in ("count", "files", "findings_sha256", "severity_counts")
            }
            for rule_id, rule in lint_summary["rules"].items()
        }
        current_measurement = {
            field: lint_summary[field]
            for field in (
                "total_findings",
                "findings_sha256",
                "files_with_findings",
                "severity_counts",
            )
        }
        current_measurement["rules"] = current_rules
        for field in current_measurement:
            if current_measurement[field] != measurement.get(field):
                raise EvidenceError("lint fingerprint does not match checked-in baseline: {}".format(field))


def validate_performance_evidence(performance_root, budget_path, source_commit, version):
    """Validate the consolidated performance evidence alongside release evidence."""

    root = Path(performance_root).resolve()
    for path in root.rglob("*"):
        if path.is_symlink():
            raise EvidenceError("performance evidence contains a symbolic link: {}".format(path))
        if path.is_file():
            _safe_relative(path.relative_to(root).as_posix(), "performance member")
    required = ("manifest.json", "budgets.json", "summary.json")
    for relative in required:
        _required_file(root, relative)
    manifest = _json(root / "manifest.json")
    if manifest.get("schema") != 1 or manifest.get("kind") != "bbtidy-performance-release":
        raise EvidenceError("performance manifest has an unsupported schema")
    if manifest.get("source_commit") != source_commit or manifest.get("version") != version:
        raise EvidenceError("performance evidence identity does not match the release")
    budget = load_budgets(root / "budgets.json")
    if Path(budget_path).read_bytes() != (root / "budgets.json").read_bytes():
        raise EvidenceError("performance evidence does not contain the checked-in budget policy")
    summary = _json(root / "summary.json")
    if summary.get("schema") != 1 or summary.get("status") != "passed":
        raise EvidenceError("performance summary is not passed")
    if summary.get("source_commit") != source_commit or summary.get("version") != version:
        raise EvidenceError("performance summary identity does not match the release")
    records = manifest.get("records")
    if not isinstance(records, list) or not records:
        raise EvidenceError("performance manifest has no records")
    reports = []
    for relative in records:
        _safe_relative(relative, "performance record")
        path = root / relative
        if not path.is_file():
            raise EvidenceError("missing performance record: {}".format(relative))
        try:
            evidence = load_evidence(path)
        except (PerformanceSchemaError, OSError, UnicodeError, ValueError) as error:
            raise EvidenceError("invalid performance record {}: {}".format(relative, error)) from error
        record_list = evidence.get("records") if evidence.get("kind") == "bbtidy-performance-suite" else [evidence]
        for record in record_list:
            if record.get("commit") != source_commit:
                raise EvidenceError("performance record uses the wrong source commit")
            if not _version_matches(record.get("version"), version):
                raise EvidenceError("performance record uses the wrong bbtidy version")
            if record["runner"]["class"] != budget["runner_class"]:
                raise EvidenceError("performance record uses the wrong runner class")
            if record["summary"]["status"] != "success":
                raise EvidenceError("performance record did not complete successfully")
            try:
                comparison = compare_record(record, budget)
            except (BudgetError, KeyError, TypeError, ValueError) as error:
                raise EvidenceError("performance budget comparison failed: {}".format(error)) from error
            if comparison["failures"]:
                raise EvidenceError("performance budget has blocking failures")
            reports.append({"path": relative, "workload": record["workload"], "comparison": comparison})
    reports.sort(key=lambda report: (report["path"], report["workload"]))
    if summary.get("records") != reports:
        raise EvidenceError("performance summary does not match record comparisons")
    return {
        "status": "passed",
        "source_commit": source_commit,
        "version": version,
        "runner_class": budget["runner_class"],
        "records": reports,
    }


def validate_evidence_bundle(bundle, expected_manifest, source_commit, version):
    """Validate one extracted compatibility artifact directory."""

    bundle = Path(bundle)
    for path in bundle.rglob("*"):
        if path.is_symlink():
            raise EvidenceError("evidence bundle contains a symbolic link: {}".format(path))
        if path.is_file():
            _safe_relative(path.relative_to(bundle).as_posix(), "evidence member")

    actual_manifest_path = _required_file(bundle, "manifest.json")
    actual_manifest = _json(actual_manifest_path)
    if actual_manifest != _json(expected_manifest):
        raise EvidenceError("checked-in manifest does not match evidence manifest for {}".format(expected_manifest))
    manifest = load_manifest(expected_manifest)
    summary = _json(_required_file(bundle, "summary.json"))
    if summary.get("schema") != 1 or summary.get("status") != "passed":
        raise EvidenceError("compatibility summary is not passed")
    corpus = summary.get("corpus")
    if not isinstance(corpus, dict) or corpus.get("id") != manifest["id"] or corpus.get("tier") != manifest["tier"]:
        raise EvidenceError("compatibility summary has the wrong corpus identity")
    if corpus.get("yocto_version") != manifest["yocto_version"] or corpus.get("bitbake_version") != manifest["bitbake_version"]:
        raise EvidenceError("compatibility summary has the wrong upstream version")
    bbtidy = summary.get("bbtidy")
    if not isinstance(bbtidy, dict) or bbtidy.get("source_revision") != source_commit:
        raise EvidenceError("evidence source commit does not match the tagged commit")
    if not _version_matches(bbtidy.get("version"), version):
        raise EvidenceError("evidence bbtidy version does not match the release version")
    _assert_repository_identity(summary, manifest)

    results = summary.get("results")
    if not isinstance(results, dict):
        raise EvidenceError("compatibility summary has no results")
    for field in (
        "metadata_files",
        "files_changed_on_first_format",
        "opaque_regions_preserved",
        "excluded_payload_files_unchanged",
    ):
        if (
            isinstance(results.get(field), bool)
            or not isinstance(results.get(field), int)
            or results[field] < 0
        ):
            raise EvidenceError("compatibility results are missing {}".format(field))
    result_lint = results.get("lint_quality")
    if not isinstance(result_lint, dict):
        raise EvidenceError("compatibility results have no lint-quality result")
    result_comparison = result_lint.get("baseline_comparison")
    if manifest["tier"] in {"supported", "pinned-community"} and (
        not isinstance(result_comparison, dict)
        or result_comparison.get("status") != "matched"
        or result_comparison.get("blocking_failures")
    ):
        raise EvidenceError("compatibility lint-quality result is not passed")
    parse_status = results.get("bitbake_differential_parse")
    if manifest["tier"] == "supported" and parse_status != "passed":
        raise EvidenceError("supported corpus did not pass the BitBake differential parse")
    for relative in REQUIRED_FILES:
        _required_file(bundle, relative)
    for relative in REQUIRED_PARSE_LOGS:
        if manifest["tier"] == "supported":
            _required_file(bundle, relative)
    source_metrics = _json(_required_file(bundle, "metrics/source.json"))
    formatted_metrics = _json(_required_file(bundle, "metrics/formatted.json"))
    _assert_metrics(source_metrics, "source")
    _assert_metrics(formatted_metrics, "formatted")
    if source_metrics["files"] != formatted_metrics["files"]:
        raise EvidenceError("source and formatted metrics cover different file counts")
    if results.get("source_metrics") != source_metrics or results.get("formatted_metrics") != formatted_metrics:
        raise EvidenceError("compatibility summary metrics do not match metric evidence")
    if results["metadata_files"] != source_metrics["files"]:
        raise EvidenceError("compatibility metadata count does not match metric evidence")
    _assert_lint(bundle, manifest)

    probes = manifest.get("bitbake", {}).get("semantic_probes", [])
    if probes:
        semantic = _json(_required_file(bundle, "semantic.json"))
        if semantic.get("schema") != 1 or semantic.get("status") != "passed":
            raise EvidenceError("semantic probe evidence is not passed")
        if not semantic.get("original") or not semantic.get("formatted"):
            raise EvidenceError("semantic probe evidence is incomplete")

    commands = _json(_required_file(bundle, "commands.json"))
    if not isinstance(commands, list) or not commands:
        raise EvidenceError("commands evidence is empty")
    for command in commands:
        if not isinstance(command, dict) or command.get("exit_code") != 0:
            raise EvidenceError("commands evidence contains a failed command")
        log = command.get("log")
        if log:
            log_path = _required_file(bundle, log)
            if not log_path.read_text(encoding="utf-8"):
                raise EvidenceError("command log is empty: {}".format(log))

    return {
        "id": manifest["id"],
        "tier": manifest["tier"],
        "manifest": _manifest_identity(manifest),
        "source_commit": source_commit,
        "version": version,
        "bundle": manifest["id"],
        "files": sorted(
            path.relative_to(bundle).as_posix()
            for path in bundle.rglob("*")
            if path.is_file()
        ),
    }


def _sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_archive_members(names):
    """Reject absolute, traversing, duplicate, or platform-specific members."""

    seen = set()
    for name in names:
        path = _safe_relative(name, "archive member")
        if name in seen or path.as_posix() != name:
            raise EvidenceError("unsafe or duplicate archive member: {}".format(name))
        seen.add(name)


def _add_bytes(tar, name, data):
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mode = 0o644
    info.mtime = 0
    tar.addfile(info, __import__("io").BytesIO(data))


def create_evidence_archive(root, bundles, index, output, checksums, performance_root=None):
    """Create a deterministic tarball and a checksum entry for it."""

    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    archive_names = ["evidence-index.json"]
    source_files = []
    for bundle in bundles:
        bundle = Path(bundle)
        corpus_id = _json(bundle / "manifest.json")["id"]
        for path in sorted(bundle.rglob("*")):
            if not path.is_file():
                continue
            relative = path.relative_to(bundle).as_posix()
            archive_names.append("evidence/{}/{}".format(corpus_id, relative))
            source_files.append((path, archive_names[-1]))
    if performance_root is not None:
        performance_root = Path(performance_root)
        for path in sorted(performance_root.rglob("*")):
            if not path.is_file():
                continue
            archive_names.append("performance/{}".format(path.relative_to(performance_root).as_posix()))
            source_files.append((path, archive_names[-1]))
    validate_archive_members(archive_names)
    index_bytes = (json.dumps(index, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8")
    with output.open("wb") as raw:
        with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                _add_bytes(archive, "evidence-index.json", index_bytes)
                for path, name in source_files:
                    _add_bytes(archive, name, path.read_bytes())
    digest = _sha256(output)
    checksums = Path(checksums)
    checksums.parent.mkdir(parents=True, exist_ok=True)
    checksums.write_text("{}  {}\n".format(digest, output.name), encoding="utf-8")
    return digest


def verify_release_evidence(
    evidence_root,
    manifests,
    source_commit,
    version,
    output=None,
    checksums=None,
    performance_root=None,
    performance_budget=None,
    require_performance=False,
):
    """Verify all blocking corpora exactly once and optionally archive them."""

    root = Path(evidence_root).resolve()
    if not root.is_dir():
        raise EvidenceError("evidence root does not exist: {}".format(root))
    expected = {}
    for manifest_path in manifests:
        manifest_path = Path(manifest_path).resolve()
        manifest = _json(manifest_path)
        if manifest.get("tier") not in {"supported", "pinned-community"}:
            continue
        corpus_id = manifest.get("id")
        if corpus_id in expected:
            raise EvidenceError("duplicate expected corpus: {}".format(corpus_id))
        expected[corpus_id] = manifest_path

    candidates = []
    for manifest_path in sorted(root.rglob("manifest.json")):
        if not manifest_path.is_file():
            continue
        manifest = _json(manifest_path)
        corpus_id = manifest.get("id")
        candidates.append((corpus_id, manifest_path.parent))
    found = {}
    for corpus_id, bundle in candidates:
        if corpus_id in found:
            raise EvidenceError("duplicate evidence for corpus: {}".format(corpus_id))
        found[corpus_id] = bundle
    missing = sorted(set(expected) - set(found))
    unexpected = sorted(set(found) - set(expected))
    if missing:
        raise EvidenceError("missing corpus evidence: {}".format(", ".join(missing)))
    if unexpected:
        raise EvidenceError("unexpected corpus evidence: {}".format(", ".join(unexpected)))

    reports = []
    bundles = []
    for corpus_id in sorted(expected):
        report = validate_evidence_bundle(found[corpus_id], expected[corpus_id], source_commit, version)
        reports.append(report)
        bundles.append(found[corpus_id])
    index = {
        "schema": 1,
        "status": "passed",
        "version": version,
        "source_commit": source_commit,
        "corpora": reports,
    }
    if require_performance and performance_root is None:
        raise EvidenceError("performance evidence is required for this release gate")
    if performance_root is not None:
        if performance_budget is None:
            raise EvidenceError("performance budget path is required with performance evidence")
        index["performance"] = validate_performance_evidence(
            performance_root, performance_budget, source_commit, version
        )
    if output is not None:
        if checksums is None:
            checksums = str(Path(output).with_suffix(Path(output).suffix + ".sha256"))
        index["archive_sha256"] = create_evidence_archive(
            root, bundles, index, output, checksums, performance_root
        )
        Path(str(output) + ".index.json").write_text(
            json.dumps(index, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
            encoding="utf-8",
        )
    return index


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence-root", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, action="append", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--checksums", type=Path)
    parser.add_argument("--performance-root", type=Path)
    parser.add_argument("--performance-budget", type=Path)
    parser.add_argument("--require-performance", action="store_true")
    arguments = parser.parse_args(argv)
    try:
        index = verify_release_evidence(
            arguments.evidence_root,
            arguments.manifest,
            arguments.source_commit,
            arguments.version,
            arguments.output,
            arguments.checksums,
            arguments.performance_root,
            arguments.performance_budget,
            arguments.require_performance,
        )
    except (EvidenceError, OSError, UnicodeError, ValueError) as error:
        print("error: {}".format(error), file=sys.stderr)
        return 1
    print("verified release evidence for {} corpora".format(len(index["corpora"])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
