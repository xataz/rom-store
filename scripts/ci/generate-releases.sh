#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
cd "$ROOT"

output=${1:-dist/release-jobs.yml}
full_history
sha=$(git rev-parse HEAD)
mkdir -p -- "$(dirname -- "$output")"
{
    echo 'include:'
    echo '  - local: .gitlab/ci/release.yml'
    count=0
    for component in client server; do
        if component_pending "$component"; then
            for type in fix minor major; do
                cat <<EOF

$component:$type:
  extends: .prepare-release
  script:
    - bash scripts/ci/gitlab-release.sh $component $type $sha
EOF
                count=$((count + 1))
            done
        fi
    done
    if ((count == 0)); then
        cat <<'EOF'

nothing-to-release:
  image: debian:bookworm-slim
  stage: release
  script:
    - echo 'Aucun changement depuis les derniers tags client et serveur.'
EOF
    fi
} > "$output"
