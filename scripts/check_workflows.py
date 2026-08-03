#!/usr/bin/env python3
"""Reject mutable third-party GitHub Actions references."""

import argparse
import re
import sys
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_WORKFLOW_DIRECTORY = PROJECT_ROOT / ".github" / "workflows"
WORKFLOW_SUFFIXES = {".yaml", ".yml"}
USES = re.compile(r"^\s*(?:-\s*)?uses:\s*(?P<value>.+?)\s*$")
COMMIT_SHA = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST = re.compile(r"^docker://[^@\s]+@sha256:[0-9a-f]{64}$")


def action_reference_error(value):
    """Returns an explanation when a ``uses`` value is not immutable."""

    value = value.split("#", 1)[0].strip()
    if not value:
        return "uses entry is empty"
    if value.startswith("./"):
        return None
    if value.startswith("docker://"):
        if IMAGE_DIGEST.fullmatch(value):
            return None
        return "container actions must use a sha256 image digest"

    action, separator, reference = value.rpartition("@")
    if not separator or not action or not reference:
        return "actions must use an explicit full commit SHA"
    if not COMMIT_SHA.fullmatch(reference):
        return "actions must use a 40-character lowercase commit SHA"
    return None


def validate_workflow(path):
    """Returns immutable-reference errors for one workflow file."""

    errors = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        match = USES.match(line)
        if match is None:
            continue
        error = action_reference_error(match.group("value"))
        if error:
            errors.append("{}:{}: {}".format(path, number, error))
    return errors


def validate_workflow_directory(directory):
    """Returns immutable-reference errors for all workflow files in a directory."""

    if not directory.is_dir():
        return ["workflow directory does not exist: {}".format(directory)]
    errors = []
    for path in sorted(directory.iterdir()):
        if path.is_file() and path.suffix in WORKFLOW_SUFFIXES:
            errors.extend(validate_workflow(path))
    return errors


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--workflow-dir", type=Path, default=DEFAULT_WORKFLOW_DIRECTORY)
    arguments = parser.parse_args(argv)

    errors = validate_workflow_directory(arguments.workflow_dir)
    if errors:
        for error in errors:
            print("error: {}".format(error), file=sys.stderr)
        return 1
    print("validated immutable action pins in {}".format(arguments.workflow_dir))
    return 0


if __name__ == "__main__":
    sys.exit(main())
