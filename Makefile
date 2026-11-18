.DEFAULT_GOAL := build

CARGO ?= cargo
PROFILE ?= release
PYTHON ?= python3
CARGO_RELEASE_VERSION ?= 1.0.0
GIT_CLIFF_VERSION ?= 2.10.1

.PHONY: all build client server test test-client test-server test-unit \
	test-unit-client test-unit-server fixtures run-server run-client help \
	release-client release-server release-tools test-ci

all: build

build: client server

client:
	$(CARGO) build --locked --profile $(PROFILE) --manifest-path client/Cargo.toml

server:
	$(CARGO) build --locked --profile $(PROFILE) --manifest-path server/Cargo.toml

test: test-client test-server

test-client:
	$(CARGO) test --locked --manifest-path client/Cargo.toml

test-server:
	$(CARGO) test --locked --manifest-path server/Cargo.toml

test-unit: test-unit-client test-unit-server

test-unit-client:
	$(CARGO) test --locked --manifest-path client/Cargo.toml --lib --bins

test-unit-server:
	$(CARGO) test --locked --manifest-path server/Cargo.toml --lib --bins

release-client:
	CARGO="$(CARGO)" bash scripts/ci/prepare-release.sh client "$(TYPE)"

release-server:
	CARGO="$(CARGO)" bash scripts/ci/prepare-release.sh server "$(TYPE)"

release-tools:
	$(CARGO) install cargo-release --version $(CARGO_RELEASE_VERSION) --locked --no-default-features --features vendored-libgit2
	$(CARGO) install git-cliff --version $(GIT_CLIFF_VERSION) --locked --no-default-features

test-ci:
	bash scripts/ci/test-ci.sh

fixtures:
	$(PYTHON) test/generate.py

run-server:
	$(CARGO) run --locked --profile $(PROFILE) --manifest-path server/Cargo.toml -- \
		serve --library test/library --system-icon test/system-icons \
		--database .romstore/test/catalog.sqlite3

run-client:
	$(CARGO) run --locked --profile $(PROFILE) --manifest-path client/Cargo.toml -- \
		--config test/client.yaml

help:
	@echo 'make [build]       Compiler le client et le serveur (release par défaut)'
	@echo 'make client/server Compiler un seul binaire'
	@echo 'make PROFILE=dev   Compiler en mode développement'
	@echo 'make test          Tests Rust unitaires et intégration (hors tests ignorés)'
	@echo 'make test-unit     Tests unitaires uniquement'
	@echo 'make test-client   Tests Rust du client uniquement'
	@echo 'make test-server   Tests Rust du serveur uniquement'
	@echo 'make run-server    Servir la bibliothèque synthétique sur 127.0.0.1:8000'
	@echo 'make run-client    Ouvrir le client avec la configuration de démonstration'
	@echo 'make fixtures      Régénérer les données synthétiques (Python 3 requis)'
	@echo 'make release-tools Installer cargo-release et git-cliff (versions épinglées)'
	@echo 'make test-ci       Tester les scripts Bash de CI/release'
	@echo 'make release-client TYPE=fix|minor|major  Préparer le commit et le tag client, sans push'
	@echo 'make release-server TYPE=fix|minor|major  Préparer le commit et le tag serveur, sans push'
