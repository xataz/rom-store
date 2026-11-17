#!/usr/bin/env python3
"""Publie un composant via l'API Forgejo après l'envoi de son archive et du SHA256."""

import json
import os
import sys
from urllib.error import HTTPError
from urllib.parse import quote, urlsplit
from urllib.request import Request, urlopen
import uuid

from package import ROOT, validate_tag


def publish(root, server, repository, tag, sha, token):
    crate, version = validate_tag(root, tag)
    if urlsplit(server).scheme != "https" or not token:
        raise ValueError("Une URL HTTPS et un jeton Forgejo sont requis")
    if len(repository.split("/")) != 2:
        raise ValueError("Dépôt attendu : propriétaire/nom")
    output = root / "dist" / tag
    files = [
        output / f"romstore-{crate}-{version}-linux-x86_64.tar.gz",
        output / "SHA256SUMS",
    ]
    for path in files:
        if not path.is_file():
            raise ValueError(f"Fichier de release absent : {path}")
    repo = "/".join(quote(part, safe="") for part in repository.split("/"))
    base = f"{server.rstrip('/')}/api/v1/repos/{repo}/releases"

    def request(method, suffix="", payload=None, content_type="application/json"):
        data = json.dumps(payload).encode() if isinstance(payload, dict) else payload
        req = Request(base + suffix, data=data, method=method, headers={
            "Authorization": f"token {token}",
            "Content-Type": content_type,
            "Accept": "application/json",
        })
        with urlopen(req, timeout=120) as response:
            return json.load(response)

    try:
        request("GET", f"/tags/{quote(tag, safe='')}")
    except HTTPError as error:
        if error.code != 404:
            raise
    else:
        raise ValueError("Cette release existe déjà : aucune modification effectuée. "
                         "Supprimer le brouillon incomplet avant de relancer, le cas échéant.")

    release = request("POST", payload={
        "tag_name": tag,
        "target_commitish": sha,
        "name": f"ROM Store {crate.capitalize()} {version}",
        "body": f"Binaire {crate} Linux x86_64 compilé et testé depuis `{sha}`.\n\n"
                + ("Client : SDL2 2.26+ et police système nécessaires. " if crate == "client" else "")
                + "Binaire dynamique compilé sous Debian 12 (glibc 2.36+).\n\n"
                "Vérification : `sha256sum -c SHA256SUMS`. "
                "Voir `docs/ci.md` dans les archives pour les instructions.",
        "draft": True,
        "prerelease": "-" in version,
    })
    release_id = release["id"]
    for path in files:
        boundary = uuid.uuid4().hex
        body = (
            f'--{boundary}\r\nContent-Disposition: form-data; name="attachment"; '
            f'filename="{path.name}"\r\nContent-Type: application/octet-stream\r\n\r\n'
        ).encode() + path.read_bytes() + f"\r\n--{boundary}--\r\n".encode()
        request("POST", f"/{release_id}/assets?name={quote(path.name)}", body,
                f"multipart/form-data; boundary={boundary}")
    request("PATCH", f"/{release_id}", {"draft": False})
    print(f"Release {tag} publiée avec {len(files)} fichiers.")


if __name__ == "__main__":
    try:
        publish(ROOT, *(os.environ[name] for name in (
            "FORGEJO_SERVER_URL", "FORGEJO_REPOSITORY", "RELEASE_TAG",
            "RELEASE_SHA", "FORGEJO_TOKEN",
        )))
    except (ValueError, KeyError, HTTPError) as error:
        sys.exit(f"Publication impossible : {error}")
