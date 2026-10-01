# Building the M1 workspace

The workspace contains one unpublished package, `rinpqc-node`. Modules separate deterministic application logic, the Malachite adapter, and infrastructure. They are boundaries for subsequent issues, not implemented payment or consensus services. Split crates only when an actual dependency boundary requires it.

## Prerequisites

The initial supported build target is Linux x86-64. Install Git, a C/C++ build toolchain, pkg-config, curl and CA certificates (Ubuntu packages: `git build-essential pkg-config curl ca-certificates`). Install Rust through rustup; `rust-toolchain.toml` selects Rust 1.88.0, rustfmt and Clippy. The initial build needs network access to download the toolchain and dependencies. Other operating systems are not yet validated.

From a clean checkout:

```sh
cargo fmt --all -- --check
cargo build --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -- --help
cargo run --locked -- --version
```

Running without arguments, with `start`, or with unsupported arguments exits unsuccessfully. No service, port, validator key, database or consensus loop is created. This prevents mistaking the scaffold for a functioning chain. The CLI integration test checks this startup boundary; payment and consensus tests arrive with their implementations.

## Dependencies and licenses

Malachite is pinned to the immutable revision in [ADR 0001](../decisions/0001-malachite-baseline.md), with default features only. `Cargo.lock` records the resolved transitive versions, source locations and registry checksums. Do not replace the revision with a branch or update the lockfile incidentally.

To inspect the resolved graph and declared license metadata:

```sh
cargo tree --locked
cargo metadata --locked --format-version 1 > dependency-metadata.json
```

The upstream Malachite packages declare Apache-2.0. Transitive packages have their own license declarations in that metadata; this is provenance, not a claim that every dependency shares one license or that a legal audit has been performed. This bootstrap does not choose a license for RinPQC's own code. Dependency changes must review the corresponding manifests and license files.

CI performs formatting, a locked build, Clippy and tests on Ubuntu. Its action revision and Rust toolchain are pinned. No secrets or machine-specific configuration are needed. Create branches using `feat/`, `chore/`, `fix/` or `docs/`.
