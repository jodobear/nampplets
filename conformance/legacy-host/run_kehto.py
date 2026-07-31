#!/usr/bin/env python3
"""Reproduce the exact-commit Kehto source-corpus build with bounded resources."""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any

from kehto_source import KehtoRunnerError, acquire_source, github_remote


ROOT = Path(__file__).resolve().parents[2]
CONFORMANCE = ROOT / "conformance"
DEFAULT_REPORT = CONFORMANCE / "reports" / "kehto-corpus.json"
INSTALL_TIMEOUT_SECONDS = 120
BUILD_TIMEOUT_SECONDS = 60
MAX_PROCESS_OUTPUT_BYTES = 256 * 1024
MAX_ARTIFACT_FILES = 512
MAX_ARTIFACT_BYTES = 32 * 1024 * 1024
PACKAGE_MANAGER = "pnpm@10.8.0"
def bounded_process(
    command: list[str],
    *,
    cwd: Path,
    timeout: int,
) -> dict[str, Any]:
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            stdout, stderr = process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            stdout, stderr = process.communicate()
        return {
            "status": "timeout",
            "returncode": None,
            "stdout": stdout[:MAX_PROCESS_OUTPUT_BYTES].decode(
                "utf-8", errors="replace"
            ),
            "stderr": stderr[:MAX_PROCESS_OUTPUT_BYTES].decode(
                "utf-8", errors="replace"
            ),
        }
    if len(stdout) > MAX_PROCESS_OUTPUT_BYTES or len(stderr) > MAX_PROCESS_OUTPUT_BYTES:
        return {
            "status": "output-limit",
            "returncode": process.returncode,
            "stdout": stdout[:MAX_PROCESS_OUTPUT_BYTES].decode(
                "utf-8", errors="replace"
            ),
            "stderr": stderr[:MAX_PROCESS_OUTPUT_BYTES].decode(
                "utf-8", errors="replace"
            ),
        }
    return {
        "status": "completed",
        "returncode": process.returncode,
        "stdout": stdout.decode("utf-8", errors="replace"),
        "stderr": stderr.decode("utf-8", errors="replace"),
    }


def git(source: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(source), *arguments],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        raise KehtoRunnerError(
            f"git {' '.join(arguments)} failed: {result.stderr.strip()}"
        )
    return result.stdout.strip()


def classify_install_failure(process: dict[str, Any]) -> dict[str, Any]:
    combined = f"{process['stdout']}\n{process['stderr']}"
    missing = re.search(
        r"missing package may be downloaded from (https://[^\s]+?\.tgz)",
        combined,
    )
    if "ERR_PNPM_NO_OFFLINE_TARBALL" in combined:
        return {
            "code": "offline-dependency-missing",
            "missing_tarball": missing.group(1) if missing else None,
        }
    if process["status"] == "timeout":
        return {"code": "dependency-install-timeout"}
    if process["status"] == "output-limit":
        return {"code": "dependency-install-output-limit"}
    return {
        "code": "dependency-install-failed",
        "returncode": process["returncode"],
    }


def artifact_inventory(dist: Path) -> dict[str, Any]:
    if not (dist / "index.html").is_file():
        raise KehtoRunnerError("build produced no dist/index.html")
    files = sorted(candidate for candidate in dist.rglob("*") if candidate.is_file())
    if len(files) > MAX_ARTIFACT_FILES:
        raise KehtoRunnerError("artifact-file-limit-exceeded")
    total = sum(file.stat().st_size for file in files)
    if total > MAX_ARTIFACT_BYTES:
        raise KehtoRunnerError("artifact-byte-limit-exceeded")
    return {
        "file_count": len(files),
        "total_bytes": total,
        "index_html": True,
    }


