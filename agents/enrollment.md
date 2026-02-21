# enrollment

instructions for setting up bettertest on a server for a given repo.

## prerequisites

the server needs:
- ssh access
- systemd
- a user with sudo

these are NOT guaranteed to be present — install them if missing:
- **docker**: `sudo dnf install -y docker && sudo systemctl enable --now docker && sudo usermod -aG docker USER`
- **git**: `sudo dnf install -y git`

python is NOT needed on the server. it's only needed inside the docker image.

## overview

you're setting up a server to run tests for a repo. by the end:
- the bettertest binary is installed on the server and running as both worker and boss via systemd
- the repo is cloned on the server with a `.bettertest/` dir containing `pipedef.py`
- a `Dockerfile` exists in the repo root
- the docker image is built on the server
- the boss is serving the frontend and running tests

## 1. get the bettertest binary

build it locally (requires trunk + rust):

```sh
cd /home/iv/bettertest && ./build.sh
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

# install system deps your project needs
RUN dnf install -y python3 git curl && dnf clean all

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
- base on `fedora:latest` unless user specifies otherwise. pin the version (e.g. `fedora:43`) for reproducibility
- clone the repo fresh inside the image (don't COPY — the image needs to be buildable on the server)
- install project deps
- run a few fast, reliable tests as `RUN` steps — this both validates the image and caches pip/uv downloads in docker layers. only use tests you're confident will pass, or the build will fail
- tests that are flaky or have known failures: either skip them in the Dockerfile entirely, or use `|| true` so the build doesn't fail. prefer skipping — `|| true` still wastes build time
- if the repo needs secrets (api keys etc), `COPY secrets.yaml .` before the install step and scp the secrets file to the server first
- add system deps as needed — some python packages need C libraries (e.g. `libpq-devel` for psycopg2, `python3-tkinter` for matplotlib). check the project's install docs or look at what `uv sync` complains about

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
- you can split a single test file into multiple tasks (one per test class or even per test method) for more parallelism. see soundscrape's pipedef for an example of this

## 8. copy files to the server

the pipedef needs to exist on the server (the boss reads it). the Dockerfile also needs to be there for the docker build.

```sh
scp ~/REPO_NAME/.bettertest/pipedef.py USER@HOST:~/REPO_NAME/.bettertest/pipedef.py
scp ~/REPO_NAME/Dockerfile USER@HOST:~/REPO_NAME/Dockerfile
```

if the repo needs secrets:
```sh
scp ~/REPO_NAME/secrets.yaml USER@HOST:~/REPO_NAME/secrets.yaml
```

## 9. build the docker image on the server

```sh
ssh USER@HOST "cd ~/REPO_NAME && docker build -t IMAGE_NAME ."
```

this will take a while the first time (pulling base image, installing deps). subsequent builds use cached layers. if it fails, check which `RUN pytest` step broke and either fix it or remove that step from the Dockerfile.

## 10. start the services

```sh
ssh USER@HOST "sudo systemctl start bettertest-worker"
```

verify the worker is up before starting the boss:
```sh
ssh USER@HOST "curl -s http://localhost:9009/health"
# should print: ok
```

then start the boss:
```sh
ssh USER@HOST "sudo systemctl start bettertest-boss"
```

verify the frontend is reachable:
```sh
curl -s -o /dev/null -w "%{http_code}" http://HOST:9001
# should print: 200
```

## 11. verify a test runs

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

## updating after code changes

to redeploy the bettertest binary:
```sh
scp target/release/bettertest USER@HOST:~/bettertest
ssh USER@HOST "sudo systemctl stop bettertest-worker bettertest-boss && sudo cp ~/bettertest /usr/local/bin/bettertest && sudo systemctl start bettertest-worker bettertest-boss"
```

to rebuild the docker image after repo changes (e.g. new deps):
```sh
ssh USER@HOST "cd ~/REPO_NAME && docker build -t IMAGE_NAME ."
```
no need to restart services — the worker pulls the image by name on each run.

to update the pipedef (e.g. new tests added):
```sh
scp ~/REPO_NAME/.bettertest/pipedef.py USER@HOST:~/REPO_NAME/.bettertest/pipedef.py
ssh USER@HOST "sudo systemctl restart bettertest-boss"
```
the boss needs a restart to pick up pipedef changes.
