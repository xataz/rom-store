#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"
tag=${1:?Tag de release manquant}
parse_tag "$tag"
full_history
previous=$(previous_tag "$COMPONENT" "$tag" "$tag")
output="dist/$tag/CHANGELOG.md"
mkdir -p -- "$(dirname -- "$output")"
range=()
if [[ -n $previous ]]; then range=("$previous..$tag"); fi
# Épingler HEAD au tag sans changer le checkout de travail. Le clone bare partagé
# ne copie pas les objets et n'utilise aucun réseau ; il couvre aussi la première
# release, pour laquelle git-cliff ne prend pas un simple tag comme range.
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
git clone --quiet --bare --shared "$ROOT" "$temporary/history.git"
git --git-dir="$temporary/history.git" update-ref --no-deref HEAD "$(git rev-parse "$tag^{commit}")"
git-cliff --workdir "$temporary/history.git" --repository "$temporary/history.git" \
    --config "$ROOT/cliff.toml" --use-branch-tags \
    --tag-pattern "^$COMPONENT-v[0-9]+\\.[0-9]+\\.[0-9]+(-[0-9A-Za-z.-]+)?$" \
    --output "$ROOT/$output" -- "${range[@]}"
