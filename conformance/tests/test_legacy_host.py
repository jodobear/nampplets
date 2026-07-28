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
            "NAP-SHELL@5ac0490461ca",
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

    def test_kehto_runner_accepts_an_explicit_offline_dependency_store(self) -> None:
        source = (
            ROOT / "conformance" / "legacy-host" / "run_kehto.py"
        ).read_text(encoding="utf-8")
        self.assertIn('"--dependency-store"', source)
        self.assertIn('"--offline"', source)
        self.assertIn('"--frozen-lockfile"', source)
        self.assertIn('"./apps/playground/napplets/**"', source)

    def test_kehto_runner_clones_the_lock_repository_only(self) -> None:
        self.assertEqual(
            kehto.github_remote("jodobear/kehto-web"),
            "https://github.com/jodobear/kehto-web.git",
        )
        for invalid in ("kehto", "https://github.com/kehto/web", "kehto/web/extra"):
            with self.assertRaisesRegex(
                kehto.KehtoRunnerError,
                "invalid-github-repository",
            ):
                kehto.github_remote(invalid)

    def test_legacy_runner_accepts_an_explicit_playwright_module_root(self) -> None:
        source = (
            ROOT / "conformance" / "legacy-host" / "run.py"
        ).read_text(encoding="utf-8")
        self.assertIn('"--playwright-module-root"', source)
        self.assertIn('module_root / "playwright"', source)

    def test_verified_package_cache_refuses_wrong_digest_offline(self) -> None:
        pin = legacy.PackagePin(
            name="@napplet/conformance",
            version="0.14.0",
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

    def test_legacy_report_separates_package_naps_from_host_protocol(self) -> None:
        report = json.loads(
            (
                ROOT / "conformance" / "reports" / "legacy-host.json"
            ).read_text(encoding="utf-8")
        )
        package_domains = set(report["domain_contract"]["package_active_domains"])
        self.assertEqual(
            report["summary"]["pinned_conformance_engine"]["fail"],
            0,
        )
        for fixture in report["fixtures"]:
            host = fixture["host"]
            if not host.get("execution_observed"):
                continue
            package_emitted = host["package_emitted"]
            controls = host["host_control_or_probe_emitted"]
            self.assertEqual(len(package_emitted) + len(controls), len(host["emitted"]))
            for envelope in package_emitted:
                self.assertIn(envelope["type"].split(".", 1)[0], package_domains)
            for envelope in controls:
                self.assertNotIn(envelope["type"].split(".", 1)[0], package_domains)

    def test_kehto_report_proves_every_pinned_application_builds(self) -> None:
        report = json.loads(
            (
                ROOT / "conformance" / "reports" / "kehto-corpus.json"
            ).read_text(encoding="utf-8")
        )
        self.assertEqual(report["install"]["status"], "pass")
        self.assertEqual(report["counts"], {"built-not-run": 15})
        for application in report["applications"]:
            self.assertEqual(application["status"], "built-not-run")
            self.assertTrue(application["artifact"]["index_html"])
            self.assertGreater(application["artifact"]["file_count"], 0)
            self.assertGreater(application["artifact"]["total_bytes"], 0)


if __name__ == "__main__":
    unittest.main()
