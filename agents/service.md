# bettertest worker systemd service

## setup

copy the binary somewhere systemd can reach (home dirs are 700, systemd can't traverse them):

```sh
sudo cp ./bettertest /usr/local/bin/bettertest
```

create the service file:

```sh
sudo tee /etc/systemd/system/bettertest-worker.service << 'EOF'
[Unit]
Description=bettertest worker
After=network.target docker.service
Requires=docker.service

[Service]
Type=simple
User=iv
ExecStart=/usr/local/bin/bettertest --worker
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
```

enable and start:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now bettertest-worker
```

## commands

```sh
systemctl status bettertest-worker     # check status
sudo journalctl -u bettertest-worker   # view logs
sudo systemctl restart bettertest-worker  # restart
```

## gotcha

the binary CANNOT live in `/home/iv/` or any home dir. systemd runs the exec step before switching to the `User=`, so it needs to be able to traverse the path as root — and home dirs with 700 perms block that. `/usr/local/bin` is the move.
