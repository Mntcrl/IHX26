# XRPL Galaxy monorepo entry point.
# Area targets delegate to owner scripts under scripts/; core-* run cargo directly.
# Override any variable on the command line, e.g. `make ios-run DEVICE=<CoreDevice id>`.

DEVICE     ?= E34AE83F-D43C-510A-94EA-8E032BCD853E
MAC_HOST   ?= unity@10.164.228.154
MAC_KEY    ?= $(HOME)/.ssh/id_ihx26
MAC_MIRROR ?= /Users/unity/Workspace/IXH26/galaxy-build
NETWORK    ?= xrpl-testnet
CARGO      ?= cargo

export DEVICE MAC_HOST MAC_KEY MAC_MIRROR NETWORK

SHELL := bash
.DEFAULT_GOAL := help

# Run an owner script, or fail with exit 2 if it does not exist yet.
# $(1) = script path relative to the repo root, $(2) = arguments.
define owner
	@if [ -x "$(1)" ]; then \
		"./$(1)" $(2); \
	else \
		echo "owner script $(1) not implemented yet" >&2; \
		exit 2; \
	fi
endef

.PHONY: help build install run \
	ios-build ios-install ios-run \
	core-build core-test \
	circuit-build circuit-test circuit-setup-dev \
	node-build node-run node-test \
	verifier-build verifier-run verifier-test \
	test integration-test doctor

help:
	@echo "PHØTON (Galaxy v1 protocol) — targets:"
	@echo "  doctor                        environment report (scripts/doctor.sh)"
	@echo "  core-build core-test          Rust workspace (native; galaxy-wasm checked for wasm32)"
	@echo "  ios-build ios-install ios-run PHØTON capture app on the Mac + device (scripts/ios.sh); build/install/run alias these"
	@echo "  circuit-build circuit-test circuit-setup-dev   circom circuit (scripts/circuit.sh)"
	@echo "  node-build node-run node-test galaxy-node daemon (scripts/node.sh)"
	@echo "  verifier-build verifier-run verifier-test   PHØTON browser verifier (scripts/web.sh)"
	@echo "  test                          core + circuit + node + verifier tests"
	@echo "  integration-test              end-to-end Testnet run (scripts/integration-test.sh)"
	@echo "Variables: DEVICE=$(DEVICE) MAC_HOST=$(MAC_HOST) MAC_MIRROR=$(MAC_MIRROR) NETWORK=$(NETWORK)"

build: ios-build
install: ios-install
run: ios-run

ios-build:
	$(call owner,scripts/ios.sh,build)
ios-install:
	$(call owner,scripts/ios.sh,install)
ios-run:
	$(call owner,scripts/ios.sh,run)

core-build:
	$(CARGO) build --workspace
	$(CARGO) check -p galaxy-wasm --target wasm32-unknown-unknown

core-test:
	$(CARGO) test --workspace --exclude galaxy-wasm

circuit-build:
	$(call owner,scripts/circuit.sh,build)
circuit-test:
	$(call owner,scripts/circuit.sh,test)
circuit-setup-dev:
	$(call owner,scripts/circuit.sh,setup-dev)

node-build:
	$(call owner,scripts/node.sh,build)
node-run:
	$(call owner,scripts/node.sh,run)
node-test:
	$(call owner,scripts/node.sh,test)

verifier-build:
	$(call owner,scripts/web.sh,build)
verifier-run:
	$(call owner,scripts/web.sh,run)
verifier-test:
	$(call owner,scripts/web.sh,test)

test: core-test circuit-test node-test verifier-test

integration-test:
	$(call owner,scripts/integration-test.sh,)

doctor:
	@./scripts/doctor.sh
