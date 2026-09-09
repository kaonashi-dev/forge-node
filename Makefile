# Forge — development shortcuts
#
# Primary GUI: Tauri + Solid (`apps/tauri`). Rust daemon + workspace live under
# `crates/`. Canonical Rust gate: `scripts/dev check` (also `make check`).
#
# Quick start:
#   make install-ui     # pnpm install in apps/tauri
#   make dev            # forge-daemon (debug) + Tauri dev shell
#   make build-release  # release workspace + installable Tauri bundle
#   make install-local  # release bundle -> Applications, then relaunch
#
SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c

ROOT        := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
TAURI_APP   := $(ROOT)/apps/tauri
DAEMON_DBG  := $(ROOT)/target/debug/forge-daemon
DAEMON_REL  := $(ROOT)/target/release/forge-daemon
ICNS        := $(ROOT)/assets/icon/forge.icns

UNAME_S := $(shell uname -s)
IS_DARWIN := $(if $(filter Darwin,$(UNAME_S)),1,)

.DEFAULT_GOAL := help

.PHONY: help \
        install-ui dev dev-vite run run-all run-ui run-tauri run-daemon latency-tauri \
        build build-frontend build-ui build-tauri build-tauri-debug build-tauri-release build-release \
        build-rust build-rust-release build-daemon \
        test test-rust test-tauri check check-fast \
        codegen tokens fixtures fmt clippy clean clean-rust clean-ui clean-tauri \
        package package-linux dist install-local info

help: ## Show this help
	@printf 'Forge make targets (macOS=%s)\n\n' "$(UNAME_S)"
	@awk 'BEGIN {FS = ":.*##"; printf "Usage: make <target>\n\nTargets:\n"} \
		/^[a-zA-Z0-9_.-]+:.*##/ { printf "  %-22s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@printf '\nTauri workflow:\n'
	@printf '  make install-ui          # once: pnpm install\n'
	@printf '  make dev                   # daemon + Tauri dev (hot reload)\n'
	@printf '  make dev-vite              # Vite only on :1420 (no Rust host)\n'
	@printf '  make build-frontend        # tsc + vite build\n'
	@printf '  make build-tauri           # debug installable bundle\n'
	@printf '  make build-release         # release workspace + bundle\n'
	@printf '  make package               # macOS Forge.app (scripts/package-macos)\n'
	@printf '  make install-local         # rebuild, replace /Applications app, relaunch\n'
	@printf '\nGate:\n'
	@printf '  make check                 # fmt + clippy + rust + vitest + tsc\n'

# ----------------------------------------------------------- Tauri: setup ---

install-ui: ## Install frontend deps (`pnpm install --frozen-lockfile`)
	cd "$(TAURI_APP)" && pnpm install --frozen-lockfile

# ------------------------------------------------------------ Tauri: dev ---

dev: run-tauri ## Daemon (debug) + Tauri dev shell — primary dev entry

dev-vite: ## Vite dev server only (http://localhost:1420, no Rust host)
	cd "$(TAURI_APP)" && pnpm dev

run: dev ## Alias for `make dev`

run-all: dev ## Alias for `make dev`

run-ui: dev ## Alias for `make dev`

run-tauri: build-daemon ## Build daemon, then `pnpm tauri dev`
	cd "$(TAURI_APP)" && FORGE_DAEMON_BIN="$(DAEMON_DBG)" pnpm tauri dev

run-daemon: ## Run forge-daemon in foreground (verbose logs)
	RUST_LOG="$${RUST_LOG:-forge=debug,daemon=debug,info}" "$(ROOT)/scripts/dev" daemon

# Never part of `check`: opens a shell on the running daemon and types into it.
latency-tauri: ## Phase 2 gate: terminal key-to-render p95 (needs a live daemon)
	cargo run -p forge-tauri --bin forge-tauri-latency

# --------------------------------------------------------- Tauri: build ---

build: build-rust build-frontend ## Rust workspace + Tauri frontend (no installer)

build-frontend: ## Typecheck + Vite bundle (`apps/tauri/dist`)
	cd "$(TAURI_APP)" && pnpm build

build-ui: build-tauri ## Alias: full Tauri debug bundle

build-tauri: build-tauri-debug ## Debug installable bundle (.app / .deb / etc.)

