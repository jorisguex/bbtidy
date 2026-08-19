# Getting started with bbtidy

This tutorial takes a repository from a read-only evaluation to pinned CI and,
optionally, BitBake-backed workspace checking. Complete the steps in order.
Replace `meta-my-layer/` and `build` with paths from your project.

The first nine steps do not write BitBake metadata. Only step 10 enables an
explicit write, after review in a clean branch or worktree.

## 1. Install a pinned version

Install the current evaluation build by exact version:

```bash
python3 -m pip install --pre "bbtidy==0.1.0a4"
```

Use `pipx install "bbtidy==0.1.0a4"` instead when you prefer an isolated
global executable. Do not use an unversioned install in CI.

## 2. Verify the executable

Record the selected version:

```bash
bbtidy --version
```

The output must identify `0.1.0-alpha.4` before you continue.

## 3. Preview formatting

Inspect the complete formatting diff without changing files:

```bash
bbtidy format --diff meta-my-layer/
```

Review unexpected file discovery or formatting before proceeding.

## 4. Run report-only recommended linting

Start with the evidence-selected recommended profile and keep findings
advisory:

```bash
bbtidy check --profile recommended --fail-on never meta-my-layer/
```

This offline command does not invoke BitBake or modify source files. It can
still return exit code `2` for an operational failure.

## 5. Create a minimal configuration

Create `.bbtidy.toml` at the repository root with only the initial policy:

```toml
[lint]
profile = "recommended"
fail_on = "never"

[paths]
exclude = ["vendor/**"]
```

The same configuration is available as the copyable
[`examples/bbtidy.toml`](../examples/bbtidy.toml) starter file.
Adjust the exclusion to match generated or externally maintained metadata in
your repository. The [configuration reference](configuration.md) documents
the complete schema when you need additional settings.

## 6. Handle existing findings with a baseline

If the initial report is not immediately clean, record the reviewed existing
findings explicitly:

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

Commit the baseline only after reviewing it. Existing entries remain visible
but non-blocking; new findings and operational errors still fail according to
the configured policy.

## 7. Enable formatting CI

After landing any intentional formatting changes separately, add the read-only
formatting gate:

```bash
bbtidy format --check meta-my-layer/
```

Pin the installation to the same exact bbtidy version used locally and print
`bbtidy --version` in the job log.

## 8. Enable lint CI

Begin with observation or the reviewed baseline, then advance to enforcement:

```bash
bbtidy check \
  --profile recommended \
  --baseline .bbtidy-baseline.json \
  --fail-on warning \
  meta-my-layer/
```

Repositories without existing findings may omit `--baseline`. See the
[CI integration reference](ci-integration.md) for generic and GitHub-specific
examples.

## 9. Optionally enable workspace linting

When a configured BitBake build is available, replace the supplied layer path
with authoritative workspace discovery:

```bash
bbtidy check --workspace build --profile recommended
```

This command invokes BitBake and may update its normal parse cache or server
metadata. Read the [BitBake integration reference](bitbake-integration.md)
before enabling it in CI.

## 10. Apply writes only in a clean branch

After reviewing the read-only results, create a clean branch or worktree and
confirm version-control status before allowing changes:

```bash
git status --short
bbtidy format --write meta-my-layer/
bbtidy check --profile recommended --fix meta-my-layer/
git diff --check
git diff
```

Review the diff, rerun `bbtidy format --check`, and run the project's normal
BitBake parse, build, package, and runtime tests before merging.

## Progressive enforcement

Adopt linting in three deliberate stages:

1. **Observe:** run `--profile recommended --fail-on never` to collect results
   without failing CI.
2. **Baseline:** review and record accepted existing findings, then compare
   every run with `--baseline .bbtidy-baseline.json` so new findings fail.
3. **Enforce:** set `fail_on = "warning"` or pass `--fail-on warning` while
   retaining the recommended profile.

Do not skip directly to enforcement merely to make the first run appear clean.
Reduce a baseline through reviewed changes rather than refreshing it
implicitly.

## Next references

- [Beta user guide](beta-user-guide.md) for production rollout and validation.
- [Lint reference](lint-rules.md) for suppressions, baseline maintenance, and
  rule details.
- [CI integration](ci-integration.md) for pinned automation examples.
