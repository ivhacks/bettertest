#!/bin/sh
# Packages the binary ./build.sh built as target/rpm/bettertest-<version>-1.<x86_64 or aarch64>.rpm, for
# this machine's architecture or the one given (./rpm.sh aarch64, after ./build.sh aarch64). It never
# builds, like deb.sh. Needs rpmbuild and the systemd rpm macros (Fedora: rpm-build
# systemd-rpm-macros).
set -e

machine=${1:-$(uname -m)}

version=$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml)

rm -rf target/rpm/rpmbuild target/rpm/bettertest-*."$machine".rpm
# --target packages the binary built for $machine, even on another architecture: the spec only
# copies files.
rpmbuild -bb packaging/rpm/bettertest.spec --target "$machine" \
    --define "bettertest_version $version" \
    --define "_sourcedir $PWD" \
    --define "_topdir $PWD/target/rpm/rpmbuild" \
    --define "_rpmdir $PWD/target/rpm" \
    --define "_build_name_fmt %%{NAME}-%%{VERSION}-%%{RELEASE}.%%{ARCH}.rpm"
