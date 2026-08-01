# bbtidy

Experimental formatter, linter, and lexer for BitBake metadata.

## Description

`bbtidy` is an experimental tool for formatting and inspecting BitBake recipes
and configuration files. It provides a lexer, a conservative formatter for
top-level metadata assignments, and file-local linting suitable for CI.

## Features

- **Tokenization**: efficiently breaks down BitBake files into tokens (Identifiers, Strings, Keywords, etc.) using `logos`.
- **Span Reporting**: Reports the exact location (line and column) of each token.
- **Modern metadata syntax**: Recognizes all eight assignment operators, colon
  overrides, key expansion, variable flags, multiline quoted values, and current
  BitBake directives.
- **Safe formatting boundaries**: Formats complete, single-line top-level
  assignments while preserving shell functions, Python functions, continued
  statements, and unsupported syntax.
- **Fail-safe writes**: Refuses to rewrite structurally incomplete input and
  replaces successfully formatted files atomically.
- **Automation-friendly CLI**: Provides explicit `format`, `check`, `lint`, and
  `lex` commands, standard-input support, unified diffs, and documented exit
  codes.
- **Layer-wide operation**: Recursively discovers supported BitBake files in
  deterministic path order.
- **Initial linting**: Reports stable rule IDs, severity, line, and column for a
  focused set of reproducibility and metadata hygiene checks.

## Installation

Install the native executable from PyPI with pip:

```bash
python -m pip install bbtidy
bbtidy --version
```

Because bbtidy is a command-line application, `pipx install bbtidy` is also a
good choice for an isolated global installation. The Python distribution does
not expose an importable module; it places the `bbtidy` executable in the
environment's scripts directory, following the same binary-wheel model used by
Rust-based Python tools such as Ruff.

Release wheels target:

- Linux x86-64 and ARM64 using glibc (`manylinux_2_17`)
- Linux x86-64 and ARM64 using musl (`musllinux_1_2`)
- macOS on Intel and Apple silicon
- Windows on x86-64

The wheels are independent of the Python interpreter implementation and support
Python 3.8 or newer. Installing a matching wheel does not require Rust. A source
distribution is also published as a fallback; building it requires Rust 1.85 or
newer.

## Usage

To inspect the tokens in a recipe:

```bash
bbtidy lex sample.bb
```

To print a formatted file without modifying it:

```bash
bbtidy format messy.bb
```

Standard input is accepted as `-`:

```bash
printf 'SUMMARY="Example"\n' | bbtidy format -
```

To inspect changes across a file or directory:

```bash
bbtidy format --diff recipes-example/
```

To check formatting in CI, then explicitly rewrite files when desired:

```bash
bbtidy check recipes-example/
bbtidy format --write recipes-example/
```

To lint a file, recipe directory, or layer:

```bash
bbtidy lint recipes-example/
```

Findings use an editor- and CI-friendly format:

```text
recipes-example/example.bb:12:11: warning[BBT004]: SRCREV uses ${AUTOREV}; pin a source revision for reproducible builds
```

Formatting is intentionally limited while BitBake syntax support is being
developed. Embedded shell and Python code is kept byte-for-byte unchanged.

Directory inputs are traversed recursively. Only `.bb`, `.bbappend`, `.bbclass`,
`.conf`, and `.inc` files discovered inside directories are processed; an
explicit file input is always processed. Paths are sorted and deduplicated
before processing. Standard input must be the only input, and `--write` cannot
be used with it.

`format` writes formatted source to standard output and requires one input
unless `--diff` or `--write` is selected. `format --diff`, `lint`, and `lex` can
process multiple inputs without changing them. Before `format --write` changes
any files, every input is read and formatted successfully; each changed file is
then replaced atomically.

### Exit codes

- `0`: the command completed successfully.
- `1`: `check` found formatting differences or `lint` found diagnostics.
- `2`: command usage, input/output, lexing, formatting, or lint analysis failed.

Operational diagnostics are written to standard error. Lexer error tokens
remain part of the token stream on standard output and cause exit code `2`.
`format --diff` returns `0` when it successfully reports differences; use
`check` when differences should fail a CI job.

