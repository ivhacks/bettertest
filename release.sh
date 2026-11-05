#!/bin/sh
# Builds everything a release ships into dist/<version>/: the static binary, the .deb and the .rpm,
# each for x86_64 and aarch64, and SHA256SUMS. Runs on an x86_64 machine (aarch64 cross-builds with
# zig, see build.sh). The version is core/Cargo.toml's; releases are tagged v<version>, semver.
set -e

version=$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml)
if [ -n "$1" ] && [ "$1" != "$version" ]; then
    echo "core/Cargo.toml says $version, not $1: bump it first" >&2
    exit 1
fi

dist=dist/$version
rm -rf "$dist"
mkdir -p "$dist"
for machine in x86_64 aarch64; do
    # One build per architecture, which every package of it then carries.
    ./build.sh "$machine"
    ./deb.sh "$machine"
    ./rpm.sh "$machine"
    cp "target/$machine-unknown-linux-musl/release/bettertest" "$dist/bettertest-$version-$machine-linux"
done
cp target/debian/bettertest_"$version"_*.deb target/rpm/bettertest-"$version"-1.*.rpm "$dist/"
(cd "$dist" && sha256sum -- * > SHA256SUMS)
ls -l "$dist"
