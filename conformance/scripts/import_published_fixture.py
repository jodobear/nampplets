#!/usr/bin/env python3
"""Fetch one exact published fixture without redirects and verify its path hash."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        request: urllib.request.Request,
        file_pointer: Any,
        code: int,
        message: str,
        headers: Any,
        new_url: str,
    ) -> None:
        return None


def find_tag(event: dict[str, Any], name: str, value: str | None = None) -> list[str]:
    matches = [
        tag
        for tag in event.get("tags", [])
        if isinstance(tag, list)
        and len(tag) >= 2
        and tag[0] == name
        and (value is None or tag[1] == value)
    ]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {name!r} tag, found {len(matches)}")
    return matches[0]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("event", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--server")
    arguments = parser.parse_args()

    event = json.loads(arguments.event.read_text(encoding="utf-8"))
    path_tag = find_tag(event, "path", "/index.html")
    expected = path_tag[2]
    if not isinstance(expected, str) or len(expected) != 64:
        raise ValueError("index path tag does not contain a SHA-256")

    if arguments.server:
        server = arguments.server
    else:
        server = find_tag(event, "server")[1]
    url = f"{server.rstrip('/')}/{expected}"

    opener = urllib.request.build_opener(NoRedirect)
    request = urllib.request.Request(
        url,
        headers={"Accept": "text/html", "User-Agent": "nmp-native-runtime-baseline/1"},
    )
    try:
        with opener.open(request, timeout=20) as response:
            if response.status != 200:
                raise ValueError(f"unexpected status {response.status}")
            content = response.read(4 * 1024 * 1024 + 1)
    except urllib.error.HTTPError as error:
        raise ValueError(f"fixture fetch failed without redirects: {error}") from error

    if len(content) > 4 * 1024 * 1024:
        raise ValueError("published fixture exceeds 4 MiB import ceiling")
    actual = hashlib.sha256(content).hexdigest()
    if actual != expected:
        raise ValueError(f"path hash mismatch: expected {expected}, found {actual}")

    arguments.destination.parent.mkdir(parents=True, exist_ok=True)
    arguments.destination.write_bytes(content)
    print(f"wrote {arguments.destination} ({len(content)} bytes, sha256 {actual})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