## Initial lint rules

| Rule | Name | Detects |
| --- | --- | --- |
| `BBT001` | `trailing-whitespace` | Spaces or tabs at the end of a line |
| `BBT002` | `final-newline` | A non-empty file without a final newline |
| `BBT003` | `summary-length` | A static, literal `SUMMARY` longer than 80 characters |
| `BBT004` | `autorev` | `SRCREV` variants that use `${AUTOREV}` |
| `BBT005` | `duplicate-inherit` | A static class inherited more than once in one file |

All initial rules are warnings. Diagnostics are sorted by source location and
exposed through the public `lint`, `lint_rules`, `LintDiagnostic`, `LintRule`,
and `LintSeverity` Rust APIs. Structurally incomplete input is reported as an
operational error instead of producing potentially misleading findings.

The semantic rules are intentionally conservative: they inspect top-level
metadata, skip embedded shell and Python bodies, and avoid evaluating dynamic
values or class names. Cross-file inheritance, configuration, suppression, and
release-specific analysis remain future work.

## Supported syntax

The `0.1.0-alpha.1` lexer recognizes:

- Assignments using `=`, `:=`, `?=`, `??=`, `+=`, `=+`, `.=` and `=.`
- Literal and dynamic overrides such as `RDEPENDS:${PN}:class-native`
- Key expansion such as `A${B}` and variable flags such as
  `do_fetch[network]`
- `include`, `include_all`, `require`, `inherit`, `inherit_defer`,
  `addfragments`, `addpylib`, `addhandler`, `addtask`, `deltask`,
  `EXPORT_FUNCTIONS`, `export` and `unset`
- Single- and double-quoted values, including multiline values

Legacy underscore overrides remain lexically accepted as identifier and
variable-reference components. They are not interpreted as override operations.
The formatter remains deliberately conservative and only changes complete,
single-line top-level assignments.

## Development

Run the test suite with:

```bash
cargo test
```

Build and verify the Python wheel and source distribution with:

```bash
maturin build --release --locked --sdist --out dist
python scripts/smoke_test_package.py --kind wheel dist
python scripts/smoke_test_package.py --kind sdist dist
```

`pip install .` uses the same PEP 517 configuration for a local source build.
The Cargo version is the release source of truth; maturin converts prereleases
to PEP 440 automatically, for example `0.1.0-alpha.1` becomes `0.1.0a1`.

The integration suite includes a representative fixture layer containing
`.bb`, `.bbappend`, `.bbclass`, `.inc`, and `.conf` files. It verifies golden
output, idempotence, byte-for-byte preservation of embedded code, structured
errors, lint rule behavior, CLI modes and exit codes, deterministic directory
handling, and the no-write guarantee for malformed input.

To validate the formatted corpus with a real BitBake parser, use a disposable
build whose environment has already been initialized:

```bash
bbtidy_repository=/absolute/path/to/bbtidy
bitbake-layers add-layer "$bbtidy_repository/tests/fixtures/corpus/expected"
BBTIDY_BITBAKE_BUILD_DIR="$BUILDDIR" \
    "$bbtidy_repository/scripts/check-bitbake-parser.sh"
```

The script is opt-in because BitBake is not a project dependency. It invokes
`bitbake --parse-only example`, ensuring the corpus recipe is available and
parseable, and does not edit the selected build configuration.

## Releasing to PyPI

The Python package workflow builds and installs a wheel on every pull request
and push to `main`. The release workflow can be run manually to inspect all
platform artifacts without publishing.

To publish a release:

1. Update the version in `Cargo.toml` and finalize the changelog.
2. Create a tag that exactly matches the Cargo version, such as
   `v0.1.0-alpha.2`.
3. Push the tag. The release workflow builds and smoke-tests all wheels and the
   source distribution, then publishes them through PyPI Trusted Publishing.

Before the first automated publication, configure the `bbtidy` project on PyPI
with GitHub owner `jorisguex`, repository `bbtidy`, workflow
`publish-pypi.yml`, and environment `pypi`. No repository API token is needed.
The version guard rejects mismatched tags before any release artifacts are
built.

## License

This project is licensed under the MIT License.
