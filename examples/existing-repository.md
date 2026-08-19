# Adopt bbtidy in an existing repository

Use this sequence when the repository already has metadata and lint findings.
Install the exact bbtidy version selected by the project before starting.
Replace `meta-my-layer/` with the intended metadata scope.

```bash
python3 -m pip install --pre "bbtidy==0.1.0a4"
bbtidy --version
```

## 1. Preview formatting

Start with a read-only diff:

```bash
bbtidy format --diff meta-my-layer/
```

## 2. Land formatting as a dedicated review

After reviewing the preview, apply formatting in a clean branch or worktree.
Keep this change separate from lint cleanup and functional changes:

```bash
bbtidy format --write meta-my-layer/
bbtidy format --check meta-my-layer/
git diff --check
git diff
```

## 3. Enable formatting CI

After the formatting review lands, add the read-only formatting gate:

```bash
bbtidy format --check meta-my-layer/
```

## 4. Start lint in report-only mode

Collect recommended-profile findings without failing CI:

```bash
bbtidy check --profile recommended --fail-on never meta-my-layer/
```

## 5. Write and review a baseline

Record existing findings, inspect the baseline, and commit it only after
review:

```bash
bbtidy check \
  --profile recommended \
  --fail-on never \
  --write-baseline .bbtidy-baseline.json \
  meta-my-layer/

git diff -- .bbtidy-baseline.json
```

## 6. Begin failing on new findings

Use the reviewed baseline with warning-level enforcement. Baseline findings
remain non-blocking; new warnings and errors fail the command:

```bash
bbtidy check \
  --profile recommended \
  --baseline .bbtidy-baseline.json \
  --fail-on warning \
  meta-my-layer/
```

## 7. Reduce the baseline over time

Fix existing findings in reviewed changes. Explicitly refresh the baseline
only after confirming that entries were removed for the intended reasons:

```bash
bbtidy check \
  --profile recommended \
  --refresh-baseline .bbtidy-baseline.json \
  meta-my-layer/

git diff -- .bbtidy-baseline.json
```

Do not refresh the baseline automatically in CI.
