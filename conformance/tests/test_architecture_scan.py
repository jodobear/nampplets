import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "nmp_architecture_scan",
    ROOT / "scripts" / "nmp_architecture_scan.py",
)
assert SPEC is not None and SPEC.loader is not None
SCANNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SCANNER
SPEC.loader.exec_module(SCANNER)


class ArchitectureScanTests(unittest.TestCase):
    def test_one_shot_javascript_deadline_is_not_polling(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "trusted-shell.js"
            source.write_text(
                "setTimeout(finishOperation, 1000);\n"
                "setInterval(checkAgain, 1000);\n",
                encoding="utf-8",
            )
            findings = SCANNER.scan(root)

        polling = [
            finding
            for finding in findings
            if finding.rule == "D8/no-polling"
        ]
        self.assertEqual(len(polling), 1)
        self.assertIn("setInterval", polling[0].match)


if __name__ == "__main__":
    unittest.main()
