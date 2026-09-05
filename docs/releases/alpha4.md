# Published alpha.4 quickstart

This page documents `0.1.0-alpha.4` (`0.1.0a4` on PyPI). It uses the older
CLI shipped by that release. The main documentation describes the unreleased
alpha.5 candidate and must not be used with this binary.

```bash
python3 -m pip install --pre "bbtidy==0.1.0a4"
bbtidy --version
bbtidy format --diff meta-my-layer/
bbtidy lint meta-my-layer/
```

These commands are read-only. `lint` can return `1` when it finds warnings.
For CI with alpha.4, use:

```bash
bbtidy check meta-my-layer/
bbtidy lint meta-my-layer/
```

In alpha.4, `check` checks formatting and `lint` reports lint findings.
It has no `format --check`, lint profiles, `--fail-on`, or adoption baselines.
The release's full documentation is available in the
[tagged README](https://github.com/jorisguex/bbtidy/blob/v0.1.0-alpha.4/README.md).

When upgrading to alpha.5 after publication, replace formatting `check` with
`format --check`, and replace `lint` with `check --profile recommended`.
See the [development tutorial](../getting-started.md) for the new workflow.
