# BitBake integration reference

bbtidy keeps fast offline checking separate from authoritative BitBake-backed
operations. Choose the narrowest mode that answers the question you have.

## Mode selection

| Mode | File scope | Semantic source | Invokes BitBake | Primary purpose |
| --- | --- | --- | --- | --- |
| `check PATH...` | Supplied files or directories | Conservative static model | No | Ordinary linting and CI |
| `check --workspace BUILD_DIR` | Complete configured workspace | BitBake for scope, static lint rules | Yes | Preferred production check |
| `check --semantic ... PATH...` | Supplied files or directories | Static rules plus target environments | Yes | Target-specific semantic linting |
| `semantic` | No ordinary lint input | BitBake reports | Yes | Metadata and build inspection |

Adoption starts with offline `format` and `check`. Workspace checking is the
preferred authoritative production mode. Semantic linting is an optional
target-specific overlay, while `semantic` is an advanced inspection command.

## Offline checking

```bash
bbtidy check --profile recommended meta-my-layer/
```

Directory input creates a conservative workspace index from the supplied
metadata. It can resolve static `include`, `include_all`, `require`, `inherit`,
and `inherit_defer` references, layer priority, collection metadata, and
literal override operations. Dynamic values, unavailable external layers, and
runtime behavior are skipped rather than guessed.

Use offline checking for fast local feedback and ordinary CI. It neither
starts BitBake nor updates a build directory.

## Authoritative workspace checking

```bash
bbtidy check --workspace build --profile recommended
```

Workspace mode requires an initialized build containing `conf/local.conf` and
`conf/bblayers.conf`. It cannot be combined with file or directory inputs.
The selected BitBake executable performs a complete parse and supplies expanded
`BBLAYERS`, `BBFILES`, `BBPATH`, and `BBINCLUDED` values. bbtidy then indexes
the resolved layers, build configuration, external includes, and classes as
one deterministic lint scope.

The normal strategy uses one long-lived Tinfoil helper for recipe includes.
When the adjacent BitBake Python library is unavailable, or a recipe is absent
from the global inventory, the runner uses an explicitly reported bounded
per-recipe fallback. Helper failure is an operational error; bbtidy never
silently returns a partial authoritative workspace.

Exclusions and `max_files`/`max_bytes` limits apply before the retained
workspace is analyzed. Excluded metadata is removed from dependency resolution
and recipe-specific discovery.

## Target-specific semantic linting

```bash
bbtidy check --semantic \
  --build-dir build \
  --target core-image-minimal \
  --variable IMAGE_FSTYPES \
  --profile recommended \
  meta-my-layer/
```

Semantic linting first runs BitBake's parse, then queries `bitbake -e` for each
selected target. BitBake warnings and errors become `BBT019` diagnostics.
Resolved environments also feed recipe identity/version, license and source
checksum, `PACKAGECONFIG`, package scope, `SRC_URI`, layer collection, source
revision, and override checks.

This mode requires file input and cannot read standard input. It does not
expand the file scope to the complete workspace; use `--workspace` when scope
completeness is the objective.

Optional `--graph`, `--dry-run`, `--inventory`, and `--packages` sections add
BitBake-produced build analysis. `--full` enables all four.

## Standalone semantic inspection

```bash
bbtidy semantic \
  --build-dir build \
  --target core-image-minimal \
  --variable PN \
  --variable OVERRIDES \
  --output json
```

`semantic` does not run ordinary file linting. It reports the selected BitBake
version, resolved project and build directories, build-context source, parse
status, target-query status, requested variables, diagnostics, target results,
and execution statistics. `--full` adds dependency graphs, the dry-run plan,
recipe/provider inventory, and package/image summaries.

Text is the default output. JSON is a versioned report. A completed analysis
still emits its report and exits `1` when BitBake reports parse or target
errors; invocation or configuration failures exit `2` without presenting a
partial report.

## Build-context discovery

For `semantic` and `check --semantic`, build context is selected in this order:

1. explicit `--build-dir PATH`;
2. `[semantic].build_dir` in `.bbtidy.toml`;
3. `BBTIDY_BITBAKE_BUILD_DIR`;
4. `BUILDDIR`;
5. a configured `build` or `build-*` directory in the project or its ancestors.

`--project-dir PATH` selects the discovery root. Multiple matching build
variants are rejected rather than guessed. `check --workspace BUILD_DIR`
receives its build directory directly from the workspace option.

The BitBake executable comes from `--bitbake`, then `[semantic].bitbake`, then
the `bitbake` command on `PATH`.

## Execution limits and cancellation

All BitBake-backed modes share one operation-scoped runner. It concurrently
drains stdout and stderr, enforces command and total deadlines, caps captured
output, limits process launches and recipe queries, and terminates the process
group on cancellation or overflow.

Configure the defaults in `[bitbake]` or use:

- `--bitbake-command-timeout-seconds`
- `--bitbake-total-timeout-seconds`
- `--bitbake-max-stdout-bytes`
- `--bitbake-max-stderr-bytes`
- `--bitbake-max-commands`
- `--bitbake-max-recipe-queries`

Pressing Ctrl-C cancels the active process group. A timeout, cancellation,
limit, overflow, helper failure, or parse failure cannot produce a partial
workspace reported as authoritative.

BitBake may update its ordinary parse cache or server metadata even though
these commands do not execute build tasks. Graph, inventory, environment, and
dry-run requests are analysis operations, not complete build equivalence
proofs.

Implementation strategy and measured process counts are documented in
[BitBake execution and scalability](bitbake-execution.md). Supported versions
and guarantees remain defined by the
[beta support contract](beta-support-contract.md).
