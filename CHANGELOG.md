# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- A representative, interconnected fixture layer covering `.bb`, `.bbappend`,
  `.bbclass`, `.inc`, and `.conf` metadata.
- Corpus-wide golden, idempotence, and byte-for-byte opaque-region tests.
- Malformed-input integration tests that verify structured errors and ensure
  the CLI never partially rewrites a file.
- An opt-in real-BitBake parse check for the formatted fixture layer.

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
