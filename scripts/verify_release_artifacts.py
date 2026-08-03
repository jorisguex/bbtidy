#!/usr/bin/env python3
"""Verify the exact wheel and source-distribution set before publication."""

import argparse
import sys
import tarfile
import zipfile
from email.parser import Parser
from pathlib import Path

try:
    from scripts.prepare_release_binaries import wheel_metadata
    from scripts.release_metadata import release_context, supported_platforms
except ImportError:
    from prepare_release_binaries import wheel_metadata  # type: ignore
    from release_metadata import release_context, supported_platforms  # type: ignore


PACKAGE_NAME = "bbtidy"


def parse_metadata(raw):
    try:
        message = Parser().parsestr(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as error:
        raise RuntimeError("invalid package metadata: {}".format(error)) from error
    return message


def verify_package_metadata(raw, expected_version, source):
    metadata = parse_metadata(raw)
    if metadata.get("Name") != PACKAGE_NAME:
        raise RuntimeError("{} has unexpected package name".format(source))
    if metadata.get("Version") != expected_version:
        raise RuntimeError(
            "{} has version {!r}; expected {!r}".format(
                source, metadata.get("Version"), expected_version
            )
        )


def verify_wheel(path, expected_version, expected_extension=None):
    version, platform = wheel_metadata(path)
    if version != expected_version:
        raise RuntimeError(
            "{} has version {!r}; expected {!r}".format(
                path.name, version, expected_version
            )
        )
    try:
        with zipfile.ZipFile(path) as archive:
            metadata_files = [
                name
                for name in archive.namelist()
                if name.endswith(".dist-info/METADATA")
            ]
            if len(metadata_files) != 1:
                raise RuntimeError(
                    "expected one dist-info/METADATA file in {}; found {}".format(
                        path.name, len(metadata_files)
                    )
                )
            verify_package_metadata(
                archive.read(metadata_files[0]), expected_version, path.name
            )
            executables = [
                name
                for name in archive.namelist()
                if name.rsplit("/", 1)[-1] in {"bbtidy", "bbtidy.exe"}
                and ".data/scripts/" in name
            ]
            if len(executables) != 1:
                raise RuntimeError(
                    "expected one packaged bbtidy executable in {}; found {}".format(
                        path.name, len(executables)
                    )
                )
            if expected_extension is not None:
                expected_executable = "bbtidy{}".format(expected_extension)
                actual_executable = executables[0].rsplit("/", 1)[-1]
                if actual_executable != expected_executable:
                    raise RuntimeError(
                        "{} contains {}; expected {}".format(
                            path.name, actual_executable, expected_executable
                        )
                    )
    except (OSError, zipfile.BadZipFile) as error:
        raise RuntimeError("could not read wheel {}: {}".format(path.name, error)) from error
    return platform


def verify_sdist(path, expected_version):
    expected_name = "{}-{}.tar.gz".format(PACKAGE_NAME, expected_version)
    if path.name != expected_name:
        raise RuntimeError(
            "unexpected source distribution {}; expected {}".format(
                path.name, expected_name
            )
        )
    try:
        with tarfile.open(path, mode="r:gz") as archive:
            members = archive.getmembers()
            for member in members:
                parts = member.name.split("/")
                if member.name.startswith("/") or ".." in parts:
                    raise RuntimeError(
                        "source distribution contains an unsafe path: {}".format(
                            member.name
                        )
                    )
            metadata_members = [
                member
                for member in members
                if member.isfile() and member.name.endswith("/PKG-INFO")
            ]
            if len(metadata_members) != 1:
                raise RuntimeError(
                    "expected one PKG-INFO file in {}; found {}".format(
                        path.name, len(metadata_members)
                    )
                )
            metadata = archive.extractfile(metadata_members[0])
            if metadata is None:
                raise RuntimeError("could not read PKG-INFO from {}".format(path.name))
            verify_package_metadata(metadata.read(), expected_version, path.name)
    except (OSError, tarfile.TarError) as error:
        raise RuntimeError(
            "could not read source distribution {}: {}".format(path.name, error)
        ) from error


def verify_distributions(distribution_directory, tag=None):
    metadata, cargo_version, python_version, _ = release_context(tag)
    distribution_directory = Path(distribution_directory)
    wheels = sorted(distribution_directory.glob("bbtidy-*.whl"))
    if not wheels:
        raise RuntimeError("no bbtidy wheels found in {}".format(distribution_directory))

    expected_platforms = set(supported_platforms(metadata))
    expected_extensions = {
        entry["wheel_platform"]: entry["binary_extension"]
        for entry in metadata["wheel_matrix"]
    }
    actual_platforms = []
    for wheel in wheels:
        platform = wheel_metadata(wheel)[1]
        actual_platforms.append(
            verify_wheel(wheel, python_version, expected_extensions.get(platform))
        )
    duplicates = sorted(
        platform
        for platform in set(actual_platforms)
        if actual_platforms.count(platform) > 1
    )
    if duplicates:
        raise RuntimeError("duplicate wheel platforms: {}".format(", ".join(duplicates)))
    actual_platform_set = set(actual_platforms)
    missing = sorted(expected_platforms - actual_platform_set)
    unexpected = sorted(actual_platform_set - expected_platforms)
    if missing or unexpected:
        details = []
        if missing:
            details.append("missing " + ", ".join(missing))
        if unexpected:
            details.append("unexpected " + ", ".join(unexpected))
        raise RuntimeError(
            "wheel platform set does not match release metadata: "
            + "; ".join(details)
        )

    sdists = sorted(distribution_directory.glob("bbtidy-*.tar.gz"))
    if len(sdists) != 1:
        raise RuntimeError(
            "expected one bbtidy source distribution; found {}".format(len(sdists))
        )
    verify_sdist(sdists[0], python_version)
    return {
        "cargo_version": cargo_version,
        "python_version": python_version,
        "wheels": wheels,
        "sdist": sdists[0],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dist-dir", type=Path, required=True)
    parser.add_argument(
        "--tag",
        help="release tag to validate; GitHub tag context is used when omitted",
    )
    arguments = parser.parse_args()

    try:
        result = verify_distributions(arguments.dist_dir, arguments.tag)
    except (
        OSError,
        RuntimeError,
        ValueError,
        zipfile.BadZipFile,
        tarfile.TarError,
    ) as error:
        print("error: {}".format(error), file=sys.stderr)
        return 1

    print(
        "verified {} wheels and {} source distribution for {}".format(
            len(result["wheels"]), result["sdist"].name, result["cargo_version"]
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
