# bettertest

## Problem

AI generates code faster than humans can verify it. YAML CI is slow, complex, and built for human operators. Tests are the only formal spec that matters: they say what the code does in a form people can actually read. Code can be slop — AI slop, legacy slop — as long as tests pass. That is the leverage point. Fearless refactoring, AI-generated code shipped now and cleaned later, a decade of spaghetti modernized, all of it only works if tests exist and they run.

## Technical approach

A single Rust binary (`bettertest-crate` from crate `bettertest-package`) with clap subcommands:

- `boss <pipedef>` — `:9009`. Embeds `frontend/dist`, sqlite, watches the pipedef file. HTTP API below. On a run, walks stages/tasks in order, POSTs each to `{task.worker}/start-task` with the whole pipedef and image, parses SSE stdout/stderr/status, writes the db.
- `worker` — `:9010`. HTTP API below. Spawns `python3 -c GLUE start <stage> <task>`, stdin JSON `{bettertest, pipedef}`, pumps child stdio as SSE.
- `dispatch <pipedef>` — parse and print. Does not hit a worker.
- `unified <pipedef>` — `tokio::try_join!(worker, boss)`.

**run is a noun, not a verb.** A run is one instance of a pipeline being executed. Create one with `POST /api/run`. Worker actions are verbs: `start-task`, `start-vm`, `start-container`. Pipedef `run(cmd)` is a Python function (`docker run ...`), not HTTP.

**Pipedefs** are imperative Python. `class` = stage, `@task(worker, image)` methods = tasks. `@task` returns a `staticmethod`. `run(cmd)` is `docker run --rm --entrypoint /bin/sh IMAGE -c cmd`. Sample: `pipedefs/glizzy.py`.

**Parse is not AST.** Rust embeds `pylib` + `core/scripts/glue.py` via `include_str!`. Glue execs the library into `sys.modules["bettertest"]`, execs the pipedef, then `dump_json()` (or calls the task). Pipedefs do `from bettertest import ...` only because glue stuffed the module first.

**Frontend** is Yew struct components (`PipelineView`, `LogsPage`), TEA, no function-components/hooks. Trunk proxies `/api/` to `:9009`. Grid + per-task logs. SSE for live updates.

**Persistence** is sqlite (`schema.sql`, rusqlite bundled). Runs / stages / tasks. Logs persist when a task finishes; live stream is in-memory broadcast.

Build: `./build.sh` (trunk release, then cargo release with static relocation + no unwind tables).

Python for pipedefs, Rust for the binary, Yew/wasm for the UI, docker on the worker for `run()`.

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
- Smallest working thing, then iterate. No speculative abstractions until there is a second real use.
- Prefer deleting a dependency over adding one. Until public release, take latest **stable** of everything, majors included.
- **No environment variables.** Config is a CLI arg, code, the db, a URL, or a cookie. Never `std::env` / `os.environ` / `$FOO`.
- **No polling. No websockets.** Push only: HTTP POST one way, SSE the other.
- **run is a noun.** A run is one instance of a pipeline being executed. `POST /api/run` creates a run. Verbs are `start-task`, `start-vm`, `start-container`.
- Frontend HTML tests assert **exact** HTML strings, not `contains()`.
- Don't make simple fixes to complicated problems, or complicated fixes to simple ones.
- Pedagogical crate/bin names (`bettertest-package` / `bettertest-crate`) are temporary; don't spread them further.

## Git

Never fork. Push branches to `iv/bettertest`. Branch names are short and literal: `trim-noise`, not `gork/trim-noise`, not a joke.

## Verification evidence
Every PR must have verification evidence (VE) in the comments. This is collected material that proves that you've run and tested the change in the PR and it works as intended. This might be logs, captured API requests or responses, or screenshots. It must be complete enough that the reviewer has no need to pull the PR and run it themself and would gather no new information by doing so. You MUST actually capture real data for VE, you can't just hallucinate it.