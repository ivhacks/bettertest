#!/bin/bash
set -e

# Build the WebAssembly frontend. Trunk runs wasm-bindgen and bundles everything into frontend/dist.
cd frontend
trunk build --release
cd ..

# Build the server binary, optimized for the smallest possible size.
#
# Why each flag:
#   -Cpanic=immediate-abort      on panic, call abort() right away; drops all the unwinding and message-formatting code
#   -Cforce-unwind-tables=no     drop the .eh_frame unwind tables too (we never unwind) -- biggest single win
#   -Crelocation-model=static    build a plain non-PIE executable; smaller than position-independent code
#   -Zlocation-detail=none       strip file/line/column strings that panics would otherwise embed
#   -Zfmt-debug=none             strip the Debug-formatting machinery
#   -Zbuild-std=std,panic_abort  recompile the standard library from source (needs nightly + the rust-src component)
#   --target x86_64-unknown-linux-gnu   build-std requires an explicit target triple
#   -Zunstable-options           unlocks the -C flags above on nightly
#
# opt-level="z" (applies to everything, std included), codegen-units=1, lto, strip and panic="abort"
# live in Cargo.toml's [profile.release]. Dependency features are trimmed in core/Cargo.toml.

RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort -Cforce-unwind-tables=no -Crelocation-model=static -Zlocation-detail=none -Zfmt-debug=none" cargo build --release -p bettertest --target x86_64-unknown-linux-gnu -Zbuild-std=std,panic_abort
