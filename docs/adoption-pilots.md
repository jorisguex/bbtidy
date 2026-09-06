# Adoption pilots and lint review

The pilot workflow is ready to use. Real-user sessions and the new per-finding
human assessments are **pending**. The checked-in packets contain measured
findings, not completed reviews. Existing release baselines keep their earlier
review decisions; these new forms do not upgrade or replace those decisions.
The built-in lint default remains `all`, and adoption commands continue to
select `--profile recommended` explicitly.

## Start a pilot

Use one of the prepared bundles under
[lint-reviews](../tests/upstream-corpora/lint-reviews/README.md):

| Corpus | Active rules | Selected findings |
| --- | ---: | ---: |
| Yocto 5.0 Scarthgap | 16 | 210 |
| Yocto 6.0 Wrynose | 16 | 214 |
| Pinned community | 20 | 150 |

Copy the chosen bundle to a local pilot directory before filling it in. Keep
participant identifiers pseudonymous; do not commit personal information or
publish session notes without the participants' agreement. Record the actual
`bbtidy --version`, repository revision, commands, and relevant output in the
session notes. Use a candidate build from the repository when evaluating the
unreleased CLI.

Run an observed session for each of these three tasks. At least one completed
session per task is required by the evaluator as an initial coverage floor;
this is not evidence of broad user adoption or statistical confidence.

| `cohort` | Participant task | Record |
| --- | --- | --- |
| `quickstart` | Follow the [getting-started tutorial](getting-started.md) through the first formatting preview and explicit recommended lint report. | Time to first useful result, help needed, and whether the participant understands the output. |
| `existing-project` | Evaluate the Observe/Baseline/Enforce workflow on a disposable checkout with existing findings. Introduce an invalid configuration and distinguish its operational failure from lint findings. | Commands, exit codes, confusion, help needed, and unintended changes. |
| `new-config` | Starting from the documentation, create a minimal configuration and run the two offline CI commands on a disposable checkout. | Elapsed configuration minutes, help needed, and unintended changes. |

Use the existing documented commands; the pilot does not need a new CLI
subcommand or initialization command. Begin with read-only previews. Any
write exercise belongs in a disposable checkout, followed by diff inspection
and the project's normal validation. Do not contact or enroll people
automatically; a project maintainer arranges the sessions.

Add actual sessions to the `sessions` array in `observations.json`, preserving
its corpus ID and finding digest. This is an **unobserved template**, not a
passing result:

```json
{
  "id": "participant-01-quickstart",
  "cohort": "quickstart",
  "completed": false,
  "new_config_minutes": null,
  "operational_failures_mistaken_for_lint": null,
  "unsafe_edits": null,
  "notes": "Record version, source revision, commands, output, time to first useful result, and help needed."
}
```

Use `null` for an unobserved value. Enter zero only after checking that no
incident occurred. Configuration timing is required for every `new-config`
session; the evaluator uses the slowest observed time. Incident counts are
summed across sessions. An observed unsafe edit or operational/lint confusion
fails even when the session is unfinished.

## Review the selected findings

`packet.json` contains full normalized diagnostics, source paths, locations,
and fingerprints. [sources.json](../tests/upstream-corpora/lint-reviews/sources.json)
records the originating CI run, source commit, artifact, checksums, and pinned
repository revisions. Locations refer to the **formatted corpus**. Reproduce
the corresponding upstream-corpus run or use its disposable formatted tree;
original upstream line numbers may differ. Inspect surrounding metadata and
the relevant [rule documentation](lint-rules.md) before classifying a sample.

In `reviews.json`, identify the human reviewer for each rule and classify each
selected fingerprint along two separate dimensions:

- Correctness: `true_positive`, `false_positive`, or `unclear`.
- Actionability: `must_fix`, `should_fix`, `context_dependent`, `policy_only`,
  or `not_actionable`.

False-positive and unclear findings require notes describing the limitation
and proposed remediation. A technically correct finding is not automatically
actionable. Leave unknown classifications `null`. Review every selected sample
for each active rule; do not select only easy cases or replace fingerprints.
The sample policy requires all findings up to five, up to eight for 6–25,
twelve for 26–100, twenty for 101–500, and thirty above 500. Sampling is
deterministic across repositories, file types, and diagnostic shapes.

## Evaluate the observations

From the repository root, with a copied bundle at `pilot/`:

```bash
python3 scripts/adoption_pilot.py evaluate \
  --packet pilot/packet.json \
  --reviews pilot/reviews.json \
  --observations pilot/observations.json \
  --output pilot/result.json
```

The evaluator derives profile rates from fingerprint-bound human decisions.
It reports sample counts and review coverage alongside each threshold:

| Metric | Required result |
| --- | --- |
| Essential false-positive fraction | At most 1% |
| Recommended false-positive fraction | At most 5% |
| Recommended unclear fraction | At most 5% |
| Recommended actionable fraction (`must_fix` + `should_fix`) | At least 70% |
| New configuration time | At most 15 minutes |
| Operational errors mistaken for lint | Zero |
| Unsafe edits | Zero |

Exit codes are `0` for passing observed evidence, `1` for a failed threshold,
`2` for invalid inputs, and `3` for insufficient evidence. Missing observations,
incomplete required reviews, or missing task coverage cannot produce a pass.
These fractions describe a stratified review sample, not estimated population
error rates. Report all three corpora separately; do not pool shared upstream
findings as independent observations or generalize from one passing corpus.

A passing result identifies a candidate for maintainer review. It does not
change the built-in profile, approve a release, or certify compatibility.
Review failures should produce a specific rule fix, documented limitation, or
profile proposal followed by fresh measurements and review. The existing
release lint-baseline workflow remains a separate explicit decision.

## Refresh the packets

Upstream CI now writes `lint/review-packet.json` and `lint/review-form.json`
alongside its findings and quality report. To prepare a local bundle from
that run's normalized `lint/findings.json`:

```bash
python3 scripts/adoption_pilot.py prepare \
  --findings compatibility-evidence/lint/findings.json \
  --output pilot-new
```

Preparation refuses an existing output directory, so it cannot overwrite
completed human forms. Evaluation refuses mismatched corpus identities,
finding digests, sample fingerprints, or profile definitions. Keep old results
with their original packet and explicitly prepare a new review when findings
change.
