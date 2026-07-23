from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def load_module(name: str, relative: str):
    path = ROOT / relative
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


legacy = load_module("legacy_host_run", "conformance/legacy-host/run.py")
kehto = load_module("kehto_corpus_run", "conformance/legacy-host/run_kehto.py")


class LegacyHostRunnerTests(unittest.TestCase):
    def test_package_domains_exclude_registry_only_shell(self) -> None:
        with (ROOT / "compatibility.lock").open("rb") as handle:
            import tomllib

            lock = tomllib.load(handle)
        package_domains = {
            domain
            for domain in lock["domain_versions"]
            if domain != "shell_handshake"
        }
        self.assertEqual(len(package_domains), 22)
        self.assertNotIn("shell", package_domains)
        self.assertEqual(
            lock["domain_versions"]["shell_handshake"],
            "NAP-SHELL@6461e4b37c29",
        )

    def test_reference_and_published_fixture_bytes_are_verified(self) -> None:
        for corpus in ("reference", "published"):
            index = json.loads(
                (
                    ROOT / "conformance" / "napplet-corpus" / corpus / "index.json"
                ).read_text(encoding="utf-8")
            )
            for fixture in index["fixtures"]:
                results = legacy.verify_fixture_bytes(
                    fixture,
                    ROOT
                    / "conformance"
                    / "napplet-corpus"
                    / corpus
                    / fixture["name"],
                )
                self.assertTrue(results)
                self.assertTrue(all(results.values()), fixture["name"])

    def test_offline_missing_tarball_is_not_run(self) -> None:
        process = {
            "status": "completed",
            "returncode": 1,
            "stdout": "",
            "stderr": (
                "ERR_PNPM_NO_OFFLINE_TARBALL A package is missing from the store "
                "but cannot download it in offline mode. The missing package may be "
                "downloaded from https://registry.npmjs.org/example/-/example-1.tgz."
            ),
        }
        reason = kehto.classify_install_failure(process)
        self.assertEqual(reason["code"], "offline-dependency-missing")
        self.assertEqual(
            reason["missing_tarball"],
            "https://registry.npmjs.org/example/-/example-1.tgz",
        )

    def test_verified_package_cache_refuses_wrong_digest_offline(self) -> None:
        pin = legacy.PackagePin(
            name="@napplet/conformance",
            version="0.13.0",
            sha256="0" * 64,
        )
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            (cache / pin.archive_name).write_bytes(b"not the package")
            with self.assertRaisesRegex(
                legacy.RunnerError, "verified-package-not-cached"
            ):
                legacy.fetch_exact(pin, cache, allow_download=False)

    def test_committed_reports_never_claim_green_incomplete_coverage(self) -> None:
        for name in ("legacy-host.json", "kehto-corpus.json"):
            path = ROOT / "conformance" / "reports" / name
            if not path.is_file():
                continue
            report = json.loads(path.read_text(encoding="utf-8"))
            if report["status"] != "pass":
                self.assertIn("claim", report)
                self.assertRegex(report["claim"], r"not|No ")

    def test_legacy_report_covers_every_committed_local_fixture(self) -> None:
        report_path = ROOT / "conformance" / "reports" / "legacy-host.json"
        if not report_path.is_file():
            self.skipTest("legacy-host report has not been generated")
        report = json.loads(report_path.read_text(encoding="utf-8"))
        expected: set[tuple[str, str]] = set()
        for corpus in ("reference", "published"):
            index = json.loads(
                (
                    ROOT / "conformance" / "napplet-corpus" / corpus / "index.json"
                ).read_text(encoding="utf-8")
            )
            expected.update((corpus, fixture["name"]) for fixture in index["fixtures"])
        actual = {
            (fixture["corpus"], fixture["name"]) for fixture in report["fixtures"]
        }
        self.assertEqual(actual, expected)
        self.assertTrue(
            all(fixture["bytes_unchanged"] for fixture in report["fixtures"])
        )

    def test_legacy_report_binds_the_executed_trusted_shell_bytes(self) -> None:
        report_path = ROOT / "conformance" / "reports" / "legacy-host.json"
        if not report_path.is_file():
            self.skipTest("legacy-host report has not been generated")
        report = json.loads(report_path.read_text(encoding="utf-8"))
        recorded = report["sources"]["trusted_shell_sha256"]
        shell_root = ROOT / "web" / "trusted-shell"
        self.assertEqual(
            recorded,
            {
                name: legacy.sha256_file(shell_root / name)
                for name in recorded
            },
        )


if __name__ == "__main__":
    unittest.main()
