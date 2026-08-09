# bbtidy

Conservative formatter, linter, and lexer for BitBake metadata.

## Description

`bbtidy` is a conservative tool for formatting and inspecting BitBake recipes
and configuration files. It provides a lexer, a formatter for supported
top-level metadata boundaries, and file-local or layer-aware linting suitable
for CI.

Current releases are alpha prereleases. The intended beta compatibility,
safety, and support commitments are defined in the [beta support
contract](docs/beta-support-contract.md). The practical rollout steps are in
the [beta user guide](docs/beta-user-guide.md).

## Features

- **Tokenization**: efficiently breaks down BitBake files into tokens (Identifiers, Strings, Keywords, etc.) using `logos`.
- **Span Reporting**: Reports the exact location (line and column) of each token.
- **Lossless concrete syntax tree**: Represents every top-level byte as a
  source-backed node with stable ranges while retaining comments, blank lines,
  unknown syntax, and embedded bodies verbatim.
- **Modern metadata syntax**: Recognizes all eight assignment operators, colon
  overrides, key expansion, variable flags, multiline quoted values, and current
  BitBake directives.
- **Safe formatting boundaries**: Normalizes top-level assignments, directives,
  and configured list layouts while preserving continuation tails, comments,
  blank lines, embedded functions, and unsupported syntax.
- **Fail-safe writes**: Refuses to rewrite structurally incomplete input and
  replaces successfully formatted files atomically.
- **Automation-friendly CLI**: Provides explicit `format`, `check`, `lint`,
  `lex`, and authoritative `semantic` commands, standard-input support,
  unified diffs, and documented exit codes.
- **Project configuration**: Loads an optional `.bbtidy.toml` with formatter
  settings, lint rule selection, severity overrides, and path exclusions.
- **Project/build-context discovery**: Finds configured BitBake build trees from
  project configuration, environment variables, or conventional ancestor
  directories, with provenance and ambiguity diagnostics.
- **Layer-wide operation**: Recursively discovers supported BitBake files in
  deterministic path order and indexes complete supplied layers for semantic
  checks.
- **Whole-build workspace linting**: `lint --workspace` asks BitBake for the
  expanded build scope, indexes every resolved layer plus build configuration,
  and resolves dynamic includes/classes across the complete parsed workspace.
- **Actionable linting**: Reports stable rule IDs, severity, source ranges, help,
  and safe edit suggestions, with transactional `lint --fix` support for
  whitespace and final-newline findings.
- **BitBake-backed semantic linting and build analysis**: Optionally runs the
  configured BitBake parser, target environment queries, dependency graph,
  dry-run scheduler, recipe/provider inventory, and package/image metadata
  analysis, preserving rich phase- and target-level output in machine-readable
  reports.
- **Broader recipe and layer QA**: Checks recipe identity/version alignment,
  license and source checksums, `PACKAGECONFIG`, package-scoped variables and
  lists, `SRC_URI` parameters, and layer collection metadata.
- **Embedded body analysis**: Conservatively checks shell control-flow and
  embedded/top-level Python syntax and indentation while preserving body bytes.

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
bbtidy lex examples/sample.bb
```

To print a formatted file without modifying it:

```bash
bbtidy format examples/messy.bb
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

To lint every configured layer and build configuration file in a BitBake
workspace:

```bash
bbtidy lint --workspace build --bitbake bitbake
```

Workspace mode asks the selected BitBake engine to parse the build, then uses
its expanded `BBLAYERS`, `BBFILES`, `BBPATH`, and per-recipe `BBINCLUDED`
values. This means dynamically computed layers, classes, includes, overrides,
and external metadata are resolved by BitBake rather than reconstructed from
source-level assignments. A BitBake invocation failure is an operational
error; bbtidy never falls back to a partial static workspace. Use
`--bitbake PATH` or `[semantic].bitbake` when the engine is not on `PATH`.

To combine static linting with authoritative BitBake semantics:

```bash
bbtidy lint --semantic \
  --build-dir build \
  --target core-image-minimal \
  recipes-example/
```

