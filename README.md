# bbtidy

Experimental formatter, linter, and lexer for BitBake metadata.

## Description

`bbtidy` is an experimental tool for formatting and inspecting BitBake recipes
and configuration files. It provides a lexer, a conservative formatter for
top-level metadata assignments, and file-local or layer-aware linting suitable
for CI.

## Features

- **Tokenization**: efficiently breaks down BitBake files into tokens (Identifiers, Strings, Keywords, etc.) using `logos`.
- **Span Reporting**: Reports the exact location (line and column) of each token.
- **Lossless concrete syntax tree**: Represents every top-level byte as a
  source-backed node with stable ranges while retaining comments, blank lines,
  unknown syntax, and embedded bodies verbatim.
- **Modern metadata syntax**: Recognizes all eight assignment operators, colon
  overrides, key expansion, variable flags, multiline quoted values, and current
  BitBake directives.
- **Safe formatting boundaries**: Normalizes top-level assignments and
  directives while preserving continuation tails, comments, shell functions,
  Python functions, and unsupported syntax.
- **Fail-safe writes**: Refuses to rewrite structurally incomplete input and
  replaces successfully formatted files atomically.
- **Automation-friendly CLI**: Provides explicit `format`, `check`, `lint`, and
  `lex` commands, standard-input support, unified diffs, and documented exit
  codes.
- **Project configuration**: Loads an optional `.bbtidy.toml` with formatter
  settings, lint rule selection, severity overrides, and path exclusions.
- **Layer-wide operation**: Recursively discovers supported BitBake files in
  deterministic path order and indexes complete supplied layers for semantic
  checks.
- **Initial linting**: Reports stable rule IDs, severity, line, and column for a
  focused set of reproducibility and metadata hygiene checks.

## Installation

Install the native executable from PyPI with pip:

```bash
python3 -m pip install bbtidy
bbtidy --version
```

Because bbtidy is a command-line application, `pipx install bbtidy` is also a
good choice for an isolated global installation. The Python distribution does
not expose an importable module; it places the `bbtidy` executable in the
environment's scripts directory, following the same binary-wheel model used by
Rust-based Python tools such as Ruff.

Release wheels target:

- Linux x86-64 and ARM64 using glibc (`manylinux_2_17`)
- Linux ARMv7 hard-float using glibc (`manylinux_2_17_armv7l`)
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

For CI integrations, select a machine-readable report format:

```bash
bbtidy lint --output json recipes-example/
bbtidy lint --output sarif recipes-example/
```

JSON output is a versioned object with `version: 1` and a `diagnostics` array.
Each diagnostic contains `path`, `line`, `column`, `severity`, `rule_id`, and
`message`. SARIF output follows SARIF 2.1.0 and includes the complete rule
catalog plus source locations. Text output remains the default. Machine output
is emitted only after all inputs analyze successfully, so an operational error
cannot leave a partial JSON or SARIF document on standard output.

Findings use an editor- and CI-friendly format:

```text
recipes-example/example.bb:12:11: warning[BBT004]: SRCREV uses ${AUTOREV}; pin a source revision for reproducible builds
```

Formatting is intentionally conservative while BitBake syntax support is being
developed. Assignment-operator spacing is normalized for both single-line and
continued assignments. Directive spacing is normalized only between the
keyword and its arguments. By default, continuation tails, argument contents,
comments, and embedded shell and Python code are kept byte-for-byte unchanged.
An opt-in layout is available for a small set of static metadata lists; its
safety limits are described in the configuration section. The opaque shell
boundary scanner understands quoted and tab-stripping
here-documents, multiple pending here-documents, shell arithmetic, and braces
inside quoted strings or comments. Runs of top-level blank lines are reduced to
one without changing blank lines inside embedded functions.

