# bbtidy starter assets

Replace `meta-my-layer/` with the metadata scope used by your repository.
These files are intentionally small so they can be copied before adopting
advanced BitBake-backed analysis.

- [`bbtidy.toml`](bbtidy.toml): minimal report-only configuration. Copy it to
  the repository root as `.bbtidy.toml`.
- [`generic-ci.txt`](generic-ci.txt): pinned, platform-neutral CI commands.
- [`github-actions.yml`](github-actions.yml): complete least-privilege GitHub
  Actions workflow with SARIF retention and exit-status enforcement.
- [`pre-commit-config.yaml`](pre-commit-config.yaml): local `language: system`
  hooks that use an already-installed bbtidy executable. Copy it to
  `.pre-commit-config.yaml`.
- [`existing-repository.md`](existing-repository.md): staged migration for a
  repository with existing formatting and lint findings.

The quickstart paths are offline and read-only. The existing-repository guide
introduces `format --write` only after a read-only diff and isolates that write
in a dedicated review. None of these assets invokes BitBake.
