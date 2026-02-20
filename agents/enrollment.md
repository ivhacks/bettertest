# enrollment

instructions for setting up bettertest on a server for a given repo. assumes the server has ssh access, docker, python3, and systemd.

## overview

you're setting up a server to run tests for a repo. by the end:
- the bettertest binary is installed on the server and running as both worker and boss via systemd
- the repo is cloned locally and has a `.bettertest/` dir with a `pipedef.py` and `Dockerfile`
- the docker image is built on the server
- the boss is serving the frontend and running tests (both from the server)

## 1. get the bettertest binary

build it locally (requires trunk + rust):

```sh
cd /home/iv/bettertest && cd frontend && trunk build && cd .. && cargo build -p bettertest --release
```

binary is at `target/release/bettertest`.

copy it to the server:

```sh
scp target/release/bettertest USER@HOST:~/bettertest
```

## 2. install the binary on the server

```sh
ssh USER@HOST "sudo cp ~/bettertest /usr/local/bin/bettertest"
```

it MUST live outside home dirs — systemd can't traverse `~` (700 perms). `/usr/local/bin` is the move.

## 3. set up systemd services

two services: worker (port 9009) and boss (port 9001). both run the same binary with different flags.

### worker service

```sh
ssh USER@HOST "sudo tee /etc/systemd/system/bettertest-worker.service << 'EOF'
[Unit]
Description=bettertest worker
After=network.target docker.service
Requires=docker.service

[Service]
Type=simple
User=USER
ExecStart=/usr/local/bin/bettertest --worker
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF"
```

### boss service

the boss needs to know the pipedef path. set this to where the repo will be cloned.

```sh
ssh USER@HOST "sudo tee /etc/systemd/system/bettertest-boss.service << 'EOF'
[Unit]
Description=bettertest boss
After=network.target bettertest-worker.service

[Service]
Type=simple
User=USER
ExecStart=/usr/local/bin/bettertest --boss --pipedef /home/USER/REPO/.bettertest/pipedef.py
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF"
```

### enable both

```sh
ssh USER@HOST "sudo systemctl daemon-reload && sudo systemctl enable bettertest-worker bettertest-boss"
```

don't start them yet — the pipedef and docker image need to exist first.

## 4. clone the repo

```sh
ssh USER@HOST "git clone REPO_URL ~/REPO_NAME"
```

## 5. create .bettertest/ in the repo

```sh
ssh USER@HOST "mkdir -p ~/REPO_NAME/.bettertest"
```

## 6. write the Dockerfile

the Dockerfile lives at `~/REPO_NAME/Dockerfile` (not inside `.bettertest/`). it builds the test environment.

**template:**

```dockerfile
FROM fedora:latest

# install system deps
RUN dnf install -y python3 git && dnf clean all

# install uv (fast python package manager)
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/root/.local/bin:$PATH"

# clone and install
RUN git clone REPO_URL /app
WORKDIR /app
RUN uv sync --extra dev
ENV PATH="/app/.venv/bin:$PATH"

# run a quick test to confirm everything works and warm caches
RUN pytest test/test_something.py -v --tb=short
```

key points:
- base on `fedora:latest` unless user specifies otherwise
- clone the repo fresh inside the image (don't COPY — the image needs to be buildable on the server)
- install project deps
- run a few fast, reliable tests as `RUN` steps — this both validates the image and caches pip/uv downloads in docker layers
- tests that are flaky or have failures go with `|| true` so the build doesn't fail
- if the repo needs secrets (api keys etc), `COPY secrets.yaml .` before the install step and scp the secrets file to the server first

## 7. write the pipedef

the pipedef lives at `~/REPO_NAME/.bettertest/pipedef.py`. it defines what tests to run.

**template:**

```python
from bettertest import Stage, run

WORKER = "http://localhost:9009"
IMAGE = "myproject-test"


class TestUnit(Stage):
    @staticmethod
    def task_models():
        run(WORKER, IMAGE, "pytest -xvs test/test_models.py")

    @staticmethod
    def task_utils():
        run(WORKER, IMAGE, "pytest -xvs test/test_utils.py")


class TestIntegration(Stage):
    @staticmethod
    def task_api():
        run(WORKER, IMAGE, "pytest -xvs test/test_api.py")
```

key points:
- `WORKER` is `http://localhost:9009` when boss and worker are on the same server
- `IMAGE` is the docker image name you'll build in the next step
- each class inherits from `Stage` — stages run sequentially
- each `task_` method within a stage runs in parallel
- group tests by speed/reliability: fast reliable stuff first, slow/flaky stuff later
- every task method calls `run(WORKER, IMAGE, "command")` — that's it
- to discover what tests exist: clone the repo, look at the test directory, read `pyproject.toml` or `pytest.ini` for test config

## 8. build the docker image on the server

```sh
ssh USER@HOST "cd ~/REPO_NAME && docker build -t IMAGE_NAME ."
```

this will take a while the first time. subsequent builds use cached layers.

## 9. start the services

```sh
ssh USER@HOST "sudo systemctl start bettertest-worker && sudo systemctl start bettertest-boss"
```

verify:
```sh
# worker health check
ssh USER@HOST "curl -s http://localhost:9009/health"

# boss should be serving the frontend
curl http://HOST:9001
```

## 10. verify a test runs

```sh
ssh USER@HOST "curl -N -X POST http://localhost:9009/run -H 'Content-Type: application/json' -d '{\"image\":\"IMAGE_NAME\",\"command\":\"echo hi\"}'"
```

should stream back SSE events ending with `event: done` and `data: 0`.

## quick reference

| thing | location on server |
|---|---|
| binary | `/usr/local/bin/bettertest` |
| worker service | `/etc/systemd/system/bettertest-worker.service` |
| boss service | `/etc/systemd/system/bettertest-boss.service` |
| repo | `~/REPO_NAME/` |
| pipedef | `~/REPO_NAME/.bettertest/pipedef.py` |
| dockerfile | `~/REPO_NAME/Dockerfile` |
| worker port | 9009 |
| boss port | 9001 |
