# bbtidy beta user guide

This guide explains how to move from a safe evaluation to production use in a
BitBake or Yocto repository. The [beta support
contract](beta-support-contract.md) is authoritative for supported versions,
guarantees, limitations, and release evidence. Start with the linear
[getting-started tutorial](getting-started.md) if you have not run bbtidy yet.

## Adoption contract

The adoption experience is deliberately divided into three paths so that
advanced BitBake-backed analysis does not obscure the safe first steps:

| Path | Intended use | Primary interface | Invokes BitBake |
| --- | --- | --- | --- |
| Quick start | First evaluation and ordinary CI | `format` and offline `check` | No |
| Production | Complete build-aware linting | `check --workspace BUILD_DIR` | Yes |
| Advanced | Target-specific linting and metadata inspection | `check --semantic` and `semantic` | Yes |

The following product decisions are frozen for the adoption-simplification
work:

- `format` and offline `check` are the primary interface.
- `check --workspace` is the preferred authoritative production check.
- `check --semantic` is an optional target-specific overlay.
- `semantic` is an inspection and reporting tool, not part of basic adoption.
- The quickstart is entirely read-only: it does not use `format --write` or
  `check --fix`, and it does not invoke BitBake.
- Every CI example must install or select an exact bbtidy version rather than a
  floating package name, branch, or tag.
- Until pilot evidence supports changing the built-in default, adoption
  documentation and CI examples use `--profile recommended` explicitly.
- This work does not add lint rules, parser features, editor integration, or an
  initialization command.

These decisions govern the adoption documentation and interface; they do not
expand the compatibility promises in the beta support contract.

## Before you adopt it

The beta contract covers Yocto Project 5.0 LTS (Scarthgap) with BitBake 2.8
and Yocto Project 6.0 LTS (Wrynose) with BitBake 2.18. Yocto and BitBake
`master` are development targets and are non-blocking. Current published
versions may still be alpha prereleases; an alpha release should be treated as
an evaluation build, not as a beta support claim.

Offline formatting and linting do not execute BitBake, expand dynamic
variables, or resolve unavailable external layers. BitBake-backed checks cover
that configured build context, but neither path rewrites embedded shell or
Python code. A successful bbtidy run is not proof that a complete build has
identical task hashes, packages, runtime behavior, or performance.

## Start with the task-based tutorial

Complete the [getting-started tutorial](getting-started.md) before enabling a
required gate. Its ten steps establish one safe path from a pinned install to
write mode:

1. verify the executable;
2. preview formatting and lint findings without changing files;
3. add a minimal explicit configuration;
4. baseline existing findings;
5. add the two offline CI gates;
6. opt into workspace linting when a configured build is available; and
7. write only from a clean branch or worktree.

The tutorial also defines the recommended Observe, Baseline, and Enforce
rollout. Use the [CI reference](ci-integration.md) for complete generic CI,
GitHub Actions, SARIF, and pre-commit examples.

## Production rollout

After the offline gates are stable, use the configured build as the
authoritative repository scope:

```bash
bbtidy check --profile recommended --workspace build
```

This invokes BitBake and uses its expanded `BBLAYERS`, `BBFILES`, `BBPATH`, and
`BBINCLUDED` values. The command either analyzes the complete discovered scope
or reports an operational error; it does not silently turn a failed BitBake
query into a successful partial check.

Add `check --semantic` only for target-specific validation of dynamic values,
overrides, anonymous Python, external layers, or machine and distro
configuration. Keep the standalone `semantic` command for metadata inspection
and reporting. The [BitBake integration reference](bitbake-integration.md)
documents discovery, execution strategies, resource limits, cancellation, and
side effects.

The [configuration reference](configuration.md) contains the complete schema
and precedence rules. The [lint rule reference](lint-rules.md) contains the
rule catalog, profiles, suppressions, baselines, safe fixes, and exit codes.

## Repository-wide safety

The default repository-wide limits are 10,000 discovered files and 256 MiB of
original source per `format` or `check` invocation. Set lower `[safety]`
limits for an initial rollout and raise them deliberately when the repository
is known to require more.

