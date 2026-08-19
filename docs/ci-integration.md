# bbtidy CI integration

CI should install an exact bbtidy version, print it in the log, and begin with
offline read-only checks. The examples below pin the current evaluation build.
The copyable [generic CI command file](../examples/generic-ci.txt) contains the
pinned install and the three platform-neutral commands as one sequence.

## Generic CI

Install and record the exact version:

```bash
python3 -m pip install --pre "bbtidy==0.1.0a4"
bbtidy --version
```

The two ordinary gates are:

```bash
bbtidy format --check meta-my-layer/
bbtidy check --profile recommended meta-my-layer/
```

Both are read-only and do not invoke BitBake. Exit code `1` represents a
formatting difference or a lint finding at the selected threshold. Exit code
`2` is an operational failure and must not be treated as an ordinary finding.

## Progressive enforcement in CI

Start without failing on findings:

```bash
bbtidy check --profile recommended --fail-on never meta-my-layer/
```

For a repository with reviewed existing findings, write the baseline locally
and commit it:

```bash
bbtidy check \
  --profile recommended \
  --write-baseline .bbtidy-baseline.json \
  meta-my-layer/
```

CI then blocks only new findings:

```bash
bbtidy check \
  --profile recommended \
  --baseline .bbtidy-baseline.json \
  --fail-on warning \
  meta-my-layer/
```

Once existing findings are removed, delete or reduce the reviewed baseline.
Never refresh it automatically in CI.

## GitHub Actions

This minimal workflow pins bbtidy and each third-party action to an immutable
revision:

```yaml
name: bbtidy

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  check:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803 # v6.1.0
      - uses: actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97 # v7.0.0
        with:
          python-version: "3.12"
      - name: Install pinned bbtidy
        run: python3 -m pip install --pre "bbtidy==0.1.0a4"
      - name: Record bbtidy version
        run: bbtidy --version
      - name: Check formatting
        run: bbtidy format --check meta-my-layer/
      - name: Check lint
        run: bbtidy check --profile recommended meta-my-layer/
```

For a complete copyable workflow that retains SARIF before enforcing the saved
lint status, use [`examples/github-actions.yml`](../examples/github-actions.yml).

Use a reviewed baseline option in the final command when adopting bbtidy in a
repository that already has findings.

## SARIF

Generate SARIF with the same explicit profile:

```bash
bbtidy check \
  --profile recommended \
  --output sarif \
  meta-my-layer/ > bbtidy.sarif
```

A lint exit code of `1` can still produce a complete SARIF document. CI must
preserve that status while retaining the report; exit code `2` indicates that
the report must not be treated as complete. This GitHub Actions fragment stores
the SARIF as an artifact and enforces the saved status afterward:

```yaml
      - name: Generate bbtidy SARIF
        id: bbtidy_sarif
        shell: bash
        run: |
          set +e
          bbtidy check --profile recommended --output sarif meta-my-layer/ > bbtidy.sarif
          status=$?
          set -e
          echo "status=$status" >> "$GITHUB_OUTPUT"
      - name: Retain SARIF
        if: always() && (steps.bbtidy_sarif.outputs.status == '0' || steps.bbtidy_sarif.outputs.status == '1')
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: bbtidy-sarif
          path: bbtidy.sarif
          if-no-files-found: error
      - name: Enforce bbtidy result
        if: always() && steps.bbtidy_sarif.outputs.status != ''
        env:
          BBTIDY_EXIT_STATUS: ${{ steps.bbtidy_sarif.outputs.status }}
        run: exit "$BBTIDY_EXIT_STATUS"
```

Replace artifact retention with the CI provider's SARIF ingestion step when
code-scanning integration is configured. Keep the saved bbtidy status as the
authoritative gate.

## BitBake-backed CI

Enable workspace checking only on a runner with an initialized build and the
supported BitBake host tools:

```bash
bbtidy check --workspace build --profile recommended
```

This invokes BitBake and may update its parse cache. Apply explicit command,
operation, output, process, and query limits suitable for the runner. Do not
substitute a partial offline layer list when authoritative workspace discovery
fails. See [BitBake integration](bitbake-integration.md).

## Pre-commit

Install the same exact executable version outside pre-commit:

```bash
pipx install "bbtidy==0.1.0a4"
bbtidy --version
```

Then add local system hooks:

```yaml
repos:
  - repo: local
    hooks:
      - id: bbtidy-format
        name: bbtidy format
        entry: bbtidy format --check
        language: system
        files: '\.(bb|bbappend|bbclass|conf|inc)$'
      - id: bbtidy-check
        name: bbtidy check
        entry: bbtidy check --profile recommended
        language: system
        files: '\.(bb|bbappend|bbclass|conf|inc)$'
```

The hooks receive staged file paths from pre-commit. Keep repository-wide
workspace checking in CI rather than invoking BitBake from a local hook.
The copyable
[`examples/pre-commit-config.yaml`](../examples/pre-commit-config.yaml) uses
these local `language: system` hooks and assumes the pinned executable is
already installed; bbtidy does not publish a dedicated pre-commit package.

## Existing repository migration

Use the copyable [existing-repository
guide](../examples/existing-repository.md) to adopt bbtidy in this order:

1. preview formatting with `format --diff`;
2. land formatting as a dedicated review;
3. enable `format --check`;
4. start recommended linting with `--fail-on never`;
5. write and review an explicit baseline;
6. fail on new findings while retaining that baseline; and
7. reduce the baseline through reviewed fixes over time.

## Machine output guarantees

Text, JSON, and SARIF diagnostics are deterministic and source ordered.
Machine-readable output is written only after the complete analysis succeeds;
an operational failure does not leave a partial document presented as valid.
Record the command, configuration, bbtidy version, and exit status together
when retaining a report.
