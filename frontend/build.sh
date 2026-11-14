#!/bin/bash
set -e

# Builds the WebAssembly frontend into frontend/dist: the one build of it, which the binary embeds
# (build.sh runs this first) and the frontend's browser tests serve.
# trunk 0.21 refuses to start when NO_COLOR is anything but true or false, and some shells set it to 1.
# wasm-bindgen never removes the snippet directories of earlier builds from its output, and trunk
# copies all of them into dist, so the binary would carry every old one.
cd "$(dirname "$0")"
rm -rf ../target/wasm-bindgen
env -u NO_COLOR trunk build --release
