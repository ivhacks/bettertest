#!/bin/sh
# Packages the binary ./build.sh built as target/debian/bettertest_<version>_<amd64 or arm64>.deb, for
# this machine's architecture or the one given (./deb.sh aarch64, after ./build.sh aarch64). It never
# builds, so a release's .deb and .rpm carry the very same binary. Needs dpkg-deb (Fedora: dpkg).
set -e

machine=${1:-$(uname -m)}

binary=target/$machine-unknown-linux-musl/release/bettertest
version=$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml)
# Debian's names for the architectures.
case $machine in
    x86_64) arch=amd64 ;;
    aarch64) arch=arm64 ;;
    *) echo "no Debian architecture for $machine" >&2; exit 1 ;;
esac

root=target/debian/bettertest_${version}_$arch
rm -rf "$root"
install -D -m 755 "$binary" "$root/usr/bin/bettertest"
install -D -m 644 packaging/bettertest-boss.service "$root/usr/lib/systemd/system/bettertest-boss.service"
install -D -m 644 packaging/bettertest-worker.service "$root/usr/lib/systemd/system/bettertest-worker.service"
install -D -m 644 packaging/sysusers.conf "$root/usr/lib/sysusers.d/bettertest.conf"
install -D -m 644 pipedefs/helloworld.py "$root/etc/bettertest/pipedefs/helloworld.py"
install -D -m 644 LICENSE "$root/usr/share/doc/bettertest/copyright"

install -d "$root/DEBIAN"
sed -e "s/@VERSION@/$version/" -e "s/@ARCH@/$arch/" packaging/deb/control > "$root/DEBIAN/control"
install -m 755 packaging/deb/postinst packaging/deb/prerm packaging/deb/postrm "$root/DEBIAN/"
# Files under /etc are the admin's: upgrades keep their edits.
echo /etc/bettertest/pipedefs/helloworld.py > "$root/DEBIAN/conffiles"
(cd "$root" && find usr etc -type f -exec md5sum {} + > DEBIAN/md5sums)

dpkg-deb --root-owner-group --build "$root" target/debian/
