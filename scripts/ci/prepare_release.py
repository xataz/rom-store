#!/usr/bin/env python3
"""Prépare un commit et un tag de release locaux, sans publication."""

import argparse
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[2]
NUMBER = r"(?:0|[1-9][0-9]*)"
VERSION = rf"({NUMBER})\.({NUMBER})\.({NUMBER})"
RELEASE_TYPES = ("fix", "minor", "major")


def run(root, *args, capture=False, quiet=False):
    return subprocess.run(
        args, cwd=root, check=True, text=True,
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL if quiet else None,
    ).stdout


def update_manifest(text, version):
    """Change uniquement package.version en conservant le formatage du manifeste."""
    section = re.search(r"(?ms)^\[package\][ \t]*\n.*?(?=^\[|\Z)", text)
    if section is None:
        raise ValueError("Section [package] absente du manifeste")
    updated, count = re.subn(
        r"(?m)^(version[ \t]*=[ \t]*)[\"'][^\"'\n]*[\"']",
        lambda match: f'{match[1]}"{version}"', section[0],
    )
    if count != 1:
        raise ValueError("Une unique entrée package.version explicite est requise")
    result = text[:section.start()] + updated + text[section.end():]
    if tomllib.loads(result)["package"]["version"] != version:
        raise ValueError("Impossible de modifier package.version")
    return result


def next_version(current, release_type):
    """Incrémente une version stable en remettant les niveaux inférieurs à zéro."""
    if release_type not in RELEASE_TYPES:
        raise ValueError("TYPE attendu : fix, minor ou major")
    match = re.fullmatch(VERSION, current) if isinstance(current, str) else None
    if match is None:
        raise ValueError("La version actuelle doit être stable au format X.Y.Z ; "
                         "préparer les préreleases manuellement")
    major, minor, fix = map(int, match.groups())
    if release_type == "major":
        return f"{major + 1}.0.0"
    if release_type == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{fix + 1}"


def prepare(root, component, release_type, cargo="cargo"):
    if component not in ("client", "server"):
        raise ValueError("Composant attendu : client ou server")
    if release_type not in RELEASE_TYPES:
        raise ValueError("TYPE attendu : fix, minor ou major")
    cargo = shlex.split(cargo)
    if not cargo:
        raise ValueError("Commande Cargo vide")
    manifest = f"{component}/Cargo.toml"
    lock = f"{component}/Cargo.lock"
    if run(root, "git", "status", "--porcelain", "--untracked-files=all", capture=True):
        raise ValueError("Le dépôt doit être propre, y compris les fichiers non suivis")
    branch = run(root, "git", "symbolic-ref", "--short", "HEAD", capture=True).strip()
    run(root, "git", "ls-files", "--error-unmatch", "--", manifest, lock, quiet=True)
    text = (root / manifest).read_text(encoding="utf-8")
    current = tomllib.loads(text)["package"]["version"]
    version = next_version(current, release_type)
    tag = f"{component}-v{version}"
    if run(root, "git", "tag", "--list", tag, capture=True).strip():
        raise ValueError(f"Le tag {tag} existe déjà localement")

    print(f"Release {component} ({release_type}) : {current} → {version}", flush=True)
    (root / manifest).write_text(update_manifest(text, version), encoding="utf-8")
    print(f"Mise à jour de {lock}…", flush=True)
    # Contrairement à generate-lockfile, metadata réutilise les versions verrouillées.
    run(root, *cargo, "metadata", "--manifest-path", manifest,
        "--format-version", "1", quiet=True)
    run(root, *cargo, "fmt", "--manifest-path", manifest, "--", "--check")
    run(root, *cargo, "clippy", "--locked", "--manifest-path", manifest,
        "--all-targets", "--", "-D", "warnings")
    run(root, *cargo, "test", "--locked", "--manifest-path", manifest)

    # Ne pas embarquer de modifications étrangères, notamment produites par les tests.
    changes = run(root, "git", "status", "--porcelain", "--untracked-files=all",
                  capture=True).splitlines()
    if any(line[3:] not in (manifest, lock) for line in changes):
        raise ValueError("Des fichiers autres que le manifeste et le lock ont changé ; vérifier git status")
    message = f"chore({component}): prépare la version {version}"
    run(root, "git", "add", "--", manifest, lock)
    run(root, "git", "commit", "-m", message)
    run(root, "git", "tag", "-a", tag, "-m", f"ROM Store {component.title()} {version}")
    print(f"\nRelease {tag} préparée localement. Aucun push effectué.")
    print("Après vérification, publier avec :")
    print(f"  git push origin {shlex.quote(branch)}")
    print(f"  git push origin {shlex.quote(tag)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("component", choices=("client", "server"))
    parser.add_argument("release_type", choices=RELEASE_TYPES,
                        help="fix : correctif, minor : version mineure, major : version majeure")
    parser.add_argument("--cargo", default="cargo", help="Commande Cargo à utiliser")
    args = parser.parse_args()
    try:
        prepare(ROOT, args.component, args.release_type, args.cargo)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Erreur : {error}", file=sys.stderr)
        print("Arrêt sans push ni annulation automatique. Vérifier git status et git log "
              "avant de reprendre.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