`lint --semantic` requires an initialized BitBake build directory and uses the
same project, environment-variable, configuration, and executable discovery
as `semantic`. It runs `bitbake --parse-only` once, then `bitbake -e` for each
selected `--target`. BitBake warnings and errors are reported as `BBT019` lint
diagnostics. Add `--variable NAME` to include additional fully expanded
variables in the semantic section of the report. `SUMMARY`, `DESCRIPTION`,
`LICENSE`, `SRCREV`/`SRCPV`, and
resolved `SRC_URI` values from target environments are also checked by the
corresponding metadata rules, so dynamic expansions are included when a target
is queried. Add `--full` to include the build analysis sections below, or
select `--graph`, `--dry-run`, `--inventory`, and `--packages` individually.
The mode is file-based and cannot be combined with standard input.

For CI integrations, select a machine-readable report format:

```bash
bbtidy lint --output json recipes-example/
bbtidy lint --output sarif recipes-example/

# Explain safe edits without changing files, or apply them transactionally.
bbtidy lint --show-fixes recipes-example/
bbtidy lint --fix recipes-example/
```

To run BitBake's authoritative parser and inspect fully expanded recipe values:

```bash
bbtidy semantic \
  --build-dir build \
  --target core-image-minimal \
  --variable PN \
  --variable OVERRIDES \
  --output json
```

`semantic` requires an existing BitBake build directory containing
`conf/local.conf` and `conf/bblayers.conf`. `--build-dir` is optional: when it
is omitted, bbtidy checks `[semantic].build_dir` in `.bbtidy.toml`, then
`BBTIDY_BITBAKE_BUILD_DIR`, then `BUILDDIR`, and finally searches the supplied
`--project-dir` (or the current directory) and its ancestors for a configured
directory or a conventional `build`/`build-*` directory. Multiple matching
build variants are rejected rather than guessed. Use `--project-dir PATH` to
control the discovery root and `--bitbake PATH` when the engine is not on
`PATH`.

It invokes the selected `bitbake` executable in the discovered directory, so
variable expansion, overrides, anonymous Python, class inheritance, layer
priorities, machine and distro configuration, and external providers are
evaluated by the installed BitBake version rather than approximated by
bbtidy. The command performs a parse-only check first; requested targets are
then queried with `bitbake -e`. `--graph` uses BitBake's graph artifacts,
`--dry-run` asks BitBake's scheduler for a non-executing build plan,
`--inventory` parses `--show-versions`, and `--packages` summarizes resolved
package, provider, runtime dependency, and image variables. These analyses do
not execute build tasks; BitBake may still update its normal parse cache.

For the complete report:

```bash
bbtidy semantic --build-dir build --target core-image-minimal --full --output json
```

Semantic JSON is a versioned object with `version: 1`, the selected BitBake
executable and version, resolved project and build directories,
`build_context_source`, requested targets and variables, parse and target-query
status, source-aware BitBake diagnostics, selected target environments, and a
target result for every requested target. Diagnostics identify their parse,
target-query, graph, dry-run, inventory, or package-summary phase, target when
applicable, output stream, severity, message,
and optional source location fields. Text output remains the default. The Rust
API retains each complete `bitbake -e` dump through
`SemanticEnvironment::raw`; the JSON report omits that verbose field. The
`lint --semantic` command embeds the same diagnostics, environments, target
results, and any requested `build_analysis` sections under its `semantic`
object, so JSON and SARIF consumers do not lose BitBake detail.
Lint JSON retains `version: 1` and adds diagnostic end positions,
byte ranges, help, `fixable`, and structured `fixes` entries. A `lint --fix`
report also contains `fixes_applied`. SARIF output follows SARIF 2.1.0 with the
complete lint rule catalog, fixability properties, source ranges, and SARIF
fixes.

Operational errors are written to standard error rather than producing a
partial machine-readable document. A completed semantic analysis still emits
its structured report when BitBake reports parse or target-query failures, so
CI can inspect the diagnostics before acting on exit code `1`.

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
unless `--diff` or `--write` is selected. `format --diff`, ordinary `lint`, and
`lex` can process multiple inputs without changing them. Before
`format --write` or `lint --fix` changes any files, every input is read,
analyzed, staged, and checked for concurrent changes. Changed files are then
replaced as one transactional batch; if a commit step fails, previously
replaced files are restored from their staged recovery copies. Symbolic links
are never replaced. `lint --fix` refuses standard input and applies only edits
proposed by safe fixable rules; structural analysis failures prevent all
writes.

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

