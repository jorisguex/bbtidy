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
    def __init__(self, pid: int, exclude_root: bool = False) -> None:
        self.pid = pid
        self.exclude_root = exclude_root
        self.peak_rss = 0
        self.read_bytes = 0
        self.written_bytes = 0
        self.cpu_ticks = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._sample, daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=1)
        self._sample_once()

    def _sample_once(self) -> None:
        if sys.platform != "linux":
            return
        pids = _linux_processes(self.pid)
        rss, read_bytes, written_bytes, cpu_ticks = _linux_process_metrics(
            pids[1:] if self.exclude_root else pids
        )
        self.peak_rss = max(self.peak_rss, rss)
        self.read_bytes = max(self.read_bytes, read_bytes)
        self.written_bytes = max(self.written_bytes, written_bytes)
        self.cpu_ticks = max(self.cpu_ticks, cpu_ticks)

    def _sample(self) -> None:
        while not self._stop.is_set():
            self._sample_once()
            self._stop.wait(0.005)


def _signal_process_group(pid: int, sig: int) -> None:
    try:
        os.killpg(pid, sig)
    except ProcessLookupError:
        pass


def run_command(
    command: list[str],
    cwd: Path | None = None,
    timeout_seconds: float | None = None,
) -> dict[str, Any]:
    # wait4 returns usage for this child, unlike cumulative RUSAGE_CHILDREN.
    # File capture avoids pipe deadlocks while the sole waiter reaps the child.
    if not hasattr(os, "wait4"):
        raise RuntimeError("performance measurement requires POSIX wait4 (Linux or macOS)")
    with open(os.devnull, "rb") as stdin_file, tempfile.TemporaryFile() as stdout_file, tempfile.TemporaryFile() as stderr_file, tempfile.NamedTemporaryFile() as usage_file:
        linux = sys.platform == "linux"
        if linux:
            if not Path("/usr/bin/time").is_file():
                raise RuntimeError("Linux performance measurement requires GNU /usr/bin/time")
            # Linux can retain pre-exec parent RSS even with posix_spawn.
            # GNU time forks the measured command from its small native image
            # and reports that child's peak, excluding the Python harness.
            command = ["/usr/bin/time", "--quiet", "--format=%M", "--output=" + usage_file.name, "--", *command]
        started = time.perf_counter()
        # Spawn a fresh session without Python work in a post-fork child.
        if cwd is not None:
            raise ValueError("run benchmarks with explicit input paths, not a cwd override")
        pid = os.posix_spawnp(command[0], command, os.environ, setsid=True, file_actions=[
            (os.POSIX_SPAWN_DUP2, stdin_file.fileno(), 0),
            (os.POSIX_SPAWN_DUP2, stdout_file.fileno(), 1),
            (os.POSIX_SPAWN_DUP2, stderr_file.fileno(), 2),
        ])
        waited = []
        done = threading.Event()

        def reap() -> None:
            try:
                _, status, usage = os.wait4(pid, 0)
                waited.append((usage, time.perf_counter(), os.waitstatus_to_exitcode(status)))
            finally:
                done.set()

        waiter = threading.Thread(target=reap)
        waiter.start()
        sampler = ProcessSampler(pid, exclude_root=linux)
        sampler.start()
        timed_out = False
        try:
            if not done.wait(timeout_seconds):
                timed_out = True
                _signal_process_group(pid, signal.SIGTERM)
                # Kill remaining descendants even if the leader exits on TERM.
                time.sleep(0.25)
                _signal_process_group(pid, signal.SIGKILL)
            waiter.join()
        finally:
            if not done.is_set():
                _signal_process_group(pid, signal.SIGKILL)
                waiter.join()
            sampler.stop()
        if not waited:
            raise RuntimeError("could not collect child process resource usage")
        usage, ended, returncode = waited[0]
        stdout_file.seek(0)
        stderr_file.seek(0)
        stdout, stderr = stdout_file.read(), stderr_file.read()
        if linux:
            usage_file.seek(0)
            native_rss = usage_file.read().strip()
            if not native_rss.isdigit() and not timed_out and returncode == 0:
                raise RuntimeError("GNU time did not report the command's peak RSS")
            max_rss = int(native_rss) * 1024 if native_rss.isdigit() else 0
        else:
            max_rss = usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024)
    status = "timed-out" if timed_out else "success" if returncode == 0 else "failed"
    return {
        "status": status,
        "exit_code": returncode,
        "wall_ms": (ended - started) * 1000,
        "user_cpu_ms": usage.ru_utime * 1000,
        "system_cpu_ms": usage.ru_stime * 1000,
        "peak_rss_bytes": max(sampler.peak_rss, int(max_rss)),
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


def _rust_version() -> str:
    result = subprocess.run(
        ["rustc", "--version"], cwd=PROJECT_ROOT, check=True,
        capture_output=True, text=True, timeout=30,
    )
    return result.stdout.strip()


def runner_metadata(
    runner_class: str | None = None,
    bbtidy: Path | None = None,
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
    return {
        "class": runner_class or detected,
        "os": platform.platform(),
        "architecture": platform.machine(),
        "cpu": platform.processor() or platform.machine(),
        "logical_cores": os.cpu_count() or 1,
        "memory_bytes": memory_bytes,
        "rust": _rust_version(),
        "measurement_contract": 2,
        "bitbake": None,
        "bbtidy_version": _binary_version(bbtidy),
        "source_commit": _source_commit(),
        "resource_backends": {
            "process_tree": "procfs" if sys.platform == "linux" else "wait4",
            "cpu": "wait4",
            "memory": "procfs+gnu-time-child" if sys.platform == "linux" else "wait4",
            "output_capture": "temporary-files",
            "spawn": "posix_spawnp-setsid",
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
    cases = [(f"recipe-{size // 1024}k", _source_for_size(size)) for size in (1024, 10 * 1024, 100 * 1024, 1024 * 1024)]
    continued = "SRC_URI = \" \\\n" + "".join(f" file://entry-{index}.patch \\\n" for index in range(1000)) + "\"\n"
    header, footer, line = "do_compile() {\n", "}\n", "    echo benchmark\n"
    remaining = 1024 * 1024 - len(header) - len(footer)
    lines, padding = divmod(remaining, len(line))
    bodies = header + line * lines + " " * padding + footer
    return cases + [("continued-1000", continued), ("shell-body-1m", bodies)]


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
    bitbake_target: str | None = None,
) -> dict[str, Any]:
    if operation == "format-check":
        command = [str(bbtidy), "--no-config", "format", "--check", str(source_root)]
    elif operation == "format":
        command = [str(bbtidy), "--no-config", "format", "--write", str(source_root)]
    elif operation in {"lint", "json", "sarif"}:
        output = "json" if operation == "json" else "sarif" if operation == "sarif" else "text"
        command = [str(bbtidy), "--no-config", "check", "--profile", profile, "--fail-on", "never", "--output", output, str(source_root)]
    elif operation in {"bitbake", "semantic"}:
        if bitbake_command is None:
            raise ValueError("--bitbake-command is required for BitBake-backed operations")
        if operation == "bitbake":
            command = [
                str(bbtidy),
                "--no-config",
                "check",
                "--workspace",
                str(source_root),
                "--bitbake",
                str(bitbake_command),
                "--fail-on",
                "never",
                "--output",
                "json",
            ]
        else:
            command = [
                str(bbtidy),
                "--no-config",
                "semantic",
                "--build-dir",
                str(source_root),
                "--bitbake",
                str(bitbake_command),
            ]
            if bitbake_target:
                command.extend(["--target", bitbake_target])
            command.extend(["--full", "--output", "json"])
    else:
        raise ValueError(f"unsupported offline operation: {operation}")
    samples = []
    last_output: dict[str, Any] | None = None
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
    original_files = {}
    expected_files = {}
    if operation == "format":
        for path in metadata_paths:
            original_files[path] = path.read_bytes()
            preview = run_command(
                [str(bbtidy), "--no-config", "format", str(path)],
                timeout_seconds=timeout_seconds,
            )
            if preview["status"] != "success":
                raise ValueError(f"could not prepare expected formatting for {path}")
            expected_files[path] = preview["stdout"]
        changed_files = sum(original_files[path] != expected_files[path] for path in metadata_paths)
        if not changed_files:
            raise ValueError("format benchmark needs unformatted input; use format-check for clean files")
    syntax_result = run_command(
        [str(bbtidy), "--no-config", "syntax-stats", "--details", str(source_root)],
        timeout_seconds=timeout_seconds,
    )
    phase_measurement["parse_ms"] = syntax_result["wall_ms"]
    syntax_output = _read_json_output(syntax_result["stdout"]) if syntax_result["status"] == "success" else None
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
    while len(samples) < target_repetitions or (
        minimum_duration_ms > 0 and total_wall_ms < minimum_duration_ms
    ):
        if len(samples) >= maximum_repetitions:
            break
        # Restore the same input before EVERY repetition, outside the timer.
        for path, original in original_files.items():
            path.write_bytes(original)
        result = run_command(command, timeout_seconds=timeout_seconds)
        if operation == "format" and result["status"] == "success":
            if any(path.read_bytes() != expected for path, expected in expected_files.items()):
                result["status"] = "failed"
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
            "source_bytes": sum(map(len, original_files.values())) if operation == "format" else source_bytes,
            "diagnostics": len(last_output.get("diagnostics", [])) if last_output else 0,
            "output_bytes": result["stdout_bytes"] + result["stderr_bytes"],
        }
        if operation == "format":
            sample_result["bbtidy"]["files_changed"] = changed_files if result["status"] == "success" else 0
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
    }


def build_record(
    workload: str,
    mode: str,
    samples: list[dict[str, Any]],
    runner_class: str | None = None,
    corpus: dict[str, Any] | None = None,
    bbtidy: Path | None = None,
) -> dict[str, Any]:
    record_samples = []
    for sample in samples:
        if "samples" in sample:
            record_samples.extend(sample["samples"])
        else:
            record_samples.append(sample)
    summary = aggregate_results(record_samples)
    runner = runner_metadata(runner_class, bbtidy)
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
        "aggregation": {"method": "median", "p90": "nearest-rank"},
    }


