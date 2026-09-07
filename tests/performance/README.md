# Performance evidence and budgets

Performance evidence uses schema 1 and is intentionally separate from lint
and parser compatibility fingerprints. A record identifies the workload,
mode, exact corpus identity, runner class, repetitions, median/p90 aggregation,
wall time, CPU time, process-tree peak RSS, read/write bytes, structural
bbtidy counters, and BitBake phase statistics when applicable.

The measurement states are:

- `cold`: a fresh disposable build/workspace with repositories already
  checked out; no page-cache flushing, dependency installation, compilation,
  or network activity is included.
- `warm`: the same unchanged inputs after one successful invocation.
- `offline`: no BitBake or network activity; filesystem-cold-ish and repeated
  warm samples are labelled explicitly by the runner.

Run the deterministic synthetic matrix with:

```bash
python3 scripts/benchmark_performance.py \
  --bbtidy target/release/bbtidy \
  --synthetic \
  --operation json \
  --runner-class github-ubuntu-24.04-x86_64 \
  --output performance/synthetic.json
```

Compare a record with [budgets.json](budgets.json):

```bash
python3 scripts/check_performance_budget.py \
  --budgets tests/performance/budgets.json \
  --evidence performance/yocto-6.0-offline.json \
  --output performance/budget-comparison.json
```

Timing and memory budgets are populated and blocking for 33 workloads on
`github-ubuntu-24.04-x86_64`:

| Workloads | Coverage | Reference runs |
| --- | --- | --- |
| 24 synthetic | Six fixtures × format-check, format, JSON, SARIF | Two runs per case, at least three samples per run |
| 3 pinned offline | Yocto 5.0, Yocto 6.0, pinned community | Two runs, three samples per run |
| 4 BitBake | Yocto 5.0/6.0 cold and warm | Two runs, one cold/two warm samples per run |
| 2 full semantic | Yocto 5.0 and 6.0 | Two runs, one sample per run |

Baselines use the median of the per-run medians, except for the six actual-write
wall-time budgets: those use the maximum of the per-run medians, an explicit
conservative envelope for hosted filesystem variability. All memory baselines
retain median aggregation. [The reference manifest](references/manifest.json) records the exact
source commits, GitHub run URLs, artifact IDs, and SHA-256 hashes of the raw
JSON files retained beside it. The current references use measurement contract
2 and the compiler reported by `rustc --version`; CI builds the release binary
with that compiler. The manifest identifies the exact measured source commit.
Their corpus identities match within every workload. The Python tests reproduce
the checked-in baselines from these files and verify that injected regressions
fail. Empty or disabled required baselines fail policy validation.

Blocking comparisons require both the configured relative and absolute
regression thresholds. Structural command/query/strategy/output invariants
remain blocking, including the common BitBake checks when a workload has its
own timing rules. Failed raw samples fail comparisons even if the summary
claims success. A different runner, mode, corpus, or measurement contract requires explicit new
reference measurements. The two generic synthetic policies remain unpopulated
templates for new cases; all current CI cases have individual blocking rules.

To refresh baselines, collect at least two independent successful native Linux
runs of each affected workload, keep their raw records, and update the manifest
with their source commits, run URLs, artifact IDs, and checksums. Then run:

```bash
python3 scripts/populate_performance_baselines.py \
  --budgets tests/performance/budgets.json \
  --references tests/performance/references/manifest.json \
  --reason "Explain the intentional change and the reference runs"
python3 -m unittest discover -s tests -p 'test_performance.py'
```

The updater checks raw-sample aggregates, hashes, successful outcomes, distinct
runs, matching input identities, measurement contracts, and compilers before writing anything. It preserves
existing relative/absolute thresholds and structural rules, and prints the
before/after values. Review those changes together with the evidence. Refreshes
are refused in CI unless `BBTIDY_ALLOW_PERFORMANCE_UPDATE=1` is explicitly set.
The older checker `--update --reason` command is available only for unreferenced
experimental workloads; it cannot replace these repeated references with one
sample. The normal CI jobs never refresh budgets automatically.

