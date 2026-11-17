#!/usr/bin/env python3
"""Prépare l'archive Linux x86_64 du composant désigné par le tag."""

import hashlib
from pathlib import Path
import re
import sys
import tarfile
import tomllib


ROOT = Path(__file__).resolve().parents[2]


def validate_tag(root, tag):
    match = re.fullmatch(
        r"(client|server)-v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)", tag
    )
    if not match:
        raise ValueError("Tag attendu : client-vX.Y.Z ou server-vX.Y.Z (suffixe -rc.1 accepté)")
    crate, version = match.groups()
    manifest = tomllib.loads((root / crate / "Cargo.toml").read_text())
    if manifest["package"]["version"] != version:
        raise ValueError(f"La version de {crate}/Cargo.toml ne correspond pas à {tag}")
    return crate, version


def package(root, tag):
    crate, version = validate_tag(root, tag)
    binary = root / crate / "target/release" / f"romstore-{crate}"
    with binary.open("rb") as stream:
        header = stream.read(20)
    if (len(header) < 20 or header[:6] != b"\x7fELF\x02\x01"
            or header[18:20] != b"\x3e\x00"):
        raise ValueError(f"Binaire ELF Linux x86_64 attendu : {binary}")

    output = root / "dist" / tag
    output.mkdir(parents=True, exist_ok=True)
    name = f"romstore-{crate}-{version}-linux-x86_64"
    archive = output / f"{name}.tar.gz"
    files = {
        binary: f"romstore-{crate}",
        root / "README.md": "README.md",
        root / "docs/ci.md": "docs/ci.md",
    }
    if crate == "client":
        files[root / "client/config.example.yaml"] = "config.example.yaml"
    else:
        files[root / "docs/server.md"] = "docs/server.md"
    with tarfile.open(archive, "w:gz") as tar:
        for source, target in files.items():
            tar.add(source, arcname=f"{name}/{target}")

    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    (output / "SHA256SUMS").write_text(
        f"{digest}  {archive.name}\n", encoding="utf-8"
    )
    print(f"Archive et SHA256SUMS disponibles dans {output}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage : python3 scripts/ci/package.py {client,server}-vX.Y.Z")
    package(ROOT, sys.argv[1])
