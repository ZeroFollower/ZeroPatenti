"""Normalize a pinned MIT exam PDF into questions.json and its local figures.

Run with Python and PyMuPDF 2.1:
    python scripts/normalize_bank.py official.pdf output_directory
"""
import argparse
import hashlib
import json
import re
from pathlib import Path

import pymupdf


def normalize(pdf_path, destination):
    manifest = Path(__file__).resolve().parents[1] / "data/listati.json"
    catalog = json.loads(manifest.read_text(encoding="utf-8"))
    file_digest = hashlib.sha256(pdf_path.read_bytes()).hexdigest()
    pin = next((bank for bank in catalog["listati"]
                if bank.get("exam", {}).get("pdf_sha256") == file_digest), None)
    if pin is None:
        raise ValueError("PDF non riconosciuto: controllare fonte ed edizione")
    version = pin["versione"]
    expected_count = pin["exam"]["row_count"]
    figures = destination / "figures"
    figures.mkdir(parents=True, exist_ok=True)
    rows = []
    topics = {}
    group = None
    figure_paths = {}
    with pymupdf.open(pdf_path) as document:
        for page in document:
            lines = []
            for block in page.get_text("dict", flags=pymupdf.TEXTFLAGS_TEXT)["blocks"]:
                for line in block["lines"]:
                    x, y, _, _ = line["bbox"]
                    text = "".join(span["text"] for span in line["spans"]).strip()
                    if text:
                        lines.append((round(y, 1), x, text))
            images = [(i["bbox"][1], i["xref"]) for i in page.get_image_info(xrefs=True)
                      if i["bbox"][0] > 430]
            current = None
            heading = False
            for y, x, text in sorted(lines):
                if x < 22 and text.startswith("Quesito n°"):
                    group_id, topic = re.fullmatch(r"Quesito n°\s*(\d+)\s*-\s*(.+)", text).groups()
                    group = int(group_id), topic
                    current = None
                    heading = True
                elif heading and x < 22:
                    group = group[0], group[1] + " " + text
                elif x < 50 and text.isdecimal():
                    heading = False
                    group_id, topic = group
                    topic_id = topics.setdefault(topic, len(topics) + 1)
                    current = dict(id=int(text), quesito_id=group_id, argomento_id=topic_id,
                                   argomento_nome=topic, testo="", risposta=None, figura=None)
                    rows.append(current)
                    nearby = [(iy, ref) for iy, ref in images if abs(iy - y) < 4]
                    for image in nearby:
                        iy, ref = image
                        if current["figura"]:
                            raise ValueError(f"Due figure per domanda {text}")
                        if ref not in figure_paths:
                            asset = document.extract_image(ref)
                            filename = f"mit-{ref}.{asset['ext']}"
                            (figures / filename).write_bytes(asset["image"])
                            figure_paths[ref] = f"figures/{filename}"
                        current["figura"] = figure_paths[ref]
                        images.remove(image)
                elif current and 74 <= x < 81 and text != "Testo domanda":
                    merged = re.fullmatch(r"(.+?)\s+(VERO|FALSO)", text)
                    if merged:
                        text, answer = merged.groups()
                        current["risposta"] = answer == "VERO"
                    current["testo"] = (current["testo"] + " " + text).strip()
                elif current and x > 300 and text in ("VERO", "FALSO"):
                    if current["risposta"] is not None:
                        raise ValueError(f"Risposta duplicata per {current['id']}")
                    current["risposta"] = text == "VERO"
            if images:
                raise ValueError(f"Figure non assegnate a pagina {page.number + 1}: {images}")
    if len(rows) != expected_count or len(set(r["id"] for r in rows)) != len(rows) or any(
        not r["testo"] or r["risposta"] is None for r in rows
    ):
        raise ValueError("Listato incompleto o ID duplicati")
    # Two headings continue across a page break. Resolve only unique prefixes.
    full_titles = sorted(title for title in topics
                         if not any(other != title and other.startswith(title) for other in topics))
    for row in rows:
        matches = [name for name in full_titles if name.startswith(row["argomento_nome"])]
        if len(matches) != 1:
            raise ValueError(f"Argomento ambiguo per {row['id']}")
        row["argomento_nome"] = matches[0]
        row["argomento_id"] = full_titles.index(matches[0]) + 1
    checksum = hashlib.sha256()
    for row in sorted(rows, key=lambda row: row["id"]):
        body = json.dumps(list(row.values()), ensure_ascii=False, separators=(",", ":"))
        checksum.update((body + "\n").encode("utf-8"))
    if checksum.hexdigest() != pin["exam"]["digest"]:
        raise ValueError("Il contenuto normalizzato differisce dall’edizione verificata")
    (destination / "questions.json").write_text(
        json.dumps({"domande": rows}, ensure_ascii=False), encoding="utf-8")
    print(f"{version}: {len(rows)} domande, {len(full_titles)} argomenti, {len(figure_paths)} figure")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pdf", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    normalize(args.pdf, args.output)
