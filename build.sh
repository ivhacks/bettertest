#!/bin/bash
set -e
cd "$(dirname "$0")"

cd frontend && trunk build --release && cd ..
cargo build -p bettertest --release
