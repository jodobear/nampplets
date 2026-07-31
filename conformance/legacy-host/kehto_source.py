"""Acquire exact pinned Kehto sources from one validated GitHub remote."""

from __future__ import annotations

import re
import subprocess
import tarfile
from pathlib import Path
from typing import Any, Callable


GITHUB_REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")


class KehtoRunnerError(RuntimeError):
    """A pinned-source or bounded-process contract failed."""


def github_remote(repository: str) -> str:
    """Return bounded HTTPS clone URL for pinned GitHub repository."""
    if GITHUB_REPOSITORY.fullmatch(repository) is None:
        raise KehtoRunnerError("invalid-github-repository")
    return f"https://github.com/{repository}.git"


def acquire_source(
    *,
    source: Path | None,
    destination: Path,
    commit: str,
    remote: str,
    allow_network: bool,
    bounded_process: Callable[..., dict[str, Any]],
    git: Callable[..., str],
) -> Path:
    if source is None:
        if not allow_network:
            raise KehtoRunnerError(
                "exact Kehto source is absent; pass --source or explicitly allow --network"
            )
        result = bounded_process(
            [
                "git",
                "clone",
                "--filter=blob:none",
                "--no-checkout",
                remote,
                str(destination),
            ],
            cwd=destination.parent,
            timeout=120,
        )
        if result["status"] != "completed" or result["returncode"] != 0:
            raise KehtoRunnerError(
                f"Kehto clone failed: {result['status']}:{result['stderr'][:1000]}"
            )
        checkout = bounded_process(
            ["git", "checkout", "--detach", commit],
            cwd=destination,
            timeout=30,
        )
        if checkout["status"] != "completed" or checkout["returncode"] != 0:
            raise KehtoRunnerError(
                f"Kehto checkout failed: {checkout['status']}:{checkout['stderr'][:1000]}"
            )
        return destination

    source = source.resolve()
    if git(source, "rev-parse", f"{commit}^{{commit}}") != commit:
        raise KehtoRunnerError("provided source does not contain the pinned commit")
    archive = destination.parent / "kehto-pinned.tar"
    with archive.open("wb") as handle:
        result = subprocess.run(
            ["git", "-C", str(source), "archive", "--format=tar", commit],
            stdout=handle,
            stderr=subprocess.PIPE,
            timeout=60,
            check=False,
        )
    if result.returncode != 0:
        raise KehtoRunnerError(
            f"git archive failed: {result.stderr.decode(errors='replace')}"
        )
    destination.mkdir(parents=True)
    with tarfile.open(archive, "r:") as bundle:
        bundle.extractall(destination, filter="data")
    return destination
