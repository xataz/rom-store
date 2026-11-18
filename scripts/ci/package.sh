#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"
tag=${1:?Tag de release manquant}
parse_tag "$tag"
[[ $(manifest_version "$COMPONENT") == "$VERSION" ]] || die 'Le tag ne correspond pas à la version du manifeste'
output="$ROOT/dist/$tag"
name="romstore-$COMPONENT-$VERSION-linux-x86_64"

if [[ ${2:-} == --release ]]; then
    [[ ${CI_PROJECT_URL:-} =~ ^https://[^/@?#]+(/[^?#]*)?$ && ${CI_JOB_ID:-} =~ ^[0-9]+$ ]] || die 'URL de projet HTTPS et identifiant de job numérique requis'
    bash scripts/ci/changelog.sh "$tag"
    for file in "$name.tar.gz" SHA256SUMS CHANGELOG.md; do
        [[ -s $output/$file ]] || die "Fichier de release absent ou vide : $file"
    done
    (cd "$output" && sha256sum -c SHA256SUMS)
    printf 'RELEASE_ARCHIVE=%s.tar.gz\nRELEASE_TITLE=ROM Store %s %s\nRELEASE_ASSET_BASE_URL=%s/-/jobs/%s/artifacts/raw/dist/%s\n' \
        "$name" "$COMPONENT" "$VERSION" "${CI_PROJECT_URL%/}" "$CI_JOB_ID" "$tag" > dist/release.env
    exit 0
fi
[[ -z ${2:-} ]] || die 'Option attendue : --release'
binary="$COMPONENT/target/release/romstore-$COMPONENT"
[[ -f $binary && -x $binary ]] || die "Binaire exécutable absent : $binary"
header=$(od -An -tx1 -N20 "$binary" | tr -d ' \n')
[[ ${header:0:12} == 7f454c460201 && ${header:36:4} == 3e00 ]] || die 'Binaire ELF64 Linux x86_64 attendu'

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
printf 'RELEASE_ARCHIVE=%s.tar.gz\nRELEASE_TITLE=ROM Store %s %s\n' \
    "$name" "$COMPONENT" "$VERSION" > "$ROOT/dist/release.env"
echo "Archive et SHA256SUMS disponibles dans $output"
