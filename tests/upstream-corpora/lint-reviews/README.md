# Lint review ledgers

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
