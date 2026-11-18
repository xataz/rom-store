#!/usr/bin/env bash
# Préparation locale uniquement : cargo-release ne pousse et ne publie jamais.
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
echo "Release $tag préparée localement. Aucun push effectué."
echo "Après vérification : git push --atomic origin HEAD:main refs/tags/$tag"
