#!/usr/bin/env python3
"""Validate regenerated legacy evidence without promoting incomplete coverage."""

from __future__ import annotations

import argparse
import hashlib
import json
import tomllib
from collections import Counter
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"


class ValidationError(RuntimeError):
    """The report is inconsistent with its pinned inputs or claimed status."""


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValidationError(message)


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def expected_legacy_fixtures() -> dict[tuple[str, str], dict[str, Any]]:
    fixtures: dict[tuple[str, str], dict[str, Any]] = {}
    for corpus in ("reference", "published"):
        index = read_json(CONFORMANCE / "napplet-corpus" / corpus / "index.json")
        for fixture in index["fixtures"]:
            requires = fixture.get("requires")
            if requires is None:
                event = read_json(
                    CONFORMANCE
                    / "napplet-corpus"
                    / corpus
                    / fixture["name"]
                    / "event.json"
                )
                requires = [
                    tag[1]
                    for tag in event["tags"]
                    if len(tag) >= 2 and tag[0] == "requires"
                ]
            fixtures[(corpus, fixture["name"])] = {
                "artifact_mode": fixture["artifact_mode"],
                "requires": requires,
                "paths": sorted(
                    item["artifact_path"]
                    for item in fixture["files"]
                    if "artifact_path" in item
                ),
            }
    return fixtures


def validate_legacy(
    report: dict[str, Any], lock: dict[str, Any], *, expect_incomplete: bool
) -> None:
    require(report.get("schema") == 1, "legacy report schema must be 1")
    require(
        report.get("baseline") == lock["baseline"]["name"],
        "legacy report baseline does not match compatibility.lock",
    )
    require(
        report.get("baseline_status") == lock["baseline"]["status"],
        "legacy baseline status does not match compatibility.lock",
    )

    sources = report.get("sources", {})
    require(
        sources.get("napplet_web_commit") == lock["napplet_packages"]["commit"],
        "legacy report napplet/web commit is not pinned",
    )
    require(
        sources.get("kehto_commit") == lock["kehto"]["commit"],
        "legacy report Kehto commit is not pinned",
    )
    conformance = sources.get("conformance_package", {})
    require(
        conformance.get("version") == lock["napplet_packages"]["conformance"],
        "legacy report conformance package version is not pinned",
    )
    require(
        conformance.get("npm_sha256")
        == lock["napplet_packages"]["npm_sha256"]["conformance"],
        "legacy report conformance package digest is not pinned",
    )
    dependencies = sources.get("package_dependency_archives", {})
    for package, lock_key in (("@napplet/core", "core"), ("@napplet/nap", "nap")):
        dependency = dependencies.get(package, {})
        require(
            dependency.get("version") == lock["napplet_packages"][lock_key],
            f"legacy report {package} version is not pinned",
        )
        require(
            dependency.get("npm_sha256")
            == lock["napplet_packages"]["npm_sha256"][lock_key],
            f"legacy report {package} digest is not pinned",
        )

    shell_hashes = sources.get("trusted_shell_sha256", {})
    expected_shell_files = {
        "trusted-shell.css",
        "trusted-shell.html",
        "trusted-shell-policy.js",
        "trusted-shell.js",
    }
    require(
        set(shell_hashes) == expected_shell_files,
        "legacy report does not bind the complete trusted shell",
    )
    for relative, digest in shell_hashes.items():
        require(
            digest == sha256_file(ROOT / "web" / "trusted-shell" / relative),
            f"legacy report trusted shell digest drifted: {relative}",
        )

    expected = expected_legacy_fixtures()
    actual_records = report.get("fixtures")
    require(isinstance(actual_records, list), "legacy fixtures must be a list")
    actual = {
        (record.get("corpus"), record.get("name")): record
        for record in actual_records
    }
    require(
        len(actual) == len(actual_records),
        "legacy report contains duplicate fixture identities",
    )
    require(set(actual) == set(expected), "legacy fixture coverage is incomplete")
    for identity, expected_record in expected.items():
        record = actual[identity]
        require(
            record.get("artifact_mode") == expected_record["artifact_mode"],
            f"{identity}: artifact mode drifted",
        )
        require(
            record.get("requires") == expected_record["requires"],
            f"{identity}: requires contract drifted",
        )
        verified = record.get("verified_committed_bytes")
        require(
            isinstance(verified, dict)
            and sorted(verified) == expected_record["paths"]
            and all(value is True for value in verified.values()),
            f"{identity}: committed bytes are not all verified",
        )
        require(record.get("bytes_unchanged") is True, f"{identity}: bytes changed")

    host_counts = Counter(
        record.get("host", {}).get("status", "not-run") for record in actual_records
    )
    conformance_counts = Counter(
        record.get("host", {}).get("conformance", {}).get("status", "not-run")
        for record in actual_records
    )
    known_statuses = {"pass", "fail", "not-run"}
    require(
        set(host_counts).issubset(known_statuses),
        "legacy host report contains an unknown verdict",
    )
    require(
        set(conformance_counts).issubset(known_statuses),
        "legacy conformance report contains an unknown verdict",
    )
    expected_summary = {
        "host": {
            status: host_counts[status] for status in ("pass", "fail", "not-run")
        },
        "pinned_conformance_engine": {
            status: conformance_counts[status]
            for status in ("pass", "fail", "not-run")
        },
    }
    require(
        report.get("summary") == expected_summary,
        "legacy report summary does not match fixture verdicts",
    )

    handshake = (
        report.get("domain_contract", {})
        .get("registry_only_shell_handshake", {})
    )
    require(
        report.get("domain_contract", {}).get("macos_advertised_domains")
        == lock["platform"]["macos"]["supported_domains"],
        "legacy report macOS domain advertisement drifted",
    )
    should_pass = (
        host_counts["fail"] == 0
        and host_counts["not-run"] == 0
        and conformance_counts["fail"] == 0
        and conformance_counts["not-run"] == 0
        and handshake.get("status") == "pass"
    )
    require(
        report.get("status") == ("pass" if should_pass else "incomplete"),
        "legacy report status does not follow its verdicts",
    )
    if expect_incomplete:
        require(
            report.get("status") == "incomplete",
            "known-incomplete legacy evidence was promoted to pass",
        )
        if handshake.get("status") == "pass":
            require(
                handshake.get("observed_by_all_executed_fixtures") is True
                and handshake.get(
                    "init_exactly_once_for_all_executed_fixtures"
                )
                is True
                and handshake.get(
                    "authoritative_supports_for_all_executed_fixtures"
                )
                is True
                and handshake.get("reason") is None,
                "registry-only shell handshake pass lacks complete evidence",
            )
        require(
            isinstance(report.get("claim"), str) and "not claim M2" in report["claim"],
            "incomplete legacy report lacks its non-green claim",
        )


