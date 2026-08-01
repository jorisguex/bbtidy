#!/usr/bin/env python3
"""Validate Cargo, Python, and release-tag versions for bbtidy."""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
SEMVER = re.compile(
    r"^(?P<base>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)"
    r"(?:-(?P<prerelease>alpha|beta|rc)\.(?P<number>0|[1-9]\d*))?$"
)


def cargo_version():
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ],
        cwd=PROJECT_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(result.stdout)
    packages = [
        package for package in metadata["packages"] if package["name"] == "bbtidy"
    ]
    if len(packages) != 1:
        raise RuntimeError("expected exactly one bbtidy package in Cargo metadata")
    return packages[0]["version"]


def pep440_version(version):
    match = SEMVER.fullmatch(version)
    if not match:
        raise ValueError(
            "version must be X.Y.Z or X.Y.Z-(alpha|beta|rc).N "
            "so it maps unambiguously to PEP 440"
        )
    base = ".".join([match.group("base"), match.group("minor"), match.group("patch")])
    prerelease = match.group("prerelease")
    if not prerelease:
        return base
    marker = {"alpha": "a", "beta": "b", "rc": "rc"}[prerelease]
    return "{}{}{}".format(base, marker, match.group("number"))


def release_tag_from_environment():
    if os.environ.get("GITHUB_REF_TYPE") == "tag":
        return os.environ.get("GITHUB_REF_NAME")
    return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--tag",
        help="release tag to validate; GitHub tag context is used when omitted",
    )
    arguments = parser.parse_args()

    version = cargo_version()
    python_version = pep440_version(version)
    tag = arguments.tag or release_tag_from_environment()
    expected_tag = "v{}".format(version)

    if tag and tag != expected_tag:
        print(
            "error: release tag {!r} does not match Cargo version; expected {!r}".format(
                tag, expected_tag
            ),
            file=sys.stderr,
        )
        return 1

    print("Cargo version: {}".format(version))
    print("Python version: {}".format(python_version))
    if tag:
        print("Release tag: {}".format(tag))
    return 0


if __name__ == "__main__":
    sys.exit(main())
