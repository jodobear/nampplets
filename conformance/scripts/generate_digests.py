#!/usr/bin/env python3
"""Generate the immutable baseline file digest manifest."""

from __future__ import annotations

import hashlib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"
OUTPUT = CONFORMANCE / "digests.sha256"

INCLUDED_ROOTS = (
    CONFORMANCE / "bdd",
    CONFORMANCE / "envelopes",
    CONFORMANCE / "legacy-host",
    CONFORMANCE / "napplet-corpus",
    CONFORMANCE / "reports",
    CONFORMANCE / "scripts",
    CONFORMANCE / "test-services",
    CONFORMANCE / "tests",
    CONFORMANCE / "vendor",
)


def main() -> int:
    files: list[Path] = []
    for included_root in INCLUDED_ROOTS:
        if included_root.exists():
            files.extend(
                candidate
                for candidate in included_root.rglob("*")
                if candidate.is_file()
                and "__pycache__" not in candidate.parts
                and candidate.suffix != ".pyc"
            )
    files = sorted(files)
    lines = [
        f"{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.relative_to(ROOT)}"
        for file in files
    ]
    OUTPUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {OUTPUT.relative_to(ROOT)} with {len(lines)} files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