def verify_source_trees(
    source: Path,
    *,
    commit: str,
    corpus_path: str,
    corpus_tree: str,
    applications: list[dict[str, Any]],
) -> None:
    actual_corpus_tree = git(source, "rev-parse", f"{commit}:{corpus_path}")
    if actual_corpus_tree != corpus_tree:
        raise KehtoRunnerError(
            f"Kehto corpus tree mismatch: expected {corpus_tree}, found {actual_corpus_tree}"
        )
    for application in applications:
        actual_tree = git(
            source,
            "rev-parse",
            f"{commit}:{corpus_path}/{application['name']}",
        )
        if actual_tree != application["source_tree"]:
            raise KehtoRunnerError(
                f"{application['name']}: expected tree "
                f"{application['source_tree']}, found {actual_tree}"
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--network",
        action="store_true",
        help="allow only the exact-commit git clone; dependency install remains offline",
    )
    parser.add_argument("--report", type=Path, default=DEFAULT_REPORT)
    parser.add_argument("--source", type=Path)
    parser.add_argument(
        "--dependency-store",
        type=Path,
        help="pnpm content-addressed store already populated from the pinned lockfile",
    )
    arguments = parser.parse_args()

    with (ROOT / "compatibility.lock").open("rb") as handle:
        lock = tomllib.load(handle)
    index = json.loads(
        (CONFORMANCE / "napplet-corpus" / "kehto" / "index.json").read_text(
            encoding="utf-8"
        )
    )
    commit = lock["kehto"]["commit"]
    repository = lock["kehto"]["repository"]
    if index["source"]["commit"] != commit:
        raise KehtoRunnerError("Kehto index/lock commit mismatch")
    if index["source"]["repository"] != repository:
        raise KehtoRunnerError("Kehto index/lock repository mismatch")

    applications = [
        {
            "name": item["name"],
            "requires": item["requires"],
            "source_tree": item["git_tree"],
            "status": "pending",
        }
        for item in index["applications"]
    ]
    with tempfile.TemporaryDirectory(prefix="nampplets-kehto-run-") as temporary:
        temporary_root = Path(temporary)
        if arguments.source is not None:
            verify_source_trees(
                arguments.source.resolve(),
                commit=commit,
                corpus_path=lock["kehto"]["corpus_path"],
                corpus_tree=lock["kehto"]["corpus_tree"],
                applications=applications,
            )
        checkout = acquire_source(
            source=arguments.source,
            destination=temporary_root / "kehto",
            commit=commit,
            remote=github_remote(repository),
            allow_network=arguments.network,
            bounded_process=bounded_process,
            git=git,
        )
        if arguments.source is None:
            verify_source_trees(
                checkout,
                commit=commit,
                corpus_path=lock["kehto"]["corpus_path"],
                corpus_tree=lock["kehto"]["corpus_tree"],
                applications=applications,
            )
        install_command = [
            "corepack",
            PACKAGE_MANAGER,
            "install",
            "--filter",
            "./apps/playground/napplets/**",
            "--offline",
            "--frozen-lockfile",
            "--ignore-scripts",
        ]
        if arguments.dependency_store is not None:
            install_command.extend(
                ["--store-dir", str(arguments.dependency_store.resolve())]
            )
        install = bounded_process(
            install_command,
            cwd=checkout,
            timeout=INSTALL_TIMEOUT_SECONDS,
        )
        install_ok = install["status"] == "completed" and install["returncode"] == 0
        if not install_ok:
            reason = classify_install_failure(install)
            for application in applications:
                application["status"] = "not-run"
                application["reason"] = reason
        else:
            for application in applications:
                package_file = (
                    checkout
                    / "apps"
                    / "playground"
                    / "napplets"
                    / application["name"]
                    / "package.json"
                )
                package = json.loads(package_file.read_text(encoding="utf-8"))
                build = bounded_process(
                    [
                        "corepack",
                        PACKAGE_MANAGER,
                        "--filter",
                        package["name"],
                        "build",
                    ],
                    cwd=checkout,
                    timeout=BUILD_TIMEOUT_SECONDS,
                )
                if build["status"] != "completed" or build["returncode"] != 0:
                    application["status"] = "fail"
                    application["reason"] = {
                        "code": (
                            "build-timeout"
                            if build["status"] == "timeout"
                            else "build-failed"
                        ),
                        "returncode": build["returncode"],
                        "stderr": build["stderr"][:2_000],
                    }
                    continue
                try:
                    application["artifact"] = artifact_inventory(
                        package_file.parent / "dist"
                    )
                except KehtoRunnerError as error:
                    application["status"] = "fail"
                    application["reason"] = {"code": str(error)}
                    continue
                application["status"] = "built-not-run"
                application["reason"] = {
                    "code": "native-provider-preflight-blocked",
                    "missing_required_domains": application["requires"],
                    "note": "macOS advertises no package-active NAP domains in this baseline",
                }

    counts = {
        status: sum(1 for application in applications if application["status"] == status)
        for status in sorted({application["status"] for application in applications})
    }
    report = {
        "schema": 1,
        "baseline": lock["baseline"]["name"],
        "source": {
            "repository": lock["kehto"]["repository"],
            "commit": commit,
            "corpus_tree": lock["kehto"]["corpus_tree"],
            "exact_source_trees_verified": True,
        },
        "package_manager": PACKAGE_MANAGER,
        "dependency_mode": "offline-frozen-lockfile",
        "dependency_store": (
            "explicit-content-addressed-store"
            if arguments.dependency_store is not None
            else "default-pnpm-store"
        ),
        "limits": {
            "install_timeout_seconds": INSTALL_TIMEOUT_SECONDS,
            "build_timeout_seconds_per_application": BUILD_TIMEOUT_SECONDS,
            "maximum_process_output_bytes": MAX_PROCESS_OUTPUT_BYTES,
            "maximum_artifact_files": MAX_ARTIFACT_FILES,
            "maximum_artifact_bytes": MAX_ARTIFACT_BYTES,
        },
        "install": {
            "status": "pass" if install_ok else "not-run",
            "reason": None if install_ok else classify_install_failure(install),
        },
        "applications": applications,
        "counts": counts,
        "status": (
            "pass"
            if applications
            and all(application["status"] == "pass" for application in applications)
            else "incomplete"
        ),
        "claim": "A build or preflight result is not a native-host boot result. No Kehto application is green unless its unchanged built bytes execute through the native host.",
    }
    arguments.report.parent.mkdir(parents=True, exist_ok=True)
    arguments.report.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(counts, sort_keys=True))
    return 0 if report["status"] == "pass" else 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KehtoRunnerError, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Kehto corpus runner FAILED: {error}", file=sys.stderr)
        raise SystemExit(1)
