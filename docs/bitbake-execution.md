# BitBake execution and scalability

bbtidy's BitBake boundary is operation-scoped and bounded. Workspace and
semantic operations share `src/bitbake.rs`, which streams both output pipes,
enforces per-command and total deadlines, caps captured bytes, counts process
launches and recipe queries, and terminates/reaps the process group on
timeout, cancellation, or overflow. Version and parse results are cached only
inside the current CLI operation; no BitBake result is persisted between runs.

## Strategy decision

The workspace resolver was compared against the following options:

1. A long-lived Tinfoil helper. BitBake 2.8.1 and 2.18.0 both expose
   `Tinfoil.prepare`, `all_recipe_files(variants=False)`,
   `parse_recipe_file`, and `shutdown`. The helper uses those calls in one
   BitBake-backed Python process and emits only recipe paths and `BBINCLUDED`.
   Rust filters the deterministic helper inventory to the already-resolved
   `BBFILES` scope, so no unrelated environment is retained. If a BitBake
   configuration omits globally skipped recipe files from that inventory,
   only those missing files go through the explicit bounded buildfile fallback.
2. Direct BitBake server APIs. These are implementation details behind
   Tinfoil, have no stable cross-release Rust interface, and would duplicate
   the Python compatibility problem.
3. A bounded batch helper. This is the selected transport: Tinfoil owns one
   server/client session and the Rust runner bounds the helper's stdout,
   stderr, command, recipe, and total-operation budgets.
4. A parsed recipe cache. This is useful only within the helper lifetime;
   configuration and metadata changes make a cross-run cache unsafe.
5. Per-recipe `--environment --buildfile` queries. This remains an explicit
   compatibility fallback when the executable cannot locate its adjacent
   BitBake Python library (including minimal fake executables). It is
   deterministic, deduplicates recipes, skips exclusions before querying, and
   is stopped by the same command/query/time/output budgets.

The Tinfoil path is authoritative for the installed BitBake engine. Helper
failure is an operational error; it never returns a partial authoritative
workspace. A helper that is unavailable before invocation may use the bounded
fallback, and a successful helper may use that same fallback only for recipe
files absent from its authoritative inventory. The mixed strategy is reported
as `tinfoil-batch+bounded-buildfile-fallback`; the all-batch path is reported
as `tinfoil-batch`, and the helper reserves stdout exclusively for JSON lines.

The source-level compatibility inspection covered dynamic includes, inherited
classes, anonymous-Python-driven metadata, duplicate recipe providers,
variants, and the `BBINCLUDED` datastore value. A Python-backed fake integration
test exercises the wire protocol and one-process property locally. The pinned
Yocto 5.0/BitBake 2.8.1 and Yocto 6.0/BitBake 2.18.0 fixtures also completed
real parse, workspace, and semantic target checks on this macOS host using
disposable host/sanity shims; those shims are not part of bbtidy and are not a
substitute for a native Linux build host.

Observed fixture results:

| release | parsed `.bb` files | recipe queries | total launches | recipe-phase launches | semantic target |
| --- | ---: | ---: | ---: | ---: | --- |
| Yocto 5.0 / BitBake 2.8.1 | 920 | 920 | 46 | 43 | `core-image-minimal`, `MACHINE=qemux86-64`, `DISTRO=poky` |
| Yocto 6.0 / BitBake 2.18.0 | 949 | 949 | 48 | 45 | `core-image-minimal`, `MACHINE=qemux86-64`, `DISTRO=nodistro` |

The legacy per-recipe implementation would launch `N + 3` commands for
these scopes (923 and 952 respectively); the measured mixed strategy reduced
that to 46 and 48 launches while retaining all recipe queries. The helper
deliberately uses the main configuration (`mc=''`), matching the existing CLI
workspace scope. A future multi-config workspace mode must add each selected
configuration to the query key and evidence set rather than silently merging
configurations.

## Baseline tooling

[`scripts/benchmark_bitbake.py`](../scripts/benchmark_bitbake.py) records the
ordered phase command list, command count, stdout/stderr byte totals, and
recipe-query count. It omits timing and host metadata by default, so its JSON
is suitable for deterministic before/after comparisons. Add
`--include-recipe-queries --recipe-list PATH` to measure the legacy N+1 path;
add `--include-timing` only for local investigation. The tool is diagnostic
and does not define a normative wall-clock threshold.

Real Yocto validation commands are:

```bash
python3 scripts/benchmark_bitbake.py \
  --bitbake /path/to/bitbake \
  --build-dir /path/to/build \
  --recipe-list /path/to/recipes.txt \
  --include-recipe-queries

bbtidy check --workspace /path/to/build --bitbake /path/to/bitbake --output json
bbtidy semantic --build-dir /path/to/build --bitbake /path/to/bitbake \
  --target core-image-minimal --output json
```

The supported-release gate must compare indexed files, layer roots, class and
include candidates, dependency edges, `BBINCLUDED` discovery, lint findings,
and semantic target values between the old fallback and the Tinfoil strategy.
The local Python-backed integration fixture demonstrates the launch bound:
three recipe queries complete in four commands (version, parse, global
environment, and one helper) rather than the legacy `N + 3` commands. It is a
synthetic scalability check, not a substitute for the supported Yocto corpus
equivalence run.
