# bbtidy

Formatter and Linter for BitBake.

## Description

`bbtidy` is a tool designed to format and lint BitBake recipes and configuration files. Currently, it includes a lexer that tokenizes BitBake syntax, identifying elements like comments, variables, assignments, and directives.

## Features

- **Tokenization**: efficiently breaks down BitBake files into tokens (Identifiers, Strings, Keywords, etc.) using `logos`.
- **Span Reporting**: Reports the exact location (line and column) of each token.

## Usage

To run the tool on the embedded sample recipe:

```bash
cargo run
```

This will output the tokens found in the sample text, along with their types and positions.

## License

This project is licensed under the MIT License.