Directory inputs are traversed recursively. `.bb`, `.bbappend`, `.bbclass`, and
`.inc` metadata files are discovered automatically, except beneath recipe
payload directories named `files`. `.conf` files are discovered inside a
layer's `conf` directory, identified by its `layer.conf`. An explicit file input
is always processed. Paths are sorted and deduplicated before processing.
Standard input must be the only input, and `--write` cannot be used with it.

`format` writes formatted source to standard output and requires one input
unless `--diff` or `--write` is selected. `format --diff`, `lint`, and `lex` can
process multiple inputs without changing them. Before `format --write` changes
any files, every input is read and formatted successfully; each changed file is
then replaced atomically.

### Configuration

By default, bbtidy searches for `.bbtidy.toml` in the current directory and
then each parent directory. Use `--config PATH` to select a specific file or
`--no-config` to disable configuration discovery. An explicit config file takes
precedence over automatic discovery. If no file is found, built-in defaults are
used and behavior remains unchanged.

The supported configuration keys are:

```toml
[format]
max_top_level_blank_lines = 1
metadata_list_layout = "preserve" # or "one-per-line"

[lint]
disable = ["BBT003"]

[lint.severity]
BBT001 = "error"
BBT004 = "info"

[paths]
exclude = ["vendor/**", "**/files/**"]
```

`metadata_list_layout` defaults to `preserve`. The opt-in `one-per-line` mode
only reindents continued, single- or double-quoted static lists for `SRC_URI`,
`DEPENDS`, and `RDEPENDS`, including colon overrides such as
`RDEPENDS:${PN}`. It keeps the existing item order and only acts when every
item is already one static whitespace-free token on a continuation line with
consistent line endings. Dynamic expansions, escaped or quoted items,
multi-item lines, generic variables, and malformed or mixed-line-ending values
remain unchanged apart from normal assignment spacing.

Lint rule IDs are the stable IDs listed in the lint-rule table. Severity values
are `info`, `warning`, or `error`. Exclusion globs are relative to the
configuration file’s directory and apply to explicit files and recursively
discovered files. Standard input is never excluded. Unknown keys, rule IDs,
severity values, malformed TOML, and invalid globs are operational errors.

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
| `BBT006` | `unresolved-require` | A static `require` target missing from the indexed layers |
| `BBT007` | `unresolved-inherit` | A static inherited class missing from the indexed layers |
| `BBT008` | `ambiguous-require` | A static `require` target matches multiple highest-priority files |
| `BBT009` | `ambiguous-inherit` | A static inherited class has multiple highest-priority definitions |
| `BBT010` | `dependency-cycle` | A resolved static `include`, `require`, or `inherit` closes a dependency cycle |

All initial rules are warnings. Diagnostics are sorted by source location and
exposed through the public `lint`, `lint_rules`, `LintDiagnostic`, `LintRule`,
and `LintSeverity` Rust APIs. Structurally incomplete input is reported as an
operational error instead of producing potentially misleading findings.

The semantic rules are intentionally conservative: they inspect top-level
metadata, skip embedded shell and Python bodies, and avoid evaluating dynamic
values or class names. When `lint` receives a complete layer directory, it
indexes the supplied metadata into a static dependency graph. The graph follows
the effective target of `include`, `require`, `inherit`, and `inherit_defer`,
and every target of `include_all`; it reports cycles but skips dynamic and
unresolved optional references. The workspace model reads `BBFILE_COLLECTIONS`,
`BBFILE_PATTERN_*`, `BBFILE_PRIORITY_*`, and simple static `BBPATH` entries
from each supplied `conf/layer.conf`. A relative `include` or `require` first
checks the directory containing the current file and then searches `BBPATH`;
`include_all` searches only `BBPATH` and retains every match. Inherited classes
are searched in `classes-recipe` directories before ordinary `classes`
directories. `include` and `include_all` are optional directives, so missing
matches do not produce unresolved-reference diagnostics; `require` remains
strict. Same-priority matches in the winning search scope produce the
ambiguity rules above instead of being silently resolved. Ambiguity and cycle
diagnostics identify the selected target's layer, collection, priority, and
search scope. The public `WorkspaceCandidate` and `WorkspaceDependency` APIs
expose the corresponding resolution and graph information. Single-file and
standard-input linting remain file-local, and dynamic references are skipped.

