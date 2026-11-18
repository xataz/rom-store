#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"
tag=${1:?Tag de release manquant}
parse_tag "$tag"
[[ $(manifest_version "$COMPONENT") == "$VERSION" ]] || die 'Le tag ne correspond pas à la version du manifeste'
binary="$COMPONENT/target/release/romstore-$COMPONENT"
[[ -f $binary && -x $binary ]] || die "Binaire exécutable absent : $binary"
header=$(od -An -tx1 -N20 "$binary" | tr -d ' \n')
[[ ${header:0:12} == 7f454c460201 && ${header:36:4} == 3e00 ]] || die 'Binaire ELF64 Linux x86_64 attendu'

output="$ROOT/dist/$tag"
name="romstore-$COMPONENT-$VERSION-linux-x86_64"
mkdir -p -- "$output"
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
mkdir -p "$temporary/$name/docs"
cp -p -- "$binary" "$temporary/$name/"
cp -- README.md "$temporary/$name/"
cp -- docs/ci.md "$temporary/$name/docs/"
if [[ $COMPONENT == client ]]; then
    cp -- client/config.example.yaml "$temporary/$name/"
else
    cp -- docs/server.md "$temporary/$name/docs/"
fi
tar -czf "$output/$name.tar.gz" -C "$temporary" "$name"
(cd "$output" && sha256sum "$name.tar.gz" > SHA256SUMS)
{
    printf 'RELEASE_ARCHIVE=%s.tar.gz\nRELEASE_TITLE=ROM Store %s %s\n' "$name" "$COMPONENT" "$VERSION"
    if [[ -n ${CI_API_V4_URL:-} ]]; then
        [[ $CI_API_V4_URL == https://* && ${CI_PROJECT_ID:-} =~ ^[0-9]+$ ]] || die 'API GitLab HTTPS et identifiant numérique requis'
        printf 'RELEASE_PACKAGE_URL=%s/projects/%s/packages/generic/romstore-%s/%s\n' \
            "${CI_API_V4_URL%/}" "$CI_PROJECT_ID" "$COMPONENT" "$VERSION"
    fi
} > "$ROOT/dist/release.env"
echo "Archive et SHA256SUMS disponibles dans $output"
