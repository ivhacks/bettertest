# The machine bettertest's own pipeline builds the packages in: what AGENTS.md says a build machine
# needs, on Fedora. Every run builds in a fresh container of it, so runs at the same time share
# nothing. The Image stage builds it on the worker; docker's cache makes that instant after the
# first time.
FROM fedora:44

RUN dnf install -y --setopt=install_weak_deps=False \
        gcc musl-gcc make perl perl-FindBin perl-IPC-Cmd perl-File-Compare perl-File-Copy perl-lib \
        perl-Time-Piece dpkg rpm-build systemd-rpm-macros binutils git tar gzip xz \
    && dnf clean all

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --target x86_64-unknown-linux-musl --target wasm32-unknown-unknown \
    && ln -s /root/.cargo/bin/* /usr/local/bin/

RUN curl -sSfL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz \
        | tar -xz -C /usr/local/bin trunk
