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

Timing budgets are advisory until repeated native-Linux reference samples are
populated. Blocking comparisons require both the configured relative and
absolute regression thresholds. Structural command/query/strategy/output
invariants remain blocking. Updating a baseline requires `--update --reason`
and is refused in CI unless `BBTIDY_ALLOW_PERFORMANCE_UPDATE=1` is explicitly
set; unaffected workloads are preserved and the before/after values are
printed.

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
