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

bbtidy's offline formatting and linting paths handle supported BitBake
metadata boundaries conservatively. They do not execute BitBake, expand
dynamic variables, or resolve unavailable external layers. The separate
`semantic` command delegates those checks to the installed BitBake engine.
Neither path rewrites embedded shell or Python code. A successful bbtidy run is
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
bbtidy format --check meta-my-layer/
bbtidy format --diff meta-my-layer/
bbtidy check --output sarif meta-my-layer/
bbtidy semantic --build-dir build --target core-image-minimal --output json
```

For a complete configured build, lint every listed layer and build
configuration file together:

```bash
bbtidy check --workspace build --bitbake bitbake
```

This invokes BitBake and uses its expanded `BBLAYERS`, `BBFILES`, `BBPATH`, and
`BBINCLUDED` values. The normal strategy uses one long-lived Tinfoil helper;
minimal executables, or recipe files omitted from BitBake's global inventory,
use an explicitly reported, bounded per-recipe fallback. Dynamically computed
layers, includes, and classes are therefore resolved by the installed engine.
A BitBake invocation or resolution failure is reported as an operational
error, so a successful run never represents only part of the configured build.
Use `--bitbake PATH` or `[semantic].bitbake` when BitBake
is not on `PATH`.

Review the diff and lint findings. Then run the repository's normal BitBake
parse, build, and test validation. If the results are acceptable, apply the
formatting in a clean branch or worktree:

```bash
bbtidy format --write meta-my-layer/
bbtidy format --check meta-my-layer/
bitbake --parse-only core-image-minimal
```

Keep the generated diff and the BitBake results with the change review. For a
large repository, start with one layer or recipe family and expand the scope
only after the smaller run is understood.

`format --check`, `format --diff`, and ordinary `check` runs do not modify source files.
`format --write` and `check --fix` are the repository-writing operations.
`check --fix` applies only safe whitespace and final-newline edits, re-runs the check,
and refuses standard input. Both write modes stage all changed files and use
the same concurrent-change checks and rollback-capable transaction. A parse or
analysis failure prevents any fix from being written.

`semantic` reads the selected BitBake build context and delegates evaluation to
the installed BitBake engine. It is the supported way to validate dynamic
expansion, overrides, anonymous Python, external layers, and machine/distro
configuration. BitBake may update its parse cache or server metadata in the
supplied build directory. The build directory can be discovered from
`.bbtidy.toml`, `BBTIDY_BITBAKE_BUILD_DIR`, `BUILDDIR`, or an ancestor project
directory containing `build`/`build-*`; use `--project-dir` to select the
discovery root. If more than one build variant matches, pass `--build-dir`
explicitly.

BitBake-backed commands are bounded independently of source safety limits.
Configure `[bitbake]` in `.bbtidy.toml` or use the matching `--bitbake-*` flags
to set command and operation timeouts, stdout/stderr caps, process-launch
budgets, and recipe-query budgets. Text mode reports command counts; JSON and
SARIF preserve execution metadata in a backward-compatible `execution` object.
Pressing Ctrl-C cancels the active BitBake process group and prevents a partial
machine-readable report.

For CI linting that should include the same authoritative checks, use
`check --semantic` with one or more explicit targets:

```bash
bbtidy check --semantic \
  --build-dir build \
  --target core-image-minimal \
  --variable IMAGE_FSTYPES \
  --output sarif meta-my-layer/ > bbtidy-semantic.sarif
