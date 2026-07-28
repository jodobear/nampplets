#!/usr/bin/env python3
"""Generate pinned upstream snapshots and compatibility inventories.

Inputs must be clean git worktrees at the exact commits in compatibility.lock.
Only an allowlisted set of compatibility-authority files is copied.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"
LOCK_PATH = ROOT / "compatibility.lock"


def fail(message: str) -> "NoReturn":
    raise SystemExit(message)


def git(root: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def require_commit(root: Path, expected: str, label: str) -> None:
    actual = git(root, "rev-parse", "HEAD")
    if actual != expected:
        fail(f"{label}: expected {expected}, found {actual}")
    status = git(root, "status", "--short")
    if status:
        fail(f"{label}: source worktree is not clean")


def copy_exact(source_root: Path, relative: str, destination_root: Path) -> None:
    source = source_root / relative
    if not source.is_file():
        fail(f"missing upstream source: {source}")
    destination = destination_root / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def canonical_digest(value: Any) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")
    return sha256_bytes(encoded)


def envelope_inventory(web_root: Path, web_commit: str) -> dict[str, Any]:
    source_relative = "packages/conformance/src/validators/envelope.ts"
    source = (web_root / source_relative).read_text(encoding="utf-8")
    matches = re.findall(
        r"'([^']+)'\s*:\s*\{\s*dir\s*:\s*'(out|in)'", source, flags=re.MULTILINE
    )
    if len(matches) < 100:
        fail(f"unexpectedly small envelope inventory: {len(matches)}")

    entries: list[dict[str, str]] = []
    seen: set[str] = set()
    for message_type, direction in matches:
        if message_type in seen:
            fail(f"duplicate envelope type in pinned validator: {message_type}")
        seen.add(message_type)
        entries.append(
            {
                "type": message_type,
                "domain": message_type.split(".", 1)[0],
                "direction": "napplet-to-shell" if direction == "out" else "shell-to-napplet",
                "validator": "pinned-conformance",
                "runtime_support": "not-advertised-m0",
            }
        )

    # Registry drift is explicit rather than being silently folded into the
    # package validator.
    for message_type, direction in (
        ("shell.ready", "napplet-to-shell"),
        ("shell.init", "shell-to-napplet"),
    ):
        entries.append(
            {
                "type": message_type,
                "domain": "shell",
                "direction": direction,
                "validator": "registry-only-handshake",
                "runtime_support": "not-advertised-m0",
            }
        )

    # The pinned NAP-INC registry revision requires a symmetric target-side
    # channel-open carrier, but the pinned @napplet/nap and conformance
    # packages do not expose or validate it. Record the gap instead of making
    # an unsupported carrier appear package-compatible.
    entries.append(
        {
            "type": "inc.channel.opened",
            "domain": "inc",
            "direction": "shell-to-napplet",
            "validator": "explicit-unsupported",
            "runtime_support": "not-advertised-m0",
        }
    )

    entries.sort(key=lambda item: item["type"])
    return {
        "schema": 1,
        "source": {
            "repository": "napplet/web",
            "commit": web_commit,
            "file": source_relative,
            "sha256": sha256_bytes(source.encode("utf-8")),
        },
        "unknown_message_policy": "ignore",
        "entries": entries,
        "counts": {
            "total": len(entries),
            "pinned_conformance": len(matches),
            "registry_only_handshake": 2,
            "explicit_unsupported": 1,
        },
    }


def parse_requires(vite_config: str) -> list[str]:
    match = re.search(r"requires\s*:\s*\[([^\]]*)\]", vite_config)
    if not match:
        return []
    return re.findall(r"['\"]([a-z][a-z0-9-]*)['\"]", match.group(1))


def kehto_corpus(
    kehto_root: Path,
    repository: str,
    commit: str,
    corpus_tree: str,
) -> dict[str, Any]:
    base = "apps/playground/napplets"
    names = git(kehto_root, "ls-tree", "-d", "--name-only", f"HEAD:{base}").splitlines()
    applications: list[dict[str, Any]] = []

    for name in sorted(filter(None, names)):
        relative_root = f"{base}/{name}"
        tree = git(kehto_root, "rev-parse", f"HEAD:{relative_root}")
        file_names = git(
            kehto_root, "ls-tree", "-r", "--name-only", f"HEAD:{relative_root}"
        ).splitlines()
        files: list[dict[str, str]] = []
        for relative in sorted(filter(None, file_names)):
            repository_relative = f"{relative_root}/{relative}"
            blob = git(kehto_root, "rev-parse", f"HEAD:{repository_relative}")
            content = subprocess.run(
                ["git", "-C", str(kehto_root), "show", f"HEAD:{repository_relative}"],
                check=True,
                capture_output=True,
            ).stdout
            files.append(
                {
                    "path": relative,
                    "git_blob": blob,
                    "sha256": sha256_bytes(content),
                }
            )

        vite = (kehto_root / relative_root / "vite.config.ts").read_text(encoding="utf-8")
        applications.append(
            {
                "name": name,
                "git_tree": tree,
                "requires": parse_requires(vite),
                "files": files,
            }
        )

    result: dict[str, Any] = {
        "schema": 1,
        "source": {
            "repository": repository,
            "commit": commit,
            "path": base,
            "git_tree": corpus_tree,
        },
        "classification": "kehto-source-corpus",
        "artifact_obligation": False,
        "applications": applications,
    }
    result["digest"] = canonical_digest(result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--nip5d", type=Path, required=True)
    parser.add_argument("--naps", type=Path, required=True)
    parser.add_argument("--napplet-web", type=Path, required=True)
    parser.add_argument("--kehto", type=Path, required=True)
    arguments = parser.parse_args()

    with LOCK_PATH.open("rb") as handle:
        lock = tomllib.load(handle)

    require_commit(arguments.nip5d, lock["nip_5d"]["commit"], "NIP-5D")
    require_commit(arguments.naps, lock["nap_registry"]["commit"], "NAP registry")
    require_commit(
        arguments.napplet_web, lock["napplet_packages"]["commit"], "napplet/web"
    )
    require_commit(arguments.kehto, lock["kehto"]["commit"], "Kehto")

    vendor = CONFORMANCE / "vendor"
    nip_destination = vendor / "nip-5d"
    nap_destination = vendor / "nap-registry"
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

    for package in ("core", "shim", "sdk", "nap", "conformance"):
        copy_exact(
            arguments.napplet_web,
            f"packages/{package}/package.json",
            web_destination,
        )
    for relative in (
        "packages/core/src/envelope.ts",
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
