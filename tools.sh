#!/bin/bash
# The programs a worker runs to give tasks their vms, built from source as static binaries, so the
# bettertest binary carries them and a worker needs nothing installed for vms: qemu-system-x86_64,
# qemu-system-aarch64 and qemu-img with the firmware they load, e2fsprogs' debugfs and e2fsck, and
# OpenSSH's ssh, scp and ssh-keygen. Zig is the C compiler for every architecture, so any build
# machine builds them for either one, without root.
#
# ./tools.sh [arch] writes target/tools/<arch>/, each file zstd-compressed, for this machine's
# architecture (x86_64 or aarch64) or the one given. core/build.rs runs it, so cargo does; it does nothing when that
# directory was made by this version of the script. Needs zig, python3, ninja, pkg-config, make,
# perl, curl, bzip2 and zstd. qemu brings the meson everything here builds with.
set -euo pipefail
arch=${1:-$(uname -m)}
root=$(cd "$(dirname "$0")" && pwd)
out=$root/target/tools/$arch
stamp=$(sha256sum "$0" | cut -c1-64)
if [ "$(cat "$out.stamp" 2>/dev/null)" = "$stamp" ]; then
    exit 0
fi
build=$root/target/tools-build/$arch
prefix=$build/prefix
rm -rf "$build" "$out" "$out.stamp"
mkdir -p "$build/src" "$build/bin" "$prefix" "$out"
jobs=$(nproc)

