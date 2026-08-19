# bbtidy configuration reference

bbtidy reads project policy from `.bbtidy.toml`. The file is optional; every
setting has a built-in default.

## Configuration discovery and precedence

Without a global configuration option, bbtidy searches the current directory
and each parent directory for the nearest `.bbtidy.toml`.

- `--config PATH` selects one file explicitly.
- `--no-config` disables discovery and uses built-in defaults.
- `--config` and `--no-config` cannot be combined.
- One-run CLI options override scalar configuration values.
- CLI `--enable` and `--disable` selections extend configured rule selections;
  contradictory selections are rejected.
- `--baseline PATH` takes precedence over `lint.baseline`.

Relative project paths are resolved from the directory containing the
configuration file. A bare semantic command name such as `bitbake` remains a
`PATH` lookup; a relative command containing a directory component is resolved
from the configuration directory.

Unknown sections, fields, rule IDs, enum values, and malformed globs are
operational errors. Numeric safety and BitBake limits must be positive, and the
total BitBake timeout must be at least the per-command timeout.

## Complete schema

This example lists every supported key and its default where one exists:

```toml
[format]
max_top_level_blank_lines = 1
metadata_list_layout = "preserve" # "preserve" or "one-per-line"

[semantic]
build_dir = "build" # optional; omit to use environment/ancestor discovery
bitbake = "bitbake" # optional command name or executable path
full = false
graph = false
dry_run = false
inventory = false
packages = false

[lint]
profile = "all" # "essential", "recommended", "strict", or "all"
enable = []
disable = []
fail_on = "warning" # "info", "warning", "error", or "never"
baseline = ".bbtidy-baseline.json" # optional

[lint.severity]
# BBT004 = "info"
# BBT019 = "error"

[paths]
exclude = []

[safety]
max_files = 10000
max_bytes = 268435456

[bitbake]
command_timeout_seconds = 1800
total_timeout_seconds = 7200
max_stdout_bytes = 268435456
max_stderr_bytes = 16777216
max_commands = 20000
max_recipe_queries = 10000
```

The built-in lint profile remains `all` for alpha compatibility. Adoption
documentation specifies `recommended` explicitly until pilot evidence supports
a default change.

## `[format]`

`max_top_level_blank_lines` controls the maximum consecutive blank lines
between top-level syntax nodes. It does not alter blank lines inside embedded
functions.

`metadata_list_layout = "preserve"` keeps continuation tails unchanged.
`"one-per-line"` lays out safely recognized static whitespace lists in common
metadata variables and directives. Dynamic expansions, comments, escaped or
quoted items, mixed line endings, and unknown variables remain unchanged apart
from ordinary assignment spacing.

## `[semantic]`

`build_dir` supplies the default initialized BitBake build directory.
`bitbake` selects the executable used by `check --workspace`,
`check --semantic`, and `semantic`.

The analysis booleans select optional standalone or semantic-lint report
sections:

- `full` enables every section below.
- `graph` collects task, recipe, and package dependency graphs.
- `dry_run` asks the scheduler for a non-executing task plan.
- `inventory` collects recipe versions and providers.
- `packages` collects resolved package, dependency, and image metadata.

Command-line `--full`, `--graph`, `--dry-run`, `--inventory`, and `--packages`
enable the corresponding section for one run. They do not disable sections
enabled in configuration.

## `[lint]` and `[lint.severity]`

`profile` selects the base catalog. `enable` adds stable rule IDs and `disable`
removes them after profile selection. A rule cannot be both enabled and
disabled. Command-line profile selection replaces the configured profile;
command-line enable and disable lists are then applied.

`fail_on` sets the lowest effective severity that returns exit code `1`.
`never` makes ordinary findings advisory. Operational errors and the
non-suppressible `BBT038` suppression rule remain failures.

`baseline` stores the default path for incremental adoption. It is resolved
relative to the configuration file. Baselines are never created or refreshed
implicitly.

Each entry in `[lint.severity]` maps a stable rule ID to `info`, `warning`, or
`error`. Severity overrides are applied after profile, enable, and disable
selection. See the [lint reference](lint-rules.md) for the catalog and baseline
workflow.

## `[paths]`

`exclude` is a list of glob patterns relative to the configuration directory.
Patterns apply to explicit files, recursive discovery, and BitBake workspace
files. Standard input is never excluded. Workspace exclusions are applied
before class/include resolution and recipe-specific environment discovery, so
an excluded dependency is not silently retained in the workspace index.

Directory discovery processes `.bb`, `.bbappend`, `.bbclass`, and `.inc`
files. It processes `.conf` files inside a layer's `conf` tree and skips recipe
payload directories named `files`. An explicitly supplied regular file is
processed unless excluded.

## `[safety]`

`max_files` and `max_bytes` bound the discovered source set for `format` and
`check`, including workspace mode. `max_bytes` counts original source bytes.
The matching `--max-files` and `--max-bytes` options override these limits for
one invocation.

These values are repository safety limits, not performance targets. A limit
failure occurs before a write is committed.

## `[bitbake]`

The BitBake limits apply only to `check --workspace`, `check --semantic`, and
`semantic`:

- `command_timeout_seconds`: deadline for one BitBake invocation.
- `total_timeout_seconds`: deadline shared by the complete operation.
- `max_stdout_bytes`: captured stdout cap for one invocation.
- `max_stderr_bytes`: captured stderr cap for one invocation.
- `max_commands`: process-launch budget for the operation.
- `max_recipe_queries`: recipe-specific environment-query budget.

Each key has a matching one-run CLI override named
`--bitbake-command-timeout-seconds`, `--bitbake-total-timeout-seconds`,
`--bitbake-max-stdout-bytes`, `--bitbake-max-stderr-bytes`,
`--bitbake-max-commands`, or `--bitbake-max-recipe-queries`.

Timeout, cancellation, output overflow, command-budget exhaustion, and query
budget exhaustion terminate the bounded operation and prevent partial
machine-readable output. See [BitBake integration](bitbake-integration.md).