[semantic]
build_dir = "build" # optional; auto-discovery is used when omitted
bitbake = "bitbake" # command name or a path such as "./tools/bitbake"
full = false # enable graph, dry-run, inventory, and package analysis
graph = false
dry_run = false
inventory = false
packages = false

[lint]
disable = ["BBT003"]
fail_on = "warning" # info, warning, error, or never

[lint.severity]
BBT001 = "error"
BBT004 = "info"

[paths]
exclude = ["vendor/**", "**/files/**"]

[safety]
max_files = 10000
max_bytes = 268435456
```

`metadata_list_layout` defaults to `preserve`. The opt-in `one-per-line` mode
reindents continued, single- or double-quoted static whitespace lists for
common metadata such as `SRC_URI`, `DEPENDS`, `RDEPENDS`, `RRECOMMENDS`,
`FILES`, `PACKAGES`, `PACKAGECONFIG`, `INHERIT`, and feature lists, including
colon and legacy underscore overrides. It also lays out static single-line or
continued `inherit`, `require`, `include`, `export`, `unset`,
`EXPORT_FUNCTIONS`, `addhandler`, and `deltask` arguments. It keeps the
existing item order and only acts when every item can be identified as one
static whitespace-free token with consistent line endings. Dynamic
expansions, escaped or quoted items, comments, unrelated variables, and
malformed or mixed-line-ending values remain unchanged apart from normal
assignment spacing.

The safety limits default to 10,000 files and 256 MiB of original source per
repository-writing `format` invocation; `lint --fix` also enforces them from
`[safety]`. They apply after recursive discovery and exclusions, so a
repository-wide write cannot silently expand beyond a bounded scope. Zero is
rejected. A file that changes after it was read is also rejected rather than
overwritten.

Lint rule IDs are the stable IDs listed in the lint-rule table. Severity values
are `info`, `warning`, or `error`. `fail_on` controls the minimum effective
severity that makes `lint` exit with code `1`; it defaults to `warning`, and
`never` makes lint advisory while still reporting findings. The command-line
`--fail-on` option overrides the configuration for one invocation. Exclusion
globs are relative to the configuration file’s directory and apply to
explicit files and recursively discovered files. Standard input is never
excluded. Unknown keys, rule IDs, severity values, failure policies,
malformed TOML, and invalid globs are operational errors.

`--show-fixes` adds indented help and edit details to text diagnostics. `--fix`
applies all safe edits, re-runs lint on the resulting source, and uses the
remaining findings for the exit status. It can still exit `1` for non-fixable
findings such as `${AUTOREV}` or unresolved references. It exits `2` when
analysis, staging, or the transactional commit fails.

### Exit codes

- `0`: the command completed successfully.
- `1`: `check` found formatting differences or `lint` found diagnostics at or
  above its configured `fail_on` threshold.
- `1`: `semantic` completed but BitBake reported parse or target-analysis
  errors.
- `2`: command usage, input/output, lexing, formatting, or lint analysis failed.

Operational diagnostics are written to standard error. Lexer error tokens
remain part of the token stream on standard output and cause exit code `2`.
`format --diff` returns `0` when it successfully reports differences; use
`check` when differences should fail a CI job.

## Lint rules

| Rule | Name | Detects | Safe fix |
| --- | --- | --- | --- |
| `BBT001` | `trailing-whitespace` | Spaces or tabs at the end of a line | Remove trailing spaces/tabs |
| `BBT002` | `final-newline` | A non-empty file without a final newline | Append a newline |
| `BBT003` | `summary-length` | A static, literal `SUMMARY` longer than 80 characters | Manual |
| `BBT004` | `autorev` | `SRCREV` variants that use `${AUTOREV}` | Manual |
| `BBT005` | `duplicate-inherit` | A static class inherited more than once in one file | Manual |
| `BBT006` | `unresolved-require` | A static `require` target missing from the indexed layers | Manual |
| `BBT007` | `unresolved-inherit` | A static inherited class missing from the indexed layers | Manual |
| `BBT008` | `ambiguous-require` | A static `require` target matches multiple highest-priority files | Manual |
| `BBT009` | `ambiguous-inherit` | A static inherited class has multiple highest-priority definitions | Manual |
| `BBT010` | `dependency-cycle` | A resolved static `include`, `require`, or `inherit` closes a dependency cycle | Manual |
| `BBT011` | `missing-summary` | A recipe in a complete indexed layer has no `SUMMARY` assignment | Manual |
| `BBT012` | `missing-description` | A recipe in a complete indexed layer has no `DESCRIPTION` assignment | Manual |
| `BBT013` | `missing-license` | A recipe in a complete indexed layer has no `LICENSE` assignment | Manual |
| `BBT014` | `file-paths-immediate` | `FILESEXTRAPATHS` does not use immediate `:=` expansion | Manual |
| `BBT015` | `git-uri-protocol` | A static `git://` `SRC_URI` entry omits its transport protocol | Manual |
| `BBT016` | `duplicate-assignment` | A variable is assigned directly more than once in one file | Manual |
| `BBT017` | `duplicate-function` | A task or function is declared more than once in one file | Manual |
| `BBT018` | `empty-directive` | A static dependency directive has no target | Manual |
| `BBT019` | `bitbake-diagnostic` | A BitBake semantic diagnostic from parsing, querying, graphing, inventory, or build-plan analysis | Manual |
| `BBT020` | `recipe-name` | An explicit `PN` does not match the recipe filename | Manual |
| `BBT021` | `recipe-version` | An explicit `PV` does not match the recipe filename | Manual |
| `BBT022` | `license-checksum` | A non-`CLOSED` recipe lacks valid license file checksums | Manual |
| `BBT023` | `source-checksum` | A remote source archive lacks a valid checksum | Manual |
| `BBT024` | `packageconfig` | An enabled `PACKAGECONFIG` feature has no definition | Manual |
| `BBT025` | `packageconfig-format` | A `PACKAGECONFIG[feature]` definition has the wrong field count | Manual |
| `BBT026` | `package-scope` | A package-scoped variable names an undeclared package | Manual |
| `BBT027` | `package-list` | `PACKAGES` declares the same package more than once | Manual |
| `BBT028` | `uri-parameters` | A `SRC_URI` parameter is invalid or conflicts with its transport | Manual |
| `BBT029` | `layer-collections` | Layer collection metadata is missing or duplicated | Manual |
| `BBT030` | `layer-pattern` | A layer collection lacks a non-empty `BBFILE_PATTERN_*` | Manual |
| `BBT031` | `layer-priority` | A layer collection lacks an integer `BBFILE_PRIORITY_*` | Manual |
| `BBT032` | `layer-depends` | `LAYERDEPENDS_*` names an unknown collection | Manual |
| `BBT033` | `layer-series-compat` | A layer collection lacks `LAYERSERIES_COMPAT_*` | Manual |
| `BBT034` | `shell-syntax` | A shell function body has unmatched control-flow constructs | Manual |
| `BBT035` | `python-syntax` | An embedded Python body has malformed syntax or delimiters | Manual |
| `BBT036` | `python-indentation` | An embedded Python body has inconsistent indentation | Manual |
| `BBT037` | `unknown-override` | A static override component is missing from `OVERRIDES` | Manual |