fetch() { # url, sha256, directory under src/ to unpack it into
    local file=$root/target/tools-build/sources/${1##*/}
    mkdir -p "${file%/*}"
    if ! echo "$2  $file" | sha256sum -c --status 2>/dev/null; then
        curl -sSfL -o "$file.part" "$1"
        echo "$2  $file.part" | sha256sum -c --quiet
        mv "$file.part" "$file"
    fi
    mkdir -p "$build/src/$3"
    tar -xf "$file" -C "$build/src/$3" --strip-components=1
}
fetch https://zlib.net/zlib-1.4.tar.gz \
    02dd159783677a749c04adbdd02b55b8a2bcc7e054f51b76b549199a916ad584 zlib
fetch https://github.com/PCRE2Project/pcre2/releases/download/pcre2-10.49/pcre2-10.49.tar.bz2 \
    53c156e1ba416a20da8e65395daa132da0d80e76910424caca3fcdae7831d384 pcre2
fetch https://github.com/libffi/libffi/releases/download/v3.8.0/libffi-3.8.0.tar.gz \
    7da3e2d9a171eb0a038f592ecad3ff2bb2550f3496d87b3b29ad0cf4430c0db4 libffi
fetch https://download.gnome.org/sources/glib/2.88/glib-2.88.4.tar.xz \
    25fd169f3b2f91fe2391ad297883ce0fcf30c69fc789ffd1c9672d1332b1c5a8 glib
fetch https://cairographics.org/releases/pixman-0.46.4.tar.xz \
    a098c33924754ad43f981b740f6d576c70f9ed1006e12221b1845431ebce1239 pixman
fetch https://gitlab.freedesktop.org/slirp/libslirp/-/archive/v4.9.7/libslirp-v4.9.7.tar.gz \
    d2567fa06a6357c7755d61c67293b2c0bf069b3f2173bde6b76cf768529a1938 slirp
fetch https://download.qemu.org/qemu-11.1.3.tar.xz \
    5a8289c7940dc77d3873c81a6c29ba3afe1446fc9d36e77fa1658cdbecb84b22 qemu
# The arm vm's device tree library. qemu would git clone it.
fetch https://mirrors.edge.kernel.org/pub/software/utils/dtc/dtc-1.8.1.tar.xz \
    23526015a6f1550e0541a53fe7acea1b5a11e3697cdf3a3bdc076abc38f6045d qemu/subprojects/dtc
fetch https://mirrors.edge.kernel.org/pub/linux/kernel/people/tytso/e2fsprogs/v1.47.4/e2fsprogs-1.47.4.tar.xz \
    fd5bf388cbdbe006a3d3b318d983b2948382440acc85a87f1e7d108653e8db0b e2fsprogs
fetch https://cdn.openbsd.org/pub/OpenBSD/OpenSSH/portable/openssh-10.6p2.tar.gz \
    9ae44872090cf2f06b3cf9ff147e7f16f309671340763ed2f99248398ae29d5b openssh

# A cross toolchain under the names autotools, meson and qemu's configure all look for. zig cc
# defines NDEBUG when optimizing, which qemu refuses.
host=$arch-linux-musl
tool() { # name, command
    printf '#!/bin/sh\nexec %s "$@"\n' "$2" > "$build/bin/$host-$1"
    chmod +x "$build/bin/$host-$1"
}
tool gcc "zig cc -target $host -Wno-gcc-install-dir-libstdcxx"
tool cc "zig cc -target $host -Wno-gcc-install-dir-libstdcxx"
tool c++ "zig c++ -target $host -Wno-gcc-install-dir-libstdcxx"
tool ar "zig ar"
tool ranlib "zig ranlib"
tool pkg-config pkg-config
export PATH=$build/bin:$PATH
export CC=$host-cc CXX=$host-c++ AR=$host-ar RANLIB=$host-ranlib
export CFLAGS="-Os -ffunction-sections -fdata-sections" LDFLAGS="-static -s -Wl,--gc-sections"
export PKG_CONFIG_LIBDIR=$prefix/lib/pkgconfig PKG_CONFIG_PATH=
cat > "$build/cross.ini" <<EOF
[binaries]
c = '$host-cc'
cpp = '$host-c++'
ar = '$host-ar'
ranlib = '$host-ranlib'
pkg-config = '$host-pkg-config'
[built-in options]
default_library = 'static'
prefer_static = true
buildtype = 'minsize'
[properties]
needs_exe_wrapper = true
[host_machine]
system = 'linux'
cpu_family = '$arch'
cpu = '$arch'
endian = 'little'
EOF

autotools() { # source dir, configure options
    local dir=$build/src/$1
    shift
    (cd "$dir" && ./configure --host="$host" --prefix="$prefix" "$@" && make -j"$jobs" install)
}
# The build machine's own meson is often too old for glib.
python3 -m zipfile -e "$build/src/qemu/python/wheels/meson-1.11.1-py3-none-any.whl" "$build/meson"
printf 'import sys\nfrom mesonbuild import mesonmain\nsys.exit(mesonmain.main())\n' \
    > "$build/meson/meson.py"
mesonbuild() { # source dir, meson options
    local dir=$build/src/$1
    shift
    python3 "$build/meson/meson.py" setup "$dir/build" "$dir" --cross-file "$build/cross.ini" \
        --prefix "$prefix" --libdir lib "$@"
    ninja -C "$dir/build" install
}

# zig links -latomic for arm from its own runtime, so glib finds no libatomic.a to go with it.
sed -i "s/cc.find_library('atomic')/cc.find_library('atomic', required: false)/" \
    "$build/src/glib/meson.build"
(cd "$build/src/zlib" && CHOST=$host ./configure --static --prefix="$prefix" && make -j"$jobs" install)
autotools pcre2 --disable-shared
autotools libffi --disable-shared --disable-docs
mesonbuild glib -Dtests=false -Dintrospection=disabled -Dlibmount=disabled -Dselinux=disabled \
    -Dxattr=false -Dnls=disabled -Dman-pages=disabled -Ddocumentation=false -Dsysprof=disabled \
    -Dglib_debug=disabled
mesonbuild pixman -Dtests=disabled -Ddemos=disabled -Dgtk=disabled -Dlibpng=disabled
mesonbuild slirp

(
    cd "$build/src/qemu"
    ./configure --cross-prefix="$host-" --static --target-list=x86_64-softmmu,aarch64-softmmu \
        --without-default-features --enable-kvm --enable-tcg --enable-slirp --enable-pixman \
        --enable-tools --enable-fdt=internal --disable-docs --disable-install-blobs \
        --extra-cflags="-Os -UNDEBUG" --extra-ldflags=-s -Ddtc:tests=false
    ninja -C build qemu-system-x86_64 qemu-system-aarch64 qemu-img
)
(
    cd "$build/src/e2fsprogs"
    ./configure --host="$host" --disable-nls --disable-fuse2fs --disable-uuidd --disable-tdb \
        --disable-imager --disable-defrag --disable-e2initrd-helper
    make -j"$jobs" libs
    make -j"$jobs" -C debugfs debugfs
    make -j"$jobs" -C e2fsck e2fsck
)
(
    # Its own ed25519 and chacha20-poly1305, which every sshd a cloud image runs speaks.
    cd "$build/src/openssh"
    ./configure --host="$host" --without-openssl --without-zlib --without-pie
    make -j"$jobs" ssh scp ssh-keygen
)

src=$build/src
bunzip2 -k "$src/qemu/pc-bios/edk2-aarch64-code.fd.bz2"
for file in qemu/build/qemu-system-x86_64 qemu/build/qemu-system-aarch64 qemu/build/qemu-img \
    qemu/pc-bios/bios-256k.bin qemu/pc-bios/kvmvapic.bin qemu/pc-bios/vgabios-stdvga.bin \
    qemu/pc-bios/efi-virtio.rom qemu/pc-bios/edk2-aarch64-code.fd \
    e2fsprogs/debugfs/debugfs e2fsprogs/e2fsck/e2fsck openssh/ssh openssh/scp openssh/ssh-keygen; do
    zstd -19 -q "$src/$file" -o "$out/${file##*/}"
done
# What they are under, beside them: qemu and e2fsprogs are GPL 2, OpenSSH is BSD.
for license in qemu/COPYING qemu/pc-bios/edk2-licenses.txt e2fsprogs/NOTICE openssh/LICENCE; do
    zstd -19 -q "$src/$license" -o "$out/LICENSE.${license%%/*}.${license##*/}"
done
echo "$stamp" > "$out.stamp"
