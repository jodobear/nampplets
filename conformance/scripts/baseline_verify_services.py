"""Deterministic relay, blob, and signer scenario checks."""

from __future__ import annotations

import hashlib
import json
import re
from typing import Any, Callable


def verify_service_scenarios(
    load_json: Callable[[str], Any], error: type[ValueError]
) -> tuple[int, int, int]:
    scenarios = load_json("conformance/test-services/scenarios.json")
    events = load_json("conformance/test-services/events.json")
    if scenarios["clock"]["kind"] != "manual":
        raise error("test service clock must be manual")
    if scenarios["secrets"]["fixture_policy"] != "no-secret-keys":
        raise error("test service fixtures may not contain secret keys")
    serialized_events = json.dumps(events, sort_keys=True, separators=(",", ":")).lower()
    if "private_key" in serialized_events or "secret_key" in serialized_events:
        raise error("test service event fixtures may not contain secret keys")
    for alias, event in events.items():
        canonical = [
            0, event["pubkey"], event["created_at"], event["kind"],
            event["tags"], event["content"],
        ]
        encoded = json.dumps(
            canonical, ensure_ascii=False, separators=(",", ":")
        ).encode("utf-8")
        expected_id = hashlib.sha256(encoded).hexdigest()
        if event["id"] != expected_id:
            raise error(f"event fixture has invalid id: {alias}")
        if not re.fullmatch(r"[0-9a-f]{128}", event["sig"]):
            raise error(f"event fixture has invalid signature encoding: {alias}")

    relay_ids = {scenario["id"] for scenario in scenarios["relay"]}
    required_relays = {
        "eose-empty", "event-then-eose", "auth-required", "auth-denied",
        "relay-error", "disconnect-reconnect", "replacement", "deletion", "expiry",
    }
    if relay_ids != required_relays:
        raise error(f"relay scenario mismatch: {relay_ids ^ required_relays}")
    for scenario in scenarios["relay"]:
        for step in scenario["script"]:
            if step.startswith("event:") and step.removeprefix("event:") not in events:
                raise error(f"relay scenario references unknown event fixture: {step}")

    blob_by_id = {scenario["id"]: scenario for scenario in scenarios["blob"]}
    blob_ids = set(blob_by_id)
    required_blobs = {
        "verified-index", "one-byte-corrupt", "missing", "redirect-public-followed",
        "redirect-unsafe-target", "redirect-hop-limit",
        "redirect-effective-url-mismatch", "mime-mismatch", "oversized", "slow-stream",
    }
    if blob_ids != required_blobs:
        raise error(f"blob scenario mismatch: {blob_ids ^ required_blobs}")
    redirect_expectations = {
        "redirect-public-followed": {
            "status": 302,
            "request_url": "https://artifacts.example/index.html",
            "effective_url": "https://artifacts.example/index.html",
            "redirect_hop": 1,
            "mutation": "location:https://cdn.example/index.html",
            "expected_policy": "follow-after-manual-revalidation",
        },
        "redirect-unsafe-target": {
            "status": 302,
            "request_url": "https://artifacts.example/index.html",
            "effective_url": "https://artifacts.example/index.html",
            "redirect_hop": 1,
            "mutation": "location:https://127.0.0.1/index.html",
            "expected_policy": "typed-refusal-non-public-address",
        },
        "redirect-hop-limit": {
            "status": 308,
            "request_url": "https://hop-5.example/index.html",
            "effective_url": "https://hop-5.example/index.html",
            "redirect_hop": 6,
            "mutation": "location:https://hop-6.example/index.html",
            "expected_policy": "typed-refusal-hop-limit",
        },
        "redirect-effective-url-mismatch": {
            "status": 200,
            "request_url": "https://artifacts.example/index.html",
            "effective_url": "https://confused.example/index.html",
            "redirect_hop": 0,
            "mutation": "none",
            "expected_policy": "typed-refusal-effective-url",
        },
    }
    for scenario_id, expected in redirect_expectations.items():
        scenario = blob_by_id[scenario_id]
        if scenario.get("response_role") != "raw-hop-response":
            raise error(f"blob scenario {scenario_id} must remain a raw hop response")
        for field, value in expected.items():
            if scenario.get(field) != value:
                raise error(f"blob scenario {scenario_id} contract drifted: {field}")

    signer_results = {scenario["result"] for scenario in scenarios["signer"]}
    if signer_results != {"approved", "rejected", "invalid", "unavailable"}:
        raise error("signer result coverage is incomplete")
    for scenario in scenarios["signer"]:
        fixture = scenario["response_fixture"]
        if fixture is not None and fixture not in events:
            raise error(f"signer scenario references unknown event fixture: {fixture}")
    return len(relay_ids), len(blob_ids), len(scenarios["signer"])
