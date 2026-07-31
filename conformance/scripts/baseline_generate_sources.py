"""Pinned-source helpers for compatibility baseline generation."""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any, NoReturn


def fail(message: str) -> NoReturn:
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


def apply_exact_patch(
    repository_root: Path,
    destination_root: Path,
    patch: Path,
    expected_sha256: str,
) -> None:
    if not patch.is_file():
        fail(f"missing compatibility patch: {patch}")
    actual_sha256 = sha256_bytes(patch.read_bytes())
    if actual_sha256 != expected_sha256:
        fail(
            "compatibility patch digest mismatch: "
            f"expected {expected_sha256}, found {actual_sha256}"
        )
    destination = destination_root.relative_to(repository_root).as_posix()
    command = [
        "git",
        "-C",
        str(repository_root),
        "apply",
        "--directory",
        destination,
        str(patch),
    ]
    check = [*command[:4], "--check", *command[4:]]
    subprocess.run(check, check=True)
    subprocess.run(command, check=True)


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
                "direction": (
                    "napplet-to-shell" if direction == "out" else "shell-to-napplet"
                ),
                "validator": "pinned-conformance",
                "runtime_support": "not-advertised-m0",
            }
        )

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

        vite = (kehto_root / relative_root / "vite.config.ts").read_text(
            encoding="utf-8"
        )
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
