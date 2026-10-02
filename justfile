# Development tasks. `just check` is the local mirror of the CI gate.
set shell := ["bash", "-uc"]

formatter := "nightly-2026-01-23"

default: check

# Install hooks after installing the tools documented in the build guide.
setup:
	lefthook install

fmt:
	cargo +{{formatter}} fmt --all

fmt-check:
	cargo +{{formatter}} fmt --all -- --check
	taplo format --check

build:
	cargo build --workspace --locked

lint:
	cargo clippy --workspace --exclude libp2p-dns --exclude netlink-packet-core --exclude arc-malachitebft-discovery --all-targets --locked -- -D warnings

test:
	cargo nextest run --workspace --locked
	cargo test --workspace --locked --doc

deny:
	cargo deny check

# Scope the iteration loop to a workspace package.
test-crate crate="rinpqc-node" *ARGS:
	cargo nextest run --locked -p {{crate}} {{ARGS}}

check: fmt-check build lint test deny
