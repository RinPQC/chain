#!/usr/bin/env bash
# Pinned Linux x86-64 release binaries for the Ubuntu CI runner.
set -euo pipefail
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]]
: "${GITHUB_PATH:?This installer is for GitHub Actions}"
ci_tools_dir=$(mktemp -d)
trap 'rm -rf "$ci_tools_dir"' EXIT
ci_tools_bin="${RUNNER_TEMP:?}/rinpqc-tools"
mkdir -p "$ci_tools_bin"
install_tool() {
    local name=$1 url=$2 sha=$3 mode=$4
    curl --fail --location --retry 3 --proto '=https' --tlsv1.2 "$url" -o "$ci_tools_dir/archive"
    printf '%s  %s\n' "$sha" "$ci_tools_dir/archive" | sha256sum --check --status
    mkdir -p "$ci_tools_dir/extract"
    if [[ "$mode" == gzip ]]; then
        gzip -dc "$ci_tools_dir/archive" > "$ci_tools_bin/$name"
    else
        tar -xzf "$ci_tools_dir/archive" -C "$ci_tools_dir/extract"
        local binary
        binary=$(find "$ci_tools_dir/extract" -type f -name "$name")
        [[ -n "$binary" && "$binary" != *$'\n'* ]]
        cp "$binary" "$ci_tools_bin/$name"
    fi
    chmod +x "$ci_tools_bin/$name"
    rm -rf "$ci_tools_dir/extract"
}
install_tool just https://github.com/casey/just/releases/download/1.46.0/just-1.46.0-x86_64-unknown-linux-musl.tar.gz 79966e6e353f535ee7d1c6221641bcc8e3381c55b0d0a6dc6e54b34f9db36eaa tar
install_tool taplo https://github.com/tamasfe/taplo/releases/download/0.10.0/taplo-linux-x86_64.gz 8fe196b894ccf9072f98d4e1013a180306e17d244830b03986ee5e8eabeb6156 gzip
install_tool cargo-nextest https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.128/cargo-nextest-0.9.128-x86_64-unknown-linux-gnu.tar.gz 83c074a33648f93d9c1cce8edf23b0687122d5941f0d2ad3361339d04e3d30ab tar
install_tool cargo-deny https://github.com/EmbarkStudios/cargo-deny/releases/download/0.19.0/cargo-deny-0.19.0-x86_64-unknown-linux-musl.tar.gz 0e8c2aa59128612c90d9e09c02204e912f29a5b8d9a64671b94608cbe09e064f tar
printf '%s\n' "$ci_tools_bin" >> "$GITHUB_PATH"
