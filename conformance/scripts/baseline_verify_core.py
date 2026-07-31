"""Core lock, digest, and envelope checks for the compatibility baseline."""

from __future__ import annotations

import hashlib
import re
from pathlib import Path
from typing import Any, Callable

import generate_digests
from baseline_verify_registry import verify_registry_snapshots


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"

NAP_DOMAINS = {
    "relay", "identity", "storage", "inc", "theme", "keys", "media",
    "notify", "config", "resource", "cvm", "outbox", "upload", "intent",
    "ble", "webrtc", "link", "count", "lists", "serial", "common", "dm",
}


class BaselineError(ValueError):
    """Raised when committed compatibility evidence is inconsistent."""


def sha256_file(file: Path) -> str:
    return hashlib.sha256(file.read_bytes()).hexdigest()


def verify_lock(lock: dict[str, Any]) -> None:
    if lock["baseline"]["schema"] != 1:
        raise BaselineError("unsupported lock schema")
    if lock["baseline"]["status"] not in {"unratified", "ratified"}:
        raise BaselineError("baseline status must be unratified or ratified")

    required_commits = (
        lock["nip_5d"]["commit"],
        lock["nap_registry"]["commit"],
        lock["napplet_packages"]["commit"],
        lock["nap_lists"]["semantic_commit"],
        lock["nap_lists"]["package_merge_commit"],
        lock["nap_lists"]["nip_51_commit"],
        lock["kehto"]["commit"],
        lock["nmp"]["commit"],
    )
    if any(not re.fullmatch(r"[0-9a-f]{40}", commit) for commit in required_commits):
        raise BaselineError("every upstream commit must be exact 40-hex")
    patch = lock["napplet_packages"]["local_patch"]
    if patch != "conformance/patches/napplet-web/compat-v2.patch":
        raise BaselineError("napplet/web compatibility patch path drifted")
    expected_patch_sha256 = lock["napplet_packages"]["local_patch_sha256"]
    if not re.fullmatch(r"[0-9a-f]{64}", expected_patch_sha256):
        raise BaselineError("napplet/web compatibility patch digest is invalid")
    if sha256_file(ROOT / patch) != expected_patch_sha256:
        raise BaselineError("napplet/web compatibility patch digest mismatch")

    source_snapshots = {
        "conformance/vendor/nip-5d/5D.md": lock["nip_5d"]["document_sha256"],
        "conformance/vendor/nap-lists/naps/NAP-LISTS.md": lock["nap_lists"][
            "document_sha256"
        ],
        "conformance/vendor/nip-51/51.md": lock["nap_lists"][
            "nip_51_document_sha256"
        ],
    }
    for relative, expected in source_snapshots.items():
        if sha256_file(ROOT / relative) != expected:
            raise BaselineError(f"authority snapshot drifted: {relative}")

    verify_registry_snapshots(CONFORMANCE, lock["nap_registry"], BaselineError)

    if lock["nip_5d"]["manifest_kinds"] != [5129, 15129, 35129]:
        raise BaselineError("manifest kind baseline drifted")
    if set(lock["artifacts"]["accepted_modes"]) != {"single-file", "external-assets"}:
        raise BaselineError("artifact modes do not match deliberate baseline")
    artifact_redirect_contract = {
        "redirect_policy": "manual-per-hop-revalidation",
        "maximum_redirect_hops": 5,
        "accepted_redirect_statuses": [301, 302, 303, 307, 308],
        "transport_auto_follow": False,
        "requires_https": True,
        "requires_credential_free_url": True,
        "requires_query_free_url": True,
        "requires_fragment_free_url": True,
        "requires_fresh_dns_each_hop": True,
        "requires_public_address_each_hop": True,
        "requires_address_pinned_connection": True,
        "requires_hostname_tls_sni": True,
        "allows_ambient_proxy": False,
        "requires_exact_effective_url_each_hop": True,
        "default_request_deadline_seconds": 15,
        "default_maximum_path_bytes": 8 * 1024 * 1024,
        "default_maximum_artifact_bytes": 32 * 1024 * 1024,
        "requires_typed_observable_refusal": True,
        "retention_execution_gate": "per-path-sha256-and-aggregate",
    }
    for field, expected in artifact_redirect_contract.items():
        if lock["artifacts"].get(field) != expected:
            raise BaselineError(f"artifact redirect contract drifted: {field}")
    if lock["web_projection"]["sandbox_tokens"] != ["allow-scripts"]:
        raise BaselineError("sandbox baseline must contain only allow-scripts")
    for required_true in (
        "forbid_allow_same_origin", "require_srcdoc", "require_source_window_binding",
        "forbid_window_nostr",
    ):
        if lock["web_projection"][required_true] is not True:
            raise BaselineError(f"web projection invariant disabled: {required_true}")
    if lock["web_projection"]["unknown_message_policy"] != "ignore":
        raise BaselineError("unknown message policy must be ignore")

    if set(lock["domain_versions"]) != NAP_DOMAINS | {"shell_handshake"}:
        raise BaselineError("domain version map is incomplete")
    for platform in ("macos", "ios", "android"):
        provider = lock["platform"][platform]
        supported = set(provider["supported_domains"])
        unsupported = set(provider["unsupported_domains"])
        if supported & unsupported:
            raise BaselineError(f"{platform}: supported/unsupported overlap")
        if supported | unsupported != NAP_DOMAINS:
            raise BaselineError(f"{platform}: provider matrix is incomplete")
        if supported:
            raise BaselineError(f"{platform}: M0 cannot advertise providers")

    profiles = lock.get("artifact_profiles", {})
    if set(profiles) != {"good_morning"}:
        raise BaselineError("exact-build artifact profile inventory drifted")
    good_morning = profiles["good_morning"]
    expected_identity = {
        "source": "published-immutable-artifacts",
        "author": (
            "266815e0c9210dfa324c6cba3573b14b"
            "ee49da4209a9456f9484e5106cd408a5"
        ),
        "d_tag": "good-morning",
        "aggregate_sha256": (
            "828a6df02afd56782ea20f805084acce6"
            "5c53f7c37554948c1e0a64aa5a2b0a8"
        ),
    }
    for field, expected in expected_identity.items():
        if good_morning.get(field) != expected:
            raise BaselineError(f"Good Morning artifact profile {field} drifted")
    required = good_morning.get("required_domains")
    optional = good_morning.get("optional_domains")
    if required != ["identity", "inc", "outbox"]:
        raise BaselineError("Good Morning required capability profile drifted")
    if optional != ["resource", "theme", "link"]:
        raise BaselineError("Good Morning optional capability profile drifted")
    if set(required) & set(optional):
        raise BaselineError("Good Morning capability classes overlap")
    if not set(required + optional) <= NAP_DOMAINS:
        raise BaselineError("Good Morning capability profile has an unknown domain")

    status = lock["baseline"]["status"]
    signoffs = lock["signoff"]
    if signoffs.get("product_owner") != "pablof7z":
        raise BaselineError("product-owner direction is not recorded")
    pending_signoffs = (
        signoffs.get("compatibility_reviewer"),
        signoffs.get("security_reviewer"),
        signoffs.get("nmp_boundary_reviewer"),
    )
    if status == "unratified" and any(pending_signoffs):
        raise BaselineError("unratified baseline must keep pending signoffs empty")
    if status == "ratified" and any(not value.strip() for value in signoffs.values()):
        raise BaselineError("ratified baseline requires every signoff")
    if any(value == "PENDING" for value in lock["corpus"].values()):
        raise BaselineError("corpus digests are not finalized")


