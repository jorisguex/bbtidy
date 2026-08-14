# Performance evidence and budgets

Performance evidence uses schema 1 and is intentionally separate from lint
and parser compatibility fingerprints. A record identifies the workload,
mode, exact corpus identity, runner class, repetitions, median/p90 aggregation,
wall time, CPU time, process-tree peak RSS, read/write bytes, structural
bbtidy counters, and BitBake phase statistics when applicable.

The checked-in reference manifest is
[github-ubuntu-24.04-x86_64.json](baselines/github-ubuntu-24.04-x86_64.json).
`budgets.json` contains policy only; reference values are keyed by runner
class, workload, mode, corpus digest, metric, and statistic in that manifest.
The checked-in manifest remains fail-closed until the Ubuntu campaign has been
reviewed and promoted; it must not be populated from a local non-reference
runner.
Ubuntu 24.04 is the reference image. Every record also captures `ImageOS`,
`ImageVersion`, architecture, CPU, logical cores, RAM, kernel, Rust, and
BitBake identities when available.

The three-run campaign is dispatched by
`.github/workflows/performance-reference.yml`; its raw artifacts are the only
inputs intended for baseline promotion.

The campaign review can also be reproduced locally after downloading the raw
artifacts:

```bash
python3 scripts/review_performance_campaign.py \
  --budgets tests/performance/budgets.json \
  --evidence run-1.json \
  --evidence run-2.json \
  --evidence run-3.json \
  --output reference-review.json
```

For an exact candidate commit, dispatch it three times (or run the sequential
three-run workflow once, which produces three independent records per
workload):

```bash
gh workflow run performance-reference.yml \
  --ref CANDIDATE_COMMIT \
  -f source_commit=CANDIDATE_COMMIT
```

The measurement states are:

- `cold`: a fresh disposable build/workspace with repositories already
  checked out; no page-cache flushing, dependency installation, compilation,
  or network activity is included.
- `warm`: the same unchanged inputs after one successful invocation.
- `offline`: no BitBake or network activity; filesystem-cold-ish and repeated
  warm samples are labelled explicitly by the runner.

Read-only and format-write synthetic workloads use at least seven samples and
one second of accumulated measurement per run. Format-write restores the
original fixture before every sample. Cold BitBake samples use a new
disposable build directory; warm samples prime once and record only the next
five invocations. Wall time uses the median and RSS uses the nearest-rank p90.

Run the deterministic synthetic matrix with:

```bash
python3 scripts/benchmark_performance.py \
  --bbtidy target/release/bbtidy \
  --synthetic \
  --operation json \
  --repetitions 7 \
  --minimum-duration-ms 1000 \
  --runner-class github-ubuntu-24.04-x86_64 \
  --output performance/synthetic.json
```

Compare a record with [budgets.json](budgets.json):

```bash
python3 scripts/check_performance_budget.py \
  --budgets tests/performance/budgets.json \
  --baselines tests/performance/baselines/github-ubuntu-24.04-x86_64.json \
  --evidence performance/yocto-6.0-offline.json \
  --output performance/budget-comparison.json
```

Blocking comparisons require both the configured relative and absolute
regression thresholds. Structural command/query/strategy/output invariants
remain blocking. Missing or stale corpus references are advisory in local
budget exploration but fail release verification.

Baseline promotion consumes a complete campaign of at least three compatible
runs. It requires a reason, verifies the source commit and corpus digest, emits
a review report and complete before/after diff, updates only selected
workloads, and records evidence identifiers and promotion history:

```bash
python3 scripts/promote_performance_baselines.py \
  --budgets tests/performance/budgets.json \
  --baselines tests/performance/baselines/github-ubuntu-24.04-x86_64.json \
  --evidence reference-run-1.json \
  --evidence reference-run-2.json \
  --evidence reference-run-3.json \
  --reason "Establish beta.1 Ubuntu 24.04 reference measurements" \
  --report reference-review.json
```

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
`baselines.json`, `summary.json`, the synthetic and pinned offline records, BitBake cold/warm
records for each supported release, raw samples, and any failure artifacts.
Hosted-runner timing is reference evidence, not a universal user guarantee.

For an alternating base/candidate comparison, collect both records with the
same runner class and corpus, then add the baseline-evidence option to the
checker invocation. The checker rejects mismatched corpus revisions, modes,
workloads, or runners before applying the relative and absolute thresholds.