Rules `BBT001` through `BBT018` and `BBT020` through `BBT037` are warnings;
`BBT019` adopts BitBake's
severity for each semantic diagnostic. Diagnostics are sorted by source location and
exposed through the public `lint`, `lint_rules`, `LintDiagnostic`, `LintFix`,
`LintRule`, and `LintSeverity` Rust APIs. `apply_lint_fixes` validates all
proposed ranges and rejects overlapping edits atomically. Structurally
incomplete input is reported as an operational error instead of producing
potentially misleading findings.

Static semantic rules are intentionally conservative: they inspect top-level
metadata and embedded body syntax, and avoid evaluating dynamic
values or class names. Recipe metadata rules `BBT011` through `BBT013` run only
when a `.bb` file belongs to a complete indexed layer; isolated files and
standard input do not receive those path-dependent findings. Recipe QA rules
`BBT020` through `BBT028` run on `.bb` files and validate static filename
identity, license/source checksums, `PACKAGECONFIG` definitions, package
declarations/scopes, and `SRC_URI` parameters. When `lint` receives a complete
layer directory, it indexes the supplied metadata into a static dependency
graph. `lint --workspace BUILD_DIR` instead loads every configured layer from
`conf/bblayers.conf` through BitBake, then includes the build's `conf`
metadata and every file reported by BitBake's `BBINCLUDED` environments in the
same graph.
The graph follows
the effective target of `include`, `require`, `inherit`, and `inherit_defer`,
and every target of `include_all`; it reports cycles but skips dynamic and
unresolved optional references. The offline workspace model reads
`BBFILE_COLLECTIONS`, `BBFILE_PATTERN_*`, `BBFILE_PRIORITY_*`, and static
`BBPATH` entries from supplied `conf/layer.conf` files. The CLI's
BitBake-backed workspace mode instead uses BitBake's expanded values, so
dynamic layer and search-path expressions are resolved by the engine. A
relative `include` or `require` first
checks the directory containing the current file and then searches `BBPATH`;
`include_all` searches only `BBPATH` and retains every match. Inherited classes
use BitBake's context-specific namespaces: recipe parsing searches
`classes-recipe` before ordinary `classes`, while configuration parsing searches
`classes-global` before `classes`. Static `INHERIT` and `USER_CLASSES`
configuration assignments and inheritance inside global classes participate in
the workspace dependency graph. Shared includes and ordinary classes retain
both possible parse contexts. `include` and `include_all` are optional
directives, so missing matches do not produce unresolved-reference diagnostics;
`require` remains strict. Same-priority matches in the winning search scope
produce the ambiguity rules above instead of being silently resolved. Ambiguity
and cycle diagnostics identify the selected target's layer, collection,
priority, and search scope. The public `WorkspaceCandidate`,
`WorkspaceClassContext`, and `WorkspaceDependency` APIs expose the corresponding
resolution and graph information. Single-file and standard-input linting remain
file-local, and dynamic references are skipped. Whole-build mode reports
BitBake resolution failures rather than analyzing only a partial workspace.
The public `WorkspaceIndex::from_build_dir` API remains available for callers
that explicitly need the offline static model; the CLI workspace mode uses
`WorkspaceIndex::from_bitbake`.

