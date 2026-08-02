#!/usr/bin/env python3
"""Extract standalone bbtidy binaries from the release wheel set."""

import argparse
import stat
import sys
import zipfile
from pathlib import Path


SUPPORTED_PLATFORMS = {
    "manylinux_2_17_x86_64": "linux-x86_64",
    "manylinux_2_17_aarch64": "linux-aarch64",
    "manylinux_2_17_armv7l": "linux-armv7",
    "musllinux_1_2_x86_64": "linux-musl-x86_64",
    "musllinux_1_2_aarch64": "linux-musl-aarch64",
    "macosx_11_0_x86_64": "macos-x86_64",
    "macosx_11_0_arm64": "macos-arm64",
    "win_amd64": "windows-x86_64",
}


def wheel_metadata(path):
    if path.suffix != ".whl":
        raise RuntimeError("not a wheel: {}".format(path))
    parts = path.stem.split("-")
    if len(parts) < 5 or parts[0] != "bbtidy":
        raise RuntimeError("unexpected bbtidy wheel name: {}".format(path.name))
    platform = parts[-1]
    if platform not in SUPPORTED_PLATFORMS:
        raise RuntimeError(
            "unsupported bbtidy wheel platform '{}'; expected one of {}".format(
                platform, ", ".join(sorted(SUPPORTED_PLATFORMS))
            )
        )
    return parts[1], platform


def extract_binary(wheel, output_directory, release_tag):
    _, platform = wheel_metadata(wheel)
    with zipfile.ZipFile(wheel) as archive:
        candidates = [
            name
            for name in archive.namelist()
            if name.rsplit("/", 1)[-1] in {"bbtidy", "bbtidy.exe"}
            and ".data/scripts/" in name
        ]
        if len(candidates) != 1:
            raise RuntimeError(
                "expected one packaged bbtidy executable in {}; found {}".format(
                    wheel.name, len(candidates)
                )
            )
        executable_name = candidates[0].rsplit("/", 1)[-1]
        suffix = ".exe" if executable_name == "bbtidy.exe" else ""
        destination = output_directory / (
            "bbtidy-{}-{}{}".format(release_tag, SUPPORTED_PLATFORMS[platform], suffix)
        )
        destination.write_bytes(archive.read(candidates[0]))
        if suffix == "":
            destination.chmod(
                destination.stat().st_mode
                | stat.S_IXUSR
                | stat.S_IXGRP
                | stat.S_IXOTH
            )
    return destination


def prepare_release_binaries(wheel_directory, output_directory, release_tag):
    wheels = sorted(wheel_directory.glob("bbtidy-*.whl"))
    if not wheels:
        raise RuntimeError("no bbtidy wheels found in {}".format(wheel_directory))

    metadata = [wheel_metadata(wheel) for wheel in wheels]
    versions = {version for version, _ in metadata}
    if len(versions) != 1:
        raise RuntimeError("wheels contain multiple versions: {}".format(", ".join(versions)))

    platforms = [platform for _, platform in metadata]
    duplicates = sorted(
        platform for platform in set(platforms) if platforms.count(platform) > 1
    )
    if duplicates:
        raise RuntimeError("duplicate wheel platforms: {}".format(", ".join(duplicates)))

    expected = set(SUPPORTED_PLATFORMS)
    actual = set(platforms)
    missing = sorted(expected - actual)
    unexpected = sorted(actual - expected)
    if missing or unexpected:
        details = []
        if missing:
            details.append("missing " + ", ".join(missing))
        if unexpected:
            details.append("unexpected " + ", ".join(unexpected))
        raise RuntimeError("wheel platform set does not match release matrix: " + "; ".join(details))

    output_directory.mkdir(parents=True, exist_ok=True)
    return [
        extract_binary(wheel, output_directory, release_tag) for wheel in wheels
    ]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--wheel-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--tag", required=True, help="GitHub release tag used in asset names")
    arguments = parser.parse_args()

    try:
        extracted = prepare_release_binaries(
            arguments.wheel_dir, arguments.output_dir, arguments.tag
        )
    except (OSError, RuntimeError, zipfile.BadZipFile) as error:
        print("error: {}".format(error), file=sys.stderr)
        return 1

    for path in extracted:
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
