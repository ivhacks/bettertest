#!/usr/bin/env bash
# Once per cloud agent environment build: what `cargo test` needs on a fresh VM. Docker is the
# only thing the tests themselves need from the machine; the rest builds bettertest.
set -euo pipefail

if ! command -v docker >/dev/null; then
    sudo apt-get update
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y docker.io
fi

rustup toolchain install stable --profile minimal
rustup default stable
rustup target add wasm32-unknown-unknown x86_64-unknown-linux-musl

if ! command -v trunk >/dev/null; then
    curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.22.1/trunk-x86_64-unknown-linux-gnu.tar.gz \
        | sudo tar -xz -C /usr/local/bin trunk
fi

bash "$(dirname "$0")/start.sh"
