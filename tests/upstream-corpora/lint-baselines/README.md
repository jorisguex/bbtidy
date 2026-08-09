# Upstream lint-quality baselines

The JSON files in this directory are generated explicitly with
`scripts/check_upstream_corpus.py --update-lint-baseline` after running the
corresponding pinned corpus. They are intentionally not bootstrapped from
diagnostic counts: each active rule must be sampled and reviewed before a
baseline is suitable for a supported-corpus gate.

Required files are:

- `yocto-5.0-scarthgap.json`
- `yocto-6.0-wrynose.json`
- `community-master.json`

Run the pinned-corpus commands from the upstream compatibility documentation
once the repositories are available locally. Moving `yocto-master` is
non-blocking and does not require a checked-in baseline.
