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


def report_error(error):
    message = "error: {}".format(error)
    print(message, file=sys.stderr)
    if os.environ.get("GITHUB_ACTIONS") == "true":
        print(
            "::error title=Upstream compatibility failed::{}".format(
                workflow_command_value(message)
            )
        )


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


def check_compatibility(arguments, workspace, evidence_dir):
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
        [arguments.bbtidy, "check"] + inputs,
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
        [arguments.bbtidy, "lint"] + inputs,
        "lint metadata",
        records,
        accepted=(0, 1),
        log_path=evidence_dir / "logs" / "lint.log",
    )
    lint_findings = len([line for line in linted.stdout.splitlines() if line.strip()])

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
            "lint_diagnostics": lint_findings,
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
    print("  lint diagnostics: {}".format(lint_findings))
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