def run_synthetic(args: argparse.Namespace) -> list[dict[str, Any]]:
    records = []
    for name, source in synthetic_cases():
        with tempfile.TemporaryDirectory(prefix="bbtidy-performance-") as temporary:
            root = Path(temporary) / "layer"
            path = root / "recipes" / "benchmark.bb"
            path.parent.mkdir(parents=True)
            # A non-canonical assignment forces a real transaction, including
            # for opaque shell bodies, without changing their contents.
            if args.operation == "format":
                source = 'BBTIDY_BENCHMARK="write"\n' + source
            path.write_text(source, encoding="utf-8")
            corpus = corpus_metadata(root)
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
                bitbake_target=args.bitbake_target,
            )
            records.append(
                build_record(
                    name + "-" + args.operation,
                    args.mode,
                    [measured],
                    args.runner_class,
                    corpus,
                    Path(args.bbtidy),
                )
            )
    return records


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bbtidy", type=Path, default=PROJECT_ROOT / "target" / "release" / "bbtidy")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--operation",
        choices=("format-check", "format", "lint", "json", "sarif", "bitbake", "semantic"),
        default="json",
    )
    parser.add_argument("--mode", choices=("cold", "warm", "offline"), default="offline")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--profile", choices=("essential", "recommended", "strict", "all"), default="all")
    parser.add_argument("--runner-class")
    parser.add_argument("--workload")
    parser.add_argument("--bitbake-command", type=Path)
    parser.add_argument("--bitbake-target")
    parser.add_argument("--minimum-duration-ms", type=float, default=0)
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
            json.dumps({"schema": 1, "kind": "bbtidy-performance-suite", "runner": runner_metadata(args.runner_class, Path(args.bbtidy)), "records": records}, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return 0 if all(record["summary"]["status"] == "success" for record in records) else 1
    if args.source_root is None:
        print("error: --source-root is required unless --synthetic is used", file=sys.stderr)
        return 2
    corpus = corpus_metadata(args.source_root)
    measured = measure_cli(
        args.bbtidy,
        args.source_root,
        args.operation,
        args.mode,
        args.repetitions,
        args.profile,
        minimum_duration_ms=args.minimum_duration_ms,
        timeout_seconds=args.timeout_seconds,
        bitbake_command=args.bitbake_command,
        bitbake_target=args.bitbake_target,
    )
    record = build_record(
        args.workload or args.source_root.name + "-" + args.operation,
        args.mode,
        [measured],
        args.runner_class,
        corpus,
        args.bbtidy,
    )
    write_record(args.output, record)
    return 0 if record["summary"]["status"] == "success" else 1


if __name__ == "__main__":
    raise SystemExit(main())
