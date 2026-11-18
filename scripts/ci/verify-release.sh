#!/usr/bin/env bash
# Les fichiers sont déjà stockés par release-files ; vérifier avant release:.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"
parse_tag "${CI_COMMIT_TAG:?Tag manquant}"
: "${CI_JOB_TOKEN:?Jeton de job manquant}" "${RELEASE_ARCHIVE:?Archive manquante}" "${RELEASE_ASSET_BASE_URL:?URL des artifacts manquante}"
[[ ${CI_API_V4_URL:-} == https://* && ${CI_PROJECT_ID:-} =~ ^[0-9]+$ &&
   ${CI_PROJECT_URL:-} =~ ^https://[^/@?#]+(/[^?#]*)?$ ]] || die 'URL GitLab HTTPS requise'
prefix="${CI_PROJECT_URL%/}/-/jobs/"
job_id=${RELEASE_ASSET_BASE_URL#"$prefix"}
job_id=${job_id%%/*}
[[ $RELEASE_ARCHIVE == "romstore-$COMPONENT-$VERSION-linux-x86_64.tar.gz" &&
   $job_id =~ ^[0-9]+$ &&
   $RELEASE_ASSET_BASE_URL == "$prefix$job_id/artifacts/raw/dist/$CI_COMMIT_TAG" ]] || die 'Archive ou URL des artifacts incompatible avec le projet et le tag'
output="dist/$CI_COMMIT_TAG"
for file in "$RELEASE_ARCHIVE" SHA256SUMS CHANGELOG.md; do
    [[ -s $output/$file ]] || die "Fichier de release absent ou vide : $file"
done
status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --header "JOB-TOKEN: $CI_JOB_TOKEN" \
    "$CI_API_V4_URL/projects/$CI_PROJECT_ID/releases/$CI_COMMIT_TAG")
[[ $status == 404 ]] || die "Release existante ou accès refusé (HTTP $status) : aucune modification effectuée"
(cd "$output" && sha256sum -c SHA256SUMS)
echo 'Artifacts vérifiés ; création de la release par GitLab.'