build-tauri-debug: build-daemon ## Debug Tauri bundle (`pnpm tauri build --debug`)
	cd "$(TAURI_APP)" && FORGE_DAEMON_BIN="$(DAEMON_DBG)" pnpm tauri build --debug

build-tauri-release: build-rust-release ## Release installable bundle
	cd "$(TAURI_APP)" && FORGE_DAEMON_BIN="$(DAEMON_REL)" pnpm tauri build

build-release: build-tauri-release ## Release Rust workspace + Tauri bundle

build-rust: ## `cargo build --workspace`
	cargo build --workspace

build-rust-release: ## `cargo build --workspace --release`
	cargo build --workspace --release

build-daemon: ## Build forge-daemon only (debug)
	cargo build -p daemon --bin forge-daemon

# --------------------------------------------------------- Tauri: codegen ---

codegen: ## Regenerate typed protocol copies for round-trip tests
	cd "$(TAURI_APP)" && pnpm codegen

fixtures: ## Export MessagePack protocol fixtures into apps/tauri/tests/fixtures
	cargo run -p protocol --bin export-fixtures

# ----------------------------------------------------------- Tauri: test ---

test: test-rust test-tauri ## Run all Rust and Tauri tests

test-rust: ## `cargo test --workspace`
	cargo test --workspace

test-tauri: ## Vitest + tsc + cargo test -p forge-tauri
	cd "$(TAURI_APP)" && pnpm test
	cd "$(TAURI_APP)" && pnpm exec tsc --noEmit
	cargo test -p forge-tauri

# The typecheck is here and not only in `test-tauri` because vitest only sees
# the modules a test imports: a rename that misses a caller is a tsc error and
# a green test run.
check: ## Full gate: fmt + clippy + rust tests + tauri tests + typecheck
	"$(ROOT)/scripts/dev" check
	cd "$(TAURI_APP)" && pnpm lint
	cd "$(TAURI_APP)" && pnpm test
	cd "$(TAURI_APP)" && pnpm exec tsc --noEmit
	cargo check -p forge-tauri --all-targets

check-fast: ## Rust fmt + clippy + tests only (no frontend)
	"$(ROOT)/scripts/dev" check

# ------------------------------------------------------ Tauri: lint / fmt ---

lint: ## Oxlint Tauri frontend (`apps/tauri`)
	cd "$(TAURI_APP)" && pnpm lint

lint-fix: ## Oxlint --fix
	cd "$(TAURI_APP)" && pnpm lint:fix

fmt-tauri: ## Format Tauri frontend (`oxfmt --write`)
	cd "$(TAURI_APP)" && pnpm fmt

fmt-check-tauri: ## Check Tauri formatting (`oxfmt --check`)
	cd "$(TAURI_APP)" && pnpm fmt:check

fmt-tauri-check: ## Alias for fmt-check-tauri
	cd "$(TAURI_APP)" && pnpm fmt:check

# ---------------------------------------------------------- Tauri: clean ---

clean: clean-rust clean-tauri ## Remove Rust and Tauri build artifacts

clean-rust: ## `cargo clean`
	cargo clean

clean-ui: clean-tauri ## Alias for clean-tauri

clean-tauri: ## Remove Tauri frontend dist and local target cache
	rm -rf "$(TAURI_APP)/dist" "$(TAURI_APP)/src-tauri/target"

# ------------------------------------------------------- Tauri: package ---

package: ## Release Forge.app (`scripts/package-macos`)
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

# --------------------------------------------------------- Rust utilities ---

fmt: ## `cargo fmt --all`
	cargo fmt --all

clippy: ## `cargo clippy --workspace --all-targets -- -D warnings`
	cargo clippy --workspace --all-targets -- -D warnings

info: ## Print resolved paths and toolchain
	@printf 'ROOT=%s\n' "$(ROOT)"
	@printf 'UNAME_S=%s\n' "$(UNAME_S)"
	@printf 'TAURI_APP=%s\n' "$(TAURI_APP)"
	@printf 'DAEMON_DBG=%s\n' "$(DAEMON_DBG)"
	@printf 'DAEMON_REL=%s\n' "$(DAEMON_REL)"
	@command -v pnpm >/dev/null && printf 'pnpm %s\n' "$$(pnpm --version)" || printf 'pnpm: not found\n'
	@command -v cargo >/dev/null && cargo --version || printf 'cargo: not found\n'