def validate_kehto(
    report: dict[str, Any], lock: dict[str, Any], *, expect_incomplete: bool
) -> None:
    require(report.get("schema") == 1, "Kehto report schema must be 1")
    require(
        report.get("baseline") == lock["baseline"]["name"],
        "Kehto report baseline does not match compatibility.lock",
    )
    source = report.get("source", {})
    require(
        source.get("repository") == lock["kehto"]["repository"],
        "Kehto repository is not pinned",
    )
    require(
        source.get("commit") == lock["kehto"]["commit"],
        "Kehto commit is not pinned",
    )
    require(
        source.get("corpus_tree") == lock["kehto"]["corpus_tree"],
        "Kehto corpus tree is not pinned",
    )
    require(
        source.get("exact_source_trees_verified") is True,
        "Kehto source trees were not verified",
    )
    require(
        report.get("package_manager") == "pnpm@10.8.0",
        "Kehto package manager is not exact",
    )
    require(
        report.get("dependency_mode") == "offline-frozen-lockfile",
        "Kehto dependency mode is not offline and frozen",
    )

    index = read_json(CONFORMANCE / "napplet-corpus" / "kehto" / "index.json")
    expected = {
        item["name"]: {
            "requires": item["requires"],
            "source_tree": item["git_tree"],
        }
        for item in index["applications"]
    }
    applications = report.get("applications")
    require(isinstance(applications, list), "Kehto applications must be a list")
    actual = {application.get("name"): application for application in applications}
    require(
        len(actual) == len(applications),
        "Kehto report contains duplicate application names",
    )
    require(set(actual) == set(expected), "Kehto report application coverage is incomplete")
    allowed_statuses = {"pass", "fail", "not-run", "built-not-run"}
    for name, expected_application in expected.items():
        application = actual[name]
        require(
            application.get("source_tree") == expected_application["source_tree"],
            f"{name}: source tree drifted",
        )
        require(
            application.get("requires") == expected_application["requires"],
            f"{name}: requires contract drifted",
        )
        require(
            application.get("status") in allowed_statuses,
            f"{name}: unknown application status",
        )

    counts = Counter(application["status"] for application in applications)
    require(
        report.get("counts") == dict(sorted(counts.items())),
        "Kehto counts do not match application verdicts",
    )
    should_pass = bool(applications) and all(
        application["status"] == "pass" for application in applications
    )
    require(
        report.get("status") == ("pass" if should_pass else "incomplete"),
        "Kehto report status does not follow its application verdicts",
    )
    if expect_incomplete:
        require(
            report.get("status") == "incomplete",
            "known-incomplete Kehto evidence was promoted to pass",
        )
        require(
            isinstance(report.get("claim"), str)
            and "No Kehto application is green" in report["claim"],
            "incomplete Kehto report lacks its non-green claim",
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--legacy", type=Path, required=True)
    parser.add_argument("--kehto", type=Path, required=True)
    parser.add_argument(
        "--expect-incomplete",
        action="store_true",
        help="fail if either known-incomplete M2 report claims pass",
    )
    arguments = parser.parse_args()

    with (ROOT / "compatibility.lock").open("rb") as handle:
        lock = tomllib.load(handle)
    validate_legacy(
        read_json(arguments.legacy),
        lock,
        expect_incomplete=arguments.expect_incomplete,
    )
    validate_kehto(
        read_json(arguments.kehto),
        lock,
        expect_incomplete=arguments.expect_incomplete,
    )
    print(
        json.dumps(
            {
                "kehto": str(arguments.kehto),
                "legacy": str(arguments.legacy),
                "status": "validated-incomplete"
                if arguments.expect_incomplete
                else "validated",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError, ValidationError) as error:
        raise SystemExit(f"legacy evidence validation FAILED: {error}")
