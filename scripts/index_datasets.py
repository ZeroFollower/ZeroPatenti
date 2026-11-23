"""Regenerate the GitHub dataset index from tracked A/B SQLite filenames."""
from contextlib import closing
from pathlib import Path
import re
import sqlite3
import subprocess


def render(root):
    paths = subprocess.check_output(["git", "ls-files", "-z", "--", "DB"], cwd=root).decode().split("\0")
    banks = {}
    for entry in filter(None, paths):
        rel = Path(entry).relative_to("DB")
        match = re.fullmatch(r"AB_([0-9]{4})\.sqlite3", rel.name)
        if not match or rel.parent.as_posix() not in (".", "obsoleto"):
            continue
        year = int(match[1])
        if year in banks:
            raise ValueError(f"Due dataset pubblicati per il {year}")
        with closing(sqlite3.connect((root / entry).as_uri() + "?mode=ro", uri=True)) as db:
            db.execute("PRAGMA trusted_schema=OFF")
            count, = db.execute("SELECT count(*) FROM questions").fetchone()
        banks[year] = rel.as_posix(), count
    latest = max(banks)
    rows = ["# Dataset A/B per il conseguimento", "",
            "| Edizione | Stato | Domande |", "|---|---|---:|"]
    for year in sorted(banks, reverse=True):
        path, count = banks[year]
        status = "**Predefinita**" if year == latest else "**Obsoleta**"
        formatted = f"{count:,}".replace(",", ".")
        rows.append(f"| [AB_{year}.sqlite3]({path}) | {status} | {formatted} |")
    return "\n".join(rows) + "\n"


if __name__ == "__main__":
    project = Path(__file__).resolve().parents[1]
    (project / "DB/README.md").write_text(render(project), encoding="utf-8", newline="\n")
