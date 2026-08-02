#!/usr/bin/env python3
"""Check bbtidy against pinned OpenEmbedded-Core and meta-openembedded layers."""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = PROJECT_ROOT / "tests" / "upstream-corpus.json"
METADATA_EXTENSIONS = {".bb", ".bbappend", ".bbclass", ".conf", ".inc"}
FUNCTION_START = re.compile(r"^[^ \t#\r\n].*\(\s*\)\s*\{\s*(?:#.*)?(?:\r?\n)?$")
PYTHON_DEF_START = re.compile(r"^def\s+[A-Za-z_][A-Za-z0-9_]*\s*\(.*\)\s*:")
REVISION = re.compile(r"^[0-9a-f]{40}$")


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
        if not name or name in repository_names:
            raise CompatibilityError("repository names must be present and unique")
        if not REVISION.fullmatch(revision):
            raise CompatibilityError(
                "repository {} does not use a full commit revision".format(name)
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

    return manifest


def run(command, cwd=None, environment=None, accepted=(0,)):
    result = subprocess.run(
        [str(argument) for argument in command],
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
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
            repository["revision"],
        ]
    )
    run(["git", "-C", destination, "checkout", "--quiet", "--detach", "FETCH_HEAD"])


def verify_repository(repository, path):
    if not path.is_dir():
        raise CompatibilityError(
            "repository {} is missing at {}".format(repository["name"], path)
        )
    revision = run(["git", "-C", path, "rev-parse", "HEAD"]).stdout.strip()
    if revision != repository["revision"]:
        raise CompatibilityError(
            "repository {} is at {}; expected {}".format(
                repository["name"], revision, repository["revision"]
            )
        )


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


def run_bitbake_parse(formatted_root, build_root, configuration):
    poky = formatted_root / configuration["repository"]
    build_root.mkdir(parents=True)
    script = """
set -e
poky=$1
build=$2
export TEMPLATECONF=$3
target=$4
shift 4
. "$poky/oe-init-build-env" "$build" >/dev/null
for layer in "$@"; do
    bitbake-layers add-layer "$layer"
done
bitbake --parse-only "$target"
"""
    run(
        [
            "bash",
            "-c",
            script,
            "bbtidy-upstream",
            poky,
            build_root,
            poky / configuration["template"],
            configuration["target"],
        ]
        + [formatted_root / path for path in configuration["additional_layers"]]
    )


def check_compatibility(arguments, workspace):
    manifest = load_manifest(arguments.manifest)
    repositories = manifest["repositories"]
    layers = manifest["layers"]
    source_root = (
        arguments.source_root.resolve()
        if arguments.source_root
        else workspace / "sources"
    )

    if arguments.source_root:
        print("Using existing pinned upstream checkouts")
    else:
        source_root.mkdir(parents=True)
        print("Fetching pinned upstream checkouts")
        for repository in repositories:
            print("  {} @ {}".format(repository["name"], repository["revision"][:12]))
            checkout_repository(repository, source_root / repository["name"])

    for repository in repositories:
        verify_repository(repository, source_root / repository["name"])

    formatted_root = workspace / "formatted"
    copy_sources(source_root, formatted_root, repositories)

    print("Discovering real-world metadata")
    metadata, excluded, total_files = verify_layers(source_root, formatted_root, layers)

    inputs = layer_paths(formatted_root, layers)
    print("Formatting {} metadata files".format(total_files))
    formatted = run([arguments.bbtidy, "format", "--write"] + inputs)
    changed_files = sum(
        line.startswith("formatted: ") for line in formatted.stdout.splitlines()
    )

    run([arguments.bbtidy, "check"] + inputs)
    linted = run([arguments.bbtidy, "lint"] + inputs, accepted=(0, 1))
    lint_findings = len([line for line in linted.stdout.splitlines() if line.strip()])

    opaque_count, excluded_count = verify_preservation(
        source_root, formatted_root, layers, metadata, excluded
    )

    parsed = False
    if not arguments.skip_bitbake:
        print("Parsing the formatted layers with BitBake")
        run_bitbake_parse(formatted_root, workspace / "build", manifest["bitbake"])
        parsed = True

    print("Upstream compatibility check passed")
    print("  metadata files: {}".format(total_files))
    print("  files changed on first format: {}".format(changed_files))
    print("  lint diagnostics: {}".format(lint_findings))
    print("  opaque regions preserved: {}".format(opaque_count))
    print("  excluded payload files unchanged: {}".format(excluded_count))
    print("  BitBake parse: {}".format("passed" if parsed else "skipped"))


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
    arguments = parser.parse_args()
    arguments.manifest = arguments.manifest.resolve()
    arguments.bbtidy = arguments.bbtidy.resolve()

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
            check_compatibility(arguments, workspace)
        else:
            with tempfile.TemporaryDirectory(prefix="bbtidy-upstream-") as temporary:
                check_compatibility(arguments, Path(temporary))
    except (CompatibilityError, OSError, UnicodeError) as error:
        report_error(error)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
