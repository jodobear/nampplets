#!/usr/bin/env python3
"""Generate the immutable baseline file digest manifest.

Only Git-tracked files in the allowlisted conformance roots are inputs. This
keeps editor caches, test leftovers, filesystem enumeration order, locale, and
file metadata out of the committed evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
OUTPUT_RELATIVE = Path("conformance/digests.sha256")
OUTPUT = ROOT / OUTPUT_RELATIVE

INCLUDED_ROOTS = (
    Path("conformance/bdd"),
    Path("conformance/envelopes"),
    Path("conformance/legacy-host"),
    Path("conformance/napplet-corpus"),
    Path("conformance/reports"),
    Path("conformance/scripts"),
    Path("conformance/test-services"),
    Path("conformance/tests"),
    Path("conformance/vendor"),
)


class DigestGenerationError(ValueError):
    """Raised when the committed digest input set cannot be read exactly."""


def tracked_inputs(root: Path = ROOT) -> tuple[Path, ...]:
    """Return the canonical, repository-relative digest input paths."""
    try:
        result = subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "ls-files",
                "-z",
                "--cached",
                "--",
                *(path.as_posix() for path in INCLUDED_ROOTS),
            ],
            check=True,
            capture_output=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise DigestGenerationError(
            "digest generation requires a readable Git index"
        ) from error

    relative_paths = {
        Path(raw.decode("utf-8"))
        for raw in result.stdout.split(b"\0")
        if raw
    }
    relative_paths.discard(OUTPUT_RELATIVE)

    for relative in relative_paths:
        candidate = root / relative
        if candidate.is_symlink():
            raise DigestGenerationError(
                f"digest input must not be a symbolic link: {relative.as_posix()}"
            )
        if not candidate.is_file():
            raise DigestGenerationError(
                f"tracked digest input is missing: {relative.as_posix()}"
            )

    return tuple(sorted(relative_paths, key=lambda path: path.as_posix()))


def manifest_bytes(root: Path = ROOT) -> bytes:
    """Render canonical UTF-8 manifest bytes with a fixed LF terminator."""
    lines = []
    for relative in tracked_inputs(root):
        digest = hashlib.sha256((root / relative).read_bytes()).hexdigest()
        lines.append(f"{digest}  {relative.as_posix()}")
    return ("\n".join(lines) + "\n").encode("utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail without writing when the committed manifest is stale",
    )
    arguments = parser.parse_args()

    try:
        generated = manifest_bytes()
    except DigestGenerationError as error:
        print(f"digest generation FAILED: {error}", file=sys.stderr)
        return 1

    if arguments.check:
        try:
            current = OUTPUT.read_bytes()
        except OSError as error:
            print(f"digest manifest is unavailable: {error}", file=sys.stderr)
            return 1
        if current != generated:
            print(
                "digest manifest is stale; run "
                "python3 conformance/scripts/generate_digests.py",
                file=sys.stderr,
            )
            return 1
        print(
            f"verified {OUTPUT_RELATIVE.as_posix()} "
            f"with {len(generated.splitlines())} files"
        )
        return 0

    OUTPUT.write_bytes(generated)
    print(
        f"wrote {OUTPUT_RELATIVE.as_posix()} "
        f"with {len(generated.splitlines())} files"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