Layer QA rules `BBT029` through `BBT033` validate static collection names,
patterns, priorities, collection dependencies, and series compatibility in
complete `conf/layer.conf` files. Dynamic collection declarations and values
remain outside the static boundary and are left for BitBake-backed analysis.

Body rules `BBT034` through `BBT036` analyze shell function control-flow and
Python delimiters, compound-statement colons, multiline strings, and
indentation. They are deliberately conservative: command expansion, external
tools, shell semantics, Python imports, and runtime behavior are not evaluated.
The formatter still treats all body text as immutable opaque payload.

Override semantics are modeled statically when `OVERRIDES` and the relevant
assignments are literal. Modern colon keys and legacy underscore keys are
normalized, key expansion is applied when its referenced value is known, and
`append`, `prepend`, and `remove` operations follow active override precedence.
The public `OverrideKey`, `OverrideResolution`, `parse_override_key`, and
`resolve_overrides` APIs expose this model. Dynamic expansion remains delegated
to BitBake.

When `lint --semantic` is selected, BitBake becomes the authoritative semantic
boundary. Its parse diagnostics are emitted as `BBT019`; target environment
queries additionally validate resolved recipe identity/version, license and
source checksums, `PACKAGECONFIG`, package declarations/scopes, `SRC_URI`
parameters, effective layer collection metadata (`BBT020` through `BBT033`),
and resolved override components (`BBT037`). This does not replace the normal
`semantic` command, which remains the query-focused API for inspecting
arbitrary expanded variables and complete BitBake reports.

## Lossless syntax tree

The Rust library exposes `parse` as the shared structural front end for
formatting and linting. Its `SyntaxTree` borrows the original source and divides
it into ordered, contiguous `SyntaxNode` ranges. Concatenating `node.text()` for
every node always reproduces the input byte-for-byte.

