#!/usr/bin/env bash
# Hook cargo-release : contrôle le composant après mise à jour de sa version.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"
component=${1:?Composant manquant}
validate_component "$component"
"${CARGO_CMD[@]}" fmt --manifest-path "$component/Cargo.toml" -- --check
"${CARGO_CMD[@]}" clippy --locked --manifest-path "$component/Cargo.toml" --all-targets -- -D warnings
"${CARGO_CMD[@]}" test --locked --manifest-path "$component/Cargo.toml"
while IFS= read -r -d '' change; do
    case "${change:3}" in
        "$component/Cargo.toml"|"$component/Cargo.lock") ;;
        *) die 'Un fichier autre que le manifeste et le lock a changé ; vérifier git status' ;;
    esac
done < <(git status --porcelain -z --untracked-files=all)