## Lossless syntax tree

The Rust library exposes `parse` as the shared structural front end for
formatting and linting. Its `SyntaxTree` borrows the original source and divides
it into ordered, contiguous `SyntaxNode` ranges. Concatenating `node.text()` for
every node always reproduces the input byte-for-byte.

Recognized nodes provide structured data and absolute source ranges for
assignments, directives, shell and Python functions, and top-level Python
definitions. Blank lines, comments, and unsupported top-level constructs remain
explicit nodes. Function and Python-definition bodies are deliberately opaque:
bbtidy finds their safe boundaries but does not interpret or rewrite the
embedded language.

```rust
use bbtidy::{SyntaxKind, format_syntax, lint_syntax, parse};

let source = "SUMMARY=\"Example\"\n";
let tree = parse(source)?;

if let SyntaxKind::Assignment(assignment) = tree.nodes()[0].kind() {
    assert_eq!(assignment.name(), "SUMMARY");
}

let formatted = format_syntax(&tree);
let diagnostics = lint_syntax(&tree);
# Ok::<(), bbtidy::SyntaxError>(())
```

`format` and `lint` are convenience entry points that parse source and then
delegate to `format_syntax` and `lint_syntax`. Callers performing multiple
operations can parse once and reuse the same tree.

## Supported syntax

The `0.1.0-alpha.4` lexer recognizes:

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
The formatter remains deliberately conservative: outside the opt-in static-list
layout above, it does not wrap values, reindent continuation lines, or format
embedded shell or Python code.

## Development

Run the test suite with:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
git diff --check
python3 -m unittest discover -s tests -p "test_*.py"
```

CI requires every third-party GitHub Action to use an immutable commit pin. Run
the repository check locally with:

```bash
python3 scripts/check_workflows.py
```

The scheduled Security workflow also runs `actionlint`, audits both Cargo lock
files against the RustSec advisory database, and Dependabot opens weekly Cargo
and GitHub Actions update pull requests.

Measure the main layer-analysis paths on a repeatable synthetic 1,000-recipe
fixture with:

```bash
cargo bench --locked --bench layer_analysis
```

The benchmark reports workspace index construction, single-file formatting,
and batch workspace-aware linting. It is intended for comparing changes across
the same machine rather than enforcing a wall-clock threshold in CI.

Exercise parser and formatter invariants with the property tests:

```bash
cargo test --test parser_properties --locked
```

The repository also contains a cargo-fuzz target for the parser, formatter, and
linter boundary. With `cargo-fuzz` and a nightly toolchain installed, run a
short local smoke test with:

```bash
cargo +nightly fuzz check parser
cargo +nightly fuzz run parser -- -max_total_time=60
```

The fuzz target checks that successful parsing is lossless, formatting remains
parseable, formatting is idempotent, and linting does not panic. Seed inputs
are stored under `fuzz/corpus/parser/`; the fuzz workflow runs a bounded smoke
test for changes affecting the parser or formatter.

The package workflow runs these quality checks before building artifacts and
smoke-tests both the wheel and source distribution. Tag-release validation uses
the same checks before enabling the PyPI publishing jobs.

Release artifact expectations are defined once in
`release-metadata.json`. The release workflows derive their wheel matrix,
standalone-binary names, and Linux container smoke tests from that manifest.
The `verify_release_artifacts.py` helper rejects missing or unexpected wheel
platforms, mismatched embedded package metadata, unsafe source-distribution
paths, and an incomplete source-distribution set before publication.

Build and verify the Python wheel and source distribution with:

```bash
maturin build --release --locked --sdist --out dist
python3 scripts/smoke_test_package.py --kind wheel dist
python3 scripts/smoke_test_package.py --kind sdist dist
```

`pip install .` uses the same PEP 517 configuration for a local source build.
The Cargo version is the release source of truth; maturin converts prereleases
to PEP 440 automatically, for example `0.1.0-alpha.4` becomes `0.1.0a4`.

The integration suite includes a representative fixture layer containing
`.bb`, `.bbappend`, `.bbclass`, `.inc`, and `.conf` files. It verifies golden
output, idempotence, byte-for-byte preservation of embedded code, structured
errors, lint rule behavior, CLI modes and exit codes, deterministic directory
handling, and the no-write guarantee for malformed input.

### Upstream compatibility corpus

The extended compatibility check uses commit-pinned snapshots of
OpenEmbedded-Core and the `meta-oe`, `meta-python`, and `meta-networking`
layers. The revisions and minimum corpus sizes are recorded in
`tests/upstream-corpus.json`; upstream repositories are downloaded into a
temporary workspace and are not vendored in this repository.

On a supported Linux build host with the standard Yocto host packages
installed, run the complete check with:

```bash
cargo build --release --locked
python3 scripts/check_upstream_corpus.py
```

The harness scans more than 3,300 real metadata files, formats a disposable
copy, verifies idempotence, exercises lint analysis, checks that embedded
functions and Python blocks remain byte-for-byte unchanged, and confirms that
recipe payload files were not touched. It then initializes a disposable Poky
build and parses `core-image-minimal` with all four formatted layers.

Existing pinned checkouts can be reused, and the BitBake parse can be omitted
on a non-Linux development machine:

```bash
python3 scripts/check_upstream_corpus.py \
    --source-root /path/containing/poky-and-meta-openembedded \
    --skip-bitbake
