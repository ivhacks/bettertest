# bettertest

a ci system designed for the age of ai-generated code. tests are the only specification that matters — if tests pass, the code is correct.

bettertest runs your tests fast, in parallel, inside docker containers, and gives you instant visual feedback in a web ui. pipeline definitions are imperative python scripts, not yaml.

## getting started

clone this repo, then tell your ai coding assistant:

> clone bettertest and build it

then give it a server and a repo:

> set up bettertest on `ssh user@my-server.space` for https://github.com/me/myproject

the agent will build the binary, deploy it to the server, write a Dockerfile and pipedef for your repo, build the docker image, and start everything up. see `agents/enrollment.md` for the full manual process.

## architecture

bettertest is a single rust binary that runs in two modes:

- **worker** (`bettertest --worker`): exposes an HTTP API on port 9009. runs commands inside docker containers and streams results back via SSE. this is where docker lives.
- **boss** (`bettertest --boss --pipedef path/to/pipedef.py`): hosts the web frontend on port 9001 and coordinates test runs. parses the pipedef to discover stages and tasks, then shells out to python to run them against a worker.

same binary, two processes. they can run on the same server or different servers — the worker doesn't know or care who's calling it.

the frontend is a yew/wasm app that gets compiled and embedded into the binary at build time. no separate static file serving needed.

### pipedefs

a pipedef is an imperative python script that defines what tests to run:

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
```

stages run sequentially. tasks within a stage run in parallel. that's the whole model.

## building

### frontend

requires [trunk](https://trunkrs.dev/):

```sh
cd frontend && trunk build
```

output goes to `frontend/dist/`. this must be built before the server binary, since the binary embeds the frontend assets.

### server (boss + worker binary)

```sh
cargo build -p bettertest --release
```

binary lands at `target/release/bettertest`.

### both at once

```sh
cd frontend && trunk build && cd .. && cargo build -p bettertest --release
```

### worker

the worker is the same binary as the boss. deploy it to any linux machine with docker:

```sh
scp target/release/bettertest user@worker-host:~/bettertest
ssh user@worker-host "sudo cp ~/bettertest /usr/local/bin/bettertest"
```

run it with `bettertest --worker`. see `agents/enrollment.md` for systemd service setup.

## project structure

```
server/       — rust binary (boss + worker modes)
frontend/     — yew wasm frontend
common/       — shared types between frontend and server
bettertest/   — python library that pipedefs import
agents/       — documentation and agent instructions
```
