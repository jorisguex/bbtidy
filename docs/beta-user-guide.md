# bbtidy beta user guide

This guide explains how to evaluate and adopt bbtidy in a real BitBake or
Yocto repository. The [beta support contract](beta-support-contract.md) is the
authoritative document for supported versions, guarantees, limitations, and
release evidence; this guide turns that contract into an operational workflow.

## Before you adopt it

The beta contract covers Yocto Project 5.0 LTS (Scarthgap) with BitBake 2.8
and Yocto Project 6.0 LTS (Wrynose) with BitBake 2.18. Yocto and BitBake
`master` are development targets and are non-blocking. Current published
versions may still be alpha prereleases; an alpha release should be treated as
an evaluation build, not as a beta support claim.

bbtidy formats supported BitBake metadata boundaries conservatively. It does
not execute BitBake, expand dynamic variables, resolve unavailable external
layers, or rewrite embedded shell and Python code. A successful bbtidy run is
not proof that a complete BitBake build has identical task hashes, packages,
runtime behavior, or performance.

## Install and verify

Install a matching native wheel when one is available for the host:

```bash
python3 -m pip install bbtidy
bbtidy --version
```

`pipx install bbtidy` is suitable for an isolated global installation. A
source distribution is available as a fallback, but building it requires Rust
1.85 or newer. Installing a wheel does not require Rust or a Python import.

Record the exact `bbtidy --version` output in CI logs and compatibility reports.
When reporting an issue, also record the installation method and the host
platform so a wheel or source-build difference can be reproduced.

## Safe adoption workflow

Run the read-only checks before allowing any repository-wide write:

```bash
bbtidy --version
bbtidy check meta-my-layer/
bbtidy format --diff meta-my-layer/
bbtidy lint --output sarif meta-my-layer/
```

Review the diff and lint findings. Then run the repository's normal BitBake
parse, build, and test validation. If the results are acceptable, apply the
formatting in a clean branch or worktree:

```bash
bbtidy format --write meta-my-layer/
bbtidy check meta-my-layer/
bitbake --parse-only core-image-minimal
```

Keep the generated diff and the BitBake results with the change review. For a
large repository, start with one layer or recipe family and expand the scope
only after the smaller run is understood.

`format --diff`, `check`, and `lint` do not modify source files. `format --write`
is the only documented repository-writing operation. Standard input
must be the only input for a command that reads `-`, and `--write` cannot be
combined with standard input.

## CI integration

Use `check` as the formatting gate and choose the lint output that matches the
CI system:

```bash
bbtidy check meta-my-layer/
bbtidy lint --output sarif meta-my-layer/ > bbtidy.sarif
```

Lint exits with code `1` only when a finding meets the configured failure
threshold. The default is warning-level gating. Use `--fail-on error` to make
warnings advisory, or `--fail-on never` for report-only mode:

```bash
bbtidy lint --fail-on error meta-my-layer/
bbtidy lint --fail-on never --output sarif meta-my-layer/ > bbtidy.sarif
```

The exit codes are:

| Code | Meaning |
| --- | --- |
| `0` | The command completed successfully; `check` found no changes and `lint` found no findings. |
| `1` | `check` found formatting differences or `lint` found diagnostics at or above its `fail_on` threshold. |
| `2` | Usage, discovery, parsing, formatting, lint analysis, or output failed. |

`format --diff` is a reporting command and returns `0` when it successfully
prints differences. Use `check` when differences must fail a CI job. JSON and
SARIF output is emitted only after all inputs have been analyzed successfully,
so an operational error does not leave a partial machine-readable report.

A minimal project configuration can make local and CI behavior explicit:

```toml
[format]
max_top_level_blank_lines = 1
metadata_list_layout = "preserve"

[lint]
disable = []
fail_on = "warning"

[paths]
exclude = ["vendor/**", "**/files/**"]

[safety]
max_files = 10000
max_bytes = 268435456
```

Configuration is discovered as `.bbtidy.toml` in the current directory and
then its parents. Use `--config PATH` to select a file explicitly or
`--no-config` to disable discovery. Exclusion globs are relative to the
configuration file and do not exclude standard input.

## Repository-wide safety

The default repository-wide limits are 10,000 discovered files and 256 MiB of
original source per formatting invocation. Set lower limits for an initial
rollout and raise them deliberately when the repository is known to require
more. `--max-files` and `--max-bytes` override the configured limits for one
invocation.

Before writing, bbtidy reads and formats the complete input set, checks the
limits, stages recovery copies, and checks that sources have not changed. It
refuses symbolic links and does not replace any file when a later input fails.
If a write or commit step fails, already replaced files are restored from the
staged recovery copies. These controls reduce accidental damage; they do not
replace version control, backups, or review.

## What to validate with BitBake

For supported releases, bbtidy's compatibility evidence includes both the
original and formatted pinned corpora, idempotence, preservation of opaque
payloads, structural coverage, a real BitBake parse-only run, and selected
semantic probes where configured. Users should still run their normal build
and test suite because parseability is not a complete semantic-equivalence
proof.

At minimum, after formatting a production layer:

1. Run `bbtidy check` on the same scope to prove the result is idempotent.
2. Run `bitbake --parse-only` for the images or recipes that the change can
   affect.
3. Run the normal build, package, and runtime tests used by the project.
4. Compare generated diffs and investigate any unexpected file outside the
   intended metadata scope.

## Reporting a compatibility issue

Open an issue with a minimal reproducible example when possible. Include:

- the exact `bbtidy --version` output and installation method;
- host operating system, architecture, and Rust/Python versions when relevant;
- Yocto Project and BitBake versions, including layer and repository commits;
- the exact command, configuration file, and input path scope;
- whether the issue occurred in `format`, `check`, `lint`, or `lex`;
- the original and formatted metadata, if it can be shared safely; and
- the first relevant bbtidy or BitBake diagnostic, exit code, and CI log.

Remove proprietary paths, credentials, and metadata before posting publicly.
Do not report a dynamic or unavailable dependency as a formatter regression
without first checking whether it is outside the supported scope in the
[contract](beta-support-contract.md).
