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
| 24 synthetic | Six fixtures × format-check, format, JSON, SARIF | Two runs per case, 6–13 samples per run |
| 3 pinned offline | Yocto 5.0, Yocto 6.0, pinned community | Two runs, three samples per run |
| 4 BitBake | Yocto 5.0/6.0 cold and warm | Two runs, one cold/two warm samples per run |
| 2 full semantic | Yocto 5.0 and 6.0 | Two runs, one sample per run |

Each baseline is the median of the per-run medians, giving each runner equal
weight. [The reference manifest](references/manifest.json) records the exact
source commits, GitHub run URLs, artifact IDs, and SHA-256 hashes of the raw
JSON files retained beside it. Synthetic references come from commits
`c1e0567` and `8cfd07b`; pinned references come from `21a7b25` and `8cfd07b`.
Their corpus identities match within every workload. The Python tests reproduce
the checked-in baselines from these files and verify that injected regressions
fail. Empty or disabled required baselines fail policy validation.

Blocking comparisons require both the configured relative and absolute
regression thresholds. Structural command/query/strategy/output invariants
remain blocking, including the common BitBake checks when a workload has its
own timing rules. Failed raw samples fail comparisons even if the summary
claims success. A different runner, mode, or corpus requires explicit new
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
runs, and matching input identities before writing anything. It preserves
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
These initial references include only two independent runners per workload;
retain more runs when evaluating a suspected regression. Small synthetic wall
times include process startup and measurement overhead. The `format` fixtures
are already formatted, so they measure the no-change `--write` path. The
historical `shell-body-1m` fixture name currently represents about 180 KB of
shell source; its byte count and digest, rather than its name, define it.
RSS is the existing procfs/process-tree plus child-rusage high-water estimate;
rusage may carry an earlier child's peak into later samples. Memory budgets
therefore detect large regressions, not precise allocation changes. Use the
Criterion suite below for in-process scaling investigations.

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
