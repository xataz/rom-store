#!/usr/bin/env python3
"""Prépare les archives Linux x86_64 à partir des binaires release déjà compilés."""

import hashlib
from pathlib import Path
import re
import sys
import tarfile
import tomllib


ROOT = Path(__file__).resolve().parents[2]


def validate_tag(root, tag):
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag):
        raise ValueError("Tag attendu : vX.Y.Z (ou vX.Y.Z-rc.1)")
    version = tag[1:]
    for crate in ("client", "server"):
        manifest = tomllib.loads((root / crate / "Cargo.toml").read_text())
        if manifest["package"]["version"] != version:
            raise ValueError(f"La version de {crate}/Cargo.toml ne correspond pas à {tag}")
    return version


def package(root, tag):
    version = validate_tag(root, tag)
    # Tout vérifier avant de commencer à écrire les archives.
    for crate in ("client", "server"):
        binary = root / crate / "target/release" / f"romstore-{crate}"
        with binary.open("rb") as stream:
            header = stream.read(20)
        if (len(header) < 20 or header[:6] != b"\x7fELF\x02\x01"
                or header[18:20] != b"\x3e\x00"):
            raise ValueError(f"Binaire ELF Linux x86_64 attendu : {binary}")

    output = root / "dist" / tag
    output.mkdir(parents=True, exist_ok=True)
    archives = []
    for crate in ("client", "server"):
        name = f"romstore-{crate}-{version}-linux-x86_64"
        archive = output / f"{name}.tar.gz"
        files = {
            root / crate / "target/release" / f"romstore-{crate}": f"romstore-{crate}",
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
        archives.append(archive)

    checksums = []
    for path in archives:
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        checksums.append(f"{digest}  {path.name}\n")
    (output / "SHA256SUMS").write_text("".join(checksums), encoding="utf-8")
    print(f"Archives et SHA256SUMS disponibles dans {output}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage : python3 scripts/ci/package.py vX.Y.Z")
    package(ROOT, sys.argv[1])