The reference policy is: synthetic timing regressions over 15% plus 50 ms,
pinned offline regressions over 20% plus 2 s, warm BitBake regressions over 25%
plus 30 s, cold BitBake regressions over 35% plus 60 s, and serialization
regressions over 20% plus 1 s. Memory budgets use the same two-part rule with
per-workload absolute caps. Scaling evidence must include N, 2N, and 4N
inputs and report the observed ratios; it is not valid to compare unrelated
corpora.
The BitBake limits in the product configuration are safety limits, not
performance budgets: they bound command count, recipe queries, timeouts,
total operation time, and stdout/stderr capture. A limit-terminated,
cancelled, timed-out, failed, or partially written run is failure evidence and
must not be used to update a timing baseline. The runner terminates the
process group and reaps children before writing a report.
Release evidence should contain `performance/manifest.json`, `budgets.json`,
`summary.json`, the synthetic and pinned offline records, BitBake cold/warm
records for each supported release, raw samples, and any failure artifacts.
Hosted-runner timing is reference evidence, not a universal user guarantee.
These references include only two independent runners per workload;
retain more runs when evaluating a suspected regression. Small synthetic wall
times include process startup and measurement overhead. The shell fixture is
exactly 1 MiB of valid source, including its closing brace. Write workloads add
an unformatted assignment, so their input is slightly larger than the named
fixture size. Recorded source-byte counts and digests identify the exact input.

The contract 2 calibration retains all 66 workload records, including a write
run whose 1 KiB median was 210.7 ms versus 3.7 ms on the other runner. Genuine
writes now include transaction synchronization; the evidence does not isolate
the cause of that runner variation. The write envelope preserves this slower
observation without changing the 15% plus 50 ms regression margins or discarding
samples. It is a regression ceiling, not an estimate of typical latency, and
is less sensitive to small write regressions. Use same-runner Criterion evidence
for fine-grained formatter changes. The per-metric `reference_aggregation`
setting and generated provenance make this exception reproducible and reviewable.

Every `format` repetition starts with the same unformatted bytes, restored
outside the timer. Expected output is prepared with a read-only formatting
preview outside the timer; each measured write must reproduce those bytes.
The sample records `bbtidy.files_changed`, and the synthetic write budgets
require one changed file. A clean/no-op input is rejected for this operation;
use `format-check` for already-formatted input. Source corpus identity is
captured before measurement, including for BitBake-generated configuration.

The POSIX runner launches commands with `posix_spawnp` and a new session.
On Linux, GNU `/usr/bin/time` reports the measured command's peak RSS from a
small native launcher, excluding the Python harness's pre-exec heap. This is
combined with procfs samples of the command's descendants, excluding the time
launcher itself. macOS uses the measured child's `wait4` RSS directly.
Neither path carries an earlier command's peak into a later sample.
Commands use explicit input paths; the runner rejects working-directory
overrides. CPU usage comes from per-command `wait4`, including the small Linux
launcher overhead and reaped descendants. Output is captured in temporary files to avoid pipe deadlocks;
wall time ends when the waiter reaps the command, before output reading and
sampler cleanup. Temporary-file capture is part of this measurement contract.
On timeout the process group receives TERM, then KILL after a grace period,
even when its leader has already exited. Linux requires GNU `/usr/bin/time`.
Linux and macOS support this harness;
the reference budgets are calibrated on Linux only.

RSS remains a high-water estimate, not an allocation profile: procfs sampling
can miss brief overlapping descendant peaks, and `wait4` does not report a
simultaneous process-tree total. Use the Criterion suite below for in-process
scaling investigations. Contract 1 references remain in Git history and must
not be mixed with contract 2 results. A contract migration deliberately rejects
the old comparisons while native raw samples are collected on temporary
branches; only successful raw samples can populate the replacement baselines.

Synthetic comparisons and raw samples are uploaded together by performance CI.
Pinned offline, cold/warm, and semantic budgets are enforced by the upstream
and release gates; changes to performance scripts or reference data trigger
that upstream gate as well.

## Rust scaling baseline

The `layer_analysis` Criterion suite isolates the main in-process scaling
risks: line/column lookup, diagnostic-dense linting, chained static override
resolution, workspace indexing, shared-include workspace linting, and
formatting by source size. Fixture creation and correctness checks happen
outside the timed loops; Criterion performs warm-up, iteration calibration,
outlier analysis, and confidence-interval estimation.

Capture a named baseline before changing runtime behavior:

```bash
cargo bench --locked --bench layer_analysis -- --save-baseline before
```

Compare the changed implementation against that exact baseline on the same
machine and build environment:

```bash
cargo bench --locked --bench layer_analysis -- --baseline before
```

The raw samples, estimates, change statistics, and HTML report are written to
`target/criterion/`. Absolute timings are meaningful only on a stable runner;
the input-size ratios are useful for identifying algorithmic scaling changes.
Pull-request CI measures the base commit first and then compares the candidate
using one runner and one shared Cargo target directory.

For an alternating base/candidate comparison, collect both records with the
same runner class and corpus, then add the baseline-evidence option to the
checker invocation. The checker rejects mismatched corpus revisions, modes,
workloads, or runners before applying the relative and absolute thresholds.
