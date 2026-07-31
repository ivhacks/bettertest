#!/bin/bash
set -e

# Build the WebAssembly frontend. Trunk runs wasm-bindgen and bundles everything into frontend/dist.
cd frontend
trunk build --release
cd ..

# Build the server binary, optimized for the smallest size on stable.
# The nightly half of the experiment (-Zbuild-std, -Cpanic=immediate-abort, -Zlocation-detail,
# -Zfmt-debug) is dropped -- these are the two -C flags that work on stable rustc:
#   -Crelocation-model=static   non-PIE executable; smaller than position-independent code
#   -Cforce-unwind-tables=no    drop the .eh_frame unwind tables -- panic=abort means we never unwind
# opt-level="z", codegen-units=1, lto, strip and panic="abort" live in Cargo.toml [profile.release].
#
# The explicit --target is load-bearing even though it's the host triple: it scopes RUSTFLAGS to the
# target build only, so relocation-model=static doesn't hit the proc-macro dylibs (serde_derive), which
# must stay position-independent. Output lands in target/x86_64-unknown-linux-gnu/release/.
RUSTFLAGS="-Crelocation-model=static -Cforce-unwind-tables=no" cargo build --release -p bettertest-package --target x86_64-unknown-linux-gnu
