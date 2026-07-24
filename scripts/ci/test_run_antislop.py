#!/usr/bin/env python3
"""Focused tests for the tracked-source AntiSlop runner."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("run_antislop.py")
SPEC = importlib.util.spec_from_file_location("run_antislop", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class AntiSlopRunnerTests(unittest.TestCase):
    def make_repository(self, root: Path, files: list[str]) -> None:
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        for relative in files:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("source\n", encoding="utf-8")
        subprocess.run(["git", "add", "--", "."], cwd=root, check=True)

    def test_tracked_sources_cover_supported_extensions_and_exclusions(self) -> None:
        included = [
            "source.cc",
            "source.cjs",
            "source.cxx",
            "source.kts",
            "source.mjs",
            "source.rs",
        ]
        excluded = [
            "README.md",
            "conformance/vendor/source.rs",
            "conformance/napplet-corpus/source.js",
            "web/trusted-shell/fixtures/source.js",
            "Packages/NMPNativeRuntime/Sources/NMPNativeRuntime/"
            "NMPNativeRuntime.swift",
        ]
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            self.make_repository(repository, included + excluded)

            self.assertEqual(
                RUNNER.tracked_sources(repository),
                sorted(included),
            )

    def test_noncanonical_supported_extension_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            self.make_repository(repository, ["source.PY"])

            with self.assertRaisesRegex(
                RuntimeError,
                "tracked source extension must be lowercase: source.PY",
            ):
                RUNNER.tracked_sources(repository)

    def test_tracked_source_symlink_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            self.make_repository(repository, ["target.txt"])
            (repository / "linked.py").symlink_to("target.txt")
            subprocess.run(
                ["git", "add", "--", "linked.py"],
                cwd=repository,
                check=True,
            )

            with self.assertRaisesRegex(
                RuntimeError,
                "tracked source is not a regular file: linked.py",
            ):
                RUNNER.tracked_sources(repository)

    def test_verify_version_accepts_only_the_pinned_release(self) -> None:
        repository = Path("/repository")
        success = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=f"antislop {RUNNER.ANTISLOP_VERSION}\n",
        )
        with mock.patch.object(RUNNER.subprocess, "run", return_value=success):
            RUNNER.verify_version("/tools/antislop", repository)

        mismatch = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout="antislop 999.0.0\n",
        )
        with (
            mock.patch.object(RUNNER.subprocess, "run", return_value=mismatch),
            self.assertRaisesRegex(RuntimeError, "expected 'antislop 0.3.0'"),
        ):
            RUNNER.verify_version("/tools/antislop", repository)

    def test_scan_places_paths_after_option_boundary_and_propagates_status(self) -> None:
        result = subprocess.CompletedProcess(args=[], returncode=7)
        with mock.patch.object(RUNNER.subprocess, "run", return_value=result) as run:
            actual = RUNNER.scan(
                "/tools/antislop",
                Path("/repository"),
                "fixture",
                ["ordinary.py", "--config=untrusted.py"],
                "--disable",
                "stub",
            )

        self.assertEqual(actual, 7)
        command = run.call_args.args[0]
        boundary = command.index("--")
        self.assertEqual(
            command[boundary + 1 :],
            ["ordinary.py", "--config=untrusted.py"],
        )
        self.assertEqual(command[1], "--extensions")
        configured_extensions = set(command[2].split(","))
        self.assertTrue(
            {".cc", ".cjs", ".cxx", ".kts", ".mjs"}
            <= configured_extensions
        )
        self.assertEqual(run.call_args.kwargs["cwd"], Path("/repository"))
        self.assertFalse(run.call_args.kwargs["check"])

    def test_scan_refuses_an_empty_selection(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "no source files selected"):
            RUNNER.scan(
                "/tools/antislop",
                Path("/repository"),
                "empty",
                [],
            )

    def test_main_fails_if_either_scan_fails(self) -> None:
        arguments = argparse.Namespace(
            binary="/tools/antislop",
            repository=Path("/repository"),
        )
        sources = [
            "ordinary.py",
            next(iter(RUNNER.TRUSTED_SHELL_FILES)),
        ]
        with (
            mock.patch.object(RUNNER, "parse_args", return_value=arguments),
            mock.patch.object(RUNNER, "verify_version"),
            mock.patch.object(RUNNER, "tracked_sources", return_value=sources),
            mock.patch.object(RUNNER, "scan", side_effect=[3, 0]) as scan,
        ):
            self.assertEqual(RUNNER.main(), 1)
        self.assertEqual(scan.call_count, 2)


if __name__ == "__main__":
    unittest.main()
