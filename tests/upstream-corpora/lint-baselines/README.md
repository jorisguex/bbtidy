# Upstream lint-quality baselines

The JSON files in this directory are generated explicitly with
`scripts/check_upstream_corpus.py --update-lint-baseline` after running the
corresponding pinned corpus. They are intentionally not bootstrapped from
diagnostic counts: every measurement is derived from normalized structured
findings, while review metadata remains human-owned and starts as
`unreviewed`.

The supported and pinned-community gates require every active rule to have a
`reviewed` or `accepted-known-limitations` decision. False-positive and
unclear samples must include remediation or limitation notes. An explicit
baseline update retains review records only when a rule's measurement is
unchanged; changed or newly active rules are reset to `unreviewed`.

Schema version 1 separates stable corpus identity, lint contract, generated
measurements, and review records. Whole-corpus and per-rule SHA-256 digests
cover normalized findings, so a count-neutral finding change is detected. The
files contain no checkout paths, timestamps, CI IDs, or other execution
metadata. The three pinned manifests explicitly reference these files;
`yocto-master.json` is moving and remains report-only.

Required files are:

- `yocto-5.0-scarthgap.json`
- `yocto-6.0-wrynose.json`
- `community-master.json`

Run the pinned-corpus commands from the upstream compatibility documentation
once the repositories are available locally. Moving `yocto-master` is
non-blocking and does not require a checked-in baseline.