Before writing, bbtidy reads and formats the complete input set, checks the
limits, stages recovery copies, and checks that sources have not changed. It
refuses symbolic links, including directory roots, and does not replace any
file when a later input fails. If a write or commit step fails, already
replaced files are restored from the staged recovery copies. These controls
reduce accidental damage; they do not replace version control, backups, or
review.

## What to validate with BitBake

For supported releases, bbtidy's compatibility evidence includes both the
original and formatted pinned corpora, idempotence, preservation of opaque
payloads, structural coverage, a real BitBake parse-only run, and selected
semantic probes where configured. Users should still run their normal build
and test suite because parseability is not a complete semantic-equivalence
proof.

After formatting a production layer:

1. Run `bbtidy format --check` on the same scope to prove idempotence.
2. Run `bitbake --parse-only` for the images or recipes the change can affect.
3. Run the project's normal build, package, and runtime tests.
4. Review the generated diff for files outside the intended metadata scope.
5. Run `bbtidy semantic` against the same build directory when the change
   affects dynamic values, overrides, or layer interactions.

## Release rehearsal and publication

The release gate has three blocking corpus checks: Yocto 5.0/BitBake 2.8,
Yocto 6.0/BitBake 2.18, and the commit-pinned community corpus. The community
manifest uses the explicit `pinned-community` tier. It runs formatter,
preservation, structural, and lint-quality checks without requiring BitBake;
Yocto `master` remains a separate scheduled, report-only development check.

`release.yml` is the only tag-triggered workflow. It validates Cargo, Python,
workflow security, Rust, packaging, and the exact tag commit, then calls the
same reusable gate used by pull requests and `main`. Publication jobs cannot
start until the gate verifies every corpus and creates
`release-evidence.tar.gz` plus `release-evidence.sha256`. The archive is
attached to the GitHub Release with the binaries and `SHA256SUMS`.

Before a beta tag, run `release.yml` through `workflow_dispatch` from the
candidate commit with `publish` set to false. Confirm that the full wheel,
source-distribution, binary, compatibility, and evidence checks complete. A
temporary lint fingerprint change, supported parse failure, or missing
community artifact must make the aggregate gate fail and leave both registry
publishers and the GitHub Release skipped. Download the evidence archive and
independently run its checksum verification, then restore the clean candidate
and repeat the green rehearsal before pushing the matching
`vX.Y.Z[-alpha|beta|rc].N` tag.

Manual publication additionally requires `publish: true`, the exact
`PUBLISH` confirmation, and approval in the protected `release-publish`
environment. Do not use the individual publisher workflows: they expose only
`workflow_call` and are invoked by the orchestrator after the shared gate.

## Reporting a compatibility issue

Open an issue with a minimal reproducible example when possible. Include:

- the exact `bbtidy --version` output and installation method;
- host operating system, architecture, and Rust/Python versions when relevant;
- Yocto Project and BitBake versions, including layer and repository commits;
- the exact command, configuration file, and input path scope;
- whether the issue occurred in `format`, `format --check`, `check`, or `lex`;
- the original and formatted metadata, if it can be shared safely; and
- the first relevant bbtidy or BitBake diagnostic, exit code, and CI log.

Remove proprietary paths, credentials, and metadata before posting publicly.
Do not report a dynamic or unavailable dependency as a formatter regression
without first checking whether it is outside the supported scope in the
[contract](beta-support-contract.md).

## Performance measurements

Use the versioned performance runner when comparing releases or reviewing a
large workspace. Keep the runner class, exact corpus digest, and command line
constant; alternate base and candidate invocations on the same runner so
thermal and host drift do not become a false regression. Use a fresh
disposable build for cold, repeated unchanged inputs for warm, and no
BitBake/network activity for offline. Collect at least three synthetic and
offline samples, one cold BitBake sample, and two warm samples.

Performance budgets are distinct from safety limits. Command/query limits,
timeouts, output caps, and cancellation protect against runaway work; they are
not targets to tune upward. A failed, cancelled, timed-out, or limit-terminated
run is invalid baseline evidence. See the [performance evidence
guide](../tests/performance/README.md) for the measurement schema, structural
invariants, budget update review process, and release evidence layout.
