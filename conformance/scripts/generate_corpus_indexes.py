#!/usr/bin/env python3
"""Generate immutable indexes for committed reference and published corpora."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "conformance" / "napplet-corpus"

REFERENCE_REQUIREMENTS = {
    "external-assets": [],
    "missing-domain": ["ble"],
    "prelude-order": [],
    "unknown-message": [],
}

REFERENCE_MODES = {
    "external-assets": "external-assets",
    "missing-domain": "single-file",
    "prelude-order": "single-file",
    "unknown-message": "single-file",
}


def sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def canonical_digest(value: Any) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")
    return sha256(encoded)


def aggregate(entries: list[dict[str, str]]) -> str:
    lines = sorted(f"{entry['sha256']} {entry['artifact_path']}\n" for entry in entries)
    return sha256("".join(lines).encode("utf-8"))


def indexed_files(root: Path) -> list[dict[str, str | int]]:
    files: list[dict[str, str | int]] = []
    for file in sorted(candidate for candidate in root.rglob("*") if candidate.is_file()):
        content = file.read_bytes()
        relative = file.relative_to(root).as_posix()
        files.append(
            {
                "path": relative,
                "artifact_path": f"/{relative}",
                "bytes": len(content),
                "sha256": sha256(content),
            }
        )
    return files


def reference_index() -> dict[str, Any]:
    root = CORPUS / "reference"
    fixtures: list[dict[str, Any]] = []
    for name in sorted(REFERENCE_REQUIREMENTS):
        fixture_root = root / name
        files = indexed_files(fixture_root)
        fixtures.append(
            {
                "name": name,
                "artifact_mode": REFERENCE_MODES[name],
                "requires": REFERENCE_REQUIREMENTS[name],
                "aggregate_sha256": aggregate(files),
                "files": files,
            }
        )
    result: dict[str, Any] = {
        "schema": 1,
        "classification": "runtime-reference-fixtures",
        "fixtures": fixtures,
    }
    result["digest"] = canonical_digest(result)
    return result


def event_id(event: dict[str, Any]) -> str:
    serial = [0, event["pubkey"], event["created_at"], event["kind"], event["tags"], event["content"]]
    encoded = json.dumps(serial, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    return sha256(encoded)


def published_index() -> dict[str, Any]:
    root = CORPUS / "published"
    fixtures: list[dict[str, Any]] = []
    for fixture_root in sorted(candidate for candidate in root.iterdir() if candidate.is_dir()):
        event_file = fixture_root / "event.json"
        index_file = fixture_root / "index.html"
        event = json.loads(event_file.read_text(encoding="utf-8"))
        if event_id(event) != event["id"]:
            raise ValueError(f"{fixture_root.name}: event id does not match canonical serialization")
        if not isinstance(event.get("sig"), str) or len(event["sig"]) != 128:
            raise ValueError(f"{fixture_root.name}: signature is not 64-byte hex")

        path_tags = [
            tag
            for tag in event["tags"]
            if isinstance(tag, list) and len(tag) >= 3 and tag[0] == "path"
        ]
        if len(path_tags) != 1 or path_tags[0][1] != "/index.html":
            raise ValueError(f"{fixture_root.name}: expected one /index.html path")
        index_hash = sha256(index_file.read_bytes())
        if index_hash != path_tags[0][2]:
            raise ValueError(f"{fixture_root.name}: committed index bytes do not match event")

        entries = [{"artifact_path": tag[1], "sha256": tag[2]} for tag in path_tags]
        computed_aggregate = aggregate(entries)
        x_tags = [
            tag
            for tag in event["tags"]
            if isinstance(tag, list)
            and len(tag) >= 3
            and tag[0] == "x"
            and tag[2] == "aggregate"
        ]
        if len(x_tags) != 1 or x_tags[0][1] != computed_aggregate:
            raise ValueError(f"{fixture_root.name}: aggregate mismatch")

        d_tags = [tag for tag in event["tags"] if tag[0] == "d"]
        fixtures.append(
            {
                "name": fixture_root.name,
                "coordinate": {
                    "kind": event["kind"],
                    "author": event["pubkey"],
                    "d_tag": d_tags[0][1],
                },
                "event_id": event["id"],
                "aggregate_sha256": computed_aggregate,
                "artifact_mode": "single-file",
                "files": [
                    {
                        "path": "event.json",
                        "bytes": event_file.stat().st_size,
                        "sha256": sha256(event_file.read_bytes()),
                    },
                    {
                        "path": "index.html",
                        "artifact_path": "/index.html",
                        "bytes": index_file.stat().st_size,
                        "sha256": index_hash,
                    },
                ],
            }
        )
    result: dict[str, Any] = {
        "schema": 1,
        "classification": "published-immutable-artifacts",
        "fixtures": fixtures,
    }
    result["digest"] = canonical_digest(result)
    return result


def write_index(destination: Path, value: dict[str, Any]) -> None:
    destination.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def main() -> int:
    reference = reference_index()
    published = published_index()
    write_index(CORPUS / "reference" / "index.json", reference)
    write_index(CORPUS / "published" / "index.json", published)
    print(f"reference digest: {reference['digest']}")
    print(f"published digest: {published['digest']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