Recognized nodes provide structured data and absolute source ranges for
assignments, directives, shell and Python functions, and top-level Python
definitions. Blank lines, comments, and unsupported top-level constructs remain
explicit nodes. Function and Python-definition nodes expose body ranges for
analysis. Body text remains immutable and byte-for-byte lossless even when
`lint` performs its conservative embedded-language checks.

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

Legacy underscore overrides are interpreted when their suffix is unambiguous
against the active `OVERRIDES` list; ambiguous or dynamic forms remain
lossless and are left to BitBake.
The formatter remains deliberately conservative: outside the opt-in static-list
layout above, it does not wrap values, reindent continuation lines, or format
embedded shell or Python code.

## BitBake version conformance

The [beta support contract](docs/beta-support-contract.md) is the authoritative
definition of supported versions, guarantees, limitations, and release
evidence. The summary below describes the current support matrix.

bbtidy continuously tests the currently supported Yocto Project release lines.
Support means that a pinned corpus from the listed release is formatted
losslessly and idempotently, structural CST coverage stays within its checked-in
thresholds, and both the original and formatted layers pass a real
`bitbake --parse-only core-image-minimal` run. Supported manifests also define
semantic probes: selected `bitbake -e` variables must match before and after
formatting after corpus-local paths are normalized.

| Support tier | Yocto release | BitBake | CI policy |
| --- | --- | --- | --- |
| Supported | 5.0 LTS (scarthgap) | 2.8 | Required on relevant changes |
| Supported | 6.0 LTS (wrynose) | 2.18 | Required on relevant changes |
| Development | master | master | Scheduled and manually triggered; non-blocking |

The pinned manifests live in `tests/upstream-corpora/`. Updating a supported
snapshot is an explicit compatibility change: update its full commit revisions,
review any CST metric movement, and require the complete differential parse
check to pass. The moving `master` corpus follows upstream branch heads to
surface upcoming syntax changes without changing the release gate.

The development-tier `community-master.json` manifest adds pinned samples from
the Arm platform/toolchain, TI BSP, and virtualization layer families. It runs
the formatter, lint, preservation, and CST checks on every listed metadata file
but skips the BitBake parse until the complete dependency stacks for those
layers are pinned. Its source and formatted CST metrics are recorded in
`tests/upstream-corpora/baselines/community-master.json` and must remain stable
unless an explicit compatibility review approves the change.

Run a stable corpus check locally after building a release binary:

```bash
cargo build --locked --release
python3 scripts/check_upstream_corpus.py \
  --manifest tests/upstream-corpora/yocto-5.0-scarthgap.json \
  --workspace compatibility-workspace \
  --evidence-dir compatibility-evidence
```

Use `--skip-bitbake` for formatter, lint, preservation, and CST checks on hosts
that cannot run BitBake. This does not satisfy the supported-release CI gate;
the evidence bundle records BitBake parsing and semantic probes as skipped.

The evidence directory contains the copied manifest, source and formatted CST
metrics, every verification command with its log, and `summary.json` with the
resolved repository commits, bbtidy revision, runner details, tree changes,
preservation counts, parse result, and semantic probe values. A pre-existing
evidence directory is rejected so an artifact cannot silently mix results from
multiple runs.

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
platforms, mismatched embedded package metadata, unsafe ZIP/TAR members,
archive links and non-regular files, missing wheel metadata files, and an
incomplete source-distribution set before publication. Release metadata also
requires unique, safe identifiers for every wheel and binary matrix entry.
Both publication workflows run the Python packaging tests, workflow-pin
validation, and Cargo package inspection before publication.

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
layers, plus the development-tier community sample described above. The
revisions and minimum corpus sizes are recorded in
`tests/upstream-corpora/`; upstream repositories are downloaded into a
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

To run the pinned community sample locally, use an existing directory
containing the four repositories named in its manifest:

```bash
python3 scripts/check_upstream_corpus.py \
    --manifest tests/upstream-corpora/community-master.json \
    --source-root /path/containing/poky-meta-arm-meta-ti-meta-virtualization \
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
   `v0.1.0-alpha.4`.
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
