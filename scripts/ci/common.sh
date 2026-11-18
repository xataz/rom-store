#!/usr/bin/env bash
# Fonctions partagées ; à sourcer depuis les scripts Bash de CI.

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
read -r -a CARGO_CMD <<< "${CARGO:-cargo}"
TAG_PATTERN='^(client|server)-v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$'

die() { echo "Erreur : $*" >&2; exit 1; }

validate_component() {
    case "$1" in client|server) ;; *) die 'Composant attendu : client ou server' ;; esac
}

full_history() {
    [[ $(git rev-parse --is-shallow-repository) == false ]] || die 'Historique incomplet : GIT_DEPTH doit valoir 0'
}

previous_tag() {
    local component=$1 ref=${2:-HEAD} excluded=${3:-} tag tags
    local matches=()
    validate_component "$component"
    full_history
    git rev-parse --verify "$ref^{commit}" >/dev/null || die "Référence Git inaccessible : $ref"
    tags=$(git tag --merged "$ref" --list "$component-v*") || die 'Impossible de lire les tags'
    while IFS= read -r tag; do
        if [[ $tag =~ $TAG_PATTERN && $tag != "$excluded" ]]; then
            matches+=(--match "$tag")
        fi
    done <<< "$tags"
    if ((${#matches[@]})); then
        git describe --tags --abbrev=0 "${matches[@]}" "$ref"
    fi
}

component_pending() {
    local component=$1 previous other status
    validate_component "$component"
    previous=$(previous_tag "$component") || die 'Impossible de déterminer le précédent tag'
    if [[ -z $previous ]]; then previous=$(git mktree </dev/null); fi
    other=client
    if [[ $component == client ]]; then other=server; fi
    # Exclure seulement l'autre composant : les fichiers partagés restent inclus.
    if git diff --quiet --no-renames "$previous" HEAD -- . ":(top,exclude)$other/**"; then
        return 1
    else
        status=$?
        [[ $status == 1 ]] || die 'Impossible de comparer les fichiers depuis le dernier tag'
    fi
}

manifest_version() {
    validate_component "$1"
    "${CARGO_CMD[@]}" metadata --locked --no-deps --format-version 1 \
        --manifest-path "$1/Cargo.toml" |
        jq -er --arg name "romstore-$1" '.packages[] | select(.name == $name) | .version'
}

parse_tag() {
    [[ $1 =~ $TAG_PATTERN ]] || die 'Tag attendu : client-vX.Y.Z ou server-vX.Y.Z'
    COMPONENT=${1%%-v*}
    VERSION=${1#*-v}
}

assert_current() {
    local remote
    remote=$(git ls-remote --exit-code origin refs/heads/main)
    [[ ${remote%%$'\t'*} == "$1" ]] || die 'Pipeline obsolète : main a avancé. Utiliser sa dernière pipeline'
}
