#!/usr/bin/env bash
# Tests isolés : vrais dépôts Git, commandes réseau simulées, aucune ROM utilisée.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
SOURCE_ROOT=$ROOT
if [[ -d /tmp/opencode && -z ${TMPDIR:-} ]]; then export TMPDIR=/tmp/opencode; fi
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME='CI Test' GIT_AUTHOR_EMAIL=test@example.org
export GIT_COMMITTER_NAME=$GIT_AUTHOR_NAME GIT_COMMITTER_EMAIL=$GIT_AUTHOR_EMAIL
# Les fixtures définissent leur propre contexte, même dans une pipeline de tag.
unset CI_COMMIT_TAG CI_COMMIT_BRANCH CI_PIPELINE_SOURCE CI_COMMIT_REF_PROTECTED
unset CI_PROJECT_URL GITLAB_RELEASE_TOKEN
export FAKE_CARGO_LOG="$temporary/cargo.log" CURL_LOG="$temporary/curl.log"
mkdir "$temporary/bin"
cat > "$temporary/bin/cargo-test" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "$FAKE_CARGO_LOG"
if [[ $1 == metadata ]]; then
    while [[ $1 != --manifest-path ]]; do shift; done
    manifest=$2
    component=${manifest%%/*}
    version=$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$manifest" | head -n1)
    jq -cn --arg name "romstore-$component" --arg version "$version" '{packages: [{name: $name, version: $version}]}'
elif [[ $1 == release && ${2:-} == --version ]]; then
    echo 'cargo-release 1.0.0'
elif [[ ${FAIL_CARGO_COMMAND:-} == "$1" ]]; then
    exit 1
fi
EOF
cat > "$temporary/bin/curl" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "$CURL_LOG"
if [[ ${MOCK_CURL_FAIL:-0} != 0 ]]; then exit 22; fi
case "$*" in
    *--write-out*) printf '%s' "${MOCK_RELEASE_STATUS:-404}" ;;
    *) exit 99 ;;
esac
EOF
cat > "$temporary/bin/git-cliff" <<'EOF'
#!/usr/bin/env bash
set -eu
while [[ $1 != --output ]]; do shift; done
printf 'Notes de release\n' > "$2"
EOF
chmod +x "$temporary/bin/"*

fixture() {
    repo=$(mktemp -d "$temporary/repo.XXXXXX")
    mkdir -p "$repo/scripts/ci" "$repo/docs"
    cp -- "$SOURCE_ROOT"/scripts/ci/*.sh "$repo/scripts/ci/"
    cp -- "$SOURCE_ROOT/Makefile" "$SOURCE_ROOT/cliff.toml" "$repo/"
    printf 'Demo\n' > "$repo/README.md"
    printf 'Demo\n' > "$repo/docs/ci.md"
    printf 'Demo\n' > "$repo/docs/server.md"
    printf 'target/\ndist/\n' > "$repo/.gitignore"
    for component in client server; do
        mkdir -p "$repo/$component/src"
        printf '[package]\nname = "romstore-%s"\nversion = "0.1.0"\nedition = "2021"\n' "$component" > "$repo/$component/Cargo.toml"
        printf '[[package]]\nname = "romstore-%s"\nversion = "0.1.0"\n' "$component" > "$repo/$component/Cargo.lock"
        printf 'fn main() {}\n' > "$repo/$component/src/main.rs"
        cp -- "$SOURCE_ROOT/$component/release.toml" "$repo/$component/"
    done
    printf 'Demo\n' > "$repo/client/config.example.yaml"
    cd "$repo"
    git init -q -b main
    git config commit.gpgsign false
    git config tag.gpgsign false
    git config core.hooksPath /dev/null
    git add .
    git commit -qm Initial
    : > "$FAKE_CARGO_LOG"
    : > "$CURL_LOG"
}

baseline() {
    git tag client-v0.1.0
    git tag server-v0.1.0
}

change() {
    mkdir -p -- "$(dirname -- "$1")"
    printf '%s\n' "${2:-change}" > "$1"
    git add .
    git commit -qm "$1"
}

fail() { echo "ÉCHEC : $*" >&2; exit 1; }
expect_failure() {
    if "$@" > "$temporary/failure.log" 2>&1; then fail "Succès inattendu : $*"; fi
}

pending() {
    # Sourcer dans un sous-shell pour recalculer ROOT depuis la fixture.
    (
        source scripts/ci/common.sh
        for component in client server; do
            if component_pending "$component"; then echo "$component"; fi
        done
    )
}

test_first_release() {
    fixture
    [[ $(pending) == $'client\nserver' ]] || fail 'Première release : les deux composants sont attendus'
}

test_no_changes() {
    fixture
    baseline
    [[ -z $(pending) ]] || fail 'Changements détectés sans modification'
}

test_component_only() {
    for component in client server; do
        fixture
        baseline
        change "$component/file.txt"
        [[ $(pending) == "$component" ]] || fail 'Mauvais composant détecté'
    done
}

test_cumulative_changes() {
    fixture
    baseline
    change client/file.txt
    change server/file.txt
    [[ $(pending) == $'client\nserver' ]] || fail 'Le dernier push masque des modifications antérieures'
}

test_shared_changes_and_independent_tags() {
    fixture
    baseline
    change README.md
    [[ $(pending) == $'client\nserver' ]] || fail 'Les fichiers partagés concernent les deux composants'
    git tag client-v0.1.1
    [[ $(pending) == server ]] || fail 'Le client a déjà publié ce changement partagé'
}

test_rename_and_deletion() {
    fixture
    change client/file.txt
    baseline
    git mv client/file.txt server/file.txt
    git commit -qm Move
    [[ $(pending) == $'client\nserver' ]] || fail 'Renommage entre composants non détecté'
    git tag client-v0.1.1
    git tag server-v0.1.1
    git rm -q server/file.txt
    git commit -qm Delete
    [[ $(pending) == server ]] || fail 'Suppression non détectée'
}

test_invalid_and_unreachable_tags() {
    fixture
    baseline
    change client/file.txt
    git tag client-v-invalid
    git switch -qc other
    change server/file.txt
    git tag client-v9.0.0
    git switch -q main
    [[ $(pending) == client ]] || fail 'Tag invalide ou non ancêtre retenu'
}

test_shallow_history() {
    fixture
    clone="$temporary/shallow"
    git clone -q --depth 1 "file://$repo" "$clone"
    cd "$clone"
    expect_failure bash -ec 'source scripts/ci/common.sh; component_pending client'
}

test_release_guards() {
    fixture
    expect_failure bash scripts/ci/prepare-release.sh client fix --push "$(git rev-parse HEAD)"
    for source in merge_request_event parent_pipeline; do
        expect_failure env CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE="$source" CI_COMMIT_REF_PROTECTED=true \
            bash scripts/ci/prepare-release.sh client fix --push "$(git rev-parse HEAD)"
    done
    expect_failure env CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE=push CI_COMMIT_REF_PROTECTED=false \
        bash scripts/ci/prepare-release.sh client fix --push "$(git rev-parse HEAD)"
    expect_failure make release-client TYPE=patch
    change client/Cargo.toml $'[package]\nname = "romstore-client"\nversion = "0.2.0-rc.1"\nedition = "2021"'
    expect_failure env CARGO="$temporary/bin/cargo-test" make release-client TYPE=fix
    fixture
    printf 'dirty\n' > notes.txt
    expect_failure env CARGO="$temporary/bin/cargo-test" make release-client TYPE=fix
    fixture
    git switch -q --detach
    expect_failure env CARGO="$temporary/bin/cargo-test" make release-client TYPE=fix
}

test_release_commit_workflow_rule() {
    local rule pattern message
    rule=$(grep 'CI_COMMIT_TITLE =~' "$SOURCE_ROOT/.gitlab-ci.yml")
    pattern=${rule#* =~ /}
    pattern=${pattern%/\'}
    for component in client server; do
        message=$(sed -n 's/^pre-release-commit-message = "\(.*\)"$/\1/p' "$SOURCE_ROOT/$component/release.toml")
        message=${message//\{\{version\}\}/1.2.3}
        [[ $message =~ $pattern ]] || fail 'Le commit de version déclencherait une pipeline sur main'
    done
    for message in 'feat(client): nouvelle fonction' 'chore(client): prépare la version invalide'; do
        if [[ $message =~ $pattern ]]; then fail 'Un commit ordinaire serait ignoré'; fi
    done
    [[ $rule == *'CI_PIPELINE_SOURCE == "push"'* && $rule == *'CI_COMMIT_BRANCH == "main"'* ]] || fail 'Exclusion trop large des pipelines de release'
    # La règle du tag doit précéder celle du commit de version.
    [[ $(grep -m1 'if:' "$SOURCE_ROOT/.gitlab-ci.yml") == *'CI_COMMIT_TAG =~'* ]] || fail 'La pipeline de tag doit rester prioritaire'
}

test_release_without_changes() {
    fixture
    baseline
    remote="$temporary/remote-no-changes.git"
    git init -q --bare "$remote"
    git remote add origin "$remote"
    git push -qu origin main --tags
    git config "url.file://$remote.insteadOf" https://oauth2@gitlab.example.test/project.git
    expect_failure env CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE=web CI_COMMIT_REF_PROTECTED=true \
        CI_PROJECT_URL=https://gitlab.example.test/project GITLAB_RELEASE_TOKEN=dummy-token \
        bash scripts/ci/prepare-release.sh client fix --push "$(git rev-parse HEAD)"
    grep -q 'Aucun changement à publier' "$temporary/failure.log"
    [[ ! -s $FAKE_CARGO_LOG ]] || fail 'Cargo appelé sans changement à publier'
}

test_stale_pipeline_remote_guard() {
    fixture
    remote="$temporary/remote-guard.git"
    git init -q --bare "$remote"
    git remote add origin "$remote"
    git push -qu origin main
    git config "url.file://$remote.insteadOf" https://oauth2@gitlab.example.test/project.git
    original=$(git rev-parse HEAD)
    change client/file.txt
    git push -q origin main
    git switch -q --detach "$original"
    expect_failure env CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE=push CI_COMMIT_REF_PROTECTED=true \
        CI_PROJECT_URL=https://gitlab.example.test/project GITLAB_RELEASE_TOKEN=dummy-token \
        bash scripts/ci/prepare-release.sh client fix --push "$original"
    grep -q 'Pipeline obsolète' "$temporary/failure.log"
    [[ -z $(git tag --list) ]] || fail 'Un tag a été créé depuis une pipeline obsolète'
}

test_release_authentication_ignores_runner_credentials() {
    fixture
    helpers="$temporary/remote-helpers"
    mkdir "$helpers"
    export AUTH_URL_LOG="$temporary/auth-url.log" AUTH_CREDENTIALS="$temporary/auth-credentials"
    cat > "$helpers/git-remote-https" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$2" > "$AUTH_URL_LOG"
printf 'url=%s\n\n' "$2" | git credential fill > "$AUTH_CREDENTIALS"
# Arrêter avant tout échange réseau : seul le choix des credentials est testé.
exit 1
EOF
    chmod +x "$helpers/git-remote-https"
    git remote add origin https://gitlab.example.test/project.git
    git remote set-url --push origin https://gitlab-ci-token:runner-token@gitlab.example.test/project.git
    git config url.https://gitlab-ci-token:runner-token@gitlab.example.test/.insteadOf https://gitlab.example.test/
    git config 'http.https://gitlab.example.test/.extraHeader' 'Authorization: Basic runner-credentials'
    git config 'credential.https://gitlab.example.test.helper' '!echo password=runner-token'
    # Les runners récents interdisent les prompts, y compris GIT_ASKPASS.
    git config credential.interactive false
    expect_failure env GIT_EXEC_PATH="$helpers" CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE=push \
        CI_COMMIT_REF_PROTECTED=true CI_PROJECT_URL=https://gitlab.example.test/project \
        GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=credential.interactive GIT_CONFIG_VALUE_0=false \
        GIT_ASKPASS=/bin/false GITLAB_RELEASE_TOKEN=release-token \
        bash scripts/ci/prepare-release.sh client fix --push "$(git rev-parse HEAD)"
    [[ $(cat "$AUTH_URL_LOG") == https://oauth2@gitlab.example.test/project.git ]] || fail 'URL réécrite avec le jeton du runner'
    grep -qx 'username=oauth2' "$AUTH_CREDENTIALS"
    grep -qx 'password=release-token' "$AUTH_CREDENTIALS"
    [[ $(git remote get-url --push origin) == https://oauth2@gitlab.example.test/project.git ]] || fail 'Ancienne URL de push conservée'
    [[ -z $(git config --get-urlmatch http.extraHeader https://oauth2@gitlab.example.test/project.git) ]] || fail 'En-tête HTTP du runner conservé'
    if grep -q release-token .git/config; then fail 'Jeton de release enregistré dans la configuration Git'; fi
}

test_denied_push_fails_before_preparing_release() {
    fixture
    remote="$temporary/remote-denied.git"
    git init -q --bare "$remote"
    git remote add origin "$remote"
    git push -qu origin main
    git config "url.file://$remote.insteadOf" https://oauth2@gitlab.example.test/project.git
    git config remote.origin.receivepack false
    original=$(git rev-parse HEAD)
    expect_failure env CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE=push CI_COMMIT_REF_PROTECTED=true \
        CI_PROJECT_URL=https://gitlab.example.test/project GITLAB_RELEASE_TOKEN=dummy-token \
        bash scripts/ci/prepare-release.sh client fix --push "$original"
    grep -q 'Accès Git en écriture refusé' "$temporary/failure.log"
    [[ $(git rev-parse HEAD) == "$original" ]] || fail 'Commit créé malgré un refus de push'
    [[ -z $(git tag --list) ]] || fail 'Tag créé malgré un refus de push'
}

test_make_uses_cargo_release_without_push_or_publish() {
    for type in fix minor major; do
        fixture
        CARGO="$temporary/bin/cargo-test" make -s release-client TYPE="$type" > /dev/null
        level=$type
        if [[ $type == fix ]]; then level=patch; fi
        grep -q "release $level --manifest-path client/Cargo.toml --no-publish --no-push --execute --no-confirm" "$FAKE_CARGO_LOG"
    done
    fixture
    CARGO="$temporary/bin/cargo-test" make -s release-server TYPE=fix > /dev/null
    grep -q 'release patch --manifest-path server/Cargo.toml' "$FAKE_CARGO_LOG"
}

test_signing_key_and_hooks() {
    fixture
    git config user.signingkey TEST-KEY
    CARGO="$temporary/bin/cargo-test" make -s release-client TYPE=fix > /dev/null
    grep -q -- '--sign$' "$FAKE_CARGO_LOG"
    CARGO="$temporary/bin/cargo-test" bash scripts/ci/check-release.sh client
    grep -q 'clippy --locked --manifest-path client/Cargo.toml --all-targets -- -D warnings' "$FAKE_CARGO_LOG"
    expect_failure env CARGO="$temporary/bin/cargo-test" FAIL_CARGO_COMMAND=clippy bash scripts/ci/check-release.sh client
    printf 'unexpected\n' > notes.txt
    expect_failure env CARGO="$temporary/bin/cargo-test" bash scripts/ci/check-release.sh client
}

package_fixture() {
    git tag client-v0.1.0
    mkdir -p client/target/release
    # En-tête synthétique ELF64 little-endian avec e_machine = EM_X86_64.
    printf '\177ELF\002\001%012d\076\000' 0 > client/target/release/romstore-client
    chmod +x client/target/release/romstore-client
    CARGO="$temporary/bin/cargo-test" CI_API_V4_URL=https://gitlab.example.test/api/v4 CI_PROJECT_ID=42 \
        bash scripts/ci/package.sh client-v0.1.0 > /dev/null
}

test_packaging() {
    fixture
    package_fixture
    (cd dist/client-v0.1.0 && sha256sum --status -c SHA256SUMS)
    tar -tzf dist/client-v0.1.0/*.tar.gz > "$temporary/archive.list"
    grep -q 'romstore-client-0.1.0-linux-x86_64/docs/ci.md' "$temporary/archive.list"
    grep -q 'config.example.yaml' "$temporary/archive.list"
    if grep -q server "$temporary/archive.list"; then fail 'Archive client contaminée'; fi
    expect_failure env CARGO="$temporary/bin/cargo-test" bash scripts/ci/package.sh client-v9.0.0
    expect_failure env CARGO="$temporary/bin/cargo-test" bash scripts/ci/package.sh ../client-v0.1.0
    printf 'invalid ELF\n' > client/target/release/romstore-client
    expect_failure env CARGO="$temporary/bin/cargo-test" bash scripts/ci/package.sh client-v0.1.0
}

release_files() {
    PATH="$temporary/bin:$PATH" CARGO="$temporary/bin/cargo-test" \
        CI_PROJECT_URL=https://gitlab.example.test/group/project CI_JOB_ID=123 \
        bash scripts/ci/package.sh client-v0.1.0 --release
}

verify_publication() {
    PATH="$temporary/bin:$PATH" CI_COMMIT_TAG=client-v0.1.0 CI_JOB_TOKEN=dummy-token \
        CI_API_V4_URL=https://gitlab.example.test/api/v4 CI_PROJECT_ID=42 \
        CI_PROJECT_URL=https://gitlab.example.test/group/project \
        RELEASE_ARCHIVE=romstore-client-0.1.0-linux-x86_64.tar.gz \
        RELEASE_ASSET_BASE_URL=https://gitlab.example.test/group/project/-/jobs/123/artifacts/raw/dist/client-v0.1.0 \
        bash scripts/ci/verify-release.sh
}

test_release_files_and_publication_guards() {
    fixture
    package_fixture
    printf 'Notes\n' > dist/client-v0.1.0/CHANGELOG.md
    release_files
    grep -qx 'RELEASE_ARCHIVE=romstore-client-0.1.0-linux-x86_64.tar.gz' dist/release.env
    grep -qx 'RELEASE_ASSET_BASE_URL=https://gitlab.example.test/group/project/-/jobs/123/artifacts/raw/dist/client-v0.1.0' dist/release.env
    [[ ! -s $CURL_LOG ]] || fail 'Appel réseau pendant la préparation des artifacts'
    verify_publication
    [[ $(wc -l < "$CURL_LOG") == 1 ]] || fail 'Seule la vérification de release existante est attendue'
    if grep -q 'packages/generic\|--upload-file' "$CURL_LOG"; then fail 'Utilisation du registre de paquets'; fi
    : > "$CURL_LOG"
    export MOCK_RELEASE_STATUS=200
    expect_failure verify_publication
    unset MOCK_RELEASE_STATUS
    : > "$CURL_LOG"
    export MOCK_CURL_FAIL=1
    expect_failure verify_publication
    unset MOCK_CURL_FAIL
    printf 'corruption\n' >> dist/client-v0.1.0/romstore-client-0.1.0-linux-x86_64.tar.gz
    expect_failure release_files
    expect_failure verify_publication
    rm dist/client-v0.1.0/SHA256SUMS
    : > "$CURL_LOG"
    expect_failure release_files
    expect_failure verify_publication
    [[ ! -s $CURL_LOG ]] || fail 'Appel API malgré un fichier absent'
}

test_changelog_with_git_cliff() {
    fixture
    baseline
    change client/file.txt 'First feature'
    git tag server-v0.9.0
    change client/second.txt
    git tag client-v0.2.0
    change client/later.txt
    CI_PROJECT_URL=https://gitlab.example.test/group/project bash scripts/ci/changelog.sh client-v0.2.0
    notes=dist/client-v0.2.0/CHANGELOG.md
    grep -q 'depuis client-v0.1.0' "$notes"
    grep -q '/-/compare/client-v0.1.0...client-v0.2.0' "$notes"
    grep -q 'client/file.txt' "$notes"
    if grep -q 'client/later.txt' "$notes"; then fail 'Commit postérieur au tag inclus'; fi
    bash scripts/ci/changelog.sh client-v0.1.0
    grep -q 'Première release' dist/client-v0.1.0/CHANGELOG.md
    grep -q Initial dist/client-v0.1.0/CHANGELOG.md
    git tag client-v0.2.1 client-v0.2.0
    bash scripts/ci/changelog.sh client-v0.2.1
    grep -q 'Aucun nouveau commit' dist/client-v0.2.1/CHANGELOG.md
}

test_real_cargo_release_and_atomic_push() {
    fixture
    # Les métadonnées/locks de la petite fixture sont générés par le vrai Cargo.
    cargo generate-lockfile --manifest-path client/Cargo.toml
    cargo generate-lockfile --manifest-path server/Cargo.toml
    git add .
    git commit -qm Locks
    remote="$temporary/remote.git"
    git init -q --bare "$remote"
    git remote add origin "$remote"
    git push -qu origin main
    git config "url.file://$remote.insteadOf" https://oauth2@gitlab.example.test/project.git
    original=$(git rev-parse HEAD)
    CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE=push CI_COMMIT_REF_PROTECTED=true \
        CI_PROJECT_URL=https://gitlab.example.test/project GITLAB_RELEASE_TOKEN=dummy-token \
        bash scripts/ci/prepare-release.sh client fix --push "$original"
    [[ $(git show HEAD:client/Cargo.toml | grep '^version') == 'version = "0.1.1"' ]] || fail 'Version incorrecte'
    [[ $(git cat-file -t client-v0.1.1) == tag ]] || fail 'Tag non annoté'
    [[ $(git --git-dir="$remote" rev-parse main) == "$(git rev-parse HEAD)" ]] || fail 'Branche non poussée'
    [[ $(git --git-dir="$remote" rev-parse 'client-v0.1.1^{commit}') == "$(git rev-parse HEAD)" ]] || fail 'Tag non poussé'
    [[ $(git show HEAD:server/Cargo.toml | grep '^version') == 'version = "0.1.0"' ]] || fail 'Version serveur modifiée'
    expect_failure env CI_COMMIT_BRANCH=main CI_PIPELINE_SOURCE=push CI_COMMIT_REF_PROTECTED=true \
        CI_PROJECT_URL=https://gitlab.example.test/project GITLAB_RELEASE_TOKEN=dummy-token \
        bash scripts/ci/prepare-release.sh server fix --push "$original"
}

test_real_make_local_release_and_failed_hook() {
    fixture
    cargo generate-lockfile --manifest-path client/Cargo.toml
    cargo generate-lockfile --manifest-path server/Cargo.toml
    git add .
    git commit -qm Locks
    remote="$temporary/remote-local.git"
    git init -q --bare "$remote"
    git remote add origin "$remote"
    git push -qu origin main
    original=$(git rev-parse HEAD)
    make -s release-server TYPE=minor
    [[ $(git cat-file -t server-v0.2.0) == tag ]] || fail 'Tag serveur manquant'
    [[ $(git --git-dir="$remote" rev-parse main) == "$original" ]] || fail 'La cible Make locale a poussé un commit'
    [[ -z $(git --git-dir="$remote" tag --list) ]] || fail 'La cible Make locale a poussé un tag'
    [[ $(git show HEAD:client/Cargo.toml | grep '^version') == 'version = "0.1.0"' ]] || fail 'Version client modifiée'
    change client/src/main.rs 'fn main(){println!("bad formatting");}'
    original=$(git rev-parse HEAD)
    expect_failure make -s release-client TYPE=fix
    [[ $(git rev-parse HEAD) == "$original" ]] || fail 'Un commit a été créé malgré un hook en échec'
    [[ -z $(git tag --list 'client-v*') ]] || fail 'Un tag a été créé malgré un hook en échec'
}

count=0
for script in "$SOURCE_ROOT"/scripts/ci/*.sh; do bash -n "$script"; done
for test in test_first_release test_no_changes test_component_only test_cumulative_changes \
    test_shared_changes_and_independent_tags test_rename_and_deletion test_invalid_and_unreachable_tags \
    test_shallow_history test_release_guards test_release_commit_workflow_rule \
    test_release_without_changes test_stale_pipeline_remote_guard \
    test_release_authentication_ignores_runner_credentials test_denied_push_fails_before_preparing_release \
    test_make_uses_cargo_release_without_push_or_publish \
    test_signing_key_and_hooks test_packaging test_release_files_and_publication_guards; do
    ("$test")
    echo "OK : $test"
    count=$((count + 1))
done
if command -v git-cliff >/dev/null; then
    (test_changelog_with_git_cliff)
    echo 'OK : git-cliff réel'
    count=$((count + 1))
else
    echo 'SKIP : intégration git-cliff (make release-tools pour l’activer)'
fi
if cargo release --version >/dev/null 2>&1; then
    (test_real_cargo_release_and_atomic_push)
    echo 'OK : cargo-release réel et push atomique vers un dépôt local isolé'
    (test_real_make_local_release_and_failed_hook)
    echo 'OK : cibles Make réelles sans push et hook bloquant en cas d’échec'
    count=$((count + 2))
else
    echo 'SKIP : intégration cargo-release (make release-tools pour l’activer)'
fi
echo "$count groupes de tests réussis."
