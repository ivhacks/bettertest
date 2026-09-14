# bettertest

## Problem

AI generates code faster than humans can verify it. YAML CI is slow, complex, and built for human operators. Tests are the only formal spec that matters: they say what the code does in a form people can actually read. Code can be slop — AI slop, legacy slop — as long as tests pass. That is the leverage point. Fearless refactoring, AI-generated code shipped now and cleaned later, a decade of spaghetti modernized, all of it only works if tests exist and they run.

## Technical approach

A single Rust binary (`bettertest`) with clap subcommands:

- `boss <pipedef>` — `:9009`. Embeds `frontend/dist`, sqlite, watches the pipedef file. HTTP API below. On a run, walks stages/tasks in order, POSTs each to `{task.worker}/start-task` with the whole pipedef and image, parses SSE stdout/stderr/status, writes the db.
- `worker` — `:9010`. HTTP API below. Spawns `python3 -c GLUE start <stage> <task>`, stdin JSON `{bettertest, pipedef}`, pumps child stdio as SSE.
- `dispatch <pipedef>` — parse and print. Does not hit a worker.
- `unified <pipedef>` — `tokio::try_join!(worker, boss)`.

**run is a noun, not a verb.** A run is one instance of a pipeline being executed. Create one with `POST /api/run`. Worker actions are verbs: `start-task`, `start-vm`, `start-container`. Pipedef `run(cmd)` is a Python function (`docker run ...`), not HTTP.

**Pipedefs** are imperative Python. `class` = stage, `@task(worker, image)` methods = tasks. `@task` returns a `staticmethod`. `run(cmd)` is `docker run --rm --entrypoint /bin/sh IMAGE -c cmd`. Sample: `pipedefs/glizzy.py`.

**Parse is not AST.** Rust embeds `pylib` + `core/scripts/glue.py` via `include_str!`. Glue execs the library into `sys.modules["bettertest"]`, execs the pipedef, then `dump_json()` (or calls the task). Pipedefs do `from bettertest import ...` only because glue stuffed the module first.

**Frontend** is Yew struct components (`PlView`, `LogsPage`), TEA, no function-components/hooks. Trunk proxies `/api/` to `:9009`. Grid + per-task logs. SSE for live updates.

**Persistence** is sqlite (`core/schema.sql`, rusqlite bundled). Runs / stages / tasks. Logs persist when a task finishes; live stream is in-memory broadcast.

Build: `./build.sh` (trunk release, then cargo release with static relocation + no unwind tables).

Python for pipedefs, Rust for the binary, Yew/wasm for the UI, docker on the worker for `run()`.

## The intern test

this is the deployment bar. if this person cannot get bettertest working, the design failed.

an intern on a classified project is tasked with standing bettertest up. they have never heard of it. they are on two hours of sleep. they have about four hours. they are handed a flash drive containing **the bettertest binary** and a handwritten word doc from the previous intern: a brief of what ci/cd is, and some commands that should work. they walk into an airgapped scif. no internet. no ai. no github. no pypi. no crates.io. no phone-a-friend.

there are two machines: one will be the worker, one will be the boss. they have to set it up.

consequences, all load-bearing:

- this is not a "quick test deploy." this **is** production. the special program will run it with zero intervention for the next five years, well after both interns are gone. no follow-up hardening, no "we'll put the db in the real place later," no babysitting. if it needs a grown-up to finish the job, the design failed.
- neither intern has sudo. they cannot mkdir in `/opt` or `/var/lib`, cannot install a systemd unit into `/etc`, cannot bind 80/443. the word doc puts boss and worker on **unprivileged ports** (>1024). if a step needs root, rewrite the product, not the intern.
- the database has to **survive reboot**. `/tmp` is tmpfs on fedora — gone. if intern 2 walks in after a reboot and history is empty, the design failed.
- the flash drive holds **one file** (the binary), not a source tree, not a wheel, not a venv. copy it onto both machines. the word doc's commands then work.
- python is assumed present (docker + python already on the box). **installing the bettertest python package is not.** there is no pip, no `uv`, no `PYTHONPATH` at a checkout that does not exist.
- the python library, parse script, and start-task harness travel **inside the binary** (`include_str!`) and/or **over the wire at dispatch time**. a worker that has only the OS, docker, python, and this binary can execute pipedefs.
- no compile-time filesystem paths (`CARGO_MANIFEST_DIR`, repo-relative pylib, etc). the intern is not in the repo. the binary they copied is the whole framework.
- the word doc is the ui of enrollment. if a step requires a tool they don't have or a site they can't reach, rewrite the product, not the intern.
- intern 1's shift ends. intern 2 logs into **the same machine as a different unix user** and runs the same word-doc commands. it Just Works. no `chown`, no `permission denied` on the db file, no "this directory is owned by intern1". the path is global; the uid of whoever ran it first is not a permission scheme. if intern 2 cannot kick a run, read logs, or start the binary because intern 1 exists, the design failed.
- the sqlite file intern 1 created (and its `-wal` / `-shm` siblings) **must be writable by intern 2**. intern 2 kicking a run is a write. sticky on `/var/tmp` does not grant that. the files are data, not executables: **`0666`**, not `0777`. the directory still needs `0777` — without `+x`, intern 2 cannot even open the file inside it. if intern 2 gets `eacces` because intern 1's umask made a `644` file, the design failed.

