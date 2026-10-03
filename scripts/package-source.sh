#!/usr/bin/env bash
# Bundle exact Rust sources (including patched and registry dependencies) for distribution.
set -euo pipefail
if [[ $# != 1 ]]; then
    echo 'Usage: bash scripts/package-source.sh OUTPUT.tar.gz' >&2
    exit 2
fi
bundle_dir=$(mktemp -d)
trap 'rm -rf "$bundle_dir"' EXIT
mkdir -p "$bundle_dir/source/.cargo"
cp -a Cargo.toml Cargo.lock rust-toolchain.toml src vendor tests LICENSE licenses docs README.md \
    Dockerfile .dockerignore scripts "$bundle_dir/source/"
(cd "$bundle_dir/source" && cargo vendor --locked --versioned-dirs dependencies > .cargo/config.toml)
cat > "$bundle_dir/source/BUILD.txt" <<'BUILD'
Build the included node sources with Rust 1.93.0, a C toolchain and pkg-config:
  cargo build --release --frozen -p rinpqc-node
The Cargo source replacements in .cargo/config.toml use the bundled dependencies.
See docs/distribution.md and licenses/ for distribution terms. OS/toolchain packages
are build prerequisites, not included Rust dependencies. The original Dockerfile and
devnet launcher are included as build/run references; Docker base images require a registry.
BUILD
tar -C "$bundle_dir" -czf "$1" source
