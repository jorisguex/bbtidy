# bbtidy

Experimental formatter and lexer for BitBake metadata.

## Description

`bbtidy` is an experimental tool for formatting and linting BitBake recipes and
configuration files. It currently provides a lexer and a conservative formatter
for top-level metadata assignments.

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

## License

This project is licensed under the MIT License.