```

This runs the BitBake parse and target environment queries in addition to the
static lint rules. BitBake diagnostics are reported as `BBT019`, and resolved
metadata checks cover dynamic `SUMMARY`, `DESCRIPTION`, `LICENSE`, recipe
identity/version, license and source checksums, `PACKAGECONFIG`, package
declarations/scopes, source revision, `SRC_URI` parameters, and effective layer
collection metadata (`BBT020` through `BBT033`), and resolved override
components (`BBT037`). It requires file inputs and an
initialized build directory; standard input is not supported in semantic mode.
Additional `--variable NAME` options are included in the machine-readable
semantic report. JSON and SARIF preserve the parse/target-query phase, output
stream, target result, diagnostics, and selected resolved environments.

Offline linting also includes recipe QA rules `BBT020` through `BBT028` for
static filename identity/version values, license and remote-source checksums,
`LIC_FILES_CHKSUM`, `PACKAGECONFIG`, package declarations/scopes, and `SRC_URI`
parameters. Layer QA rules `BBT029` through `BBT033` validate collection names,
patterns, priorities, dependencies, and `LAYERSERIES_COMPAT` in
`conf/layer.conf`.
Dynamic values are skipped by these checks and should be validated with
`check --semantic` or the project's normal BitBake workflow.

Lint also analyzes embedded bodies with `BBT034` through `BBT036`: shell
control-flow pairing, Python delimiters/compound-statement syntax, and Python
indentation. These checks do not execute code or external tools, and formatting
continues to leave shell and Python body bytes unchanged.

When `OVERRIDES` is statically known, offline lint normalizes modern colon and
unambiguous legacy underscore keys, resolves active precedence, and applies
`append`, `prepend`, and `remove` operations. `BBT037` identifies literal
override components missing from `OVERRIDES`; `check --semantic` performs the
same check against BitBake's expanded environment.

## CI integration

Use `format --check` as the formatting gate and choose the check output that
matches the CI system:

```bash
bbtidy format --check meta-my-layer/
bbtidy check --output sarif meta-my-layer/ > bbtidy.sarif
```

Lint exits with code `1` only when a finding meets the configured failure
threshold. The default is warning-level gating. Use `--fail-on error` to make
warnings advisory, or `--fail-on never` for report-only mode:

```bash
bbtidy check --fail-on error meta-my-layer/
bbtidy check --fail-on never --output sarif meta-my-layer/ > bbtidy.sarif
```

For a local cleanup pass, preview or apply the safe lint edits explicitly:

```bash
bbtidy check --show-fixes meta-my-layer/
bbtidy check --fix meta-my-layer/
```

The fix command reports the edits it applied, then reports any remaining
findings. `BBT003` through `BBT037` remain manual findings because changing
them requires project or BitBake semantics rather than a syntax-preserving
edit.

The exit codes are:

| Code | Meaning |
| --- | --- |
| `0` | The command completed successfully; `format --check` found no changes and `check` found no findings. |
| `1` | `format --check` found formatting differences or `check` found diagnostics at or above its `fail_on` threshold. |
| `2` | Usage, discovery, parsing, formatting, lint analysis, or output failed. |

`format --diff` is a reporting command and returns `0` when it successfully
prints differences. Use `format --check` when differences must fail a CI job.
JSON and SARIF output is emitted only after all inputs have been analyzed successfully,
so an operational error does not leave a partial machine-readable report.

A minimal project configuration can make local and CI behavior explicit:

```toml
[format]
max_top_level_blank_lines = 1
metadata_list_layout = "preserve"

[semantic]
build_dir = "build" # optional
bitbake = "bitbake" # command name or relative executable path

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
configuration file and apply to explicit paths, recursive discovery, and
BitBake workspace files; they do not exclude standard input. Excluded
workspace metadata is removed before class/include resolution and
recipe-specific BitBake discovery.

## Repository-wide safety

The default repository-wide limits are 10,000 discovered files and 256 MiB of
original source per `format` or `check` invocation. Set lower limits for an
initial rollout and raise them deliberately when the repository is known to
require more. `--max-files` and `--max-bytes` override the configured limits
for either command, including `check --workspace`.

Before writing, bbtidy reads and formats the complete input set, checks the
limits, stages recovery copies, and checks that sources have not changed. It
refuses symbolic links, including directory roots, and does not replace any
file when a later input fails.
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

1. Run `bbtidy format --check` on the same scope to prove the result is idempotent.
2. Run `bitbake --parse-only` for the images or recipes that the change can
   affect.
3. Run the normal build, package, and runtime tests used by the project.
4. Compare generated diffs and investigate any unexpected file outside the
   intended metadata scope.
5. Run `bbtidy semantic` against the same build directory when the change
   affects dynamic values, overrides, or layer interactions.

## Reporting a compatibility issue

Open an issue with a minimal reproducible example when possible. Include:

- the exact `bbtidy --version` output and installation method;
- host operating system, architecture, and Rust/Python versions when relevant;
- Yocto Project and BitBake versions, including layer and repository commits;
- the exact command, configuration file, and input path scope;
- whether the issue occurred in `format`, `format --check`, `check`, or `lex`;
- the original and formatted metadata, if it can be shared safely; and
- the first relevant bbtidy or BitBake diagnostic, exit code, and CI log.

Remove proprietary paths, credentials, and metadata before posting publicly.
Do not report a dynamic or unavailable dependency as a formatter regression
without first checking whether it is outside the supported scope in the
[contract](beta-support-contract.md).
