# Parser precision and embedded-language evidence

This document records the evidence behind the parser-boundary changes in the
`0.1.0-alpha.4` development line. The parser remains lossless: a newly
recognized node must retain its original bytes, and shell/Python function
bodies remain opaque to the formatter.

## Reviewed unknown-node inventory

The pre-change community snapshot contained 30 unknown nodes totaling 1,753
bytes. The detailed inventory is checked in at
[`tests/upstream-corpora/inventories/community-master.json`](../tests/upstream-corpora/inventories/community-master.json).
Every record includes its path, byte range, length, excerpt, normalized
signature, neighboring node kinds, classification, and the corresponding
post-format record.

The records form two reviewed groups, all valid BitBake syntax:

| Construct | Count | Decision |
| --- | ---: | --- |
| `PREFERRED_PROVIDER_virtual/<provider> = "..."` | 2 | Recognize as an assignment |
| `PREFERRED_PROVIDER_virtual/<provider> ?= "..."` | 28 | Recognize as an assignment |

The assignment recognizer is intentionally limited to this evidence-backed
slash-bearing provider/version and override-scoped assignment namespaces.
Newly recognized slash-bearing assignments are formatter-verbatim; recognizing
them improves CST coverage without expanding formatting behavior. The reviewed
Yocto 5.0 pre-change sweep is checked in at
[`tests/upstream-corpora/inventories/yocto-5.0-prechange-unknown.json`](../tests/upstream-corpora/inventories/yocto-5.0-prechange-unknown.json):
68 `LICENSE:<scope>/<component>` assignments and one
`PREFERRED_VERSION_virtual/<provider>` assignment, all classified as valid
BitBake syntax. The final Yocto 5.0 and 6.0 scans have zero unknown nodes.

The inventory was generated from detailed statistics with:

```sh
bbtidy --no-config syntax-stats --details <pinned-corpus-paths> > syntax-details.json
python3 scripts/syntax_inventory.py \
  --stats-json syntax-details.json \
  --corpus-id community-master \
  --source-root <pinned-source-root> \
  --output community-master-inventory.json
```

## Corpus baselines

The current parser and formatter were rerun against the pinned corpus set on
2026-08-11.

| Corpus | Files | Structured nodes | Unknown nodes | Unknown bytes | Body findings |
| --- | ---: | ---: | ---: | ---: | --- |
| Community master, source | 667 | 22,301 | 0 | 0 | BBT034/035/036: 0/0/0 |
| Community master, formatted | 667 | 22,301 | 0 | 0 | — |
| Yocto 5.0 / BitBake 2.8 | 3,359 | 71,335 | 0 | 0 | BBT034/035/036: 0/0/0 |
| Yocto 6.0 / BitBake 2.18 | 3,643 | 66,099 | 0 | 0 | BBT034/035/036: 0/0/0 |

The Yocto rows were verified with `scripts/check_upstream_corpus.py
--skip-bitbake`; the repository's full harness also checked formatting
idempotence, metadata-file coverage, opaque-region preservation, excluded
payload preservation, and lint output. BitBake differential parsing remains a
separate Linux/BitBake gate.

The rule/message/construct/function-kind review is checked in at
[`tests/upstream-corpora/inventories/body-diagnostics.json`](../tests/upstream-corpora/inventories/body-diagnostics.json).
It records the removed shell false positives from the first body scan and the
focused true-positive coverage for BBT034, BBT035, and BBT036; the final pinned
corpora have no unclear findings.

## Boundary and body precision

The focused syntax-boundary fixtures cover legacy and modern overrides,
dynamic values, directives, shell here-documents, embedded Python, decorated
and multiline top-level definitions, and intentionally unsupported syntax.
Their expected diagnostics assert exact rule ID, severity, message, byte range,
and line/column endpoints.

Shell analysis is lexical and conservative. It ignores quoted words, BitBake
`${...}` expressions, command/process substitutions, backticks, and here-doc
payloads. A typed phase stack distinguishes `if`/`elif`/`else`, loops, and
`case` patterns from commands inside case arms. After a phase mismatch, the
analyzer emits the primary range and suppresses dependent cascade findings.

Python analysis tracks quote/triple-quote state, typed delimiters, line
continuations, compound-statement colons, and indentation only where those
signals are reliable. A mismatched closer or lexical syntax error suppresses
dependent indentation/compound diagnostics. Findings are sorted by exact
source range, use UTF-8-safe boundaries, and never execute Python or inspect
imports/runtime behavior.

The property tests and parser fuzz target assert losslessness, format
idempotence, deterministic body findings, ordered valid ranges, and valid
UTF-8 boundaries for generated and adversarial embedded-language inputs.
