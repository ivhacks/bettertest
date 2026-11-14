#!/usr/bin/env bash
# Every boot of a cloud agent VM: docker is the one thing bettertest's tests need from the
# machine, so its daemon has to be up. The VM has no systemd to start it.
set -euo pipefail

if ! sudo docker info >/dev/null 2>&1; then
    sudo rm -f /var/run/docker.pid
    sudo sh -c 'nohup dockerd > /var/log/dockerd.log 2>&1 &'
    for _ in $(seq 60); do
        sudo docker info >/dev/null 2>&1 && break
        sleep 1
    done
fi
# The agent's user is not in the docker group, and a group added now would only reach new logins.
sudo chmod 666 /var/run/docker.sock
docker info --format 'docker {{.ServerVersion}} is up'
