#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"
component=${1:?Composant manquant}
type=${2:?Type manquant}
expected=${3:?Commit parent manquant}
validate_component "$component"
case "$type" in fix|minor|major) ;; *) die 'Type attendu : fix, minor ou major' ;; esac
[[ ${CI_COMMIT_BRANCH:-} == main && ${CI_PIPELINE_SOURCE:-} == parent_pipeline &&
   ${CI_COMMIT_REF_PROTECTED:-} == true ]] || die 'La release exige un job enfant sur main protégée'
: "${GITLAB_RELEASE_TOKEN:?Variable protégée GITLAB_RELEASE_TOKEN manquante}"
[[ ${CI_PROJECT_URL:-} =~ ^https://[^/@?#]+(/[^?#]*)?$ ]] || die 'CI_PROJECT_URL doit être une URL HTTPS sans identifiants'
[[ $(git rev-parse HEAD) == "$expected" ]] || die 'Le checkout ne correspond pas au commit parent'

export GIT_TERMINAL_PROMPT=0
# Le runner peut réécrire https://host/... vers une URL contenant CI_JOB_TOKEN.
# L'utilisateur explicite évite cette réécriture, sans mettre de secret dans l'URL.
release_url="https://oauth2@${CI_PROJECT_URL#https://}"
release_url="${release_url%/}.git"
git remote set-url origin "$release_url"
git remote set-url --push origin "$release_url"
git config --local credential.helper ''
git config --local "credential.$release_url.helper" ''
# Les helpers fonctionnent même avec credential.interactive=false, contrairement
# à GIT_ASKPASS. Les quotes simples conservent une référence à la variable :
# aucune valeur de jeton n'est écrite dans .git/config. Ignorer store/erase.
git config --local --add "credential.$release_url.helper" \
    '!f() { if test "$1" = get; then printf "%s\n" "username=oauth2" "password=$GITLAB_RELEASE_TOKEN"; fi; }; f'
git config --local "http.$release_url.extraHeader" ''
assert_current "$expected"
git fetch origin --tags
if ! component_pending "$component"; then die 'Aucun changement à publier pour ce composant'; fi
# Tester l'accès au service de push avant les compilations et le commit de version.
# Un dry-run ne vérifie toutefois pas les hooks serveur des branches/tags protégés.
if ! git push --dry-run origin HEAD:refs/heads/main; then
    die 'Accès Git en écriture refusé avec GITLAB_RELEASE_TOKEN ; vérifier sa valeur, son rôle et les protections du dépôt'
fi
git switch -c "release-${component}-${CI_JOB_ID:-$BASHPID}-$RANDOM" "$expected"
git config user.name 'ROM Store Release'
git config user.email 'romstore-release@users.noreply.gitlab.com'
make "release-$component" TYPE="$type"
tag="$component-v$(manifest_version "$component")"
git rev-parse --verify "$tag^{commit}" >/dev/null
assert_current "$expected"
git push --atomic origin HEAD:refs/heads/main "refs/tags/$tag"
