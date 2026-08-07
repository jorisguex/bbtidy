# bbtidy beta support contract

This document defines what bbtidy beta releases support, what they deliberately
do not support, and the evidence required before a release can claim support.
It is the authoritative compatibility and safety contract for the beta series.

## Contract summary

For the supported Yocto and BitBake versions listed below, bbtidy promises that
its supported formatting operations are:

- lossless at the source-file boundary, except for documented formatting
  changes;
- idempotent, so formatting an already formatted file produces no further
  changes;
- parse-safe, because both the original and formatted supported corpora pass a
  real BitBake parse check;
- deterministic, with stable file ordering, diagnostics, and machine-readable
  output; and
- fail-safe, refusing incomplete input, bounding repository-wide work, and
  avoiding partial batch writes.

These guarantees apply to bbtidy's formatting and static analysis behavior.
They do not prove that a complete BitBake build produces identical task hashes,
packages, runtime behavior, or performance. Users must continue to run their
normal BitBake build and test validation.

## Support tiers

### Supported

The beta release gate initially covers:

| Yocto Project release | BitBake version | Status |
| --- | --- | --- |
| 5.0 LTS (Scarthgap) | 2.8 | Supported |
| 6.0 LTS (Wrynose) | 2.18 | Supported |

Supported means that the corresponding commit-pinned compatibility manifest
passes the complete release gate described in [Evidence required for a
release](#evidence-required-for-a-release). A supported release remains in the
beta contract until it is explicitly deprecated in a release note.

### Development

Yocto Project and BitBake `master` are development targets. They are tested on
the scheduled compatibility workflow when practical, but their results are
non-blocking. New syntax found there may be unsupported until a focused
regression test and a supported-release decision are made.

### Unsupported

The beta contract does not promise compatibility for:

- BitBake versions older than 2.8;
- vendor-modified or otherwise unpinned BitBake forks;
- dynamic file and class references that bbtidy cannot resolve statically;
- external classes or files not included in the indexed input set;
- the semantics of embedded shell or Python code;
- legacy underscore overrides as semantic override operations; or
- generated build output unless it is explicitly supplied as an input.

Unsupported input is preserved where possible. Preservation does not mean that
the input was fully understood or semantically validated.

## Command guarantees

### `format`

The formatter may normalize top-level assignment spacing, directive spacing,
configured top-level blank-line runs, and the explicitly opt-in static metadata
list layout. It does not interpret or rewrite embedded shell or Python bodies.

By default, bbtidy preserves continuation tails, comments, argument contents,
unknown top-level syntax, and embedded code byte-for-byte. Structurally
incomplete input is an operational error. In `--write` mode, all inputs must be
read, formatted, staged, and checked for concurrent changes before any file is
replaced. The complete write set is committed transactionally with recovery
copies; symbolic links are never replaced. Repository-wide formatting is also
bounded by configurable file-count and source-byte limits.

### `check`

- Exit code `0` means all selected inputs already match bbtidy's configured
  formatting.
- Exit code `1` means at least one selected input would change.
- Exit code `2` means input discovery, parsing, formatting, or output failed.

### `lint`

Linting reports findings only within the selected analysis scope. File-local
linting does not claim to resolve a complete layer. Layer-aware linting only
uses files supplied in the indexed input set and only resolves static
references.

Dynamic values, dynamic class names, unavailable external providers, and
embedded shell or Python behavior are not evaluated. A clean lint result means
that no enabled rule found a diagnostic in the analyzed scope; it does not mean
that the entire BitBake build is free of issues.

Text diagnostics, versioned JSON, and SARIF output must describe the same
findings in deterministic source order.

### `lex`

Lexing reports token spans against the original source. Lexer errors remain
visible in the token stream and cause exit code `2`. Lexing never modifies
files.

## Source and scope boundaries

Recursive discovery processes supported BitBake metadata extensions according
to the CLI discovery rules. Recipe payload directories named `files` are not
treated as metadata trees. An explicit file input is always processed, even if
it would not be discovered recursively.

The workspace model is intentionally conservative. It understands the static
layer metadata and search behavior documented by bbtidy, but it does not
execute BitBake variable expansion or Python expressions. Users requiring full
BitBake semantics must run BitBake itself.

## Evidence required for a release

Every supported beta release must have evidence for both the original and
formatted versions of each supported manifest:

1. The pinned repository commits, layer paths, and expected file counts are
   verified.
2. The original corpus passes the configured BitBake parse-only target.
3. A disposable copy is formatted without changing the source checkout.
4. Formatted output remains valid UTF-8 and all expected files remain present.
5. Recipe payload files and excluded files are byte-for-byte unchanged.
6. Formatting is idempotent across the complete corpus.
7. The formatted corpus passes the same BitBake parse-only target.
8. Structural coverage does not regress beyond the approved manifest
   thresholds.
9. The complete copied repository tree is unchanged except for the discovered
   metadata files, and the formatter's reported change count matches the tree
   diff.
10. Where configured, selected `bitbake -e` semantic probe values are equal
    before and after formatting after temporary corpus paths are normalized.
11. Diagnostics and machine-readable reports are deterministic.
12. No operational error causes a partial batch rewrite.
13. Repository-wide runs respect the configured file-count and source-byte
    limits, and write runs refuse symbolic links and concurrent source changes.
14. Release artifacts match the checked-in release manifest; archives contain
    only safe regular-file members, required package metadata, and the expected
    versioned executable; and both publication workflows pass their packaging
    and workflow-validation gates before publication is enabled.

The release record must identify the bbtidy version, source commit, corpus
commits, BitBake version, runner environment, command lines, result summaries,
and links to the original and formatted parse logs. The machine-readable
evidence bundle must contain `manifest.json`, `summary.json`, `commands.json`,
`metrics/source.json`, `metrics/formatted.json`, and the command logs.

Parseability is a required compatibility signal, not a complete semantic
equivalence proof. Release notes must not describe the parse gate as proving
identical build outputs.

## Compatibility change policy

Any change that can affect parsing, formatting, discovery, lint resolution, or
diagnostic output must include:

- a focused regression fixture;
- an explanation of the affected contract boundary;
- a run of all relevant Rust and Python tests;
- the supported compatibility corpus gate; and
- a changelog entry when user-visible behavior changes.

Updating a supported corpus is an explicit compatibility change. The update
must record the new repository commits, review file-count and structural-metric
movement, and pass the complete differential parse gate before it is merged.

Adding a new supported Yocto release requires a new pinned manifest, a
successful full release gate, and a documented support decision. Removing a
supported release requires an explicit deprecation notice before removal.

## Reporting a compatibility issue

Users reporting a beta issue should include:

- bbtidy version and installation method;
- Yocto and BitBake versions;
- affected layer and repository commits;
- the exact bbtidy command and configuration file;
- whether the issue occurred during formatting, checking, linting, or lexing;
- the original and formatted metadata when it can be shared safely; and
- the BitBake parse or build error, including the first relevant diagnostic.

Security-sensitive reports should use the repository's security reporting
channel once one is published. Do not include proprietary metadata in public
issues.
