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
RUNS_ON = re.compile(r"^\s*runs-on:\s*(?P<value>[^\s#]+)")
RUNNER_CLASS = re.compile(r"--runner-class\s+(?P<value>[^\s\\]+)")
RUNNER_CLASS_INPUT = re.compile(r"runner-class:\s*(?P<value>[^\s#]+)")
RUNNER_CLASS_BY_IMAGE = {
    "ubuntu-22.04": "github-ubuntu-22.04-x86_64",
    "ubuntu-24.04": "github-ubuntu-24.04-x86_64",
}


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


def validate_runner_identity(directory=DEFAULT_WORKFLOW_DIRECTORY):
    """Reject performance evidence labelled for a different GitHub image."""

    errors = []
    if not directory.is_dir():
        return ["workflow directory does not exist: {}".format(directory)]
    for path in sorted(directory.iterdir()):
        if not path.is_file() or path.suffix not in WORKFLOW_SUFFIXES:
            continue
        text = path.read_text(encoding="utf-8")
        jobs = list(re.finditer(r"^  (?P<job>[A-Za-z0-9_-]+):\n", text, re.MULTILINE))
        for index, match in enumerate(jobs):
            block = text[match.end() : jobs[index + 1].start() if index + 1 < len(jobs) else len(text)]
            image = RUNS_ON.search(block)
            if image is None:
                continue
            expected = RUNNER_CLASS_BY_IMAGE.get(image.group("value"))
            if expected is None:
                continue
            labels = [item.group("value") for item in RUNNER_CLASS.finditer(block)]
            labels.extend(item.group("value") for item in RUNNER_CLASS_INPUT.finditer(block))
            for label in labels:
                if label != expected:
                    errors.append(
                        "{} job {} uses {} but disagrees with evidence label {}; expected {}".format(
                            path, match.group("job"), image.group("value"), label, expected
                        )
                    )
    return errors


def _workflow_text(directory, name):
    path = Path(directory) / name
    if not path.is_file():
        raise ValueError("required workflow does not exist: {}".format(path))
    return path.read_text(encoding="utf-8")


def _job_block(text, job):
    marker = "  {}:\n".format(job)
    start = text.find(marker)
    if start < 0:
        return ""
    remainder = text[start + len(marker) :]
    next_job = re.search(r"^  [A-Za-z0-9_-]+:\n", remainder, re.MULTILINE)
    return remainder[: next_job.start()] if next_job else remainder


def validate_release_topology(directory=DEFAULT_WORKFLOW_DIRECTORY):
    """Validate the release graph and the static no-bypass invariants.

    This intentionally uses a small, conservative textual model.  It is a
    regression guard for the security properties of these workflows, while
    actionlint remains responsible for GitHub Actions expression semantics.
    """

    errors = validate_runner_identity(directory)
    try:
        release = _workflow_text(directory, "release.yml")
        gate = _workflow_text(directory, "release-gate.yml")
        crates = _workflow_text(directory, "publish-crates.yml")
        pypi = _workflow_text(directory, "publish-pypi.yml")
    except (OSError, UnicodeError, ValueError) as error:
        return [str(error)]

    tag_workflows = []
    for path in sorted(Path(directory).iterdir()):
        if path.suffix not in WORKFLOW_SUFFIXES or not path.is_file():
            continue
        text = path.read_text(encoding="utf-8")
        if 'tags: ["v*"]' in text:
            tag_workflows.append(path.name)
    if tag_workflows != ["release.yml"]:
        errors.append(
            "exactly one workflow may handle version-tag pushes; found {}".format(
                ", ".join(tag_workflows) or "none"
            )
        )

    for name, text in (("publish-crates.yml", crates), ("publish-pypi.yml", pypi)):
        if "on:\n  workflow_call:" not in text:
            errors.append("{} must expose workflow_call".format(name))
        if re.search(r"^  (push|workflow_dispatch|pull_request|schedule):", text, re.MULTILINE):
            errors.append("{} must not have a direct event trigger".format(name))
        if "github.event_name" in text or "github.ref_type" in text:
            errors.append("{} must not infer publication from the caller event".format(name))

    if "uses: ./.github/workflows/release-gate.yml" not in release:
        errors.append("release.yml must call the blocking release gate")
    if "uses: ./.github/workflows/publish-crates.yml" not in release:
        errors.append("release.yml must call the crates publisher")
    if "uses: ./.github/workflows/publish-pypi.yml" not in release:
        errors.append("release.yml must call the Python publisher")
    if "needs: [metadata, release-gate]" not in release:
        errors.append("publishers must depend on metadata and release-gate")
    if "protected_confirmation" not in release or "environment:\n      name: release-publish" not in release:
        errors.append("manual publication requires an explicit protected input and environment")

    supported = _job_block(gate, "supported-compatibility")
    community = _job_block(gate, "pinned-community")
    if not supported or "tests/upstream-corpora/yocto-5.0-scarthgap.json" not in supported or "tests/upstream-corpora/yocto-6.0-wrynose.json" not in supported:
        errors.append("blocking compatibility must include both supported corpora")
    if "--skip-bitbake" in supported:
        errors.append("supported compatibility cannot skip BitBake")
    if "tests/upstream-corpora/community-master.json" not in community:
        errors.append("blocking compatibility must include pinned-community")
    if (
        "BBTIDY_PERFORMANCE_SOURCE_ROOT" not in supported
        or '"$performance_root"' not in supported
    ):
        errors.append(
            "supported performance evidence must benchmark manifest-declared layers"
        )
    if (
        'build_dir="compatibility-workspace/build-original"' not in supported
        or 'bitbake/bin/bitbake' not in supported
        or 'test -f "$build_dir/conf/bblayers.conf"' not in supported
    ):
        errors.append(
            "supported performance evidence must use deterministic compatibility paths"
        )
    for job in ("supported-compatibility", "pinned-community"):
        if "continue-on-error" in _job_block(gate, job):
            errors.append("blocking job {} cannot continue-on-error".format(job))
    if "if-no-files-found: error" not in gate:
        errors.append("release evidence uploads must fail when files are missing")
    if "scripts/verify_release_evidence.py" not in gate:
        errors.append("release gate must verify consolidated evidence")
    if "if: always()" not in gate:
        errors.append("raw evidence uploads must run after failures")

    if "id-token: write" not in crates or "id-token: write" not in pypi:
        errors.append("publisher trusted-publishing jobs must request only an OIDC token")
    if "pypa/gh-action-pypi-publish@" not in pypi:
        errors.append("PyPI publisher must use trusted publishing")
    if "rust-lang/crates-io-auth-action@" not in crates:
        errors.append("crates publisher must use crates.io trusted publishing")
    return errors


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--workflow-dir", type=Path, default=DEFAULT_WORKFLOW_DIRECTORY)
    arguments = parser.parse_args(argv)

    errors = validate_workflow_directory(arguments.workflow_dir)
    errors.extend(validate_release_topology(arguments.workflow_dir))
    if errors:
        for error in errors:
            print("error: {}".format(error), file=sys.stderr)
        return 1
    print("validated immutable action pins in {}".format(arguments.workflow_dir))
    return 0


if __name__ == "__main__":
    sys.exit(main())
