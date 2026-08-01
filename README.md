# bbtidy

Formatter and Linter for BitBake.

## Description

`bbtidy` is an experimental tool for formatting and linting BitBake recipes and
configuration files. It currently provides a lexer and a conservative formatter
for top-level metadata assignments.

## Features

- **Tokenization**: efficiently breaks down BitBake files into tokens (Identifiers, Strings, Keywords, etc.) using `logos`.
- **Span Reporting**: Reports the exact location (line and column) of each token.
- **Safe formatting boundaries**: Formats complete, single-line top-level
  assignments while preserving shell functions, Python functions, continued
  statements, and unsupported syntax.
- **Fail-safe writes**: Refuses to rewrite structurally incomplete input and
  replaces successfully formatted files atomically.

## Usage

To inspect the tokens in a recipe:

```bash
cargo run -- sample.bb
```

To format one or more files in place:

```bash
cargo run -- --format messy.bb
```

Formatting is intentionally limited while BitBake syntax support is being
developed. Embedded shell and Python code is kept byte-for-byte unchanged.

## Development

Run the test suite with:

```bash
cargo test
```

## License

This project is licensed under the MIT License.
