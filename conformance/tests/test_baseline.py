from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "conformance" / "scripts"))

import verify_baseline  # noqa: E402
import generate_digests  # noqa: E402


class BaselineTests(unittest.TestCase):
    def test_digest_generation_is_deterministic_and_current(self) -> None:
        first = generate_digests.manifest_bytes()
        second = generate_digests.manifest_bytes()
        self.assertEqual(first, second)
        self.assertEqual(
            first,
            (ROOT / "conformance/digests.sha256").read_bytes(),
        )

    def test_digest_generation_ignores_untracked_environment_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            evidence = root / "conformance/bdd/evidence.json"
            evidence.parent.mkdir(parents=True)
            evidence.write_text("{}\n", encoding="utf-8")
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            subprocess.run(
                ["git", "-C", str(root), "add", evidence.relative_to(root)],
                check=True,
            )

            expected = generate_digests.manifest_bytes(root)
            (root / "conformance/bdd/.DS_Store").write_bytes(b"environment")
            (root / "conformance/bdd/editor.tmp").write_bytes(b"environment")

            self.assertEqual(expected, generate_digests.manifest_bytes(root))

    def test_full_offline_baseline(self) -> None:
        result = verify_baseline.verify()
        self.assertEqual(result["status"], "unratified")
        self.assertGreaterEqual(result["envelopes"], 200)
        self.assertEqual(result["falsifiers"], 10)
        self.assertGreaterEqual(result["corpus"]["published"], 1)

    def test_one_byte_fixture_mutation_fails_hash(self) -> None:
        index = json.loads(
            (
                ROOT / "conformance/napplet-corpus/published/index.json"
            ).read_text(encoding="utf-8")
        )
        record = next(
            item
            for item in index["fixtures"][0]["files"]
            if item["path"] == "index.html"
        )
        source = (
            ROOT
            / "conformance/napplet-corpus/published"
            / index["fixtures"][0]["name"]
            / "index.html"
        )
        content = bytearray(source.read_bytes())
        content[0] ^= 1
        with tempfile.TemporaryDirectory() as temporary:
            mutated = Path(temporary) / "index.html"
            mutated.write_bytes(content)
            actual = hashlib.sha256(mutated.read_bytes()).hexdigest()
        self.assertNotEqual(actual, record["sha256"])

    def test_unknown_message_policy_is_forward_compatible(self) -> None:
        lock = verify_baseline.load_lock()
        inventory = verify_baseline.load_json(
            "conformance/envelopes/inventory.json"
        )
        self.assertEqual(lock["web_projection"]["unknown_message_policy"], "ignore")
        self.assertEqual(inventory["unknown_message_policy"], "ignore")

    def test_shell_handshake_drift_is_not_hidden(self) -> None:
        inventory = verify_baseline.load_json(
            "conformance/envelopes/inventory.json"
        )
        handshake = {
            item["type"]: item["validator"]
            for item in inventory["entries"]
            if item["domain"] == "shell"
        }
        self.assertEqual(
            handshake,
            {
                "shell.init": "registry-only-handshake",
                "shell.ready": "registry-only-handshake",
            },
        )

    def test_m0_advertises_no_domains(self) -> None:
        lock = verify_baseline.load_lock()
        for platform in ("macos", "ios", "android"):
            self.assertEqual(lock["platform"][platform]["supported_domains"], [])


if __name__ == "__main__":
    unittest.main()
