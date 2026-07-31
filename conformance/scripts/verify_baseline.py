#!/usr/bin/env python3
"""Offline verifier for the committed M0 compatibility baseline."""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path
from typing import Any

from baseline_verify_core import (
    BaselineError,
    NAP_DOMAINS,
    sha256_file,
    verify_digest_manifest,
    verify_envelopes as _verify_envelopes,
    verify_lock,
)
from baseline_verify_evidence import (
    verify_corpus as _verify_corpus,
    verify_falsifiers as _verify_falsifiers,
    verify_upgrade_report as _verify_upgrade_report,
)
from baseline_verify_services import verify_service_scenarios as _verify_services


ROOT = Path(__file__).resolve().parents[2]


def load_json(relative: str) -> Any:
    return json.loads((ROOT / relative).read_text(encoding="utf-8"))


def load_lock() -> dict[str, Any]:
    with (ROOT / "compatibility.lock").open("rb") as handle:
        return tomllib.load(handle)


def verify_envelopes(lock: dict[str, Any]) -> int:
    return _verify_envelopes(lock, load_json)


def verify_upgrade_report(lock: dict[str, Any]) -> int:
    return _verify_upgrade_report(lock, load_json, BaselineError)


def verify_corpus(lock: dict[str, Any]) -> tuple[int, int, int]:
    return _verify_corpus(lock, load_json, sha256_file, BaselineError)


def verify_falsifiers(lock: dict[str, Any]) -> int:
    return _verify_falsifiers(lock, load_json, BaselineError)


def verify_service_scenarios() -> tuple[int, int, int]:
    return _verify_services(load_json, BaselineError)


def verify() -> dict[str, Any]:
    lock = load_lock()
    verify_lock(lock)
    files = verify_digest_manifest()
    envelopes = verify_envelopes(lock)
    upgrade = verify_upgrade_report(lock)
    reference, kehto, published = verify_corpus(lock)
    falsifiers = verify_falsifiers(lock)
    relay, blob, signer = verify_service_scenarios()
    return {
        "baseline": lock["baseline"]["name"],
        "status": lock["baseline"]["status"],
        "verified_files": files,
        "envelopes": envelopes,
        "upgrade_behaviors": upgrade,
        "corpus": {
            "reference": reference,
            "kehto": kehto,
            "published": published,
        },
        "falsifiers": falsifiers,
        "service_scenarios": {"relay": relay, "blob": blob, "signer": signer},
    }


def main() -> int:
    try:
        result = verify()
    except (
        BaselineError,
        KeyError,
        OSError,
        json.JSONDecodeError,
        tomllib.TOMLDecodeError,
    ) as error:
        print(f"compatibility baseline FAILED: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
