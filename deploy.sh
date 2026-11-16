#!/bin/bash
# Puts build.sh's binary on a machine that runs the .deb's or .rpm's services, over ssh as root:
# backs up its db, swaps the binary in, restarts boss and worker. Run ./build.sh first (for the
# machine's architecture: ./build.sh aarch64 for an arm one).
# Also publishes that binary at /var/tmp/bettertest/dl/bettertest-<arch>, which install.sh serves.
# The other architecture is a separate copy of its musl binary to bettertest-<other> there.
#   ./deploy.sh root@real-live.bettertest.dev
set -euo pipefail

host=${1:?usage: ./deploy.sh root@host}
arch=$(ssh "$host" uname -m)
binary=target/$arch-unknown-linux-musl/release/bettertest
commit=$(git rev-parse --short HEAD)
if [ ! -f "$binary" ]; then
    echo "no $binary: run ./build.sh $arch first" >&2
    exit 1
fi

# A copy of the db as it was before this binary touched it, named for the commit that replaced it.
ssh "$host" "sqlite3 /var/tmp/bettertest/bettertest.db \".backup /var/tmp/bettertest/bettertest.db.bak-$(date -u +%Y%m%dT%H%M%SZ)-pre-$commit\""
scp -q "$binary" "$host:/usr/bin/bettertest.new"
# The running binary cannot be written to ("text file busy"), but it can be renamed over. The
# capability lets a boss told --boss-port 443 bind it without running as root.
ssh "$host" 'setcap cap_net_bind_service=+ep /usr/bin/bettertest.new && mv -f /usr/bin/bettertest.new /usr/bin/bettertest && systemctl restart bettertest-boss bettertest-worker && systemctl is-active bettertest-boss bettertest-worker && bettertest --version'
# Strangers pipe this to a shell, so it stays root's and 0644, not the 0666 of the db.
ssh "$host" "mkdir -p /var/tmp/bettertest/dl && chmod 755 /var/tmp/bettertest/dl"
scp -q "$binary" "$host:/var/tmp/bettertest/dl/bettertest-$arch.new"
ssh "$host" "mv -f /var/tmp/bettertest/dl/bettertest-$arch.new /var/tmp/bettertest/dl/bettertest-$arch && chmod 644 /var/tmp/bettertest/dl/bettertest-$arch"
