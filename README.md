# bbtidy

Experimental formatter and lexer for BitBake metadata.

## Description

`bbtidy` is an experimental tool for formatting and inspecting BitBake recipes
and configuration files. It provides a lexer, a conservative formatter for
top-level metadata assignments, and a formatting check suitable for CI.

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
- **Automation-friendly CLI**: Provides explicit `format`, `check`, and `lex`
  commands, standard-input support, unified diffs, and documented exit codes.
- **Layer-wide operation**: Recursively discovers supported BitBake files in
  deterministic path order.

## Usage

To inspect the tokens in a recipe:

```bash
cargo run -- lex sample.bb
```

To print a formatted file without modifying it:

```bash
cargo run -- format messy.bb
```

Standard input is accepted as `-`:

```bash
printf 'SUMMARY="Example"\n' | cargo run -- format -
```

To inspect changes across a file or directory:

```bash
cargo run -- format --diff recipes-example/
```

To check formatting in CI, then explicitly rewrite files when desired:

```bash
cargo run -- check recipes-example/
cargo run -- format --write recipes-example/
```

Formatting is intentionally limited while BitBake syntax support is being
developed. Embedded shell and Python code is kept byte-for-byte unchanged.

Directory inputs are traversed recursively. Only `.bb`, `.bbappend`, `.bbclass`,
`.conf`, and `.inc` files discovered inside directories are processed; an
explicit file input is always processed. Paths are sorted and deduplicated
before processing. Standard input must be the only input, and `--write` cannot
be used with it.

`format` writes formatted source to standard output and requires one input
unless `--diff` or `--write` is selected. `format --diff` and `lex` can process
multiple inputs without changing them. Before `format --write` changes any
files, every input is read and formatted successfully; each changed file is
then replaced atomically.

### Exit codes

- `0`: the command completed successfully.
- `1`: `check` found files that would be reformatted.
- `2`: command usage, input/output, lexing, or formatting failed.

Operational diagnostics are written to standard error. Lexer error tokens
remain part of the token stream on standard output and cause exit code `2`.
`format --diff` returns `0` when it successfully reports differences; use
`check` when differences should fail a CI job.

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

The integration suite includes a representative fixture layer containing
`.bb`, `.bbappend`, `.bbclass`, `.inc`, and `.conf` files. It verifies golden
output, idempotence, byte-for-byte preservation of embedded code, structured
errors, CLI modes and exit codes, deterministic directory handling, and the
no-write guarantee for malformed input.

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

## License

This project is licensed under the MIT License.
