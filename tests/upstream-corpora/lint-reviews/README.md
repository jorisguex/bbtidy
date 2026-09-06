# Lint review ledgers

Ready-to-review packets for the three pinned corpora are checked in here:

- [Yocto 5.0 Scarthgap](yocto-5.0-scarthgap/packet.json): 210 selected findings.
- [Yocto 6.0 Wrynose](yocto-6.0-wrynose/packet.json): 214 selected findings.
- [Pinned community](community-master/packet.json): 150 selected findings.

Each directory includes blank `reviews.json` and `observations.json` forms.
Their status is **awaiting human review and user sessions**. No classifications
or pilot outcomes have been inferred from the existing aggregate baselines.
[sources.json](sources.json) retains provenance and checksums. Follow the
[pilot protocol](../../../docs/adoption-pilots.md) to inspect the formatted
source, record actual decisions, and evaluate the results.

These per-finding pilot forms use their own schema 1 and are separate from the
aggregate schema 2 release review ledgers described below. They are not
automatically imported into a release baseline.

Corpus runs write deterministic, stratified review candidates to the lint
quality evidence bundle. A checked-in review ledger uses schema 2 and keeps
measurement separate from human classification:

```json
{
  "schema": 2,
  "status": "reviewed",
  "rules": {
    "BBT001": {
      "status": "reviewed",
      "sample_size": 8,
      "correctness": {
        "true_positive": 8,
        "false_positive": 0,
        "unclear": 0
      },
      "actionability": {
        "must_fix": 4,
        "should_fix": 4,
        "context_dependent": 0,
        "policy_only": 0,
        "not_actionable": 0
      },
      "repositories": ["poky"],
      "file_types": [".bb"],
      "diagnostic_shapes": ["BBT001:trailing whitespace"],
      "sample_fingerprints": ["<sha256-from-quality-report>"],
      "notes": ""
    }
  }
}
```

The placeholder above is a schema example, not review evidence. Real
fingerprints come from `scripts/lint_quality.py` and must be copied from the
quality report after human inspection. Legacy v1 corpus baselines remain
readable for compatibility; new or refreshed ledgers must use the v2 nested
correctness/actionability fields and tiered sample minimums.
