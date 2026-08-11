#!/usr/bin/env python3
"""Turn ``syntax-stats --details`` output into reviewed construct evidence.

The Rust command owns parsing and formatting.  This small tool adds corpus
identity, stable construct signatures, grouping, and an explicit review
classification so aggregate syntax counters cannot hide individual unknown
constructs.
"""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter, defaultdict
from pathlib import Path


SCHEMA = 1
STRING = re.compile(r'("[^"\r\n]*"|\'[^\'\r\n]*\')')
BITBAKE_REFERENCE = re.compile(r"\$\{[^}\r\n]*\}")
REVIEWED_PREFERRED = re.compile(r"PREFERRED_(?:PROVIDER|VERSION)_virtual/[^\s:=]+")
REVIEWED_LICENSE = re.compile(r"LICENSE:[^\s:=]+/[^\s:=]+")


def normalize_signature(excerpt: str) -> str:
    """Normalize literals and provider names while retaining syntax shape."""

    value = excerpt.strip()
    value = BITBAKE_REFERENCE.sub("${VAR}", value)
    value = STRING.sub(
        lambda match: '"STRING"' if match.group(0).startswith('"') else "'STRING'",
        value,
    )
    value = REVIEWED_PREFERRED.sub(
        lambda match: re.sub(r"/[^\s:=]+$", "/<provider>", match.group(0)),
        value,
    )
    value = REVIEWED_LICENSE.sub("LICENSE:<scope>/<component>", value)
    value = re.sub(r"\s+", " ", value)
    return value[:200]


def classify(signature: str) -> str:
    if signature.startswith((
        "PREFERRED_PROVIDER_virtual/<provider> ",
        "PREFERRED_VERSION_virtual/<provider> ",
        "LICENSE:<scope>/<component> ",
    )):
        return "valid_bitbake_syntax"
    return "uncertain_requires_bitbake_probe"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stats-json", required=True, type=Path)
    parser.add_argument("--corpus-id", required=True)
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def relative_path(raw: str, source_root: Path) -> str:
    path = Path(raw).resolve()
    try:
        return path.relative_to(source_root.resolve()).as_posix()
    except ValueError as error:
        raise ValueError(f"{raw} is outside source root {source_root}") from error


def main() -> int:
    args = parse_args()
    report = json.loads(args.stats_json.read_text(encoding="utf-8"))
    if report.get("version") != 1 or not isinstance(report.get("details"), list):
        raise ValueError("syntax statistics must be schema 1 detailed output")

    records = []
    groups = defaultdict(list)
    before_count = 0
    after_count = 0
    for file_record in report["details"]:
        path = relative_path(file_record["path"], args.source_root)
        after_items = file_record.get("after", [])
        after_signatures = Counter(normalize_signature(item["excerpt"]) for item in after_items)
        for item in file_record.get("before", []):
            before_count += 1
            signature = normalize_signature(item["excerpt"])
            exact_after = [
                candidate
                for candidate in after_items
                if candidate["start_byte"] == item["start_byte"]
                and candidate["end_byte"] == item["end_byte"]
            ]
            candidates = exact_after or [
                candidate
                for candidate in after_items
                if normalize_signature(candidate["excerpt"]) == signature
            ]
            matching_after = [
                {
                    "start_byte": candidate["start_byte"],
                    "end_byte": candidate["end_byte"],
                    "length": candidate["length"],
                    "first_line": candidate["excerpt"],
                    "previous_kind": candidate["previous_kind"],
                    "next_kind": candidate["next_kind"],
                }
                for candidate in candidates
            ]
            record = {
                "path": path,
                "start_byte": item["start_byte"],
                "end_byte": item["end_byte"],
                "length": item["length"],
                "first_line": item["excerpt"],
                "signature": signature,
                "classification": classify(signature),
                "previous_kind": item["previous_kind"],
                "next_kind": item["next_kind"],
                "appears_after_formatting": after_signatures[signature] > 0,
                "after": matching_after,
            }
            records.append(record)
            groups[signature].append(record)
        after_count += len(file_record.get("after", []))

    group_records = []
    for signature in sorted(groups):
        members = groups[signature]
        group_records.append(
            {
                "signature": signature,
                "count": len(members),
                "classification": classify(signature),
                "records": members,
            }
        )

    output = {
        "schema": SCHEMA,
        "corpus_id": args.corpus_id,
        "source_metrics": {
            key: report[key]
            for key in ("files", "structured_nodes", "total_nodes", "trivia_nodes", "unknown_nodes", "unknown_bytes")
            if key in report
        },
        "summary": {
            "unknown_nodes_before_formatting": before_count,
            "unknown_nodes_after_formatting": after_count,
            "groups": len(group_records),
            "all_groups_classified": all(
                group["classification"] != "uncertain_requires_bitbake_probe"
                for group in group_records
            ),
        },
        "groups": group_records,
        "records": records,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(output["summary"], sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