def verify_digest_manifest() -> int:
    manifest = CONFORMANCE / "digests.sha256"
    expected_paths = {relative.as_posix() for relative in generate_digests.tracked_inputs()}
    observed_paths: set[str] = set()
    count = 0
    for line_number, line in enumerate(manifest.read_text(encoding="utf-8").splitlines(), 1):
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            raise BaselineError(f"invalid digest line {line_number}")
        expected, relative = match.groups()
        if relative in observed_paths:
            raise BaselineError(f"duplicate digest target: {relative}")
        observed_paths.add(relative)
        file = ROOT / relative
        if not file.is_file():
            raise BaselineError(f"digest target missing: {relative}")
        actual = sha256_file(file)
        if actual != expected:
            raise BaselineError(
                f"digest mismatch for {relative}: expected {expected}, found {actual}"
            )
        count += 1
    missing = expected_paths - observed_paths
    unexpected = observed_paths - expected_paths
    if missing or unexpected:
        raise BaselineError(
            "digest target set drifted: "
            f"missing={sorted(missing)}, unexpected={sorted(unexpected)}"
        )
    if manifest.read_bytes() != generate_digests.manifest_bytes():
        raise BaselineError("digest manifest is not in canonical order or encoding")
    if count < 20:
        raise BaselineError("digest manifest is unexpectedly small")
    return count


def verify_envelopes(lock: dict[str, Any], load_json: Callable[[str], Any]) -> int:
    inventory = load_json("conformance/envelopes/inventory.json")
    source = inventory["source"]
    if source["repository"] != lock["napplet_packages"]["repository"]:
        raise BaselineError("envelope inventory source repository mismatch")
    if source["commit"] != lock["napplet_packages"]["commit"]:
        raise BaselineError("envelope inventory source commit mismatch")
    expected_source = "packages/conformance/src/validators/envelope.ts"
    if source["file"] != expected_source:
        raise BaselineError("envelope inventory source file mismatch")
    source_relative = Path(source["file"])
    if source_relative.is_absolute() or ".." in source_relative.parts:
        raise BaselineError("envelope inventory source path is invalid")
    source_file = CONFORMANCE / "vendor" / "napplet-web" / source_relative
    if not source_file.is_file() or sha256_file(source_file) != source["sha256"]:
        raise BaselineError("envelope inventory source digest mismatch")
    if inventory["unknown_message_policy"] != "ignore":
        raise BaselineError("envelope unknown-message policy mismatch")
    entries = inventory["entries"]
    types = [entry["type"] for entry in entries]
    if len(types) != len(set(types)):
        raise BaselineError("duplicate envelope inventory type")
    domains = {entry["domain"] for entry in entries}
    if not NAP_DOMAINS <= domains:
        raise BaselineError(f"envelope inventory misses domains: {NAP_DOMAINS - domains}")
    handshake = {
        entry["type"]: entry["validator"]
        for entry in entries if entry["domain"] == "shell"
    }
    if handshake != {
        "shell.init": "registry-only-handshake",
        "shell.ready": "registry-only-handshake",
    }:
        raise BaselineError("NAP-SHELL drift records changed")
    unsupported = {
        entry["type"]: entry["validator"]
        for entry in entries if entry["validator"] == "explicit-unsupported"
    }
    if unsupported != {"inc.channel.opened": "explicit-unsupported"}:
        raise BaselineError("registry/package unsupported records changed")
    allowed = {"pinned-conformance", "registry-only-handshake", "explicit-unsupported"}
    if any(entry["validator"] not in allowed for entry in entries):
        raise BaselineError("envelope entry has no validator or unsupported record")
    return len(entries)
