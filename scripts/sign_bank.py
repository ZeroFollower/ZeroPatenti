"""Firma un listato normalizzato con una chiave editore Ed25519 e OpenSSL 3."""

import argparse
import csv
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def sign_bank(folder: Path, version: str, key: Path, openssl: str) -> str:
    folder = folder.resolve(strict=True)
    key = key.resolve(strict=True)
    if key.is_relative_to(folder):
        raise ValueError("Conserva la chiave privata fuori dalla cartella distribuita")
    candidates = [folder / name for name in ("questions.json", "questions.csv", "AB2020.json")
                  if (folder / name).is_file()]
    if len(candidates) != 1:
        raise ValueError("Scegli un solo listato tra questions.json, questions.csv e AB2020.json")
    questions, = candidates
    if questions.resolve(strict=True) != questions:
        raise ValueError("Il listato deve essere un file nella cartella del pacchetto")
    with questions.open(encoding="utf-8", newline="") as stream:
        rows = json.load(stream)["domande"] if questions.suffix == ".json" else csv.DictReader(stream)
        names = {row["figura"] for row in rows if row.get("figura")}
    names.add(questions.name)
    hashes = {}
    for name in sorted(names):
        if ("\\" in name or ":" in name or any(not part or part in (".", "..")
                or part.endswith((" ", ".")) for part in name.split("/"))):
            raise ValueError(f"Percorso non valido: {name}")
        path = folder / name
        if path.resolve(strict=True) != path:
            raise ValueError(f"Collegamento non ammesso nel pacchetto: {name}")
        if not path.is_relative_to(folder) or not path.is_file():
            raise ValueError(f"File fuori dal pacchetto: {name}")
        with path.open("rb") as stream:
            hashes[name] = hashlib.file_digest(stream, "sha256").hexdigest()
    # RFC 8410 SubjectPublicKeyInfo prefix for an Ed25519 public key.
    spki = subprocess.check_output([openssl, "pkey", "-in", str(key), "-pubout", "-outform", "DER"])
    prefix = bytes.fromhex("302a300506032b6570032100")
    if not spki.startswith(prefix) or len(spki) != len(prefix) + 32:
        raise ValueError("La chiave privata deve usare Ed25519")
    manifest = {"format": 1, "version": version, "files": hashes}
    with tempfile.TemporaryDirectory(dir=folder) as temp:
        body = Path(temp) / "manifest.json"
        sig = Path(temp) / "manifest.sig"
        body.write_bytes((json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode("utf-8"))
        subprocess.run([openssl, "pkeyutl", "-sign", "-rawin", "-inkey", str(key),
                        "-in", str(body), "-out", str(sig)], check=True)
        body.replace(folder / body.name)
        sig.replace(folder / sig.name)
    return spki[len(prefix):].hex()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cartella", type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--private-key", type=Path, required=True)
    parser.add_argument("--openssl", default="openssl")
    args = parser.parse_args()
    public = sign_bank(args.cartella, args.version, args.private_key, args.openssl)
    print(f"Chiave pubblica Ed25519 dell’editore: {public}")
