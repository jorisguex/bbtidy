# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Fixed

- Workspace class resolution now distinguishes BitBake's global and recipe
  parsing contexts, searches `classes-global` or `classes-recipe` before the
  shared `classes` fallback, and avoids false unresolved-inherit findings for
  global classes.

### Added

- Static `INHERIT` and `USER_CLASSES` configuration assignments now contribute
  context-aware class dependencies and cycle analysis to complete workspace
  indexes.
- A beta support contract defining supported Yocto and BitBake versions,
  formatter and linter guarantees, unsupported boundaries, compatibility
  evidence, and the policy for compatibility changes.
- A development-tier compatibility corpus with pinned Arm, TI, and
  virtualization layer samples, focused syntax-boundary fixtures, and checked-in
  CST baseline metrics.
- A differential compatibility verification harness that checks the complete
  disposable repository tree, idempotence, BitBake semantic probes, and emits
  machine-readable evidence bundles with command logs.
- Transactional repository-wide writes with staged recovery copies,
  concurrent-change detection, symbolic-link refusal, and configurable file and
  source-size safety limits.

## [0.1.0-alpha.4] - 2026-08-03

### Fixed

- Release artifact verification now accepts composite manylinux compatibility
  tags and the Intel macOS wheel platform produced by Maturin, allowing the
  complete PyPI and GitHub Release publication flow to run.

## [0.1.0-alpha.3] - 2026-08-03

### Changed

- Development and packaging examples now use the portable `python3` command.
- The formatter now normalizes assignment-operator spacing on continued
  assignments while preserving every byte of the continuation tail.
- Top-level directives now use one separator between the keyword and their
  arguments without rewriting argument or comment contents.
- Completed continued assignments with an unclosed quote now fail structural
  validation before any file can be rewritten.

### Added

- An opt-in, conservative one-item-per-line formatter layout for static
  continued `SRC_URI`, `DEPENDS`, and `RDEPENDS` values, configured through
  `metadata_list_layout = "one-per-line"`.
- A dedicated security workflow that validates immutable GitHub Action pins,
  lints workflows, and audits both locked Rust dependency graphs.
- Dependabot coverage for root and fuzz Cargo dependencies plus GitHub Actions.
- A repository-owned workflow pin validator with regression tests.
- A conservative static workspace dependency graph for `include`,
  `include_all`, `require`, `inherit`, and `inherit_defer`, with a `BBT010`
  cycle diagnostic and resolution explanations for ambiguous providers.
- Shell-function boundary detection for quoted, tab-stripping, and multiple
  pending here-documents, including protection against braces in their bodies.
- Compatibility fixtures covering shell arithmetic, function modifiers,
  anonymous Python functions, multiline directives, unusual quoting, and
  legacy underscore overrides.
- A typed `.bbtidy.toml` configuration layer with formatter options, lint rule
  selection, severity overrides, and path exclusions.
- A conservative layer-aware workspace index for complete supplied layers,
  including static unresolved `require` and `inherit` diagnostics.
- In-memory workspace lookup fast paths and a dependency-free benchmark harness
  covering indexing, formatting, and batch semantic linting at layer scale.
- CI quality gates for formatting, clippy, all-target tests, benchmark health,
  repository whitespace, and wheel/source-distribution smoke coverage.
- A PyPI release-matrix entry for Linux ARMv7 hard-float
  (`manylinux_2_17_armv7l`).
- GitHub Release assets containing standalone binaries for every Python-wheel
  platform, extracted from and validated against the release wheel set.
- A tag-gated crates.io Trusted Publishing workflow with validation-only manual
  runs.
- Immutable commit pins for the release-critical GitHub Actions.
- Linux release binaries are smoke-tested under matching glibc and musl
  containers, and GitHub Releases include a `SHA256SUMS` manifest.
- Workspace-aware linting now respects `BBFILE_PRIORITY_*` and reports
  same-priority `require` and `inherit` ambiguities.
- Workspace resolution now models static `BBPATH`, layer collections and
  patterns, BitBake class search scopes, and `include_all` multi-match lookup.
- Linting now supports versioned JSON and SARIF output, with complete rule
  metadata and source locations for CI integrations.
- Added parser conformance/property tests and a cargo-fuzz target covering
  losslessness, parseability, idempotence, and lint robustness.
- Consolidated the release wheel, binary, and container verification matrix in
  `release-metadata.json`; publication now verifies embedded distribution
  metadata and exact standalone-binary versions before registry upload.

