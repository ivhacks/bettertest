#!/bin/bash
set -e

# Build the WebAssembly frontend. Trunk runs wasm-bindgen and bundles everything into frontend/dist.
cd frontend
trunk build --release
cd ..

# Build the server binary for this machine's architecture (x86_64 or aarch64): fully static, so it
# runs on any Linux of that architecture without a single library from the machine. The musl target links statically; sqlite and OpenSSL are compiled in from source
# (rusqlite "bundled", reqwest "native-tls-vendored"), which needs musl-gcc (Fedora: musl-gcc).
#   -Crelocation-model=static   non-PIE executable; smaller than position-independent code
#   -Cforce-unwind-tables=no    drop the .eh_frame unwind tables -- panic=abort means we never unwind
# opt-level="z", codegen-units=1, lto, strip and panic="abort" live in Cargo.toml [profile.release].
#
# The explicit --target also scopes RUSTFLAGS to the target build only, so relocation-model=static
# doesn't hit the proc-macro dylibs (serde_derive), which must stay position-independent.
# Output lands in target/<arch>-unknown-linux-musl/release/, for this machine's arch or the one given.
# ./build.sh aarch64 on an x86_64 machine (or x86_64 on aarch64) cross-builds with zig as the C
# compiler (cargo zigbuild; zig on PATH), which brings every target's C toolchain without root.
arch=${1:-$(uname -m)}
# The static qemu, firmware, e2fsprogs and ssh the binary carries for vms (see tools.sh).
./tools.sh "$arch"
build=build
if [ "$arch" != "$(uname -m)" ]; then
    build=zigbuild
fi
RUSTFLAGS="-Crelocation-model=static -Cforce-unwind-tables=no" cargo $build --release -p bettertest --target "$arch-unknown-linux-musl"
