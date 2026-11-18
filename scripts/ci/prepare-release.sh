#!/usr/bin/env bash
# Préparation locale ; --push active les garde-fous et le push depuis GitLab.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"

component=${1:?Composant manquant}
type=${2:-}
validate_component "$component"
case "$type" in
    fix) level=patch ;;
    minor|major) level=$type ;;
    *) die 'TYPE attendu : fix, minor ou major' ;;
esac
[[ -z ${3:-} || ${3:-} == --push ]] || die 'Option attendue : --push'
if [[ ${3:-} == --push ]]; then
    expected=${4:?Commit de pipeline manquant}
    [[ ${CI_COMMIT_BRANCH:-} == main && ${CI_COMMIT_TAG:-} == '' &&
       ${CI_PIPELINE_SOURCE:-} =~ ^(push|web|api|schedule|trigger|pipeline)$ &&
       ${CI_COMMIT_REF_PROTECTED:-} == true ]] || die 'La release exige une pipeline de main protégée'
    : "${GITLAB_RELEASE_TOKEN:?Variable protégée GITLAB_RELEASE_TOKEN manquante}"
    [[ ${CI_PROJECT_URL:-} =~ ^https://[^/@?#]+(/[^?#]*)?$ ]] || die 'CI_PROJECT_URL doit être une URL HTTPS sans identifiants'
    [[ $(git rev-parse HEAD) == "$expected" ]] || die 'Le checkout ne correspond pas au commit de pipeline'

    export GIT_TERMINAL_PROMPT=0
    # L'utilisateur explicite évite les réécritures d'URL du runner avec CI_JOB_TOKEN.
    release_url="https://oauth2@${CI_PROJECT_URL#https://}"
    release_url="${release_url%/}.git"
    git remote set-url origin "$release_url"
    git remote set-url --push origin "$release_url"
    git config --local credential.helper ''
    git config --local "credential.$release_url.helper" ''
    # Ne conserver que la référence au secret, jamais sa valeur. Ignorer store/erase.
    git config --local --add "credential.$release_url.helper" \
        '!f() { if test "$1" = get; then printf "%s\n" "username=oauth2" "password=$GITLAB_RELEASE_TOKEN"; fi; }; f'
    git config --local "http.$release_url.extraHeader" ''
    assert_current "$expected"
    git fetch origin --tags
    component_pending "$component" || die 'Aucun changement à publier pour ce composant'
    # Vérifier les droits avant toute compilation ou création de commit.
    if ! git push --dry-run origin HEAD:refs/heads/main; then
        die 'Accès Git en écriture refusé avec GITLAB_RELEASE_TOKEN ; vérifier sa valeur, son rôle et les protections du dépôt'
    fi
    git switch -c "release-${component}-${CI_JOB_ID:-$BASHPID}-$RANDOM" "$expected"
    git config user.name 'ROM Store Release'
    git config user.email 'romstore-release@users.noreply.gitlab.com'
fi
[[ -z $(git status --porcelain --untracked-files=all) ]] || die 'Le dépôt doit être propre, fichiers non suivis compris'
git symbolic-ref --short HEAD >/dev/null
version=$(manifest_version "$component")
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || die 'La version actuelle doit être stable : X.Y.Z'
"${CARGO_CMD[@]}" release --version >/dev/null || die 'Installer cargo-release avec make release-tools'

signing=()
if key=$(git config --get user.signingkey); then
    if [[ -n $key ]]; then signing=(--sign); fi
else
    status=$?
    [[ $status == 1 ]] || die 'Impossible de lire la configuration de signature Git'
fi
"${CARGO_CMD[@]}" release "$level" --manifest-path "$component/Cargo.toml" \
    --no-publish --no-push --execute --no-confirm "${signing[@]}"
tag="$component-v$(manifest_version "$component")"
if [[ ${3:-} == --push ]]; then
    git rev-parse --verify "$tag^{commit}" >/dev/null
    assert_current "$expected"
    # workflow:rules ignore ce commit sur main, mais accepte son tag.
    git push --atomic origin HEAD:refs/heads/main "refs/tags/$tag"
    exit 0
fi
echo "Release $tag préparée localement. Aucun push effectué."
echo "Après vérification : git push --atomic origin HEAD:main refs/tags/$tag"
