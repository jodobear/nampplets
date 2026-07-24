#!/usr/bin/env python3
"""Run AntiSlop against tracked, first-party source files."""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys


ANTISLOP_VERSION = "0.3.0"
SOURCE_EXTENSIONS = frozenset(
    {
        ".bash",
        ".c",
        ".cpp",
        ".cs",
        ".dart",
        ".fish",
        ".go",
        ".h",
        ".hpp",
        ".java",
        ".js",
        ".jsx",
        ".kt",
        ".php",
        ".py",
        ".rb",
        ".rs",
        ".sh",
        ".swift",
        ".ts",
        ".tsx",
        ".zsh",
    }
)

# AntiSlop 0.3.0 parses exclude globs but does not apply them in its walker.
# Select tracked source explicitly so generated and third-party compatibility
# inputs cannot create findings that this repository does not own.
EXCLUDED_PREFIXES = (
    "conformance/napplet-corpus/",
    "conformance/vendor/",
    "platforms/apple/Sources/NMPNativeRuntimeApple/Resources/TrustedShell/fixtures/",
    "web/trusted-shell/fixtures/",
)
EXCLUDED_FILES = frozenset(
    {
        "Packages/NMPNativeRuntime/Sources/NMPNativeRuntime/NMPNativeRuntime.swift",
    }
)

# The 0.3.0 JavaScript AST heuristic treats every `return null` as a stub.
# These byte-identical trusted-shell sources use null as a bounded protocol
# result and return a generated compatibility prelude. Keep all non-stub
# AntiSlop categories active for them.
TRUSTED_SHELL_FILES = frozenset(
    {
        "platforms/apple/Sources/NMPNativeRuntimeApple/Resources/TrustedShell/trusted-shell.js",
        "web/trusted-shell/trusted-shell.js",
    }
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary",
        default="antislop",
        help="AntiSlop executable (default: antislop from PATH)",
    )
    parser.add_argument(
        "--repository",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="repository root",
    )
    return parser.parse_args()


def tracked_sources(repository: Path) -> list[str]:
    result = subprocess.run(
        ["git", "-C", str(repository), "ls-files", "-z"],
        check=True,
        capture_output=True,
    )
    paths = result.stdout.decode("utf-8").split("\0")
    return [
        path
        for path in paths
        if path
        and Path(path).suffix in SOURCE_EXTENSIONS
        and path not in EXCLUDED_FILES
        and not path.startswith(EXCLUDED_PREFIXES)
    ]


def verify_version(binary: str, repository: Path) -> None:
    result = subprocess.run(
        [binary, "--version"],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
    )
    actual = result.stdout.strip()
    expected = f"antislop {ANTISLOP_VERSION}"
    if actual != expected:
        raise RuntimeError(f"expected {expected!r}, got {actual!r}")


def scan(
    binary: str,
    repository: Path,
    label: str,
    paths: list[str],
    *options: str,
) -> int:
    if not paths:
        raise RuntimeError(f"no source files selected for {label}")

    print(f"AntiSlop: scanning {len(paths)} {label} files", flush=True)
    result = subprocess.run(
        [binary, *options, *paths],
        cwd=repository,
        check=False,
    )
    return result.returncode


def main() -> int:
    args = parse_args()
    repository = args.repository.resolve()
    verify_version(args.binary, repository)

    sources = tracked_sources(repository)
    trusted_shell = sorted(path for path in sources if path in TRUSTED_SHELL_FILES)
    regular = sorted(path for path in sources if path not in TRUSTED_SHELL_FILES)

    regular_status = scan(args.binary, repository, "first-party", regular)
    shell_status = scan(
        args.binary,
        repository,
        "trusted-shell",
        trusted_shell,
        "--disable",
        "stub",
    )
    return 1 if regular_status or shell_status else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"AntiSlop runner failed: {error}", file=sys.stderr)
        sys.exit(2)
