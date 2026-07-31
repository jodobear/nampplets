#!/usr/bin/env python3
"""Generate pinned upstream snapshots and compatibility inventories.

Inputs must be clean git worktrees at the exact commits in compatibility.lock.
Only an allowlisted set of compatibility-authority files is copied.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tomllib
from pathlib import Path

from baseline_generate_sources import (
    copy_exact,
    envelope_inventory,
    kehto_corpus,
    require_commit,
)


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"
LOCK_PATH = ROOT / "compatibility.lock"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--nip5d", type=Path, required=True)
    parser.add_argument("--naps", type=Path, required=True)
    parser.add_argument("--nap-lists", type=Path, required=True)
    parser.add_argument("--nip51", type=Path, required=True)
    parser.add_argument("--napplet-web", type=Path, required=True)
    parser.add_argument("--kehto", type=Path, required=True)
    arguments = parser.parse_args()

    with LOCK_PATH.open("rb") as handle:
        lock = tomllib.load(handle)

    require_commit(arguments.nip5d, lock["nip_5d"]["commit"], "NIP-5D")
    require_commit(arguments.naps, lock["nap_registry"]["commit"], "NAP registry")
    require_commit(
        arguments.nap_lists,
        lock["nap_lists"]["semantic_commit"],
        "NAP-LISTS",
    )
    require_commit(arguments.nip51, lock["nap_lists"]["nip_51_commit"], "NIP-51")
    require_commit(
        arguments.napplet_web, lock["napplet_packages"]["commit"], "napplet/web"
    )
    require_commit(arguments.kehto, lock["kehto"]["commit"], "Kehto")

    vendor = CONFORMANCE / "vendor"
    nip_destination = vendor / "nip-5d"
    nap_destination = vendor / "nap-registry"
    nap_lists_destination = vendor / "nap-lists"
    nip51_destination = vendor / "nip-51"
    web_destination = vendor / "napplet-web"

    copy_exact(arguments.nip5d, "5D.md", nip_destination)
    for relative in (
        "README.md",
        "ARCHETYPES.md",
        "naps/NAP-IDENTITY.md",
        "naps/NAP-INC.md",
        "naps/NAP-INTENT.md",
        "naps/NAP-SHELL.md",
        "naps/NAP-THEME.md",
        "projections/web.md",
    ):
        copy_exact(arguments.naps, relative, nap_destination)

    copy_exact(arguments.nap_lists, "naps/NAP-LISTS.md", nap_lists_destination)
    copy_exact(arguments.nip51, "51.md", nip51_destination)

    for package in ("core", "shim", "sdk", "nap", "conformance"):
        copy_exact(
            arguments.napplet_web,
            f"packages/{package}/package.json",
            web_destination,
        )
    for relative in (
        "packages/core/src/envelope.ts",
        "packages/core/src/types/lists.ts",
        "packages/nap/src/lists/shim.ts",
        "packages/nap/src/lists/types.ts",
        "packages/shim/src/prelude.ts",
        "packages/conformance/src/validators/envelope.ts",
        "packages/conformance/src/validators/manifest.ts",
        "packages/conformance/src/checks/catalog.ts",
        "packages/conformance/src/shell/reference-shell.ts",
        "packages/conformance/src/run/boot.ts",
    ):
        copy_exact(arguments.napplet_web, relative, web_destination)

    inventory = envelope_inventory(
        arguments.napplet_web, lock["napplet_packages"]["commit"]
    )
    inventory_path = CONFORMANCE / "envelopes" / "inventory.json"
    inventory_path.parent.mkdir(parents=True, exist_ok=True)
    inventory_path.write_text(
        json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    corpus = kehto_corpus(
        arguments.kehto,
        lock["kehto"]["repository"],
        lock["kehto"]["commit"],
        lock["kehto"]["corpus_tree"],
    )
    corpus_path = CONFORMANCE / "napplet-corpus" / "kehto" / "index.json"
    corpus_path.parent.mkdir(parents=True, exist_ok=True)
    corpus_path.write_text(
        json.dumps(corpus, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    bundled_corpus_path = (
        ROOT
        / "apps"
        / "workbench-macos"
        / "RuntimeWorkbenchPackage"
        / "Sources"
        / "RuntimeWorkbenchFeature"
        / "Resources"
        / "Catalog"
        / "kehto-index.json"
    )
    bundled_corpus_path.write_text(
        json.dumps(corpus, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    feature_source = (
        ROOT
        / "nmp-native-runtime-spec-bundle"
        / "nmp-native-runtime-core-bdd.feature"
    )
    feature_destination = CONFORMANCE / "bdd" / "core.feature"
    feature_destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(feature_source, feature_destination)

    print(
        f"generated {len(inventory['entries'])} envelope records and "
        f"{len(corpus['applications'])} Kehto corpus entries"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
