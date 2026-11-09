# Forge — development shortcuts
#
# Primary GUI: Rune (`apps/ui`, `forge-ui`). Rust daemon and workspace live
# under `crates/`. Canonical gate: `scripts/dev check` (also `make check`).
#
# Quick start:
#   make run-rune        # Rune client on recorded fixture data
#   make run-live        # Rune client against the daily daemon socket
#   make open-rune       # build a review bundle and open it (own data)
#   make package         # release bundle in dist/macos
#
SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c

ROOT        := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
DAEMON_DBG  := $(ROOT)/target/debug/forge-daemon
DAEMON_REL  := $(ROOT)/target/release/forge-daemon
ICNS        := $(ROOT)/assets/icon/forge.icns

UNAME_S := $(shell uname -s)
IS_DARWIN := $(if $(filter Darwin,$(UNAME_S)),1,)

# xcrun can pick an SDK newer than this ld; scripts/macos-sdk skips those.
ifeq ($(IS_DARWIN),1)
SDKROOT := $(shell "$(ROOT)/scripts/macos-sdk")
ifeq ($(strip $(SDKROOT)),)
$(error no linker-compatible macOS SDK — see scripts/macos-sdk)
endif
export SDKROOT
endif

.DEFAULT_GOAL := help

.PHONY: help dev daemon run run-live run-rune build build-rust build-daemon \
        build-rune build-rune-debug app-rune open-rune test check check-fast \
        fmt clippy clean clean-rust clean-ui package package-linux dist \
        install-local info

help: ## Show this help
	@printf 'Forge make targets (macOS=%s)\n\n' "$(UNAME_S)"
	@awk 'BEGIN {FS = ":.*##"; printf "Usage: make <target>\n\nTargets:\n"} \
		/^[a-zA-Z0-9_.-]+:.*##/ { printf "  %-22s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@printf '\nRune client (apps/ui, own toolchain):\n'
	@printf '  make run-rune              # recorded fixture, no daemon\n'
	@printf '  make run-live              # connect to the daily daemon\n'
	@printf '  make open-rune             # build Forge Node Rune.app and open it\n'
	@printf '  make build-rune            # release forge-ui\n'
	@printf '  make check-rune            # apps/ui gate\n'
	@printf '\nGate:\n'
	@printf '  make check                 # root + apps/ui (fmt, clippy, tests)\n'

dev: run-live ## Alias for run-live

daemon: ## Run forge-daemon in the foreground with an isolated scratch home
	"$(ROOT)/scripts/dev" daemon

run: run-rune ## Alias for run-rune

run-live: ## Run the Rune client against the daemon FORGE_SOCKET names
	"$(ROOT)/scripts/dev" ui run --daemon

run-rune: ## Run the Rune client on recorded fixture data (no daemon)
	"$(ROOT)/scripts/dev" ui run

build: build-rust build-rune ## Build the workspace and the Rune client

build-rust: ## `cargo build --workspace`
	cargo build --workspace

build-daemon: ## Build forge-daemon only (debug)
	cargo build -p daemon --bin forge-daemon

build-rune: ## Release build of the Rune client (`forge-ui`)
	"$(ROOT)/scripts/dev" ui build

build-rune-debug: ## Debug build of the Rune client
	cd "$(ROOT)" && RUSTUP_TOOLCHAIN="$$(sed -n 's/^channel = "\(.*\)"/\1/p' apps/ui/rust-toolchain.toml)" \
		cargo build --manifest-path apps/ui/Cargo.toml

app-rune: ## Build dist/rune/Forge Node Rune.app with its own daemon and data
	"$(ROOT)/scripts/rune-app"

open-rune: ## Build Forge Node Rune.app and open it
	"$(ROOT)/scripts/rune-app" --open

test: ## `cargo test --workspace` plus the Rune client tests
	cargo test --workspace
	"$(ROOT)/scripts/dev" ui check

check: ## Full gate: root workspace + Rune client
	"$(ROOT)/scripts/dev" check

check-rune: ## Rune gate: fmt + clippy + tests for apps/ui
	"$(ROOT)/scripts/dev" ui check

check-fast: ## Root workspace gate only
	"$(ROOT)/scripts/dev" check --root-only

fmt: ## `cargo fmt --all` (root and Rune client)
	cargo fmt --all
	cd "$(ROOT)" && RUSTUP_TOOLCHAIN="$$(sed -n 's/^channel = "\(.*\)"/\1/p' apps/ui/rust-toolchain.toml)" \
		cargo fmt --manifest-path apps/ui/Cargo.toml --all

clippy: ## `cargo clippy --workspace --all-targets -- -D warnings`
	cargo clippy --workspace --all-targets -- -D warnings

clean: clean-rust clean-ui ## Remove Rust and Rune build artifacts
	cargo clean

clean-rust: ## `cargo clean`
	cargo clean

clean-ui: ## Remove the Rune client's target directory
	rm -rf "$(ROOT)/apps/ui/target"

package: ## Release Forge Node.app (`scripts/package-macos`)
ifndef IS_DARWIN
	$(error package requires macOS)
endif
	"$(ROOT)/scripts/package-macos"

# Never run by CI or by `scripts/dev`: build on the oldest distro you intend
# to support — glibc-linked binaries run forward, not backward.
package-linux: ## Release .deb and AppImage (`scripts/package-linux`)
ifdef IS_DARWIN
	$(error package-linux must run on the Linux host you are targeting)
endif
	"$(ROOT)/scripts/package-linux" all

dist: ## Release tarball (`scripts/dist build`)
ifndef IS_DARWIN
	$(error dist requires macOS)
endif
	"$(ROOT)/scripts/dist" build

install-local: ## Build release, replace the local app, and relaunch it
ifndef IS_DARWIN
	$(error install-local requires macOS)
endif
	"$(ROOT)/scripts/dist" local

info: ## Print resolved paths and toolchain
	@printf 'ROOT=%s\n' "$(ROOT)"
	@printf 'UNAME_S=%s\n' "$(UNAME_S)"
	@printf 'SDKROOT=%s\n' "$${SDKROOT:-<unset>}"
	@printf 'DAEMON_DBG=%s\n' "$(DAEMON_DBG)"
	@printf 'DAEMON_REL=%s\n' "$(DAEMON_REL)"
	@command -v cargo >/dev/null && cargo --version || printf 'cargo: not found\n'
	@printf 'rune toolchain: %s\n' "$$(sed -n 's/^channel = "\(.*\)"/\1/p' apps/ui/rust-toolchain.toml)"
