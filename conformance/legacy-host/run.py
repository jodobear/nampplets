#!/usr/bin/env python3
"""Run committed legacy fixtures through the executable trusted host boundary."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from playwright_environment import resolve_playwright_environment


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"
LEGACY_HOST = CONFORMANCE / "legacy-host"
DEFAULT_REPORT = CONFORMANCE / "reports" / "legacy-host.json"
MAX_PACKAGE_BYTES = 8 * 1024 * 1024
MAX_PROCESS_OUTPUT_BYTES = 256 * 1024
PROCESS_TIMEOUT_SECONDS = 30
BROWSER_TIMEOUT_MS = 10_000


class RunnerError(RuntimeError):
    """An execution prerequisite or bounded runner contract failed."""


@dataclass(frozen=True)
class PackagePin:
    name: str
    version: str
    sha256: str

    @property
    def archive_name(self) -> str:
        return f"{self.name.rsplit('/', 1)[-1]}-{self.version}.tgz"

    @property
    def url(self) -> str:
        scope, package = self.name.split("/", 1)
        return (
            f"https://registry.npmjs.org/{scope}/{package}/-/"
            f"{package}-{self.version}.tgz"
        )


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def bounded_run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    timeout: int = PROCESS_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[bytes]:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise RunnerError(
            f"process-timeout:{timeout}s:{' '.join(command[:3])}"
        ) from error
    if len(result.stdout) > MAX_PROCESS_OUTPUT_BYTES:
        raise RunnerError("process-stdout-limit-exceeded")
    if len(result.stderr) > MAX_PROCESS_OUTPUT_BYTES:
        raise RunnerError("process-stderr-limit-exceeded")
    return result


def fetch_exact(pin: PackagePin, cache: Path, allow_download: bool) -> Path:
    cache.mkdir(parents=True, exist_ok=True)
    destination = cache / pin.archive_name
    if destination.is_file() and sha256_file(destination) == pin.sha256:
        return destination
    if destination.exists():
        destination.unlink()
    if not allow_download:
        raise RunnerError(f"verified-package-not-cached:{pin.name}@{pin.version}")

    request = urllib.request.Request(
        pin.url,
        headers={"User-Agent": "nmp-native-runtime-legacy-conformance/1"},
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            length = response.headers.get("Content-Length")
            if length and int(length) > MAX_PACKAGE_BYTES:
                raise RunnerError(f"package-size-refused:{pin.name}")
            content = response.read(MAX_PACKAGE_BYTES + 1)
    except OSError as error:
        raise RunnerError(f"package-download-failed:{pin.name}:{error}") from error
    if len(content) > MAX_PACKAGE_BYTES:
        raise RunnerError(f"package-size-refused:{pin.name}")
    actual = hashlib.sha256(content).hexdigest()
    if actual != pin.sha256:
        raise RunnerError(
            f"package-digest-mismatch:{pin.name}:expected={pin.sha256}:found={actual}"
        )
    destination.write_bytes(content)
    return destination


def extract_package(archive: Path, destination: Path) -> None:
    with tarfile.open(archive, "r:gz") as bundle:
        members = bundle.getmembers()
        if len(members) > 2_000:
            raise RunnerError(f"package-entry-limit-exceeded:{archive.name}")
        for member in members:
            if member.issym() or member.islnk():
                raise RunnerError(f"package-link-refused:{archive.name}:{member.name}")
            relative = Path(member.name)
            if relative.is_absolute() or ".." in relative.parts:
                raise RunnerError(f"package-path-refused:{archive.name}:{member.name}")
        bundle.extractall(destination, filter="data")


def package_environment(
    lock: dict[str, Any], cache: Path, allow_download: bool, root: Path
) -> Path:
    packages = lock["napplet_packages"]
    pins = (
        PackagePin(
            "@napplet/core",
            packages["core"],
            packages["npm_sha256"]["core"],
        ),
        PackagePin(
            "@napplet/nap",
            packages["nap"],
            packages["npm_sha256"]["nap"],
        ),
        PackagePin(
            "@napplet/conformance",
            packages["conformance"],
            packages["npm_sha256"]["conformance"],
        ),
    )
    for pin in pins:
        archive = fetch_exact(pin, cache, allow_download)
        staging = root / f"staging-{pin.name.rsplit('/', 1)[-1]}"
        staging.mkdir(parents=True)
        extract_package(archive, staging)
        source = staging / "package"
        destination = root / "node_modules" / "@napplet" / pin.name.rsplit("/", 1)[-1]
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(source, destination)
    return root / "node_modules" / "@napplet" / "conformance" / "dist" / "index.js"


def verify_fixture_bytes(
    fixture: dict[str, Any], fixture_root: Path
) -> dict[str, bool]:
    results: dict[str, bool] = {}
    for record in fixture["files"]:
        if "artifact_path" not in record:
            continue
        file = fixture_root / record["path"]
        results[record["artifact_path"]] = (
            file.is_file()
            and file.stat().st_size == record["bytes"]
            and sha256_file(file) == record["sha256"]
        )
    return results


def invoke_browser(
    *,
    engine: Path,
    fixture: dict[str, Any],
    fixture_root: Path,
    classification: str,
    manifest_event: dict[str, Any] | None,
    missing_domains: list[str],
    package_domains: list[str],
    conformance_version: str,
    environment: dict[str, str],
    browser_channel: str,
) -> dict[str, Any]:
    request = {
        "browserChannel": browser_channel,
        "classification": classification,
        "conformanceEntry": str(engine),
        "conformanceVersion": conformance_version,
        "fixtureRoot": str(fixture_root),
        "limits": {"browserTimeoutMs": BROWSER_TIMEOUT_MS},
        "manifestEvent": manifest_event,
        "name": fixture["name"],
        "packageActiveDomains": package_domains,
        "preflightReject": bool(missing_domains),
        "shellRoot": str(ROOT / "web" / "trusted-shell"),
        "verifiedArtifactPaths": sorted(
            record["artifact_path"]
            for record in fixture["files"]
            if "artifact_path" in record
        ),
    }
    process = subprocess.Popen(
        ["node", str(LEGACY_HOST / "browser-host.cjs")],
        cwd=ROOT,
        env=environment,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(
            input=json.dumps(request, separators=(",", ":")).encode("utf-8"),
            timeout=PROCESS_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        os.killpg(process.pid, 15)
        try:
            stdout, stderr = process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, 9)
            stdout, stderr = process.communicate()
        raise RunnerError(
            f"browser-host-timeout:{fixture['name']}:{PROCESS_TIMEOUT_SECONDS}s"
        ) from error
    result = subprocess.CompletedProcess(
        process.args,
        process.returncode,
        stdout,
        stderr,
    )
    if len(result.stdout) > MAX_PROCESS_OUTPUT_BYTES:
        raise RunnerError("browser-stdout-limit-exceeded")
    if len(result.stderr) > MAX_PROCESS_OUTPUT_BYTES:
        raise RunnerError("browser-stderr-limit-exceeded")
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise RunnerError(f"browser-host-failed:{detail[:2_000]}")
    return json.loads(result.stdout)


def fixture_records(
    lock: dict[str, Any],
    engine: Path,
    environment: dict[str, str],
    browser_channel: str,
) -> list[dict[str, Any]]:
    package_domains = sorted(
        domain
        for domain in lock["domain_versions"]
        if domain != "shell_handshake"
    )
    advertised = set(lock["platform"]["macos"]["supported_domains"])
    collections = (
        (
            "reference",
            "runtime-reference-fixtures",
            read_json(CONFORMANCE / "napplet-corpus" / "reference" / "index.json"),
        ),
        (
            "published",
            "published-immutable-artifacts",
            read_json(CONFORMANCE / "napplet-corpus" / "published" / "index.json"),
        ),
    )
    records: list[dict[str, Any]] = []
    for corpus_name, classification, index in collections:
        for fixture in index["fixtures"]:
            fixture_root = CONFORMANCE / "napplet-corpus" / corpus_name / fixture["name"]
            manifest_event = None
            requires = fixture.get("requires", [])
            if corpus_name == "published":
                manifest_event = read_json(fixture_root / "event.json")
                requires = [
                    tag[1]
                    for tag in manifest_event["tags"]
                    if len(tag) >= 2 and tag[0] == "requires"
                ]
            missing_domains = sorted(set(requires) - advertised)
            hashes = verify_fixture_bytes(fixture, fixture_root)
            record: dict[str, Any] = {
                "name": fixture["name"],
                "corpus": corpus_name,
                "artifact_mode": fixture["artifact_mode"],
                "requires": requires,
                "missing_required_domains": missing_domains,
                "verified_committed_bytes": hashes,
                "bytes_unchanged": bool(hashes) and all(hashes.values()),
            }
            if not record["bytes_unchanged"]:
                record["host"] = {
                    "status": "not-run",
                    "reason": "fixture-byte-verification-failed",
                }
            else:
                record["host"] = invoke_browser(
                    engine=engine,
                    fixture=fixture,
                    fixture_root=fixture_root,
                    classification=classification,
                    manifest_event=manifest_event,
                    missing_domains=missing_domains,
                    package_domains=package_domains,
                    conformance_version=lock["napplet_packages"]["conformance"],
                    environment=environment,
                    browser_channel=browser_channel,
                )
            records.append(record)
    return records


def summarize(records: list[dict[str, Any]]) -> dict[str, int]:
    summary = {"pass": 0, "fail": 0, "not-run": 0}
    for record in records:
        status = record["host"]["status"]
        summary[status] += 1
    return summary


def summarize_conformance(records: list[dict[str, Any]]) -> dict[str, int]:
    summary = {"pass": 0, "fail": 0, "not-run": 0}
    for record in records:
        status = record["host"].get("conformance", {}).get("status", "not-run")
        summary[status] += 1
    return summary


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--allow-package-download",
        action="store_true",
        help="download exact npm archives when they are absent from --package-cache",
    )
    parser.add_argument("--browser-channel", default="chrome")
    parser.add_argument("--package-cache", type=Path)
    parser.add_argument(
        "--playwright-module-root",
        type=Path,
        help="node_modules directory containing an exact Playwright installation",
    )
    parser.add_argument("--report", type=Path, default=DEFAULT_REPORT)
    arguments = parser.parse_args()

    with (ROOT / "compatibility.lock").open("rb") as handle:
        lock = tomllib.load(handle)

    with tempfile.TemporaryDirectory(prefix="nampplets-legacy-host-") as temporary:
        temporary_root = Path(temporary)
        cache = arguments.package_cache or (temporary_root / "package-cache")
        engine = package_environment(
            lock,
            cache,
            arguments.allow_package_download,
            temporary_root / "packages",
        )
        environment = resolve_playwright_environment(
            arguments.playwright_module_root,
            root=ROOT,
            bounded_run=bounded_run,
            error_type=RunnerError,
        )
        records = fixture_records(
            lock,
            engine,
            environment,
            arguments.browser_channel,
        )

    summary = summarize(records)
    conformance_summary = summarize_conformance(records)
    executed = [
        record for record in records if record["host"].get("execution_observed")
    ]
    ready_observed = all(
        record["host"].get("assertions", {}).get(
            "registry_shell_handshake_observed", False
        )
        for record in executed
    )
    init_exactly_once = all(
        record["host"].get("assertions", {}).get(
            "registry_shell_init_exactly_once", False
        )
        for record in executed
    )
    authoritative_supports = all(
        record["host"].get("assertions", {}).get(
            "registry_shell_supports_authoritative", False
        )
        for record in executed
    )
    shell_handshake_passes = bool(executed) and all(
        (ready_observed, init_exactly_once, authoritative_supports)
    )
    shell_handshake = {
        "authority": "registry-only",
        "types": ["shell.ready", "shell.init"],
        "observed_by_all_executed_fixtures": ready_observed,
        "init_exactly_once_for_all_executed_fixtures": init_exactly_once,
        "authoritative_supports_for_all_executed_fixtures": authoritative_supports,
        "status": "pass" if shell_handshake_passes else "fail",
        "reason": (
            None
            if shell_handshake_passes
            else "the bounded host did not complete and validate shell.ready/shell.init for every executed fixture"
        ),
    }
    report = {
        "schema": 1,
        "baseline": lock["baseline"]["name"],
        "baseline_status": lock["baseline"]["status"],
        "sources": {
            "napplet_web_commit": lock["napplet_packages"]["commit"],
            "conformance_package": {
                "name": "@napplet/conformance",
                "version": lock["napplet_packages"]["conformance"],
                "npm_sha256": lock["napplet_packages"]["npm_sha256"]["conformance"],
            },
            "package_dependency_archives": {
                "@napplet/core": {
                    "version": lock["napplet_packages"]["core"],
                    "npm_sha256": lock["napplet_packages"]["npm_sha256"]["core"],
                },
                "@napplet/nap": {
                    "version": lock["napplet_packages"]["nap"],
                    "npm_sha256": lock["napplet_packages"]["npm_sha256"]["nap"],
                },
            },
            "trusted_shell_sha256": {
                relative: sha256_file(ROOT / "web" / "trusted-shell" / relative)
                for relative in (
                    "trusted-shell.css",
                    "trusted-shell.html",
                    "trusted-shell-policy.js",
                    "trusted-shell-prelude-domains.js",
                    "trusted-shell.js",
                )
            },
            "kehto_commit": lock["kehto"]["commit"],
        },
        "limits": {
            "browser_timeout_ms": BROWSER_TIMEOUT_MS,
            "maximum_captured_envelopes": 256,
            "maximum_fixture_or_package_bytes": MAX_PACKAGE_BYTES,
            "maximum_process_output_bytes": MAX_PROCESS_OUTPUT_BYTES,
            "process_timeout_seconds": PROCESS_TIMEOUT_SECONDS,
        },
        "domain_contract": {
            "package_active_domains": sorted(
                domain
                for domain in lock["domain_versions"]
                if domain != "shell_handshake"
            ),
            "registry_only_shell_handshake": shell_handshake,
            "macos_advertised_domains": lock["platform"]["macos"][
                "supported_domains"
            ],
        },
        "fixtures": records,
        "summary": {
            "host": summary,
            "pinned_conformance_engine": conformance_summary,
        },
        "classification_findings": [
            {
                "code": "registry-control-and-host-probes-separated-from-package-traffic",
                "fixtures": sorted(
                    record["name"]
                    for record in records
                    if record["corpus"] == "reference"
                    and record["host"].get("host_control_or_probe_emitted")
                ),
                "effect": "The pinned package validator receives only package-active NAP envelopes. Registry-only shell control and explicit forward-compatibility probes remain visible in the host evidence and are judged by host assertions.",
            }
        ],
        "status": (
            "pass"
            if summary["fail"] == 0
            and summary["not-run"] == 0
            and conformance_summary["fail"] == 0
            and shell_handshake["status"] == "pass"
            else "incomplete"
        ),
        "claim": "This report does not claim M2 compatibility while any fixture fails, is not run, or the registry-only shell handshake is absent.",
    }
    arguments.report.parent.mkdir(parents=True, exist_ok=True)
    arguments.report.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(report["summary"], sort_keys=True))
    return 0 if report["status"] == "pass" else 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RunnerError, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"legacy host runner FAILED: {error}", file=sys.stderr)
        raise SystemExit(1)
