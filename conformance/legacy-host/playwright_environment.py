"""Resolve one explicit or system Playwright module root for legacy probes."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Callable


def resolve_playwright_environment(
    module_root: Path | None,
    *,
    root: Path,
    bounded_run: Callable[..., subprocess.CompletedProcess[bytes]],
    error_type: type[RuntimeError],
) -> dict[str, str]:
    if module_root is None:
        result = bounded_run(["npm", "root", "-g"], cwd=root)
        if result.returncode != 0:
            raise error_type("playwright-module-root-unavailable")
        module_root = Path(result.stdout.decode("utf-8").strip())
    else:
        module_root = module_root.resolve()
    if not (module_root / "playwright").is_dir():
        raise error_type("playwright-module-unavailable")
    environment = os.environ.copy()
    environment["NODE_PATH"] = str(module_root)
    return environment
