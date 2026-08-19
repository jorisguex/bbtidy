# bbtidy lint reference

`bbtidy check` reports deterministic diagnostics with stable rule IDs,
severity, source ranges, help, suppression state, baseline state, and safe fix
metadata. Ordinary file or directory checking is offline; the
[BitBake integration reference](bitbake-integration.md) describes authoritative
workspace and target-specific modes.

## Profiles and failure policy

Always select `--profile recommended` in adoption and CI commands until pilot
evidence supports changing the alpha-compatible built-in `all` default.

| Profile | Intended use | Rules omitted |
| --- | --- | --- |
| `essential` | Highest-confidence syntax, resolution, and authoritative findings | `BBT003`, `BBT011`, `BBT012`, `BBT013`, `BBT016`, `BBT020`, `BBT021` |
| `recommended` | Default adoption policy | `BBT003`, `BBT011`, `BBT012`, `BBT013`, `BBT016`, `BBT021` |
| `strict` | Complete rule catalog | None |
| `all` | Alpha compatibility profile | None |

`--enable RULE` adds a rule and `--disable RULE` removes one. Configuration
severity overrides apply after selection. `--fail-on info|warning|error|never`
controls the minimum effective severity that returns exit code `1`; findings
remain visible regardless of that threshold.

Rules other than `BBT019` and `BBT038` are warnings by default. `BBT019`
retains the severity reported by BitBake. `BBT038` is a non-suppressible error.

## Rule catalog

| Rule | Name | Detects | Safe fix |
| --- | --- | --- | --- |
| `BBT001` | `trailing-whitespace` | Spaces or tabs at the end of a line | Remove trailing spaces/tabs |
| `BBT002` | `final-newline` | A non-empty file without a final newline | Append a newline |
| `BBT003` | `summary-length` | A static literal `SUMMARY` longer than 80 characters | Manual |
| `BBT004` | `autorev` | `SRCREV` variants that use `${AUTOREV}` | Manual |
| `BBT005` | `duplicate-inherit` | A static class inherited more than once in one file | Manual |
| `BBT006` | `unresolved-require` | A static `require` target missing from indexed layers | Manual |
| `BBT007` | `unresolved-inherit` | A static inherited class missing from indexed layers | Manual |
| `BBT008` | `ambiguous-require` | A static `require` target has multiple highest-priority matches | Manual |
| `BBT009` | `ambiguous-inherit` | A static class has multiple highest-priority definitions | Manual |
| `BBT010` | `dependency-cycle` | A resolved include, require, or inherit closes a cycle | Manual |
| `BBT011` | `missing-summary` | A complete indexed recipe lacks `SUMMARY` | Manual |
| `BBT012` | `missing-description` | A complete indexed recipe lacks `DESCRIPTION` | Manual |
| `BBT013` | `missing-license` | A complete indexed recipe lacks `LICENSE` | Manual |
| `BBT014` | `file-paths-immediate` | `FILESEXTRAPATHS` does not use immediate `:=` expansion | Manual |
| `BBT015` | `git-uri-protocol` | A static `git://` `SRC_URI` omits its transport protocol | Manual |
| `BBT016` | `duplicate-assignment` | A variable is assigned directly more than once in one file | Manual |
| `BBT017` | `duplicate-function` | A task or function is declared more than once in one file | Manual |
| `BBT018` | `empty-directive` | A static dependency directive has no target | Manual |
| `BBT019` | `bitbake-diagnostic` | A diagnostic emitted by BitBake-backed analysis | Manual |
| `BBT020` | `recipe-name` | Explicit `PN` disagrees with the recipe filename | Manual |
| `BBT021` | `recipe-version` | Explicit `PV` disagrees with the recipe filename | Manual |
| `BBT022` | `license-checksum` | A non-`CLOSED` recipe lacks a valid `LIC_FILES_CHKSUM` | Manual |
| `BBT023` | `source-checksum` | A remote source archive lacks a valid checksum | Manual |
| `BBT024` | `packageconfig` | An enabled `PACKAGECONFIG` feature lacks a definition | Manual |
| `BBT025` | `packageconfig-format` | A `PACKAGECONFIG` definition has more than six fields | Manual |
| `BBT026` | `package-scope` | A package-scoped variable names an undeclared package | Manual |
| `BBT027` | `package-list` | `PACKAGES` declares a package more than once | Manual |
| `BBT028` | `uri-parameters` | A `SRC_URI` parameter is invalid or conflicts with its transport | Manual |
| `BBT029` | `layer-collections` | Layer collection metadata is missing or duplicated | Manual |
| `BBT030` | `layer-pattern` | A collection lacks a non-empty `BBFILE_PATTERN_*` | Manual |
| `BBT031` | `layer-priority` | A collection lacks an integer `BBFILE_PRIORITY_*` | Manual |
| `BBT032` | `layer-depends` | `LAYERDEPENDS_*` names an unknown collection | Manual |
| `BBT033` | `layer-series-compat` | A collection lacks `LAYERSERIES_COMPAT_*` | Manual |
| `BBT034` | `shell-syntax` | A shell body has unmatched control-flow constructs | Manual |
| `BBT035` | `python-syntax` | Embedded Python has malformed syntax or delimiters | Manual |
| `BBT036` | `python-indentation` | Embedded Python has inconsistent indentation | Manual |
| `BBT037` | `unknown-override` | A static override component is absent from `OVERRIDES` | Manual |
| `BBT038` | `suppression` | A suppression is unknown, malformed, or unused | Manual |

