#!/usr/bin/env python3
"""Install a built bbtidy distribution and execute its installed binary."""

import argparse
import subprocess
import sys
import tempfile
import venv
from pathlib import Path

try:
    from scripts.check_release_version import cargo_version, pep440_version
except ModuleNotFoundError:
    from check_release_version import cargo_version, pep440_version


def select_distribution(path, kind):
    path = path.resolve()
    if path.is_file():
        return path
    if not path.is_dir():
        raise FileNotFoundError("distribution path does not exist: {}".format(path))

    pattern = "*.whl" if kind == "wheel" else "*.tar.gz"
    distributions = sorted(path.glob(pattern))
    if len(distributions) != 1:
        raise RuntimeError(
            "expected exactly one {} in {}, found {}".format(
                kind, path, len(distributions)
            )
        )
    return distributions[0]


def environment_executable(environment, name):
    scripts = "Scripts" if sys.platform == "win32" else "bin"
    suffix = ".exe" if sys.platform == "win32" else ""
    return environment / scripts / "{}{}".format(name, suffix)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("distribution", type=Path)
    parser.add_argument(
        "--kind",
        choices=["wheel", "sdist"],
        default="wheel",
        help="distribution type to select when a directory is supplied",
    )
    arguments = parser.parse_args()

    distribution = select_distribution(arguments.distribution, arguments.kind)
    version = cargo_version()
    python_version = pep440_version(version)

    with tempfile.TemporaryDirectory(prefix="bbtidy-install-") as temporary:
        environment = Path(temporary) / "venv"
        venv.EnvBuilder(with_pip=True).create(environment)
        python = environment_executable(environment, "python")
        subprocess.run(
            [
                str(python),
                "-m",
                "pip",
                "--disable-pip-version-check",
                "install",
                "--no-deps",
                str(distribution),
            ],
            check=True,
        )

        installed_version = subprocess.run(
            [
                str(python),
                "-c",
                "import importlib.metadata as m; print(m.version('bbtidy'))",
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        if installed_version != python_version:
            raise RuntimeError(
                "installed Python version {!r} does not match {!r}".format(
                    installed_version, python_version
                )
            )

        executable = environment_executable(environment, "bbtidy")
        output = subprocess.run(
            [str(executable), "--version"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        expected_output = "bbtidy {}".format(version)
        if output != expected_output:
            raise RuntimeError(
                "installed executable returned {!r}; expected {!r}".format(
                    output, expected_output
                )
            )

    print("Installed {} and verified {}".format(distribution.name, expected_output))
    return 0


if __name__ == "__main__":
    sys.exit(main())
