#!/usr/bin/env python3
"""Collect deterministic BitBake subprocess baselines.

The command list is explicit and sorted, so the JSON can be compared across
implementations without making machine-specific wall-clock values normative.
Use --include-timing only when investigating elapsed time locally.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path


def run_command(bitbake: str, build_dir: Path, arguments: list[str]) -> dict[str, object]:
    started = time.monotonic()
    process = subprocess.Popen(
        [bitbake, *arguments],
        cwd=build_dir,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout, stderr = process.communicate()
    return {
        "arguments": [
            "<recipe>" if index and arguments[index - 1] == "--buildfile" else value
            for index, value in enumerate(arguments)
        ],
        "status": process.returncode,
        "stdout_bytes": len(stdout),
        "stderr_bytes": len(stderr),
        "elapsed_seconds": time.monotonic() - started,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bitbake", required=True, help="BitBake executable")
    parser.add_argument("--build-dir", required=True, type=Path)
    parser.add_argument(
        "--recipe-list",
        type=Path,
        help="newline-delimited recipe paths; order is normalized before querying",
    )
    parser.add_argument(
        "--include-recipe-queries",
        action="store_true",
        help="add the legacy --environment --buildfile query for every listed recipe",
    )
    parser.add_argument(
        "--include-timing",
        action="store_true",
        help="retain nondeterministic elapsed_seconds fields in the JSON",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    recipes: list[str] = []
    if args.recipe_list:
        recipes = sorted(
            {
                str(Path(line.strip()).resolve())
                for line in args.recipe_list.read_text(encoding="utf-8").splitlines()
                if line.strip()
            }
        )
    commands: list[tuple[str, list[str]]] = [
        ("version", ["--version"]),
        ("parse", ["--parse-only"]),
        ("global_environment", ["--environment"]),
    ]
    if args.include_recipe_queries:
        commands.extend(
            ("recipe_environment", ["--environment", "--buildfile", recipe])
            for recipe in recipes
        )

    measurements = []
    for phase, command in commands:
        measurement = run_command(args.bitbake, args.build_dir, command)
        measurement["phase"] = phase
        measurements.append(measurement)
        if measurement["status"] != 0:
            break

    result: dict[str, object] = {
        "schema": 1,
        "bitbake": Path(args.bitbake).name,
        "build_dir": args.build_dir.name,
        "recipe_count": len(recipes),
        "command_count": len(measurements),
        "commands_by_phase": {
            phase: sum(measurement["phase"] == phase for measurement in measurements)
            for phase in sorted({measurement["phase"] for measurement in measurements})
        },
        "stdout_bytes": sum(measurement["stdout_bytes"] for measurement in measurements),
        "stderr_bytes": sum(measurement["stderr_bytes"] for measurement in measurements),
        "measurements": measurements,
        "environment": {"python": os.sys.version.split()[0]},
    }
    if not args.include_timing:
        for measurement in measurements:
            measurement.pop("elapsed_seconds", None)
        result.pop("environment")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if measurements and measurements[-1]["status"] == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
