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

temporary=$(mktemp -d)
trap 'rm -f -- "$temporary/askpass.sh"; rmdir -- "$temporary"' EXIT
cat > "$temporary/askpass.sh" <<'EOF'
#!/usr/bin/env bash
case "$1" in
    *sername*) printf '%s\n' oauth2 ;;
    *) printf '%s\n' "$GITLAB_RELEASE_TOKEN" ;;
esac
EOF
chmod 700 "$temporary/askpass.sh"
export GIT_ASKPASS="$temporary/askpass.sh" GIT_TERMINAL_PROMPT=0
git remote set-url origin "${CI_PROJECT_URL%/}.git"
git config --local credential.helper ''
assert_current "$expected"
git fetch origin --tags
if ! component_pending "$component"; then die 'Aucun changement à publier pour ce composant'; fi
git switch -c "release-${component}-${temporary##*/}" "$expected"
git config user.name 'ROM Store Release'
git config user.email 'romstore-release@users.noreply.gitlab.com'
make "release-$component" TYPE="$type"
tag="$component-v$(manifest_version "$component")"
git rev-parse --verify "$tag^{commit}" >/dev/null
assert_current "$expected"
git push --atomic origin HEAD:refs/heads/main "refs/tags/$tag"