## Analysis scope

File-local checks operate on supplied source. When a complete layer directory
is supplied, bbtidy builds an offline workspace index for static include,
require, inherit, collection, recipe, and cycle checks. Dynamic names and
values are skipped rather than guessed.

`check --workspace BUILD_DIR` lets BitBake determine the complete file scope.
`check --semantic` additionally validates selected target environments and
converts BitBake diagnostics to `BBT019`. These modes never silently return a
partial authoritative result.

Body rules are conservative lexical checks. They do not execute shell or
Python, resolve imports, or inspect runtime behavior. The formatter continues
to preserve embedded body bytes.

## Progressive enforcement

**Observe:** collect recommended-profile findings without failing CI.

```bash
bbtidy check --profile recommended --fail-on never meta-my-layer/
```

**Baseline:** review current findings, write them explicitly, and fail only on
newly introduced findings.

```bash
bbtidy check \
  --profile recommended \
  --write-baseline .bbtidy-baseline.json \
  meta-my-layer/

bbtidy check \
  --profile recommended \
  --baseline .bbtidy-baseline.json \
  meta-my-layer/
```

**Enforce:** retain the recommended profile and fail at warning severity.

```bash
bbtidy check --profile recommended --fail-on warning meta-my-layer/
```

## Baselines

A baseline stores normalized identities for reviewed existing findings. A
matching finding remains visible but does not block the command; new findings
and operational errors still do. The file records the selected profile and
catalog fingerprint so an incompatible policy is rejected rather than applied
silently.

`--show-existing` includes matching existing findings in the blocking set.
`--refresh-baseline PATH` explicitly replaces a baseline after comparing the
current run. Refresh is never implicit, and stale entries are reported.

Baseline files reject absolute or escaping paths, duplicate identities,
unsupported schemas, excessive size, and mismatched profiles or catalogs.

## Inline suppressions

Suppressions must name a known rule and include a reason:

```bitbake
# bbtidy: ignore-next-line[BBT004] -- development recipe intentionally floats
SRCREV = "${AUTOREV}"

SRC_URI = "git://example.invalid/repo;protocol=https" # bbtidy: ignore[BBT015] -- mirror policy
```

A file-wide suppression is valid only on the first line:

```bitbake
# bbtidy: disable-file[BBT012] -- generated recipe metadata
```

Unknown, malformed, reasonless, and unused suppressions emit `BBT038` and
cannot suppress that error. `--show-suppressed` retains matched findings in
machine output together with suppression counts.

## Fixes

`--show-fixes` adds help and edit details without changing files. `--fix`
applies only safe `BBT001` and `BBT002` edits, re-lints the result, and reports
remaining findings:

```bash
bbtidy check --profile recommended --show-fixes meta-my-layer/
bbtidy check --profile recommended --fix meta-my-layer/
```

Fix mode refuses standard input. It analyzes and stages the complete input set
before committing, detects concurrent changes, refuses symbolic links, and
rolls back earlier replacements if a later commit step fails.

## Output formats and exit codes

`--output text|json|sarif` selects the report. Text is the default. JSON output
supports schema versions 1 and 2 through `--output-version`; SARIF follows
SARIF 2.1.0 and includes the complete catalog, locations, properties, and safe
fix metadata.

- `0`: analysis completed and no enabled finding met the failure threshold.
- `1`: an enabled finding met the threshold.
- `2`: usage, discovery, parsing, analysis, baseline, fix, or output failed.

Machine-readable output is emitted only after the complete input set has been
analyzed successfully, so consumers never receive a partial report presented
as complete.
