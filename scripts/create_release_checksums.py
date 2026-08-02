#!/usr/bin/env python3
"""Create a sha256sum-compatible manifest for release binaries."""

import argparse
import hashlib
import sys
from pathlib import Path


CHUNK_SIZE = 1024 * 1024


def file_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(CHUNK_SIZE), b""):
            digest.update(chunk)
    return digest.hexdigest()


def create_checksums(binary_directory, output_path):
    binaries = sorted(
        path
        for path in binary_directory.glob("bbtidy-*")
        if path.is_file()
    )
    if not binaries:
        raise RuntimeError("no release binaries found in {}".format(binary_directory))

    output = "".join(
        "{}  {}\n".format(file_digest(path), path.name) for path in binaries
    )
    output_path.write_text(output, encoding="utf-8")
    return output_path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()

    try:
        output = create_checksums(arguments.binary_dir, arguments.output)
    except (OSError, RuntimeError) as error:
        print("error: {}".format(error), file=sys.stderr)
        return 1

    print(output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
