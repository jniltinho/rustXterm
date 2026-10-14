# Developer entry points. Every recipe runs in Docker (scripts/cargo-docker.sh or docker);
# nothing here needs a host Rust toolchain. See docs/DEVELOPMENT.md.
#
#   make help                       list targets
#   make dist VERSION=0.0.0-test    release artifacts into dist/
#   CARGO_TARGET_DIR=target/agent-foo make test   per-agent target dir

SHELL := /bin/sh
.DEFAULT_GOAL := help

CARGO   := scripts/cargo-docker.sh cargo
IMAGE   ?= rustxterm-toolchain:dev
DIST    ?= dist
VERSION ?=

DENY        := cargo-deny-0.20.2-x86_64-unknown-linux-musl
DENY_SHA256 := 9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f

.PHONY: help toolchain build release dist dist-check test test-ui snapshots fmt fmt-check clippy \
	check layers scenarios guard guard-ci deny lab-up lab-test lab-down prints matrix clean shell

help: ## Show this help
	@awk 'BEGIN { FS = ":.*## " } /^[a-zA-Z_-]+:.*## / { printf "  %-12s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

toolchain: ## Build (or refresh) the pinned toolchain image
	docker build --target toolchain -t $(IMAGE) .

build: ## Debug build of the workspace
	$(CARGO) build --workspace

release: ## Release build (host-glibc binary in $CARGO_TARGET_DIR/release; use `dist` for portable artifacts)
	$(CARGO) build --workspace --release

dist: ## Release artifacts (binary, AppImage, tarball, SHA256SUMS) into DIST=dist; optional VERSION=X.Y.Z
	docker build --target dist $(if $(VERSION),--build-arg RXT_VERSION=$(VERSION)) --output type=local,dest=$(DIST)/ .

dist-check: ## Check the artifacts in DIST=dist (glibc, NEEDED libs, self-test, checksums, guard)
	$(CARGO) xtask dist-check $(DIST)

test: ## Run all workspace tests (lab tests are ignored)
	$(CARGO) test --workspace --locked

test-ui: ## Run the rxt-ui kittest UI and snapshot tests
	$(CARGO) test -p rxt-ui --locked

snapshots: ## Regenerate the rxt-ui snapshot images (UPDATE_SNAPSHOTS=1)
	UPDATE_SNAPSHOTS=1 $(CARGO) test -p rxt-ui --locked

fmt: ## Format the workspace
	$(CARGO) fmt --all

fmt-check: ## Check formatting
	$(CARGO) fmt --all --check

clippy: ## Lint with clippy, warnings are errors
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

check: fmt-check clippy test layers scenarios ## CI checks: fmt-check, clippy, test, layers, scenarios

layers: ## Enforce crate layer rules
	$(CARGO) xtask layers

scenarios: ## Check the spec scenario map
	$(CARGO) xtask scenarios

guard: ## Publication guard with base/ mounted read-only; run before every push
	RXT_MOUNT_BASE=1 $(CARGO) xtask guard

guard-ci: ## Publication guard as CI runs it (no base/)
	$(CARGO) xtask guard --ci

deny: ## cargo deny check with the same pinned, SHA-256 checked binary as CI
	@set -eu; \
	if [ ! -x target/tools/cargo-deny ]; then \
		mkdir -p target/tools; \
		curl --fail --show-error --silent --location --retry 3 \
			"https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/$(DENY).tar.gz" -o target/tools/deny.tgz; \
		echo "$(DENY_SHA256)  target/tools/deny.tgz" | sha256sum --check --strict -; \
		tar -xzf target/tools/deny.tgz -C target/tools --strip-components=1 "$(DENY)/cargo-deny"; \
		rm -f target/tools/deny.tgz; \
	fi
	scripts/cargo-docker.sh target/tools/cargo-deny check

lab-up: ## Start the Docker protocol lab (127.0.0.1 only)
	lab/lab.sh up

lab-test: ## Run the lab integration tests (must match lab/expected-tests.txt)
	lab/lab.sh test

lab-down: ## Stop the lab and remove its volumes
	lab/lab.sh down

prints: ## Write UI screenshots to docs/prints/ (local-only)
	RXT_MOUNT_PRINTS=1 $(CARGO) xtask prints

matrix: ## Distro matrix self-tests and GUI smoke on the artifacts in DIST=dist
	scripts/distro-matrix.sh $(DIST)

clean: ## Delete target/ (all build dirs, cargo home, tools) and dist/ without asking
	rm -rf target $(DIST)

shell: ## Interactive shell in the toolchain container
	scripts/cargo-docker.sh bash