## HTTP API (goal)

Pipeline page talks to boss. Boss talks to worker. Worker is stateless: it streams a task start-to-finish and forgets.

```
Pipeline page
  GET  static files
  GET  /api/pipeline/{pipeline}
  GET  /api/pipeline/{pipeline}/events     SSE

Boss :9009
  static files
  GET  /api/pipeline/{pipeline}
  GET  /api/pipeline/{pipeline}/events     SSE
  GET  /api/run/{run}                     {run} is a uuid
  GET  /api/run/{run}/events              SSE
  POST /api/run                           start a run (noun: create a run)
  GET  /artifact/{artifact}

  boss → worker, with whole pipedef + image:
  POST {worker}/start-task

Worker :9010
  GET    /health
  POST   /start-container
  DELETE /container/{id}
  POST   /start-vm
  DELETE /vm/{id}
  POST   /start-task                      start a particular task of the pipedef
```

Known gap: this does not handle a spotty connection from worker to boss. If the worker→boss stream breaks, the task outcome is lost — the worker stores no state and there is no way to ask.

## Coding style

- Simple, readable, minimal. Write for the next person reading the file.
- One approach that always works. No try-this-then-fallback. No "just for now" second path.
- **Crashes are catastrophic.** Five years, no intern, no restart. Writing careful code is not enough — pick the API that **cannot panic**. A `Result` you `unwrap` is a crash you chose. Eliminate the possibility, don't just avoid it.
- Smallest working thing, then iterate. No speculative abstractions until there is a second real use.
- **Cargo.toml.** Pin minor versions only (`serde = "1"`, never `1.0.229`). Stay on the latest minor (`cargo update`). Never `default-features = false` — readable toml beats stripping features for crates that would not compile in anyway. Given those rules, trim aggressively: no crate we do not need, be wary of adding any. Prefer deleting a dependency over adding one.
- **The intern test is the deployment bar.** See that section. Sleep-deprived intern, flash drive (binary + word doc), two airgapped machines, no internet, no ai, ~4 hours, no sudo, unprivileged ports. This is the production system for the next five years, not a trial. Db survives reboot. Then a second intern, different unix user, same machine, same commands, Just Works — including writes to the db intern 1 created. If either intern cannot get it running, we failed. No pip, no source checkout, no `PYTHONPATH`, no second artifact besides the binary, no leftover ownership from the previous user.
- **No environment variables.** Config is a CLI arg, code, the db, a URL, or a cookie. Never `std::env` / `os.environ` / `$FOO`.
- **No polling. No websockets.** Push only: HTTP POST one way, SSE the other.
- **run is a noun.** A run is one instance of a pipeline being executed. `POST /api/run` creates a run. Verbs are `start-task`, `start-vm`, `start-container`.
- Frontend HTML tests assert **exact** HTML strings, not `contains()`.
- Don't make simple fixes to complicated problems, or complicated fixes to simple ones.
- **Names.** Package = product slice (`bettertest`, `bettertest-shared`, `bettertest-frontend`). Bin intern copies = `bettertest`. Lib `use` is the package with `-` → `_` (`bettertest_shared`). Modules are `db`, `boss`, `worker` — not packages. No `-package` / `-crate` suffixes.

## Git

Never fork. Push branches to `iv/bettertest`. Branch names are short and literal: `trim-noise`, not `gork/trim-noise`, not a joke.

## Verification evidence
Every PR must have verification evidence (VE) in the comments. This is collected material that proves that you've run and tested the change in the PR and it works as intended. This might be logs, captured API requests or responses, or screenshots. It must be complete enough that the reviewer has no need to pull the PR and run it themself and would gather no new information by doing so. You MUST actually capture real data for VE, you can't just hallucinate it.