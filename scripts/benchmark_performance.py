#!/usr/bin/env python3
"""Measure bbtidy offline workloads with versioned process/resource evidence.

The wrapper measures the command and its descendants. It deliberately does not
flush the host page cache: ``cold`` means a fresh disposable build/workspace,
while ``warm`` means a repeated invocation over unchanged inputs.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import resource
import shutil
import statistics
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Iterable

try:
    from scripts.performance_schema import aggregate_results, write_record
except ModuleNotFoundError:  # direct script execution
    from performance_schema import aggregate_results, write_record  # type: ignore[no-redef]


PROJECT_ROOT = Path(__file__).resolve().parents[1]
METADATA_SUFFIXES = {".bb", ".bbappend", ".bbclass", ".conf", ".inc"}


def _linux_processes(root_pid: int) -> list[int]:
    children: dict[int, list[int]] = {}
    try:
        entries = Path("/proc").iterdir()
    except OSError:
        return [root_pid]
    for entry in entries:
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text(encoding="ascii")
            after_name = stat.rsplit(")", 1)[1].split()
            parent = int(after_name[1])
            children.setdefault(parent, []).append(int(entry.name))
        except (OSError, ValueError, IndexError):
            continue
    result = [root_pid]
    index = 0
    while index < len(result):
        result.extend(children.get(result[index], []))
        index += 1
    return result


def _linux_process_metrics(pids: Iterable[int]) -> tuple[int, int, int, int]:
    rss = read_bytes = written_bytes = 0
    cpu_ticks = 0
    for pid in pids:
        try:
            status = (Path("/proc") / str(pid) / "status").read_text(encoding="ascii")
            match = re.search(r"^VmRSS:\s+(\d+)\s+kB$", status, re.MULTILINE)
            if match:
                rss += int(match.group(1)) * 1024
            stat = (Path("/proc") / str(pid) / "stat").read_text(encoding="ascii")
            fields = stat.rsplit(")", 1)[1].split()
            cpu_ticks += int(fields[11]) + int(fields[12])
            io = (Path("/proc") / str(pid) / "io").read_text(encoding="ascii")
            for line in io.splitlines():
                name, _, value = line.partition(":")
                if name == "read_bytes":
                    read_bytes += int(value.strip())
                elif name == "write_bytes":
                    written_bytes += int(value.strip())
        except (OSError, ValueError, IndexError):
            continue
    return rss, read_bytes, written_bytes, cpu_ticks


class ProcessSampler:
    def __init__(self, pid: int) -> None:
        self.pid = pid
        self.peak_rss = 0
        self.read_bytes = 0
        self.written_bytes = 0
        self.cpu_ticks = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._sample, daemon=True)

    def start(self) -> None:
        # Capture the process before starting the polling thread.  Very short
        # commands can otherwise exit before the first scheduled poll, which
        # would record an impossible zero RSS value on Linux.
        self._sample_once()
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=1)
        self._sample_once()

    def _sample_once(self) -> None:
        if sys.platform != "linux":
            return
        rss, read_bytes, written_bytes, cpu_ticks = _linux_process_metrics(
            _linux_processes(self.pid)
        )
        self.peak_rss = max(self.peak_rss, rss)
        self.read_bytes = max(self.read_bytes, read_bytes)
        self.written_bytes = max(self.written_bytes, written_bytes)
        self.cpu_ticks = max(self.cpu_ticks, cpu_ticks)

    def _sample(self) -> None:
        while not self._stop.is_set():
            self._sample_once()
            self._stop.wait(0.005)


def _terminate_process_group(process: subprocess.Popen[bytes]) -> None:
    if os.name == "posix":
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        deadline = time.monotonic() + 0.25
        while time.monotonic() < deadline:
            if process.poll() is not None:
                return
            time.sleep(0.005)
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    else:
        process.kill()


def run_command(
    command: list[str],
    cwd: Path | None = None,
    timeout_seconds: float | None = None,
) -> dict[str, Any]:
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter()
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=(os.name == "posix"),
    )
    sampler = ProcessSampler(process.pid)
    sampler.start()
    timed_out = False
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
    except subprocess.TimeoutExpired as error:
        timed_out = True
        _terminate_process_group(process)
        stdout, stderr = process.communicate()
        if not stdout and error.stdout:
            stdout = error.stdout
        if not stderr and error.stderr:
            stderr = error.stderr
    sampler.stop()
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    wall_ms = (time.perf_counter() - started) * 1000
    # Linux RSS is sampled from the complete process tree.  ru_maxrss is a
    # process-wide cumulative maximum for RUSAGE_CHILDREN, so merging it here
    # would allow an earlier, unrelated command to inflate this sample.
    if sys.platform != "linux":
        max_rss = after.ru_maxrss
        if sys.platform != "darwin":
            max_rss *= 1024
        sampler.peak_rss = max(sampler.peak_rss, int(max_rss))
    if sys.platform == "linux":
        ticks_per_second = os.sysconf("SC_CLK_TCK")
        user_cpu_ms = sampler.cpu_ticks / ticks_per_second * 1000
        # /proc exposes combined ticks; rusage gives a portable split.
        user_cpu_ms = max(0.0, (after.ru_utime - before.ru_utime) * 1000)
        system_cpu_ms = max(0.0, (after.ru_stime - before.ru_stime) * 1000)
    else:
        user_cpu_ms = max(0.0, (after.ru_utime - before.ru_utime) * 1000)
        system_cpu_ms = max(0.0, (after.ru_stime - before.ru_stime) * 1000)
    status = "timed-out" if timed_out else "success" if process.returncode == 0 else "failed"
    return {
        "status": status,
        "exit_code": process.returncode,
        "wall_ms": wall_ms,
        "user_cpu_ms": user_cpu_ms,
        "system_cpu_ms": system_cpu_ms,
        "peak_rss_bytes": sampler.peak_rss,
        "read_bytes": sampler.read_bytes,
        "written_bytes": sampler.written_bytes,
        "stdout_bytes": len(stdout),
        "stderr_bytes": len(stderr),
        "stdout": stdout,
        "stderr": stderr,
    }


def _binary_version(bbtidy: Path | None) -> str | None:
    if bbtidy is None:
        return None
    try:
        result = subprocess.run(
            [str(bbtidy), "--version"],
            cwd=PROJECT_ROOT,
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout.strip() or result.stderr.strip() or None


def _source_commit() -> str | None:
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=PROJECT_ROOT,
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout.strip() or None


def _command_version(command: Path | None, arguments: list[str]) -> str | None:
    if command is None:
        return None
    try:
        result = subprocess.run(
            [str(command), *arguments],
            cwd=PROJECT_ROOT,
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout.strip() or result.stderr.strip() or None


def _rust_version() -> str | None:
    return _command_version(Path("rustc"), ["--version"])


def runner_metadata(
    runner_class: str | None = None,
    bbtidy: Path | None = None,
    bitbake: Path | None = None,
) -> dict[str, Any]:
    memory_bytes = 0
    if sys.platform == "linux":
        try:
            memory_text = Path("/proc/meminfo").read_text(encoding="ascii")
            match = re.search(r"^MemTotal:\s+(\d+)\s+kB$", memory_text, re.MULTILINE)
            memory_bytes = int(match.group(1)) * 1024 if match else 0
        except (OSError, ValueError):
            pass
    detected = f"{platform.system().lower()}-{platform.machine().lower()}"
    architecture = os.environ.get("RUNNER_ARCH") or platform.machine()
    architecture = {
        "X64": "x86_64",
        "ARM64": "aarch64",
    }.get(architecture.upper(), architecture)
    return {
        "class": runner_class or detected,
        "os": platform.platform(),
        "image_os": os.environ.get("ImageOS"),
        "image_version": os.environ.get("ImageVersion"),
        "kernel": platform.release(),
        "architecture": architecture,
        "cpu": platform.processor() or platform.machine(),
        "logical_cores": os.cpu_count() or 1,
        "memory_bytes": memory_bytes,
        "rust": _rust_version() or "unknown",
        "bitbake": _command_version(bitbake, ["--version"]),
        "bbtidy_version": _binary_version(bbtidy),
        "source_commit": _source_commit(),
        "resource_backends": {
            "process_tree": "procfs" if sys.platform == "linux" else "rusage",
            "cpu": "rusage",
            "memory": "procfs+rusage" if sys.platform == "linux" else "rusage",
            "io": "procfs" if sys.platform == "linux" else "unavailable",
            "cgroup": Path("/sys/fs/cgroup").is_dir(),
            "gnu_time": shutil.which("time") is not None,
        },
    }


def corpus_metadata(source_root: Path) -> dict[str, Any]:
    digest = hashlib.sha256()
    files = 0
    source_bytes = 0
    for path in sorted(
        path
        for path in source_root.rglob("*")
        if path.is_file() and path.suffix in METADATA_SUFFIXES
    ):
        relative = path.relative_to(source_root).as_posix().encode("utf-8")
        data = path.read_bytes()
        digest.update(len(relative).to_bytes(8, "big"))
        digest.update(relative)
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
        files += 1
        source_bytes += len(data)
    return {
        "id": source_root.name,
        "revision_digest": digest.hexdigest(),
        "files": files,
        "source_bytes": source_bytes,
    }


def _source_for_size(size: int) -> str:
    header = 'SUMMARY = "benchmark recipe"\nLICENSE = "MIT"\n'
    body = 'SRC_URI = "https://example.invalid/source.tar.gz;sha256sum=abc"\n'
    filler = "# deterministic benchmark payload\n"
    source = header + body
    while len(source.encode()) < size:
        source += filler
    return source


def synthetic_cases() -> list[tuple[str, str]]:
    cases = [
        (name, _source_for_size(size))
        for name, size in (
            ("recipe-1k", 1024),
            ("recipe-10k", 10 * 1024),
            ("recipe-100k", 100 * 1024),
            ("recipe-1m", 1024 * 1024),
        )
    ]
    continued = "SRC_URI = \" \\\n" + "".join(f" file://entry-{index}.patch \\\n" for index in range(1000)) + "\"\n"
    bodies = "do_compile() {\n" + "    echo benchmark\n" * 10000 + "}\n"
    return cases + [("continued-1000", continued), ("shell-body-1m", bodies[:1024 * 1024])]


def _read_json_output(stdout: bytes) -> dict[str, Any] | None:
    try:
        import json

        value = json.loads(stdout.decode("utf-8"))
        return value if isinstance(value, dict) else None
    except (UnicodeDecodeError, ValueError):
        return None


def measure_cli(
    bbtidy: Path,
    source_root: Path,
    operation: str,
    mode: str,
    repetitions: int,
    profile: str = "all",
    minimum_duration_ms: float = 0,
    timeout_seconds: float | None = None,
    bitbake_command: Path | None = None,
) -> dict[str, Any]:
    def command_for(root: Path) -> list[str]:
        if operation == "format-check":
            return [str(bbtidy), "--no-config", "format", "--check", str(root)]
        if operation in {"format", "format-write"}:
            return [str(bbtidy), "--no-config", "format", "--write", str(root)]
        if operation in {"lint", "json", "sarif"}:
            output = (
                "json" if operation == "json" else "sarif" if operation == "sarif" else "text"
            )
            return [
                str(bbtidy),
                "--no-config",
                "check",
                "--profile",
                profile,
                "--fail-on",
                "never",
                "--output",
                output,
                str(root),
            ]
        if operation in {"bitbake", "semantic"}:
            if bitbake_command is None:
                raise ValueError("--bitbake-command is required for BitBake-backed operations")
            if operation == "bitbake":
                return [
                    str(bbtidy),
                    "--no-config",
                    "check",
                    "--workspace",
                    str(root),
                    "--bitbake",
                    str(bitbake_command),
                    "--fail-on",
                    "never",
                    "--output",
                    "json",
                ]
            return [
                str(bbtidy), "--no-config", "semantic", "--build-dir", str(root),
                "--bitbake", str(bitbake_command), "--full", "--output", "json",
            ]
        raise ValueError(f"unsupported offline operation: {operation}")

    if operation not in {
        "format-check",
        "format",
        "format-write",
        "lint",
        "json",
        "sarif",
        "bitbake",
        "semantic",
    }:
        raise ValueError(f"unsupported offline operation: {operation}")
    samples = []
    last_output: dict[str, Any] | None = None
    preparation: dict[str, Any] = {}
    phase_measurement: dict[str, Any] = {
        "config_ms": 0,
        "exclusion_ms": 0,
        "index_ms": 0,
        "resolution_ms": 0,
        "override_ms": 0,
        "body_ms": 0,
        "rule_ms": 0,
        "sort_ms": 0,
        "baseline_ms": 0,
        "suppression_ms": 0,
        "serialization_ms": 0,
        "diff_ms": 0,
        "transaction_ms": 0,
        "method": "operation-level timing; unavailable subphases are zero and labelled",
    }
    traversal_started = time.perf_counter()
    metadata_paths = [
        path
        for path in source_root.rglob("*")
        if path.is_file() and path.suffix in METADATA_SUFFIXES
    ]
    phase_measurement["traversal_ms"] = (time.perf_counter() - traversal_started) * 1000
    read_started = time.perf_counter()
    source_bytes = 0
    for path in metadata_paths:
        source_bytes += len(path.read_bytes())
    phase_measurement["source_read_ms"] = (time.perf_counter() - read_started) * 1000
    syntax_result = run_command(
        [str(bbtidy), "--no-config", "syntax-stats", "--details", str(source_root)],
        timeout_seconds=timeout_seconds,
    )
    phase_measurement["parse_ms"] = syntax_result["wall_ms"]
    syntax_output = (
        _read_json_output(syntax_result["stdout"])
        if syntax_result["status"] == "success"
        else None
    )
    structural = {
        key: syntax_output.get(key, 0)
        for key in (
            "files",
            "total_nodes",
            "structured_nodes",
            "trivia_nodes",
            "unknown_nodes",
            "unknown_bytes",
        )
    } if syntax_output else {}
    total_wall_ms = 0.0
    target_repetitions = max(1, repetitions)
    maximum_repetitions = max(target_repetitions, 256)

    # Warm BitBake measurements include one unrecorded priming invocation.
    # The recorded samples therefore represent repeated execution over the
    # already-primed workspace, rather than mixing priming cost into the
    # reference statistic.
    if mode == "warm" and operation in {"bitbake", "semantic"}:
        prime = run_command(command_for(source_root), timeout_seconds=timeout_seconds)
        preparation["warm_prime"] = {
            key: value for key, value in prime.items() if key not in {"stdout", "stderr"}
        }
        if prime["status"] != "success":
            sample_result = {
                key: value for key, value in prime.items() if key not in {"stdout", "stderr"}
            }
            samples.append({"result": sample_result, "bbtidy": {}})
            return {
                "samples": samples,
                "mode": mode,
                "phase_timings": phase_measurement,
                "structural": structural,
                "preparation": preparation,
            }

    while len(samples) < target_repetitions or (
        minimum_duration_ms > 0 and total_wall_ms < minimum_duration_ms
    ):
        if len(samples) >= maximum_repetitions:
            break
        isolated_temporary = None
        sample_root = source_root
        isolated = (
            operation in {"format", "format-write"}
            or (operation in {"bitbake", "semantic"} and mode == "cold")
        )
        if isolated:
            isolated_temporary = tempfile.TemporaryDirectory(
                prefix="bbtidy-performance-sample-"
            )
            sample_root = Path(isolated_temporary.name) / source_root.name
            shutil.copytree(source_root, sample_root, symlinks=True)
        try:
            result = run_command(command_for(sample_root), timeout_seconds=timeout_seconds)
        finally:
            if isolated_temporary is not None:
                isolated_temporary.cleanup()
        total_wall_ms += result["wall_ms"]
        last_output = _read_json_output(result["stdout"])
        files_discovered = sum(
            1
            for path in source_root.rglob("*")
            if path.is_file() and path.suffix in METADATA_SUFFIXES
        )
        source_bytes = sum(
            path.stat().st_size
            for path in source_root.rglob("*")
            if path.is_file() and path.suffix in METADATA_SUFFIXES
        )
        sample_result = {
            key: value
            for key, value in result.items()
            if key not in {"stdout", "stderr"}
        }
        sample_result["bbtidy"] = {
            "files_discovered": files_discovered,
            "files_parsed": files_discovered if result["status"] == "success" else 0,
            "source_bytes": source_bytes,
            "diagnostics": len(last_output.get("diagnostics", [])) if last_output else 0,
            "output_bytes": result["stdout_bytes"] + result["stderr_bytes"],
        }
        if last_output and isinstance(last_output.get("execution"), dict):
            sample_result["bbtidy"]["bitbake"] = last_output["execution"]
        phase_measurement["rule_ms"] = result["wall_ms"]
        phase_measurement["serialization_ms"] = result["wall_ms"]
        samples.append({"result": sample_result, "bbtidy": sample_result["bbtidy"]})
        if result["status"] != "success":
            break
    return {
        "samples": samples,
        "mode": mode,
        "phase_timings": phase_measurement,
        "structural": structural,
        "preparation": preparation,
    }


def build_record(
    workload: str,
    mode: str,
    samples: list[dict[str, Any]],
    runner_class: str | None = None,
    corpus: dict[str, Any] | None = None,
    bbtidy: Path | None = None,
    bitbake_command: Path | None = None,
) -> dict[str, Any]:
    record_samples = []
    for sample in samples:
        if "samples" in sample:
            record_samples.extend(sample["samples"])
        else:
            record_samples.append(sample)
    summary = aggregate_results(record_samples)
    runner = runner_metadata(runner_class, bbtidy, bitbake_command)
    counters = {}
    nested_counters: dict[str, dict[str, Any]] = {}
    phases = [
        sample["phase_timings"]
        for sample in samples
        if isinstance(sample.get("phase_timings"), dict)
    ]
    structural_values = [
        sample["structural"]
        for sample in samples
        if isinstance(sample.get("structural"), dict)
    ]
    preparations = [
        sample["preparation"]
        for sample in samples
        if isinstance(sample.get("preparation"), dict)
    ]
    for sample in record_samples:
        for key, value in sample.get("bbtidy", {}).items():
            if isinstance(value, (int, float)) and not isinstance(value, bool):
                counters[key] = statistics.median(
                    [float(item.get("bbtidy", {}).get(key, 0)) for item in record_samples]
                )
            elif isinstance(value, dict):
                for nested_key, nested_value in value.items():
                    if isinstance(nested_value, (int, float)) and not isinstance(nested_value, bool):
                        nested_counters.setdefault(key, {})[nested_key] = statistics.median(
                            [
                                float(item.get("bbtidy", {}).get(key, {}).get(nested_key, 0))
                                for item in record_samples
                            ]
                        )
                    elif nested_key not in nested_counters.setdefault(key, {}):
                        nested_counters[key][nested_key] = nested_value
    summary["bbtidy"] = counters
    summary.update(nested_counters)
    if phases:
        summary["phases"] = {
            key: statistics.median(
                float(item[key]) for item in phases if isinstance(item.get(key), (int, float))
            )
            for key in phases[0]
            if all(isinstance(item.get(key), (int, float)) for item in phases)
        }
    if structural_values:
        summary["structural"] = {
            key: statistics.median(
                float(item.get(key, 0)) for item in structural_values
            )
            for key in structural_values[0]
            if isinstance(structural_values[0].get(key), (int, float))
        }
    return {
        "schema": 1,
        "kind": "bbtidy-performance",
        "workload": workload,
        "mode": mode,
        "runner": runner,
        "commit": runner.get("source_commit") or "unknown",
        "version": runner.get("bbtidy_version") or "unknown",
        "corpus": corpus or {"id": "synthetic", "revision_digest": None, "files": None, "source_bytes": None},
        "sample_count": len(record_samples),
        "samples": record_samples,
        "summary": summary,
        "phase_timings": next(
            (
                sample["phase_timings"]
                for sample in samples
                if isinstance(sample.get("phase_timings"), dict)
            ),
            {},
        ),
        "structural": next(
            (
                sample["structural"]
                for sample in samples
                if isinstance(sample.get("structural"), dict)
            ),
            {},
        ),
        "aggregation": {
            "method": "selected-statistics",
            "wall_ms": "median",
            "peak_rss_bytes": "p90",
            "p90": "nearest-rank",
        },
        "preparation": preparations[0] if preparations else {},
    }


def run_synthetic(args: argparse.Namespace) -> list[dict[str, Any]]:
    records = []
    for name, source in synthetic_cases():
        with tempfile.TemporaryDirectory(prefix="bbtidy-performance-") as temporary:
            root = Path(temporary) / "layer"
            path = root / "recipes" / "benchmark.bb"
            path.parent.mkdir(parents=True)
            path.write_text(source, encoding="utf-8")
            measured = measure_cli(
                Path(args.bbtidy),
                root,
                args.operation,
                args.mode,
                args.repetitions,
                args.profile,
                minimum_duration_ms=args.minimum_duration_ms,
                timeout_seconds=args.timeout_seconds,
                bitbake_command=args.bitbake_command,
            )
            records.append(
                build_record(
                    "synthetic."
                    + name
                    + "."
                    + (
                        "format-write"
                        if args.operation in {"format", "format-write"}
                        else args.operation
                    ),
                    args.mode,
                    [measured],
                    args.runner_class,
                    corpus_metadata(root),
                    Path(args.bbtidy),
                    args.bitbake_command,
                )
            )
    return records


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bbtidy", type=Path, default=PROJECT_ROOT / "target" / "release" / "bbtidy")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--operation",
        choices=(
            "format-check",
            "format",
            "format-write",
            "lint",
            "json",
            "sarif",
            "bitbake",
            "semantic",
        ),
        default="json",
    )
    parser.add_argument("--mode", choices=("cold", "warm", "offline"), default="offline")
    parser.add_argument("--repetitions", type=int, default=7)
    parser.add_argument("--profile", choices=("essential", "recommended", "strict", "all"), default="all")
    parser.add_argument("--runner-class")
    parser.add_argument("--workload")
    parser.add_argument("--bitbake-command", type=Path)
    parser.add_argument("--minimum-duration-ms", type=float, default=1000)
    parser.add_argument("--timeout-seconds", type=float)
    parser.add_argument("--synthetic", action="store_true")
    parser.add_argument("--source-root", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.bbtidy.is_file():
        print(f"error: bbtidy executable not found: {args.bbtidy}", file=sys.stderr)
        return 2
    if args.synthetic:
        records = run_synthetic(args)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(
            json.dumps(
                {
                    "schema": 1,
                    "kind": "bbtidy-performance-suite",
                    "runner": runner_metadata(
                        args.runner_class,
                        Path(args.bbtidy),
                        args.bitbake_command,
                    ),
                    "records": records,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        return 0 if all(record["summary"]["status"] == "success" for record in records) else 1
    if args.source_root is None:
        print("error: --source-root is required unless --synthetic is used", file=sys.stderr)
        return 2
    measured = measure_cli(
        args.bbtidy,
        args.source_root,
        args.operation,
        args.mode,
        args.repetitions,
        args.profile,
        timeout_seconds=args.timeout_seconds,
        bitbake_command=args.bitbake_command,
    )
    record = build_record(
        args.workload or args.source_root.name + "-" + args.operation,
        args.mode,
        [measured],
        args.runner_class,
        corpus_metadata(args.source_root),
        args.bbtidy,
        args.bitbake_command,
    )
    write_record(args.output, record)
    return 0 if record["summary"]["status"] == "success" else 1


if __name__ == "__main__":
    raise SystemExit(main())
