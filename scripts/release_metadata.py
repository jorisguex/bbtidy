#!/usr/bin/env python3
"""Load and validate the checked-in release artifact contract."""

import argparse
import json
import sys
from pathlib import Path

try:
    from scripts.check_release_version import (
        cargo_version,
        pep440_version,
        release_tag_from_environment,
    )
except ImportError:
    from check_release_version import (  # type: ignore
        cargo_version,
        pep440_version,
        release_tag_from_environment,
    )


PROJECT_ROOT = Path(__file__).resolve().parents[1]
METADATA_PATH = PROJECT_ROOT / "release-metadata.json"
MATRIX_FIELDS = ("name", "runner", "target", "manylinux", "smoke_test")
REQUIRED_FIELDS = MATRIX_FIELDS + (
    "wheel_platform",
    "binary_asset",
    "binary_extension",
)


def load_release_metadata(path=METADATA_PATH):
    try:
        metadata = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError("could not read release metadata: {}".format(error)) from error
    validate_release_metadata(metadata)
    return metadata


def validate_release_metadata(metadata):
    if not isinstance(metadata, dict):
        raise ValueError("release metadata must be a JSON object")
    if metadata.get("schema_version") != 1:
        raise ValueError("release metadata schema_version must be 1")

    package = metadata.get("package")
    if not isinstance(package, dict):
        raise ValueError("release metadata must contain a package object")
    if package.get("name") != "bbtidy":
        raise ValueError("release metadata package name must be 'bbtidy'")
    if package.get("tag_prefix") != "v":
        raise ValueError("release metadata tag_prefix must be 'v'")

    entries = metadata.get("wheel_matrix")
    if not isinstance(entries, list) or not entries:
        raise ValueError("release metadata wheel_matrix must be a non-empty array")

    names = set()
    wheel_platforms = set()
    binary_assets = set()
    for entry in entries:
        if not isinstance(entry, dict):
            raise ValueError("each release matrix entry must be an object")
        missing = [field for field in REQUIRED_FIELDS if field not in entry]
        if missing:
            raise ValueError(
                "release matrix entry is missing: {}".format(", ".join(missing))
            )
        for field in REQUIRED_FIELDS:
            if field == "smoke_test":
                valid_type = isinstance(entry[field], bool)
            else:
                valid_type = isinstance(entry[field], str)
            if not valid_type:
                raise ValueError("release matrix field '{}' has an invalid type".format(field))
        if entry["name"] in names:
            raise ValueError("duplicate release matrix name: {}".format(entry["name"]))
        if entry["wheel_platform"] in wheel_platforms:
            raise ValueError(
                "duplicate wheel platform: {}".format(entry["wheel_platform"])
            )
        if entry["binary_asset"] in binary_assets:
            raise ValueError(
                "duplicate release binary asset: {}".format(entry["binary_asset"])
            )
        names.add(entry["name"])
        wheel_platforms.add(entry["wheel_platform"])
        binary_assets.add(entry["binary_asset"])

        extension = entry.get("binary_extension")
        if extension not in ("", ".exe"):
            raise ValueError(
                "release binary extension must be empty or '.exe': {}".format(
                    entry["name"]
                )
            )
        if entry["wheel_platform"] == "win_amd64" and extension != ".exe":
            raise ValueError("Windows release binary must use the .exe extension")
        if entry["wheel_platform"] != "win_amd64" and extension:
            raise ValueError("non-Windows release binaries cannot use an extension")

        is_linux = entry["wheel_platform"].startswith(("manylinux_", "musllinux_"))
        container = entry.get("container")
        if is_linux:
            if not isinstance(container, dict):
                raise ValueError("Linux release entries require a container verifier")
            if not isinstance(container.get("platform"), str) or not isinstance(
                container.get("image"), str
            ):
                raise ValueError("Linux container verifiers require platform and image")
        elif container is not None:
            raise ValueError("non-Linux release entries cannot define a container verifier")

    return metadata


def wheel_matrix(metadata=None):
    metadata = metadata or load_release_metadata()
    return [
        {field: entry[field] for field in MATRIX_FIELDS}
        for entry in metadata["wheel_matrix"]
    ]


def wheel_entries(metadata=None):
    metadata = metadata or load_release_metadata()
    return metadata["wheel_matrix"]


def supported_platforms(metadata=None):
    return {
        entry["wheel_platform"]: entry["binary_asset"]
        for entry in wheel_entries(metadata)
    }


def release_context(tag=None):
    metadata = load_release_metadata()
    version = cargo_version()
    python_version = pep440_version(version)
    release_tag = tag or release_tag_from_environment()
    expected_tag = metadata["package"]["tag_prefix"] + version
    if release_tag and release_tag != expected_tag:
        raise ValueError(
            "release tag {!r} does not match Cargo version; expected {!r}".format(
                release_tag, expected_tag
            )
        )
    return metadata, version, python_version, release_tag


def linux_smoke_plan(metadata, asset_tag, version):
    plan = []
    for entry in wheel_entries(metadata):
        container = entry.get("container")
        if container is None:
            continue
        binary = "bbtidy-{}-{}{}".format(
            asset_tag, entry["binary_asset"], entry["binary_extension"]
        )
        plan.append(
            {
                "platform": container["platform"],
                "image": container["image"],
                "binary": binary,
                "expected_version": "bbtidy {}".format(version),
            }
        )
    return plan


def write_github_output(path, metadata):
    matrix = json.dumps(wheel_matrix(metadata), separators=(",", ":"))
    with path.open("a", encoding="utf-8") as output:
        output.write("matrix={}\n".format(matrix))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--tag",
        help="release tag to validate; GitHub tag context is used when omitted",
    )
    parser.add_argument(
        "--github-output",
        type=Path,
        help="write the validated wheel matrix as a GitHub Actions output",
    )
    parser.add_argument(
        "--linux-smoke-plan",
        action="store_true",
        help="print tab-separated Linux container smoke-test records",
    )
    parser.add_argument(
        "--asset-tag",
        help="tag used in binary asset names for --linux-smoke-plan",
    )
    arguments = parser.parse_args()

    try:
        metadata, version, python_version, release_tag = release_context(arguments.tag)
        if arguments.github_output:
            write_github_output(arguments.github_output, metadata)
        if arguments.linux_smoke_plan:
            if not arguments.asset_tag:
                raise ValueError("--asset-tag is required with --linux-smoke-plan")
            for item in linux_smoke_plan(metadata, arguments.asset_tag, version):
                print(
                    "\t".join(
                        [
                            item["platform"],
                            item["image"],
                            item["binary"],
                            item["expected_version"],
                        ]
                    )
                )
        elif not arguments.github_output:
            print("Cargo version: {}".format(version))
            print("Python version: {}".format(python_version))
            if release_tag:
                print("Release tag: {}".format(release_tag))
        return 0
    except (OSError, RuntimeError, ValueError) as error:
        print("error: {}".format(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
