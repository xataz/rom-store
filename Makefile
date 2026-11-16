.DEFAULT_GOAL := build

CARGO ?= cargo
PROFILE ?= release
PYTHON ?= python3

.PHONY: all build client server test test-client test-server test-unit \
	test-unit-client test-unit-server fixtures run-server run-client help

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
