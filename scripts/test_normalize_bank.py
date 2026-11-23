import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pymupdf
import normalize_bank


class BankCompatibility(unittest.TestCase):
    def test_ab_preserves_chapter_ids_and_line_wrapped_compounds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pdf = root / "bank.pdf"
            with pymupdf.open() as document:
                # The PDF page order and alphabetical title order both differ
                # from the chapter order established by the official group IDs.
                for group, topic, question, lines in [
                    (4105, "Alfa", 100, ["Primo rigo", "secondo rigo"]),
                    (4001, "Zeta", 200, ["Un air-", "bag"]),
                ]:
                    page = document.new_page()
                    page.insert_text((20, 60), f"Quesito n° {group} - {topic}")
                    page.insert_text((23, 90), str(question))
                    for index, line in enumerate(lines):
                        page.insert_text((76, 90 + 14 * index), line)
                    page.insert_text((355, 90), "VERO")
                document.save(pdf)
            expected = [
                [100, 4105, 2, "Alfa", "Primo rigo secondo rigo", True, None],
                [200, 4001, 1, "Zeta", "Un air-bag", True, None],
            ]
            bodies = "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in expected)
            pin = {"versione": "AB-7020", "exam": {
                "pdf_sha256": hashlib.sha256(pdf.read_bytes()).hexdigest(),
                "row_count": 2, "digest": hashlib.sha256(bodies.encode()).hexdigest(),
            }}
            (root / "data").mkdir()
            (root / "data/listati.json").write_text(json.dumps({"listati": [pin]}))
            with patch.object(normalize_bank, "__file__", str(root / "scripts/normalize_bank.py")):
                normalize_bank.normalize(pdf, root / "out")
                normalize_bank.normalize(pdf, root / "AB-2024")
            self.assertEqual((root / "AB-2024/AB2020.json").read_bytes(), (root / "out/questions.json").read_bytes())
            self.assertFalse((root / "AB-2024/questions.json").exists())
            rows = json.loads((root / "out/questions.json").read_text(encoding="utf-8"))["domande"]
            self.assertEqual([list(row.values()) for row in rows], expected)


if __name__ == "__main__":
    unittest.main()
