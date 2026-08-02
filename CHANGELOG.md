# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Changed

- The formatter now normalizes assignment-operator spacing on continued
  assignments while preserving every byte of the continuation tail.
- Top-level directives now use one separator between the keyword and their
  arguments without rewriting argument or comment contents.
- Completed continued assignments with an unclosed quote now fail structural
  validation before any file can be rewritten.

### Added

- Shell-function boundary detection for quoted, tab-stripping, and multiple
  pending here-documents, including protection against braces in their bodies.
- Compatibility fixtures covering shell arithmetic, function modifiers,
  anonymous Python functions, multiline directives, unusual quoting, and
  legacy underscore overrides.
- A typed `.bbtidy.toml` configuration layer with formatter options, lint rule
  selection, severity overrides, and path exclusions.
- A conservative layer-aware workspace index for complete supplied layers,
  including static unresolved `require` and `inherit` diagnostics.

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
- The project does not yet provide lint rules or semantic BitBake validation.
- Legacy underscore overrides are tokenized but not interpreted.
