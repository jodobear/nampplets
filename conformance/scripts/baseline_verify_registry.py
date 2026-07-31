"""Lock-bound verification for the vendored NAP registry subset."""

from __future__ import annotations

import hashlib
import re
from pathlib import Path
from typing import Any


def git_object_oid(kind: str, content: bytes) -> str:
    header = f"{kind} {len(content)}\0".encode("ascii")
    return hashlib.sha1(header + content).hexdigest()


def git_blob_oid(file: Path) -> str:
    return git_object_oid("blob", file.read_bytes())


def git_tree_oid(directory: Path, error: type[ValueError]) -> str:
    entries = []
    for file in sorted(directory.iterdir(), key=lambda item: item.name.encode("utf-8")):
        if not file.is_file():
            raise error(f"nested authority tree entry is unsupported: {file}")
        entries.append(
            b"100644 "
            + file.name.encode("utf-8")
            + b"\0"
            + bytes.fromhex(git_blob_oid(file))
        )
    return git_object_oid("tree", b"".join(entries))


def verify_registry_snapshots(
    conformance: Path,
    registry: dict[str, Any],
    error: type[ValueError],
) -> None:
    object_ids = (
        registry["naps_tree"],
        registry["archetypes_blob"],
        registry["readme_blob"],
        registry["web_projection_blob"],
    )
    if any(not re.fullmatch(r"[0-9a-f]{40}", oid) for oid in object_ids):
        raise error("every NAP registry object must be exact 40-hex")

    root = conformance / "vendor" / "nap-registry"
    expected_files = {
        Path("ARCHETYPES.md"),
        Path("README.md"),
        Path("naps/NAP-IDENTITY.md"),
        Path("naps/NAP-INC.md"),
        Path("naps/NAP-INTENT.md"),
        Path("naps/NAP-SHELL.md"),
        Path("naps/NAP-THEME.md"),
        Path("projections/web.md"),
    }
    actual_files = {file.relative_to(root) for file in root.rglob("*") if file.is_file()}
    if actual_files != expected_files:
        raise error("NAP registry vendored file set drifted")

    blobs = {
        "ARCHETYPES.md": registry["archetypes_blob"],
        "README.md": registry["readme_blob"],
        "projections/web.md": registry["web_projection_blob"],
    }
    for relative, expected in blobs.items():
        if git_blob_oid(root / relative) != expected:
            raise error(f"NAP registry blob drifted: {relative}")
    if git_tree_oid(root / "naps", error) != registry["naps_tree"]:
        raise error("NAP registry naps tree drifted")