```

The upstream compatibility workflow runs for relevant pull requests and pushes
to `main`, as well as on a weekly schedule. A compatibility failure should be
reduced to a focused local regression test before the pinned snapshot is
advanced.

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

## Releasing

The Python package workflow builds and installs a wheel on every pull request
and push to `main`. The release workflow can be run manually to inspect all
platform artifacts without publishing. The crates.io workflow also supports
manual validation, but its publication job only runs for a pushed `v*` tag.

Tag releases also create a GitHub Release with standalone `bbtidy` binaries
for every Python-wheel platform: Linux glibc x86-64, ARM64, and ARMv7;
Linux musl x86-64 and ARM64; macOS Intel and Apple silicon; and Windows
x86-64. The release job extracts these binaries from the exact wheel set and
rejects an incomplete or unexpected platform set before publishing the assets.
Linux binaries are smoke-tested in matching glibc or musl containers under
native execution or QEMU, and each standalone binary is accompanied by a
`SHA256SUMS` entry.

To publish a release:

1. Update the version in `Cargo.toml` and finalize the changelog.
2. Create a tag that exactly matches the Cargo version, such as
   `v0.1.0-alpha.3`.
3. Push the tag. The release workflows run their validation gates. The Python
   workflow builds and smoke-tests the wheels and source distribution, then
   publishes through PyPI Trusted Publishing and creates the GitHub Release.
   The crates.io workflow validates the Cargo package and publishes it through
   crates.io Trusted Publishing.

Before the first automated publication, configure the `bbtidy` project on PyPI
with GitHub owner `jorisguex`, repository `bbtidy`, workflow
`publish-pypi.yml`, and environment `pypi`. No repository API token is needed.
For crates.io, configure the `bbtidy` crate with owner `jorisguex`, repository
`bbtidy`, workflow `publish-crates.yml`, and environment `crates-io-release`.
No repository API token is needed for either publisher. The version guard
rejects mismatched tags before release artifacts are built, and manual workflow
runs never publish to either registry. Registry publication and GitHub Release
asset creation wait for the complete distribution and binary verification jobs;
Linux smoke tests also compare each executable's `--version` output with the
Cargo release version.

## License

This project is licensed under the MIT License.
