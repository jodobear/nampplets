#!/usr/bin/env python3
"""Validate D0-D10 evidence and write a deterministic architecture report."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
EXPECTED_RULES = [f"D{index}" for index in range(11)]
ALLOWED_DISPOSITIONS = {
    "boundary-delegated",
    "design-evidence",
    "not-exercised",
}


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def validate_evidence(document: dict[str, Any]) -> list[dict[str, Any]]:
    if document.get("schema") != 1:
        raise ValueError("unsupported architecture evidence schema")
    rules = document.get("rules")
    if not isinstance(rules, list):
        raise ValueError("architecture evidence rules must be a list")
    identifiers = [rule.get("id") for rule in rules]
    if identifiers != EXPECTED_RULES:
        raise ValueError(f"architecture evidence must list {EXPECTED_RULES} in order")
    for rule in rules:
        if rule.get("disposition") not in ALLOWED_DISPOSITIONS:
            raise ValueError(f"{rule.get('id')}: invalid disposition")
        if not isinstance(rule.get("rationale"), str) or not rule["rationale"].strip():
            raise ValueError(f"{rule.get('id')}: rationale is required")
        paths = rule.get("evidence")
        if not isinstance(paths, list) or not paths:
            raise ValueError(f"{rule.get('id')}: at least one evidence path is required")
        for relative in paths:
            path = ROOT / relative
            if not path.exists():
                raise ValueError(f"{rule.get('id')}: evidence path is missing: {relative}")
    return rules


def validate_findings(
    document: Any,
) -> tuple[list[dict[str, Any]], int, bool, int, dict[str, int]]:
    if not isinstance(document, dict) or document.get("schema") != 1:
        raise ValueError("scanner output must use schema 1")
    raw_findings = document.get("findings")
    total = document.get("total_findings")
    truncated = document.get("truncated")
    limit = document.get("limit")
    severity_counts = document.get("severity_counts")
    if not isinstance(raw_findings, list):
        raise ValueError("scanner findings must be a JSON list")
    if not isinstance(total, int) or total < len(raw_findings):
        raise ValueError("scanner total_findings is invalid")
    if not isinstance(truncated, bool) or truncated != (total > len(raw_findings)):
        raise ValueError("scanner truncation metadata is inconsistent")
    if not isinstance(limit, int) or limit <= 0:
        raise ValueError("scanner report limit must be finite and positive")
    if (
        not isinstance(severity_counts, dict)
        or set(severity_counts) != {"error", "warning"}
        or any(
            not isinstance(count, int) or count < 0
            for count in severity_counts.values()
        )
        or sum(severity_counts.values()) != total
    ):
        raise ValueError("scanner severity counts are inconsistent")
    findings: list[dict[str, Any]] = []
    required = {"severity", "rule", "path", "line", "match", "reason"}
    for finding in raw_findings:
        if not isinstance(finding, dict) or set(finding) != required:
            raise ValueError("scanner finding has an unexpected shape")
        if finding["severity"] not in {"warning", "error"}:
            raise ValueError("scanner finding has an invalid severity")
        findings.append(finding)
    return (
        sorted(
            findings,
            key=lambda finding: (
                finding["path"],
                finding["line"],
                finding["rule"],
                finding["match"],
            ),
        ),
        total,
        truncated,
        limit,
        severity_counts,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scan", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    (
        findings,
        total_findings,
        truncated,
        finding_limit,
        severity_counts,
    ) = validate_findings(load_json(arguments.scan))
    rules = validate_evidence(load_json(arguments.evidence))
    included_blocking = [
        finding for finding in findings if finding["severity"] == "error"
    ]
    report = {
        "schema": 1,
        "status": (
            "triage-blocked"
            if severity_counts["error"] > 0
            else "triage-complete"
        ),
        "claim": (
            "Static triage plus evidence inventory; this artifact does not by itself "
            "prove architectural compliance."
        ),
        "scanner": {
            "path": "scripts/nmp_architecture_scan.py",
            "finding_count": total_findings,
            "included_finding_count": len(findings),
            "finding_limit": finding_limit,
            "truncated": truncated,
            "severity_counts": severity_counts,
            "blocking_finding_count": severity_counts["error"],
            "included_blocking_finding_count": len(included_blocking),
            "findings": findings,
        },
        "doctrine": rules,
    }
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(
        f"d0-d10-report: {len(rules)} rules, {total_findings} scanner findings, "
        f"{severity_counts['error']} blocking"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
