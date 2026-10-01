# Building the M1 workspace

The workspace contains the unpublished node package, `rinpqc-node`, and two temporary network compatibility patches described below. Modules separate deterministic application logic, the Malachite adapter, and infrastructure. They are boundaries for subsequent issues, not implemented payment or consensus services. Split crates only when an actual dependency boundary requires it.

## Prerequisites

The initial supported build target is Linux x86-64. Install Git, a C/C++ build toolchain, pkg-config, curl and CA certificates (Ubuntu packages: `git build-essential pkg-config curl ca-certificates`). Install Rust through rustup; `rust-toolchain.toml` selects Rust 1.93.0, rustfmt, Clippy and rust-analyzer. The initial build needs network access to download the toolchain and dependencies. Other operating systems are not yet validated.

Install the pinned development tools after adding Cargo's bin directory to your PATH:

```sh
rustup toolchain install 1.93.0 --profile minimal --component clippy --component rust-analyzer --component rustfmt
rustup toolchain install nightly-2026-01-23 --profile minimal --component rustfmt
cargo install just --version 1.46.0 --locked
cargo install taplo-cli --version 0.10.0 --locked
cargo install cargo-nextest --version 0.9.128 --locked
cargo install cargo-deny --version 0.19.0 --locked
```

Install [Lefthook v2.1.15](https://github.com/evilmartians/lefthook/releases/tag/v2.1.15) for your operating system, verify the release checksum, and place its executable on PATH. It is only needed for local Git hooks; CI runs the same gate directly.

From a clean checkout:

```sh
just setup
just check
cargo run --locked -- --help
cargo run --locked -- --version
```

`just setup` installs Git hooks. Pre-commit formats Rust and TOML when matching files are staged and stages the formatted matching files. Inspect the resulting diff before committing, especially with partially staged files: formatting applies to the workspace and may affect working-tree changes. Pre-push runs only `just fmt-check lint`. The full completion gate is `just check`, both locally and in CI.

| Command | Purpose |
| --- | --- |
| `just fmt` | Format Rust with the pinned nightly rustfmt. |
| `taplo format` | Format TOML, excluding build artifacts. |
| `just fmt-check` | Check Rust and TOML without writes. |
| `just build` | Locked workspace build with stable Rust. |
| `just lint` | Clippy on first-party targets, denying warnings; vendored compatibility patches retain upstream lint policy. |
| `just test` | Nextest unit/integration tests, then Cargo doctests. |
| `just test-crate rinpqc-node` | Run one package's tests during development. |
| `just deny` | Check advisories, declared licenses, duplicate dependencies and source policy. |
| `just check` | Run all CI gates in sequence. |

Rust builds use 1.93.0; only formatting uses `nightly-2026-01-23` because import grouping and comment wrapping need unstable rustfmt options. Pinning the formatter prevents formatting drift. Keep both toolchain pins synchronized between the justfile, toolchain file, CI and these instructions when updating them. Taplo sorts keys and arrays; avoid applying it to future TOML arrays whose ordering has application meaning without a scoped exception.

Running without arguments, with `start`, or with unsupported arguments exits unsuccessfully. The help/version commands create no state. The [identity commands](identities.md) separately generate keys and initialize configuration; no command starts a consensus loop yet. This prevents mistaking the scaffold for a functioning chain. The CLI integration test checks this startup boundary; payment and consensus tests arrive with their implementations.

## Dependencies and licenses

Malachite is pinned to the immutable revision in [ADR 0001](../decisions/0001-malachite-baseline.md), with default features only. `Cargo.lock` records the resolved transitive versions, source locations and registry checksums. Do not replace the revision with a branch or update the lockfile incidentally.

To inspect the resolved graph and declared license metadata:

```sh
cargo tree --locked
cargo metadata --locked --format-version 1 > dependency-metadata.json
```

The upstream Malachite packages declare Apache-2.0. Transitive packages have their own license declarations in that metadata; this is provenance, not a claim that every dependency shares one license or that a legal audit has been performed. RinPQC's own code is covered by the repository's existing [MIT license](../../LICENSE), also declared in `Cargo.toml`. Dependency changes must review the corresponding manifests and license files.

CI installs the same tool versions from SHA-256-pinned upstream release binaries using `scripts/install-ci-tools.sh` and runs `just check` on Ubuntu, including the dependency policy in `deny.toml`. Advisory results depend on the current RustSec database; a new advisory can fail an unchanged lockfile. The existing transitive `attohttpc` 0.30.1 dependency has a version-scoped MPL-2.0 exception; its file-level license obligations still apply. Duplicate versions are warnings because the upstream graph currently includes multiple major versions. Unknown registries and Git repositories are rejected, and Git sources must specify a revision. Its action revision and Rust toolchain are pinned. No secrets or machine-specific configuration are needed. Create branches using `feat/`, `chore/`, `fix/` or `docs/`.

## Dependency remediation

The initial gate exposed two Hickory advisories and unmaintained `paste`. [Temporary network compatibility patches](../../vendor/README.md) migrate the active DNS dependency graph to Hickory 0.26.3 and replace `paste` with `pastey`, without changing the pinned Malachite engine. No advisory ignores or CI bypasses are configured. Patch provenance, tests and removal conditions are recorded alongside their source.
