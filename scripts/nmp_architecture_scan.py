#!/usr/bin/env python3
"""Deterministic static triage for common RMP/NMP architecture violations.

This repository-owned scanner is a deterministic port of the scanner shipped
with the nmp-app-architecture skill. It deliberately favors actionable
suspicion over completeness and does not claim to prove compliance.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable


EXCLUDED_DIRS = {
    ".build",
    ".git",
    ".gradle",
    ".idea",
    ".swiftpm",
    "DerivedData",
    "build",
    "dist",
    "docs",
    "examples",
    "fixtures",
    "node_modules",
    "target",
    "testdata",
    "tests",
    "vendor",
    "wiki",
}

TEXT_EXTENSIONS = {
    ".c",
    ".cc",
    ".cpp",
    ".h",
    ".hpp",
    ".java",
    ".js",
    ".jsx",
    ".kt",
    ".kts",
    ".mjs",
    ".rs",
    ".swift",
    ".ts",
    ".tsx",
}

NATIVE_EXTENSIONS = {
    ".java",
    ".js",
    ".jsx",
    ".kt",
    ".kts",
    ".mjs",
    ".swift",
    ".ts",
    ".tsx",
}


@dataclass(frozen=True)
class Finding:
    severity: str
    rule: str
    path: str
    line: int
    match: str
    reason: str


RULES = [
    (
        "error",
        "D8/no-polling",
        re.compile(
            r"\b(thread::sleep|Task\.sleep|Timer\.scheduledTimer|setInterval|setTimeout|"
            r"DispatchQueue\.[A-Za-z0-9_.]+\.asyncAfter|while\s+.*sleep|"
            r"try_recv\b.*sleep|sleep\b.*try_recv)"
        ),
        "Polling or sleep-check loops are forbidden; use blocking primitives or callbacks.",
        None,
    ),
    (
        "error",
        "D3/no-hardcoded-relay",
        re.compile(r"wss://[A-Za-z0-9._~:/?#\[\]@!$&'()*+,;=%-]+"),
        "Hardcoded relay URLs in production code usually bypass outbox routing.",
        None,
    ),
    (
        "warning",
        "D9/kernel-owns-time",
        re.compile(
            r"\b(SystemTime::now|Instant::now|Date\(\)|Date\.now|"
            r"currentTimeMillis|NSDate\(\))\b"
        ),
        "Reducer, replay, routing, or policy code must use an injected clock.",
        None,
    ),
    (
        "warning",
        "D6/no-ffi-errors",
        re.compile(
            r"(#\[uniffi::export\]|extern\s+\"C\"|@Throws|throws\b|"
            r"try\s*\{|catch\s*\(|->\s*Result\s*<)"
        ),
        "Errors must surface as state, not native exceptions or FFI Result types.",
        None,
    ),
    (
        "warning",
        "D5/bounded-snapshot",
        re.compile(
            r"\b(AppState|Snapshot|FullState)\b.*"
            r"\b(Vec<.*Event|Vec<.*Note|event_store|history|all_events)\b",
            re.I,
        ),
        "Snapshots must be screen-shaped and bounded by open views.",
        None,
    ),
    (
        "warning",
        "D7/native-policy-smell",
        re.compile(
            r"\b(shouldRetry|isRecoverable|retryCount|relayUrl|relay_url|"
            r"publishRelay|decrypt|encrypt|signEvent|nostrEvent|NostrEvent|"
            r"Filter|Kind|kind\s*[=:])\b"
        ),
        "Native code may render or execute capabilities only; verify this is not policy.",
        NATIVE_EXTENSIONS,
    ),
    (
        "warning",
        "D4/native-cache-smell",
        re.compile(r"\b(cache|cached|RoomDatabase|SwiftData|UserDefaults|SharedPreferences)\b"),
        "Native caches must not mirror Rust-owned app facts.",
        NATIVE_EXTENSIONS,
    ),
    (
        "warning",
        "no-debt",
        re.compile(
            r"(\bTODO\b|\bFIXME\b|\bHACK\b|\btemporary\b|"
            r"\bfor now\b|\bstub\b|\bworkaround\b)"
        ),
        "Temporary debt requires canonical tracking or removal.",
        None,
    ),
]


def iter_files(root: Path) -> Iterable[Path]:
    for path in sorted(root.rglob("*"), key=lambda candidate: candidate.as_posix()):
        if not path.is_file():
            continue
        if any(part in EXCLUDED_DIRS for part in path.parts):
            continue
        if path.suffix not in TEXT_EXTENSIONS:
            continue
        yield path


def should_skip_line(path: Path, line: str) -> bool:
    parts = set(path.parts)
    if {"docs", "wiki"} & parts:
        return True
    if path.name in {"README.md", "CHANGELOG.md", "AGENTS.md", "CLAUDE.md"}:
        return True
    if "test" in path.name.lower() or "tests" in parts:
        return True
    stripped = line.strip()
    return (
        not stripped
        or stripped.startswith(("//", "#", "*", "Text(", "Label("))
    )


def scan(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for path in iter_files(root):
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except UnicodeDecodeError:
            continue
        relative = path.relative_to(root).as_posix()
        test_boundary = None
        if path.suffix == ".rs":
            test_boundary = next(
                (number for number, line in enumerate(lines, 1) if "#[cfg(test)]" in line),
                None,
            )
        for line_number, line in enumerate(lines, 1):
            if test_boundary is not None and line_number >= test_boundary:
                continue
            if should_skip_line(path, line):
                continue
            for severity, rule, pattern, reason, extension_filter in RULES:
                if extension_filter is not None and path.suffix not in extension_filter:
                    continue
                if rule == "D6/no-ffi-errors" and path.suffix == ".rs":
                    context = "\n".join(lines[max(0, line_number - 6) : line_number + 1])
                    if not (
                        "uniffi::export" in context
                        or 'extern "C"' in context
                        or "ffi" in relative.lower()
                    ):
                        continue
                match = pattern.search(line)
                if match is None:
                    continue
                if rule == "D3/no-hardcoded-relay" and (
                    "assert" in line
                    or "relay.example" in line
                    or "wss://x" in line
                ):
                    continue
                findings.append(
                    Finding(
                        severity=severity,
                        rule=rule,
                        path=relative,
                        line=line_number,
                        match=line.strip()[:180],
                        reason=reason,
                    )
                )
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", nargs="?", default=".")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--limit", type=int, default=200)
    parser.add_argument(
        "--fail-on",
        choices=["never", "warning", "error"],
        default="never",
    )
    arguments = parser.parse_args()
    if arguments.limit < 0:
        parser.error("--limit must be non-negative")

    root = Path(arguments.root).resolve()
    findings = scan(root)
    visible = findings if arguments.limit == 0 else findings[: arguments.limit]

    if arguments.json:
        print(
            json.dumps(
                {
                    "schema": 1,
                    "limit": arguments.limit,
                    "total_findings": len(findings),
                    "severity_counts": {
                        "error": sum(
                            finding.severity == "error" for finding in findings
                        ),
                        "warning": sum(
                            finding.severity == "warning" for finding in findings
                        ),
                    },
                    "truncated": len(visible) < len(findings),
                    "findings": [asdict(finding) for finding in visible],
                },
                indent=2,
                sort_keys=True,
            )
        )
    else:
        if not findings:
            print("nmp-architecture-scan: no findings")
        for finding in visible:
            print(
                f"{finding.severity.upper()} {finding.rule} "
                f"{finding.path}:{finding.line}"
            )
            print(f"  {finding.match}")
            print(f"  {finding.reason}")
        if len(visible) < len(findings):
            print(
                f"... {len(findings) - len(visible)} more finding(s); "
                "rerun with --limit 0 to show all"
            )

    severities = {finding.severity for finding in findings}
    if arguments.fail_on == "error" and "error" in severities:
        return 2
    if arguments.fail_on == "warning" and findings:
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
