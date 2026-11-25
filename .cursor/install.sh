#!/usr/bin/env bash
# Once per cloud agent environment build: what `cargo test` needs on a fresh VM. Docker is the
# only thing the tests themselves need from the machine; the rest builds bettertest.
set -euo pipefail

# What tools.sh (run by cargo, see core/build.rs) builds the vm tools with, and docker.
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y python3 python3-venv ninja-build pkg-config \
    make perl curl bzip2 zstd xz-utils
if ! command -v docker >/dev/null; then
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y docker.io
fi

if ! command -v zig >/dev/null; then
    curl -fsSL "https://ziglang.org/download/0.17.1/zig-$(uname -m)-linux-0.17.1.tar.xz" \
        | sudo tar -xJ -C /opt
    sudo ln -sf "/opt/zig-$(uname -m)-linux-0.17.1/zig" /usr/local/bin/zig
fi

rustup toolchain install stable --profile minimal
rustup default stable
rustup target add wasm32-unknown-unknown x86_64-unknown-linux-musl

if ! command -v trunk >/dev/null; then
    curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.22.1/trunk-x86_64-unknown-linux-gnu.tar.gz \
        | sudo tar -xz -C /usr/local/bin trunk
fi

bash "$(dirname "$0")/start.sh"
