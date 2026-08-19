# bbtidy

Conservative formatting and linting for BitBake metadata.

## What bbtidy does

`bbtidy` formats supported top-level BitBake metadata, reports static or
build-aware lint findings, and preserves comments, unsupported syntax, and
embedded shell and Python bodies. Its ordinary `format` and `check` paths do
not execute BitBake.

Formatting is intentionally narrow: it normalizes assignment and directive
spacing, top-level blank lines, and optionally selected static lists. Linting
reports findings without changing files unless `check --fix` is selected.

## Current release status and supported Yocto versions

The current release is the `0.1.0-alpha.4` evaluation build. Alpha releases do
not carry the beta support claim. The intended beta contract covers:

| Yocto Project release | BitBake version | Status |
| --- | --- | --- |
| 5.0 LTS (Scarthgap) | 2.8 | Supported beta target |
| 6.0 LTS (Wrynose) | 2.18 | Supported beta target |
| `master` | Development | Non-blocking compatibility target |

Read the [beta support contract](docs/beta-support-contract.md) for exact
guarantees, unsupported boundaries, and required release evidence.

## Verified installation command

Pin the evaluation release so package managers do not select a different
version:

```bash
python3 -m pip install --pre "bbtidy==0.1.0a4"
bbtidy --version
```

For an isolated executable installation:

```bash
pipx install "bbtidy==0.1.0a4"
bbtidy --version
```

Published wheels support Linux glibc and musl on x86-64 and ARM64, Linux
glibc on ARMv7, macOS on Intel and Apple silicon, and Windows on x86-64.
Installing a wheel requires Python 3.8 or newer but does not require Rust. A
source build requires Rust 1.85 or newer.

## Five-minute read-only trial

Use a layer path in place of `meta-my-layer/`:

```bash
bbtidy --version
bbtidy format --diff meta-my-layer/
bbtidy check --profile recommended --fail-on never meta-my-layer/
```

| Question | Answer |
| --- | --- |
| What will this change? | The diff previews supported formatting changes; lint only reports findings. |
| Will it write files? | No. These commands are read-only. |
| Will it invoke BitBake? | No. The trial uses only offline analysis. |
| Which command should I run first? | Start with `bbtidy format --diff meta-my-layer/`. |
| What does exit code `1` mean? | Formatting differs or lint found a diagnostic at the configured failure threshold; it is not an operational crash. |

`format --diff` returns `0` after successfully showing a diff.
`--fail-on never` keeps lint findings report-only. Exit code `2` means usage,
discovery, parsing, analysis, or output failed.

Follow the [getting-started tutorial](docs/getting-started.md) to add a small
configuration, handle existing findings, enable CI, and optionally move to
BitBake-backed workspace checking.

## Two CI commands

After pinning and recording the exact bbtidy version in CI, the ordinary gates
are:

```bash
bbtidy format --check meta-my-layer/
bbtidy check --profile recommended meta-my-layer/
```

Both commands are read-only and offline. `format --check` returns `1` when it
would reformat a file; `check` returns `1` when an enabled finding meets the
configured failure threshold. See [CI integration](docs/ci-integration.md) for
the pinned install, staged enforcement, GitHub Actions, SARIF, and pre-commit
examples.

## Production and advanced documentation

- [Copyable starter assets](examples/README.md): minimal configuration,
  generic CI, GitHub Actions, pre-commit, and existing-repository migration.
- [Getting started](docs/getting-started.md): one linear adoption workflow.
- [Beta user guide](docs/beta-user-guide.md): production rollout, validation,
  release rehearsal, and issue reporting.
- [Configuration reference](docs/configuration.md): every `.bbtidy.toml`
  setting, default, and precedence rule.
- [Lint reference](docs/lint-rules.md): rule catalog, profiles, suppressions,
  baselines, fixes, and output formats.
- [BitBake integration](docs/bitbake-integration.md): when to use offline
  checking, workspace checking, semantic linting, or semantic inspection.
- [CI integration](docs/ci-integration.md): generic CI, GitHub Actions, SARIF,
  and pre-commit examples.

The preferred authoritative production check asks BitBake for the complete
configured workspace:

```bash
bbtidy check --workspace build --profile recommended
```

Target-specific semantic linting and the standalone `semantic` report are
advanced workflows because they invoke BitBake and may update its normal parse
cache or server metadata.

## Core behavior

bbtidy discovers `.bb`, `.bbappend`, `.bbclass`, `.inc`, and layer `.conf`
files in deterministic path order. It skips recipe payload directories named
`files` during recursive discovery and applies project exclusions before
analysis.

The parser builds a lossless top-level concrete syntax tree. Supported
assignments, directives, functions, Python definitions, comments, blank lines,
and unknown syntax retain source-backed byte ranges. Formatting is idempotent,
and structurally incomplete input is rejected before any write.

Text diagnostics use editor-friendly paths, ranges, severities, and stable rule
IDs. JSON and SARIF are available for automation. Safe fixes are limited to
trailing whitespace and final newlines; all other findings require review.

Repository-wide writes are explicit and transactional. `format --write` and
`check --fix` stage the complete change set, reject symbolic links and
concurrent source changes, and restore earlier replacements if a later commit
step fails. The defaults cap one invocation at 10,000 files and 256 MiB of
source.

## Rust library

The crate exposes the lossless syntax tree, formatter, lint diagnostics,
override resolver, workspace index, build-context discovery, bounded BitBake
runner, and semantic reports. The library and executable use the same parser
and analysis implementation. The [configuration](docs/configuration.md),
[lint](docs/lint-rules.md), and [BitBake](docs/bitbake-integration.md)
references describe the corresponding public behavior.

## Compatibility evidence

Pinned Yocto 5.0, Yocto 6.0, and community corpora exercise formatting,
idempotence, opaque-payload preservation, parser coverage, lint fingerprints,
and supported BitBake parsing. Detailed evidence lives in:

- [Parser precision and embedded-language evidence](docs/parser-precision.md)
- [BitBake execution and scalability](docs/bitbake-execution.md)
- [Performance evidence and budgets](tests/performance/README.md)
- [Upstream lint-quality baselines](tests/upstream-corpora/lint-baselines/README.md)

The evidence proves the documented compatibility properties; it does not prove
identical task hashes, packages, runtime behavior, or complete builds.

## Development

The project uses Rust 1.91.1 for development while declaring Rust 1.85 as the
minimum supported source-build version. Run the local quality gates with:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
git diff --check
python3 -m unittest discover -s tests -p "test_*.py"
```

Additional compatibility and performance commands are documented in the
evidence guides above. The parser fuzz target can be exercised with:

```bash
cargo +nightly fuzz check parser
cargo +nightly fuzz run parser -- -max_total_time=60
```

Third-party GitHub Actions must use immutable commit pins. The security
workflow validates workflow pins, runs `actionlint`, and audits the root and
fuzz Cargo lockfiles.

## Releasing

`release.yml` is the sole tag-triggered orchestrator. A release candidate must
first pass a non-publishing rehearsal of source quality, packaging, supported
Yocto compatibility, pinned-community lint evidence, performance evidence, and
artifact verification. Publication then uses trusted publishing for PyPI and
crates.io and attaches verified standalone binaries and checksums to the
GitHub Release.

The complete procedure and negative-gate rehearsal are documented in the
[beta user guide](docs/beta-user-guide.md#release-rehearsal-and-publication)
and [beta support contract](docs/beta-support-contract.md#release-gate-and-rehearsal-procedure).

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE).