## [0.1.0-alpha.2] - 2026-08-02

### Added

- A representative, interconnected fixture layer covering `.bb`, `.bbappend`,
  `.bbclass`, `.inc`, and `.conf` metadata.
- Corpus-wide golden, idempotence, and byte-for-byte opaque-region tests.
- Malformed-input integration tests that verify structured errors and ensure
  the CLI never partially rewrites a file.
- An opt-in real-BitBake parse check for the formatted fixture layer.
- Explicit `format`, `check`, and `lex` commands with stable exit-code
  semantics.
- Standard-input formatting and lexing through the `-` input.
- Unified diff output through `format --diff`.
- Deterministic recursive discovery of `.bb`, `.bbappend`, `.bbclass`, `.conf`,
  and `.inc` files.
- CLI integration tests covering every mode, recursive filtering, input
  ordering, and batch failure safety.
- A reusable lint API with stable rule metadata, severities, source locations,
  and deterministic diagnostics.
- A `lint` command supporting files, recursive directories, and standard input.
- Initial rules for trailing whitespace, missing final newlines, long literal
  summaries, `${AUTOREV}` source revisions, and duplicate static inherits.
- Lint unit and integration coverage, including a clean representative layer
  and malformed-input behavior.
- Complete PyPI metadata and maturin binary-wheel configuration so installing
  `bbtidy` places the native executable on the environment path.
- Python distribution smoke tests that install built wheels and source
  distributions in isolated environments and execute `bbtidy --version`.
- CI packaging validation for pull requests and pushes to `main`.
- A tag-gated PyPI Trusted Publishing workflow for manylinux, musllinux, macOS,
  and Windows wheels plus a source distribution.
- Cargo, PEP 440, and release-tag version consistency checks.
- Unit coverage for packaging version conversion and artifact selection.
- A commit-pinned compatibility corpus covering more than 3,300
  OpenEmbedded-Core, `meta-oe`, `meta-python`, and `meta-networking` files.
- Automated upstream formatting, idempotence, lint, opaque-region preservation,
  payload protection, and real BitBake parse checks.
- Weekly and change-triggered upstream compatibility CI.
- Support for triple-quoted strings and combined `fakeroot python` modifiers
  when preserving embedded Python functions.
- A public, lossless top-level concrete syntax tree with contiguous byte ranges
  and structured assignment, directive, function, and Python-definition nodes.
- Parse-once `format_syntax` and `lint_syntax` APIs for reusing a syntax tree.

### Changed

- Formatting now writes to standard output by default and only modifies files
  when `format --write` is explicitly selected.
- Batch writes validate every input before modifying the first file.
- Replaced the original positional lexer and `--format` interface with
  subcommands.
- Exit code `1` now also represents lint findings; lint analysis failures use
  exit code `2`.
- Line and column reporting now counts Unicode characters rather than UTF-8
  bytes.
- Directory discovery now skips recipe payload directories and only discovers
  `.conf` files within an identifiable layer configuration tree.
- Formatting and linting now share the concrete syntax tree as their structural
  representation; the duplicate lint-side statement scanner has been removed.

## [0.1.0-alpha.1] - 2026-08-01

### Added

- Conservative formatting boundaries that preserve embedded shell and Python
  code byte-for-byte.
- Atomic file replacement and fail-safe handling for structurally incomplete
  input.
- Tokens for modern BitBake directives, path separators, and override
  separators.
- A typed `AssignmentOperator` API exposing the semantics and lexeme of every
  BitBake assignment operator.
- Golden metadata fixtures covering the supported alpha grammar.

### Changed

- Corrected assignment token semantics:
  - `:=` is now `ImmediateAssign`.
  - `?=` is now `DefaultAssign`.
  - `??=` is now `WeakDefaultAssign`.
  - `.=` is now `AppendNoSpaceAssign`.
- Added the previously missing `=+` and `=.` operators.
- Expanded tokenization to represent hyphens, periods, and multiple literal or
  dynamic override components.
- Updated `format` to return `Result<String, FormatError>`.

### Known limitations

- Formatting is limited to complete, single-line top-level assignments.
- Continued assignments and embedded functions are preserved rather than
  reformatted.
- Layer-aware semantic checks only resolve files and classes supplied in the
  indexed input set; external BitBake classes and dynamic references are not
  evaluated.
- Legacy underscore overrides are tokenized but not interpreted.
