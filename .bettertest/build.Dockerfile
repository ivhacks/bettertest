# The machine bettertest's own pipeline builds the packages in: what AGENTS.md says a build machine
# needs, on Fedora. Every run builds in a fresh container of it, so runs at the same time share
# nothing. The Prepare stage builds it on the worker, from the root of the checkout; docker's cache
# makes that instant after the first time.
FROM fedora:44

RUN dnf install -y --setopt=install_weak_deps=False \
        gcc musl-gcc make perl perl-FindBin perl-IPC-Cmd perl-File-Compare perl-File-Copy perl-lib \
        perl-Time-Piece dpkg rpm-build systemd-rpm-macros binutils git tar gzip xz \
        python3 ninja-build pkgconf-pkg-config bzip2 zstd \
    && dnf clean all

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --target x86_64-unknown-linux-musl --target wasm32-unknown-unknown \
    && ln -s /root/.cargo/bin/* /usr/local/bin/

RUN curl -sSfL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz \
        | tar -xz -C /usr/local/bin trunk

RUN curl -sSfL https://ziglang.org/download/0.17.1/zig-x86_64-linux-0.17.1.tar.xz \
        | tar -xJ -C /opt && ln -s /opt/zig-x86_64-linux-0.17.1/zig /usr/local/bin/zig

# The vm tools the binary carries take minutes to build, so they are built here, where docker
# keeps them until tools.sh changes. A build copies them into its checkout's target/.
COPY tools.sh /tools/
RUN /tools/tools.sh x86_64 && rm -rf /tools/target/tools-build
