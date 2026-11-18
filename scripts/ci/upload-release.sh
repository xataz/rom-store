#!/usr/bin/env bash
# Le mot-clé GitLab release: est exécuté seulement après la réussite de ce script.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"
parse_tag "${CI_COMMIT_TAG:?Tag manquant}"
: "${CI_JOB_TOKEN:?Jeton de job manquant}" "${RELEASE_ARCHIVE:?Archive manquante}" "${RELEASE_PACKAGE_URL:?URL de paquet manquante}"
[[ ${CI_API_V4_URL:-} == https://* && ${CI_PROJECT_ID:-} =~ ^[0-9]+$ && $RELEASE_PACKAGE_URL == https://* ]] || die 'API GitLab HTTPS requise'
[[ $RELEASE_ARCHIVE == "romstore-$COMPONENT-$VERSION-linux-x86_64.tar.gz" &&
   $RELEASE_PACKAGE_URL == "${CI_API_V4_URL%/}/projects/$CI_PROJECT_ID/packages/generic/romstore-$COMPONENT/$VERSION" ]] || die 'Archive ou URL de paquet incompatible avec le tag'
output="dist/$CI_COMMIT_TAG"
for file in "$RELEASE_ARCHIVE" SHA256SUMS CHANGELOG.md; do
    [[ -f $output/$file ]] || die "Fichier de release absent : $file"
done
status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --header "JOB-TOKEN: $CI_JOB_TOKEN" \
    "$CI_API_V4_URL/projects/$CI_PROJECT_ID/releases/$CI_COMMIT_TAG")
[[ $status == 404 ]] || die "Release existante ou accès refusé (HTTP $status) : aucune modification effectuée"
(cd "$output" && sha256sum -c SHA256SUMS)
for file in "$RELEASE_ARCHIVE" SHA256SUMS CHANGELOG.md; do
    curl --fail-with-body --silent --show-error --header "JOB-TOKEN: $CI_JOB_TOKEN" \
        --upload-file "$output/$file" "$RELEASE_PACKAGE_URL/$file"
done
