"""Upgrade, corpus, and falsifier evidence checks."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"


def verify_upgrade_report(
    lock: dict[str, Any],
    load_json: Callable[[str], Any],
    error: type[ValueError],
) -> dict[str, int]:
    report = load_json("conformance/reports/compatibility-v2.json")
    if report["baseline"] != lock["baseline"]["name"]:
        raise error("upgrade report baseline mismatch")
    if report["captured_at"] != lock["baseline"]["created_at"]:
        raise error("upgrade report capture date mismatch")
    if report["status"] != lock["baseline"]["status"]:
        raise error("upgrade report status mismatch")
    expected_authorities = {
        "nip_5d": lock["nip_5d"]["commit"],
        "nap_registry": lock["nap_registry"]["commit"],
        "napplet_web": lock["napplet_packages"]["commit"],
        "nap_lists_semantic": lock["nap_lists"]["semantic_commit"],
        "nap_lists_package_merge": lock["nap_lists"]["package_merge_commit"],
        "nip_51": lock["nap_lists"]["nip_51_commit"],
        "kehto": lock["kehto"]["commit"],
    }
    if report["to"] != expected_authorities:
        raise error("upgrade report authority mismatch")
    if report["signoff"] != lock["signoff"]:
        raise error("upgrade report signoff mismatch")
    if report["source_repositories"] != {
        "kehto": lock["kehto"]["repository"],
        "kehto_upstream": lock["kehto"]["upstream_repository"],
        "napplet_web": lock["napplet_packages"]["repository"],
    }:
        raise error("upgrade report source repository mismatch")
    if report.get("local_patches") != {
        "napplet_web": {
            "path": lock["napplet_packages"]["local_patch"],
            "sha256": lock["napplet_packages"]["local_patch_sha256"],
        }
    }:
        raise error("upgrade report local patch mismatch")
    expected_decisions = {
        "accepted": [
            "runtime-attested-inc-sender",
            "queryless-convention-transposition",
            "normalized-intent-request",
            "independent-intent-delivery",
            "strict-child-csp-guidance",
            "kehto-artifacts-without-modulepreload-fetch",
            "released-nap-lists-wire-schema",
        ],
        "rejected": [
            "caller-supplied-inc-sender",
            "malformed-intent-fields-or-conflicting-aliases",
            "kehto-artifacts-with-modulepreload-fetch",
            "raw-nip51-list-item-wire",
        ],
        "explicitly_unsupported": ["registry-only-inc.channel.opened"],
        "migration": {
            "legacy_intent_protocol_alias": "accepted-at-rust-provider-boundary",
            "legacy_optional_intent_fields": (
                "accepted-and-defaulted-at-web-and-rust-boundaries"
            ),
            "platform_domains_advertised": [],
        },
    }
    for decision, expected in expected_decisions.items():
        if report.get(decision) != expected:
            raise error(f"upgrade report {decision} decisions drifted")
    return {
        "accepted": len(report["accepted"]),
        "rejected": len(report["rejected"]),
        "explicitly_unsupported": len(report["explicitly_unsupported"]),
    }


def verify_corpus(
    lock: dict[str, Any],
    load_json: Callable[[str], Any],
    sha256_file: Callable[[Path], str],
    error: type[ValueError],
) -> tuple[int, int, int]:
    reference = load_json("conformance/napplet-corpus/reference/index.json")
    kehto = load_json("conformance/napplet-corpus/kehto/index.json")
    published = load_json("conformance/napplet-corpus/published/index.json")
    if reference["digest"] != lock["corpus"]["reference_fixture_digest"]:
        raise error("reference corpus digest mismatch")
    if kehto["digest"] != lock["corpus"]["kehto_fixture_digest"]:
        raise error("Kehto corpus digest mismatch")
    if published["digest"] != lock["corpus"]["published_fixture_digest"]:
        raise error("published corpus digest mismatch")
    if kehto["source"]["commit"] != lock["kehto"]["commit"]:
        raise error("Kehto corpus commit mismatch")
    if kehto["source"]["repository"] != lock["kehto"]["repository"]:
        raise error("Kehto corpus repository mismatch")
    if kehto["source"]["git_tree"] != lock["kehto"]["corpus_tree"]:
        raise error("Kehto corpus tree mismatch")

    for collection, base in (
        (reference["fixtures"], CONFORMANCE / "napplet-corpus" / "reference"),
        (published["fixtures"], CONFORMANCE / "napplet-corpus" / "published"),
    ):
        for fixture in collection:
            fixture_root = base / fixture["name"]
            for record in fixture["files"]:
                file = fixture_root / record["path"]
                if sha256_file(file) != record["sha256"]:
                    raise error(f"corpus file mismatch: {file}")
                if file.stat().st_size != record["bytes"]:
                    raise error(f"corpus byte count mismatch: {file}")
    if len(published["fixtures"]) < 1:
        raise error("published corpus must contain a real immutable artifact")
    return (
        len(reference["fixtures"]),
        len(kehto["applications"]),
        len(published["fixtures"]),
    )


def verify_falsifiers(
    lock: dict[str, Any],
    load_json: Callable[[str], Any],
    error: type[ValueError],
) -> int:
    feature = (CONFORMANCE / "bdd" / "core.feature").read_text(encoding="utf-8")
    matrix = load_json("conformance/bdd/falsifiers.json")
    if matrix.get("baseline") != lock["baseline"]["name"]:
        raise error("falsifier baseline mismatch")
    entries = matrix["entries"]
    invariants = {entry["invariant"] for entry in entries}
    expected = {f"I-{index:02d}" for index in range(1, 11)}
    if invariants != expected:
        raise error(f"falsifier coverage mismatch: {invariants ^ expected}")
    for entry in entries:
        if f"Scenario: {entry['scenario']}" not in feature:
            raise error(f"falsifier scenario missing: {entry['scenario']}")
        if entry["current_expected"] != "red":
            raise error("M0 falsifiers must honestly remain red")
        if not entry["falsifier"].strip():
            raise error("empty falsifier")
    return len(entries)
