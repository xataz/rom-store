#!/usr/bin/env python3
"""Notes de release issues des tags et messages de commit, sans accès réseau."""

import re
import subprocess
from urllib.parse import quote


TAG = re.compile(r"(client|server)-v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?")


def git(root, *args):
    result = subprocess.run(
        ("git", *args), cwd=root, text=True, encoding="utf-8", errors="replace",
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if result.returncode:
        raise ValueError(f"Historique Git inaccessible : {result.stderr.strip()}")
    return result.stdout


def release_notes(root, server, repository, tag):
    match = TAG.fullmatch(tag)
    if not match:
        raise ValueError("Tag de release client-vX.Y.Z ou server-vX.Y.Z attendu")
    if git(root, "rev-parse", "--is-shallow-repository").strip() != "false":
        raise ValueError("Historique Git incomplet : utiliser fetch-depth: 0 pour les notes de release")
    git(root, "rev-parse", "--verify", f"{tag}^{{commit}}")
    component = match[1]
    candidates = [
        name for name in git(root, "tag", "--merged", tag, "--list", f"{component}-v*").splitlines()
        if name != tag and TAG.fullmatch(name)
    ]
    previous = None
    if candidates:
        matches = [argument for name in candidates for argument in ("--match", name)]
        previous = git(root, "describe", "--tags", "--abbrev=0", *matches, tag).strip()
    revision = f"{previous}..{tag}" if previous else tag
    repo_url = server.rstrip("/") + "/" + "/".join(
        quote(part, safe="") for part in repository.split("/")
    )
    if previous:
        lines = [
            f"## Modifications depuis {previous}", "",
            f"[Comparer les versions]({repo_url}/compare/{quote(previous, safe='')}...{quote(tag, safe='')})",
            "",
        ]
    else:
        lines = ["## Modifications — première release du composant", "",
                 "Aucun tag précédent du même composant : historique complet jusqu'à cette version.", ""]
    # Chaque entrée possède trois champs séparés par NUL ; les retours à la ligne
    # des corps de commit ne peuvent donc pas être confondus avec une autre entrée.
    fields = git(root, "log", "--reverse", "--no-show-signature", "--no-notes", "--no-color",
                 "--format=%H%x00%s%x00%b%x00", revision, "--").split("\0")
    commits = 0
    for index in range(0, len(fields) - 1, 3):
        sha, subject, body = fields[index:index + 3]
        sha = sha.strip()
        subject = re.sub(r"([\\`*_\[\]<>])", r"\\\1", subject)
        lines.append(f"- [{sha[:8]}]({repo_url}/commit/{sha}) {subject}")
        if body.strip():
            # Bloc de code dans la liste : conserve les détails sans interpréter
            # le Markdown/HTML éventuellement présent dans les messages Git.
            lines.extend(["", *("      " + line for line in body.strip().splitlines()), ""])
        commits += 1
    if not commits:
        lines.append("Aucun nouveau commit entre ces deux tags.")
    return "\n".join(lines).rstrip() + "\n"
