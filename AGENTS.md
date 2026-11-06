# bettertest

CI that doesn't suck. One binary, pipelines written in real Python, a web UI that shows green and red instantly. No YAML.

## Problem

AI generates code faster than humans can verify it. YAML CI is slow, complex, and built for human operators. Tests are the only formal spec that matters: they say what the code does in a form people can actually read. Code can be slop — AI slop, legacy slop — as long as tests pass. That is the leverage point. Fearless refactoring, AI-generated code shipped now and cleaned later, a decade of spaghetti modernized, all of it only works if tests exist and they run.

AI writes correct tests more reliably than correct code, and humans verify tests more easily than code. bettertest exists to make running those tests trivial, everywhere, forever.

## Mode: shipping

bettertest was a learning project; it is now a product we are trying to launch. Speed is the only moat.

- Agents write the code. Ship complete, working changes fast, with verification evidence. Don't ask permission for things this file already settles.
- Speed never buys a crash, a root requirement, or a broken intern test. Those rules below are not negotiable; everything else is.
- Prefer the smallest change that fully solves the problem, merged today, over the perfect one next week. Iterate in public on master.

## Glossary

- **pipeline** — one pipedef file, named after the file (`helloworld.py` → `helloworld`).
- **pipedef** — a Python file that defines a pipeline. Imperative, real code with real IDE support.
- **stage** — a `class X(Stage)`: a step, not a group. Stages run one at a time, top to bottom, in the order the pipedef defines them; once one fails, the rest are skipped. A stage holds every task that can run at that point, whatever it is about: what a task needs comes from an earlier stage, and nothing else decides where it goes. Debian and Fedora are not stages; Install, Reboot and Check are, each with a Debian and a Fedora task.
- **task** — an `@task(...)` method in a stage. A stage's tasks run in parallel, each in its own sandbox, unless they share one: the stage's (then one at a time, in order, or all at once with `parallel`), or a `Sandbox` of the pipeline's that lives across stages. A task is the unit of dispatch: its whole body runs on one worker.
- **run** — a noun: one execution of a pipeline. Shown by its 6-digit short id (`6dfde7`), like a git hash.
- **boss** — serves the UI and API, owns history, decides what runs where.
- **worker** — a machine that runs tasks: plain compute with docker, or a hardware fixture (robot arm, PCBA rig, GPIO). Workers are test fixtures, not dev machines.
- **sandbox** — a container or VM a task runs in. Started and stopped separately from tasks.
- **artifact** — an immutable file stored on the boss, published by a task with `artifact()` (build outputs, screenshots) or uploaded by hand. A sandbox image is not one of these: the pipedef names the image, and the worker pulls it.

## Product principles

- **One binary.** The flash drive holds one file. Python library, glue script and frontend travel inside it (`include_str!`, `rust-embed`).
- **Fully static.** The binary links nothing from the machine: no libc, no libssl, nothing (`ldd`: "not a dynamic executable"). It runs on any x86-64 or aarch64 Linux (one binary per architecture) whose kernel Rust supports, whatever the distro or its age. Built for the musl target, with sqlite and OpenSSL compiled in.
- **Platforms.** Officially supported targets, the machines bettertest runs on: 64-bit x86 Linux, 64-bit ARM Linux, Apple silicon macOS, 64-bit x86 Windows. Officially supported build platforms: x86 Linux and ARM Linux, and each of them must be able to build bettertest for every target. Cross-building uses zig as the C compiler (`cargo zigbuild`), which brings the C toolchain and libc headers for every target in one directory, without root.
- **Trust the machine's CA certificates.** bettertest may run on a corporate machine whose infosec team intercepts TLS with their own CA, installed in the system trust store. HTTPS must verify against that store, found at runtime (e.g. `/etc/ssl/certs/ca-certificates.crt`), never against roots baked into the binary. That is why TLS is OpenSSL (vendored, statically linked) through reqwest's `native-tls-vendored`, not rustls with bundled roots.
- **The pipedef is the source of truth** for pipeline structure. Nothing else introspects task names.
- **Pipedef code runs on the worker, natively** (not in a container), because a test may need to toggle a GPIO pin or talk to a serial bus. The worker handles isolation through sandboxes. A buggy pipedef can break its worker, never the boss.
- **Workers are disposable.** Workers join and leave the pool often, and may be wiped at any time. Nothing may rely on a worker keeping state between tasks. Once a worker finishes a task, the boss preserves whatever output and state of that task matters (logs, result files, build outputs); the worker knows only which files it produced, not how they will be used.
- **Push only.** HTTP POST one way, SSE the other. No polling anywhere, ever. No websockets. Everything stays inspectable with curl and mitmproxy.
- **Workers know the boss's address and can call boss APIs.** A worker learns it from the incoming API call: the boss's IP is the peer address of the `start-task` connection, and the boss port is `--boss-port` (7357 unless told otherwise), sent in that same form because TCP's source port is not the listen port. Those details persist on the worker for as long as that task runs, so the task calls the boss directly (e.g. `get_artifact()` is one GET). Never relay worker→boss requests through the task's SSE stream or invent callback schemes.
- **No environment variables.** Config is, in order of preference: nothing, code, the db, a URL, a cookie, a CLI arg. Never `std::env`, `os.environ`, `$FOO`.
- **Rootless.** Nothing needs sudo: no `/opt`, no `/etc`, no systemd units, no ports below 1024, no registry daemon config, no network bridges. The .deb is an extra for machines where root is available, never a requirement.
- **UI:** as few pages as possible (pipeline grid, task log, workers, settings, users). Fast enough that latency is never noticed. Keyboard navigable. The page never scrolls; only content regions do. Sharp square corners. Green good, red bad.
- **Builds cache, tests don't.**
- **Dev workflow:** a developer reserves a worker and runs `bettertest dispatch` from their laptop against it. Same code path as the boss, no daemon, no UI, no db. The worker can't tell the difference.

## The intern test

This is the deployment bar. If this person cannot get bettertest working, the design failed.

An intern on a classified project is tasked with standing bettertest up. They have never heard of it. They are on two hours of sleep. They have about four hours. They are handed a flash drive containing **the bettertest binary** and a handwritten word doc from the previous intern: a brief of what CI/CD is, and some commands that should work. They walk into an airgapped SCIF. No internet. No AI. No GitHub. No PyPI. No crates.io. No phone-a-friend.

There are two machines: one will be the worker, one will be the boss. They have to set it up.

Consequences, all load-bearing:

- This is not a "quick test deploy." This **is** production. The program will run it with zero intervention for the next five years, well after both interns are gone. No follow-up hardening, no "we'll put the db in the real place later," no babysitting. If it needs a grown-up to finish the job, the design failed.
- Neither intern has sudo. They cannot mkdir in `/opt` or `/var/lib`, cannot install a systemd unit into `/etc`, cannot bind 80/443. Boss and worker run on **unprivileged ports** (>1024). If a step needs root, rewrite the product, not the intern.
- The database has to **survive reboot**. `/tmp` is tmpfs on Fedora — gone. If intern 2 walks in after a reboot and history is empty, the design failed.
- The flash drive holds **one file** (the binary), not a source tree, not a wheel, not a venv. Copy it onto both machines. The word doc's commands then work.
- Python, docker (and qemu for VM tasks) are assumed present. **Installing the bettertest Python package is not.** There is no pip, no `uv`, no `PYTHONPATH` at a checkout that does not exist.
- The Python library, parse script and start-task harness travel **inside the binary** and/or **over the wire at dispatch time**. A worker that has only the OS, docker, Python and this binary can execute pipedefs.
- No compile-time filesystem paths (`CARGO_MANIFEST_DIR`, repo-relative pylib, etc). The intern is not in the repo.
- The word doc is the UI of enrollment. If a step requires a tool they don't have or a site they can't reach, rewrite the product, not the intern.
- Intern 1's shift ends. Intern 2 logs into **the same machine as a different unix user** and runs the same commands. It Just Works. No `chown`, no `permission denied`, no "this directory is owned by intern1". If intern 2 cannot kick a run, read logs, or start the binary because intern 1 exists, the design failed.
- The sqlite file intern 1 created (and its `-wal` / `-shm` siblings) **must be writable by intern 2**. Files are **`0666`**, directories **`0777`**, set explicitly because umask would make them 644/755. sqlite gives `-wal`/`-shm` the db file's mode, so a 0666 db covers all three.

## Scale

Assume bettertest runs nonstop for five years at a company with thousands of compute nodes and thousands of employees using it. Nothing may grow without bound in memory or on the wire: a pipeline with two million events in its history still loads with one bounded snapshot (the newest `RECENT_RUNS` runs), never by replaying history. Pick designs that stay correct and cheap at that size, even when the demo has three runs.

## Architecture

### Binary

`bettertest` with clap subcommands:

- `boss [--sources FILE] [--cert C --key K] [--boss-port N] <pipedef, directory or repo url>...` — `:7357` ("test" in leetspeak) unless `--boss-port` says otherwise. HTTPS with `--cert`/`--key`, plain HTTP without (see HTTPS). `--sources` reads more sources from a file, one per line (`#` comments): the place for repo urls with credentials, so they never show in the process list. One pipeline per pipedef file; a directory stands for every `*.py` in it, read at startup (no pipedefs is fine); a git repo url (`http://user:token@host/owner/name.git`) for every `.bettertest/*.py` on its default branch. See Repos. Re-reads and re-parses a pipedef whenever it is needed (page load, new run), so edits show up without a restart. A run: stages one at a time, top to bottom; a failed stage skips the rest.
- `worker [--label L]... [--tasks N] [--worker-port P]` — `:7358` unless `--worker-port` says otherwise. Runs tasks and their sandboxes, at most `N` at once (1 unless told otherwise: a worker with hardware on it can rarely run two tests at the same time). Its labels are its architecture's (`x86`, `arm`) plus each `--label`. On start, removes containers and VMs a crashed worker left behind.
- `dispatch <pipedef> --stage S --task T [--worker URL]` — runs one task on its worker (`--worker` for one that picks by labels, or to override the url the pipedef names) with no boss (in its stage's sandbox, if the stage has one), prints the log, exits 0 on pass and 1 on fail.
- `parse <pipedef>` — prints the pipedef's stages and tasks as JSON. Touches no worker.
- `unified [--boss-port N] [--worker-port P] <pipedef>...` — boss and worker in one process (`tokio::try_join!`). Same port flags as the separate commands, same defaults.

### Code map

```
Cargo.toml              workspace: shared, frontend, core; release profile (lto, z, abort, strip)
shared/src/lib.rs       bettertest-shared: wire types, Event, Live<S>, snapshots and their apply()
core/                   bettertest (the binary)
  schema.sql            the db's schema, the only one there has ever been
  scripts/glue.py       python side of the language boundary
  src/main.rs           clap subcommands, lock() helper
  src/auth.rs           who may do what: roles per route, Argon2id passwords, session tokens, lockout
  src/boss/mod.rs       the boss: its state, router, live SSE, settings, embedded UI
  src/boss/runs.rs      starting, running, cancelling and rerunning runs; run and task pages' API
  src/boss/workers.rs   the connection to each worker, and which worker a task runs on
  src/boss/users.rs     who a request is, logins (passwords, certificates, passkeys), users page
  src/boss/artifacts.rs uploaded artifacts and the ones tasks publish
  src/client.rs         talking to workers; shared by boss and dispatch
  src/db.rs             sqlite: open (a new db gets schema.sql), runs/stages/tasks/log lines
  src/dispatch.rs       dispatch and parse subcommands
  src/passkey.rs        passkeys: webauthn-rs fitted to the boss (origin from Host, user handles)
  src/pipedef.rs        embeds pylib + glue, parse() via python3
  src/repo.rs           git mirrors of repos, git archive, commit statuses on the forge
  src/tls.rs            HTTPS for the boss: loading --cert/--key, the listener
  src/worker.rs         worker HTTP API, containers, docker-load artifacts, start-task, task files
  src/vm.rs             rootless qemu VMs
  src/workdir.rs        /var/tmp/bettertest and its 0777/0666 permissions
  test/                 the tests of src/, one file per module they test
frontend/               bettertest-frontend: Yew/wasm, built by trunk, embedded in the binary
  src/main.rs           client-side routing (History API), who is logged in, live streams
  src/pages/            one Yew component per page: pipeline, task, workers, settings, users, login
  src/passkey.rs        the browser's half of passkeys (WebAuthn's JSON forms)
  src/view.rs           pure view functions
  test/view.rs          their exact-HTML tests
  style.css             the brand guide's tokens, type and cut corners
  assets/               fonts (with their OFL licenses), the mark, favicons; copied as they are
brand/                  the brand guide: style_guide.typ (Typst source), style_guide.pdf built from it by
                        build.sh, and the logo files it describes
pylib/src/bettertest/   the python library pipedefs import (Stage, task, run, copy, reboot)
pipedefs/               sample pipelines: helloworld (shipped in the .deb), random_time, weather,
                        artifact, vm
.bettertest/            bettertest's own CI pipeline (bettertest.py) and the container its packages build in
packaging/              systemd units and sysusers.d entry, shared by both packages
  deb/                  control file and maintainer scripts
  rpm/bettertest.spec   spec file
build.sh [arch]         trunk release build, then the static binary for this machine's arch or the one given
                        (another arch cross-builds with cargo zigbuild)
deb.sh [arch]           packages build.sh's binary as target/debian/bettertest_<version>_<amd64|arm64>.deb
rpm.sh [arch]           packages it as target/rpm/bettertest-<version>-1.<x86_64|aarch64>.rpm
release.sh [version]    everything a release ships, both archs, into dist/<version>/ (see Releases)
prettify.sh             clippy --fix, fmt, ruff, ty
```

Names: package = product slice (`bettertest`, `bettertest-shared`, `bettertest-frontend`). `use` paths swap `-` for `_`. Modules are `db`, `boss`, `worker`, not packages. No `-package` / `-crate` suffixes. The `core/` directory holds package `bettertest` because `core` clashes with Rust's standard library.

### Pipedefs

```python
import subprocess

from bettertest import Labels, Stage, copy, run, task


class Label(Labels):
    x86 = "x86"


IMAGE = "debian:trixie"


class Test(Stage):
    # Each task gets its own container; the tasks run at the same time. The task body runs on the
    # worker, in a checkout of the commit; commands in the container see only what is copied in.
    # labels are what a worker must have. The boss picks any enrolled worker that has them.
    labels = (Label.x86,)

    @task(image=IMAGE)
    def unit():
        subprocess.run(["tar", "-czf", "../src.tgz", "."], check=True)
        copy("../src.tgz", "/src.tgz")
        run("mkdir /src && tar -xzf /src.tgz -C /src && cd /src && cargo test")


class Deploy(Stage):
    # One shared container; the tasks run one at a time, in order, each building on the last.
    labels = (Label.x86,)
    image = IMAGE

    @task()
    def install():
        copy("dist/app.tar", "/tmp/app.tar")
        run("tar -xf /tmp/app.tar -C /opt")

    @task()
    def smoke():
        run("/opt/app/bin/app --version")
```

- `@task(worker=None, image=None, vm=None, arch=None, timeout=None, labels=())`. A task names its `worker` by url, or the `labels` a worker must have. With labels and no url, the boss runs it on any connected worker whose labels are a superset of that set, chosen uniformly at random among the ones with a free slot. A worker gets no more tasks at once than `worker --tasks` says it takes; while every matching worker is full, the task waits for a slot (its log says so, and cancelling the run ends the wait). The worker refuses (409) a task beyond that too. Dispatch reports that and stops. The boss waits and tries again: its count of running tasks and the worker's can disagree for a moment after a task exits, and a dispatch may hold a slot the boss does not know about. A stage that shares a sandbox holds one slot from its first task until its sandbox has stopped. With no connected worker whose labels cover the task's, the task fails at once, naming the connected workers and their labels, and any worker whose label is a likely typo of one asked for (same but for case, or a letter or two off): on purpose, since waiting would be silent and likely forever. It is never placed on a worker that lacks a label it asked for. A worker's labels are its architecture's, `x86` or `arm`, plus any it was started with (`worker --label gpu`). Labels are members of the pipedef's one `Labels` subclass (`class Label(Labels): x86 = "x86"`, then `labels=[Label.x86]`), so the IDE flags a label the pipeline did not declare, and parse refuses anything else. A stage may set `worker` or `labels` for all its tasks. A task with neither, whose stage sets neither, is a parse error, unless it runs in a Sandbox (that sandbox names the worker or the labels). Naming a worker and giving labels is a parse error. `image` is a docker image name (`debian:trixie`) or the URL of a `docker save` tarball. `vm` is the URL of a qcow2 image, the one the distro publishes. Never both. Either way the pipedef names the image's own source: the worker fetches it, and the pipedef does not point it at the boss or at a cache. `arch` (`"x86_64"` or `"aarch64"`, a `Literal`) is the vm's architecture: the worker's own when left out, run at full speed with kvm; another one is emulated, slowly. `timeout` is how many seconds the task body may take (starting its sandbox does not count); then the caller hangs up, which kills the task, and the task fails with "timed out after 2.5s". Tasks in a shared-sandbox stage may set it too. `@task` returns a `staticmethod`.
- Stages run in the order they are defined, one at a time; a failed stage skips every stage below it. Parallelism lives inside a stage: its tasks run at the same time. Setting `after` on a stage is a parse error.
- A stage that sets `worker` or `labels`, plus `image` or `vm`, gives all its tasks one shared sandbox. Its tasks use `@task()` (setting their own is an error), run in order, and once one fails the rest are skipped. Starting the sandbox shows in the first task's log, stopping it in the last one that ran. With `parallel = True` as well, they share the sandbox but run at the same time, each exec'd into it (`docker exec`, or ssh for a VM): for small tasks, where starting a sandbox each would take longer than the tasks themselves. Each still holds a slot of the worker, so a worker that takes fewer at once runs them a few at a time; a failed one fails the stage but stops none of the others, and rerunning one carries over its siblings. `parallel` on a stage without a sandbox is a parse error: other stages' tasks run at the same time anyway.
- `Sandbox(worker=..., labels=..., image=..., vm=..., arch=...)` at the top of a pipedef (`DEBIAN = Sandbox(...)`) is a sandbox that tasks of several stages share: `@task(sandbox=DEBIAN)`. The boss starts it on the worker it names, or on one whose labels cover `labels`, when the first stage that uses it comes up (it shows in that task's log), and stops it once the last stage that uses it is over, or when the run fails or is cancelled. So what one stage does to it the next finds: install in one stage, reboot in the next, check in the one after, with the same steps of other sandboxes alongside in each stage. Tasks of one stage in the same sandbox run one at a time, in the order they are defined, and once one fails the rest are skipped; different sandboxes run at the same time. Its name is the one the pipedef assigns it to. A Sandbox no task uses, a task in one that sets its own worker, labels, image, vm or arch, one with neither a worker nor labels, and one that is neither an image nor a vm are parse errors. A rerun needs each sandbox it uses built up again, so it starts at the first stage that uses any of them, whole stages.
- `run(cmd)` executes a shell command inside the task's sandbox: `docker exec` for a container, `ssh` for a VM. State carries between `run()` calls. A non-zero exit fails the task. With no sandbox, `run()` raises; use `subprocess` for host-side work.
- `copy(local, remote)` copies a file from the worker into the sandbox (`docker cp` / `scp`).
- `reboot()` cleanly reboots the task's VM and returns once it has booted again.
- `artifact(path, name="app.deb")` publishes a file as `name` (required, unique within the task). Like `run()`, `path` is inside the task's sandbox (`docker cp` / `scp` out), or on the worker when there is none. The boss keeps it once the task ends (pass or fail), lists it on the task page with a download link, and gives it to tasks that ask for it.
- Two ways to fetch one. Both return a `pathlib.Path` to the artifact on the worker (the file, not its contents: `copy()` it into the sandbox, or open it), GET it from the boss into the task's `inputs/` on the first call, and raise `LookupError` with the boss's reason when there is none.
  - `get_artifact_this_run(Build.packages, name="app.deb")`: this run. The task is a function of this pipedef, so the IDE checks it. The order of the stages alone decides what exists ("Can't get artifact note.txt of MadeLater.note because MadeLater.note hasn't run yet. Move MadeLater.note to a stage that runs before this one…").
  - `get_artifact(pipeline="app", stage="Build", task="packages", name="app.deb", run=LATEST)`: any run of any pipeline, this one included, all named by strings because an earlier run may have had other tasks. `run=LATEST` is the newest run in which the task passed and published it; `run="6dfde7"` (or more of the id, or all of it) pins one, ambiguous prefixes are refused. Logs `got app.deb of app Build.packages from run 6dfde7`.
- The task body is arbitrary Python running on the worker: subprocess, files, network, hardware.
- **Parse is not AST.** The binary embeds `pylib` and `core/scripts/glue.py`. Glue execs the library into `sys.modules["bettertest"]`, execs the pipedef (compiled as `pipedef.py` so tracebacks point at it), then either dumps the structure as JSON or calls one task. Pipedefs `from bettertest import ...` only works because glue planted the module first. Glue reads `{bettertest, pipedef, target, files}` as JSON on stdin; `target` tells `run()` where to send commands, `files` where artifacts go and which boss, pipeline and run to fetch them from.

### Sandboxes

Sandboxes are decoupled from tasks so one can be shared by several tasks, or (later) started early to be warm. The boss calls `start-container` / `start-vm`, then `start-task` with `target` for each task that uses it, then `DELETE`. A sandbox that fails to stop fails its (last) task: a leak is not a pass.

- **Containers:** `docker run --detach --init --label bettertest.worker` idling on a shell loop; `run()` execs into it. The label lets a restarted worker find leftovers.
- **Images the worker fetches:** `image` is a docker image name (`debian:trixie`); the worker pulls it. A URL instead is a `docker save` tarball, streamed into `docker load` once and tagged `bettertest-artifact:<hash of url>`. `vm` is the qcow2's own URL; the worker downloads it once into `vm-images/`. The pipedef names that source and never the boss. There is no registry of our own: docker's `insecure-registries` needs root on every worker, and a cache, when there is one, is the worker's.
- **VMs, rootless:** `qemu-system-x86_64` or `qemu-system-aarch64 -M virt,gic-version=max,highmem=off -bios QEMU_EFI.fd` (arm boots through UEFI firmware, from `edk2-aarch64` or `qemu-efi-aarch64`; `highmem=off` because an M1's kvm maps only 36 bits), with `-accel kvm -accel tcg` (kvm when the user may use `/dev/kvm` and the vm is the worker's own architecture, emulation otherwise) with user-mode networking: the worker reaches the VM's sshd via a port forwarded on 127.0.0.1; the VM reaches the worker at 10.0.2.2. cloud-init (NoCloud via SMBIOS serial) fetches user-data from the worker and installs `bettertest-ready.service`, which POSTs to `/vm/{id}/ready` at the end of every boot, after sshd is up. So nothing polls or retries ssh, on the first boot or after `reboot()` (`POST /vm/{id}/reboot` runs `sudo systemctl reboot --no-block` over ssh, so the command returns once the reboot is queued rather than dying with the shutdown, and waits for that call). A copy-on-write overlay sits on a cached base image (`/var/tmp/bettertest/vm-images/<hash>.qcow2`). Per-VM ed25519 key, user `bt` with passwordless sudo. Each VM gets 2048 MiB and a virtio rng. A native vm has 300s to finish booting. An emulated one has 30 minutes: TCG keeps guest time on the wall clock, and Debian cloud's `/boot/efi` is required by `local-fs.target`, whose failure is emergency mode (sulogin, and the image locks root, so the boot never continues). The worker writes `systemd.default_device_timeout_sec=900 systemd.default_timeout_start_sec=900` into the image's grub.cfg once, with debugfs (e2fsprogs), before the first overlay: it removes the old name and writes a new file, then `e2fsck -fn` has to pass. debugfs `ln` does not keep a link count, so a name left on a deleted inode still reads back and the guest's journal takes that inode; the next boot stops at the GRUB prompt. Failures include the qemu log and console tail, with AppArmor's per-profile lines left out so the failing unit stays visible.

### Live updates: snapshot, then events, never a gap

State lives in the db as state (runs / stages / tasks / log lines), not as an event history to replay. Every SSE stream starts with a snapshot of current state, then carries events, both as one SSE message type (`Live<S>`) so the browser can never reorder them.

The boss writes to the db and broadcasts the event under one lock (`Boss::record`), and takes a snapshot and subscribes under that same lock (`Boss::subscribe`). So every event is either already in the snapshot or arrives afterwards — never both, never neither. A client that falls behind the broadcast buffer gets a fresh snapshot instead of a gap. With nothing else to say, a stream sends a `ping` event every 15 s (an event, not an SSE comment, because browsers keep comments from the page). The browser reconnects a dropped stream by itself; the page says the boss is lost only once it has heard nothing, not even a ping, for a minute, so restarts and wifi blips pass unremarked. Snapshots are bounded (`RECENT_RUNS` = 50 runs for a pipeline). The fold (`apply`) lives in `shared` and is what the frontend runs. A stress test (20 concurrent runs, 60 clients joining at random) shows 0/60 diverging; a negative-control build with snapshot and subscribe 20 ms apart diverges 2/60.

### Repos

A pipeline that lives in a git repo is tied to commits. Every run records the commit and branch it tests (shown on the task page and as the run's tooltip in the grid), its pipedef is read from that commit, and each task runs in a fresh checkout of it.

- The boss keeps a bare mirror per repo in `/var/tmp/bettertest/repos/` (git `--shared=0666`, for intern 2), fetched at startup, when "new run" is pressed, and when a push arrives. Git gets the url without credentials, never in the mirror's config: it finds them through its `store` helper, in a 0600 file written for that one git command and removed after it (deliberately not 0666: it is a secret, and it outlives nothing). So with `--sources`, the token shows in no process's command line. A repo the forge cannot be reached for still serves from its mirror.
- Pipelines are the `.bettertest/*.py` files of the default branch (added to as fetches find more). The page shows the pipedef as the default branch has it; "new run" runs the default branch's newest commit.
- `POST /api/push-hook` takes a Forgejo/Gitea push webhook: fetch, then start every pipeline the pushed commit has, at that commit. Tags and deleted branches are ignored.
- Each run reports its state to the forge as a commit status, context `bettertest/<pipeline>`: pending when it starts, success or failure when it ends, linking to the pipeline page as the webhook or browser reached the boss. Forgejo and Gitea take that at `/api/v1/repos/<owner>/<repo>/statuses/<sha>` on the repo's host. A github.com repo is reported at `https://api.github.com/repos/<owner>/<repo>/statuses/<sha>`, with `User-Agent: bettertest` and the token from the repo url (`https://<token>@github.com/...` or `https://user:<token>@github.com/...`) as a bearer token. The token is not in that url, which is what a failure logs. GitHub answers 403, citing its user-agent rule, when the header is missing, and 401 when the header is present but there is no token. GitHub has no `warning` state and keeps 140 characters of description, so a cancelled run is `failure` there. A report the forge rejects is logged and the run still finishes. A pipedef that does not parse at the pushed commit is reported as failure.
- Only the newest commit of a branch is worth finishing. A run starting for a commit cancels the unfinished runs of the same pipeline on the same branch at other commits: the running task's stream ends with the reason ("cancelled: main @ 3fe66e3 replaced it"), so the worker kills the task while its sandbox is still stopped the normal way, and the stages left are skipped. Cancelled tasks show as skipped; the run counts as failed, and the forge gets a `warning` status (GitHub records that as `failure`, since it has no warning).
- Anyone with `write` can cancel a running run the same way, with the red X before its id on the grid or the task page (`POST /api/cancel-run/{run}`): running tasks are hung up on, and so killed, every task still to come is skipped, and a sandbox still starting stops being waited for (the worker finishes starting it, and it is stopped as soon as it is up).
- The forge's branch protection makes `bettertest/<pipeline>` a required check, so a PR cannot merge until its newest commit is green.
- Workers need neither git nor the repo's credentials: they get `git archive` of the commit from the boss.
- The python library is the boss's, whatever the commit: a pipedef at an old commit runs against today's API.

### Reruns

A finished run can be rerun whole, from a stage, or from one task (`POST /api/rerun/{run}[/{stage}[/{task}]]`, buttons on the task page). A rerun is a new run at the same commit, with the pipedef as that commit has it (a pipedef file is read as it is now). What comes before the rerun point is carried over instead of run again. What comes after runs again, since it builds on what changed.
- Rerunning stage S carries over every stage before S. Each of their tasks must have passed: a stage after a failed one never ran, so its task page offers only "rerun run" (the API answers 409).
- Rerunning task T of S also carries over T's siblings, with whatever result they had, unless S shares a sandbox (not `parallel`) or T runs in one of the pipeline's sandboxes. There each task builds on the one before, so the whole stage reruns.
- A rerun from S that needs one of the pipeline's sandboxes (`Sandbox`) starts at the first stage that uses it instead: the sandbox lived only as long as the earlier run, so it is built up again from there.
- A carried-over task is a new task run with the old state, worker and times, plus `carried_from`, which points at the task run that really ran. Its page shows that task's log and artifacts and links to its run, and `get_artifact_this_run` follows the pointer. On the grid, carried cells are faded.

### Workers page

The boss keeps a connection to every worker it knows: each one a pipedef names by url (enrolled when its pipeline page loads or a run starts) and each one added on the workers page, kept in the `workers` table. A task that only names labels runs on any enrolled worker whose labels cover that set. The connection is the worker's `GET /events` stream: one `worker` event saying what it is (hostname, arch, version), then keep-alives every 15 s. While that stream lasts the worker is up. When it ends, or stays quiet for 45 s (a frozen or unplugged machine never says goodbye), the worker is down, and the boss tries again after 1 s, doubling up to 30 s. The boss also tracks which tasks run on each worker. The workers page (`/workers`) shows all of it live, through the same snapshot-then-events stream as the other pages (`Event::Worker`, `Event::WorkerForgotten`). The worker state lives in memory, changed only under the db lock as well, so the snapshot is gapless. "forget" removes a worker; a pipedef that names it adds it back.

### Users

Every request acts as a user: the one whose session cookie it carries, or else **logged out** (users row 0, always there). Roles are `none` < `read` < `write` (start and rerun runs, add workers, upload artifacts) < `admin` (settings and users). A request's role is the higher of its user's and logged out's, so logging in never takes anything away. Logged out starts as admin: a boss nobody locked down works as it always did, and the first admin is made by whoever opens the users page. To lock down, add an admin who can log in, then lower logged out (to `none`, everything needs a login).

- `auth::needed` maps a request to the role it needs: pages, assets, login, logout and `/api/me` need none (the API calls a page makes are checked on their own); reads need read, other methods write; users, and changing settings, admin. Not enough is 401 when logged out (with what to do) and 403 when logged in.
- Passwords are Argon2id through the RustCrypto `argon2` crate, with its defaults (OWASP's minimum: 19 MiB, 2 passes), kept as PHC strings; at least 8 characters, hashed and checked off the async threads (the crate compares in constant time). Login gives the same answer for an unknown name as for a wrong password.
- Sessions: 32 random bytes in an HttpOnly, SameSite=Lax cookie; the db keeps only their sha256. 30 days since last use, extended at most once a day.
- `credentials` holds every way a user can log in, several per user (`kind`: password, certificate, passkey); the users page has a column per kind.
- The boss refuses (409) any change that would leave nobody able to administer it: logged out below admin and no admin with a credential (`auth::locks_out`), since undoing that would mean editing the db by hand.
- **Certificate login** (needs HTTPS). An admin clicks New Certificate on a user's row: the boss makes a P-256 key and a self-signed client certificate for that user (CN = the name, two years, `clientAuth`), packs them as PKCS#12 under a random password, and hands both over once, as a download and a line to copy. It keeps only the certificate's SHA-256 fingerprint, label and expiry, as a `certificate` credential; at most 5 per user (`MOST_CERTIFICATES`), numbered on so no two share a name. Over TLS the boss asks every browser for a client certificate without insisting and takes any (`tls::Peer`, from the connection), and a known fingerprint is that user on every request, no password. bettertest is still no CA: nothing chains to anything, the fingerprint is the identity. Revoke deletes the credential, and the certificate logs nobody in from then on.
- **Passkeys** (need HTTPS, or a browser at `localhost`: browsers offer WebAuthn nowhere else). Users add their own from Add Passkey in the header; an admin may add one for anyone from New Passkey on their row, with an authenticator at hand (a security key to hand over, say). Log In With Passkey on the login page asks for no name: the passkeys are discoverable, so the authenticator offers the ones it holds for this boss. Every check of the ceremonies (challenge, origin, host name, user presence and verification, signature, signature counter) is webauthn-rs's, Kanidm's WebAuthn library, on the OpenSSL compiled in; `passkey.rs` only builds it for the origin the request came to (its Host header, https when the boss speaks it) and turns user ids into user handles. The browser's JSON forms (`parseCreationOptionsFromJSON`, `toJSON`) carry the options and answers, so the frontend only passes JSON along. A ceremony's state lives in memory for five minutes under an id the browser sends back, and works once (at most 10 000 under way). Registration asks for no attestation: whoever may add the passkey is trusted, like a password set by an admin. The db keeps webauthn-rs's record of each passkey (its public key and counter) under its credential id; at most 5 per user (`MOST_PASSKEYS`), listed on the users page with Revoke, like certificates. A passkey logs in to a session cookie, like a password.
- **Tokens.** `Authorization: Bearer <token>` acts as someone without a cookie. A running task's token: the boss makes one per task, good while it runs (`BossTask`, in memory, hashed), and sends it in `start-task` as `boss_token`; the worker and the task's python send it when fetching the checkout and artifacts, and it counts as `read`, so a boss locked down to `none` still runs its pipelines. An API token: an admin makes one for a user on the users page (`POST /api/new-token`), shown once, kept hashed in `api_tokens`; it acts as that user, for scripts and curl. `POST /api/revoke-tokens` removes a user's.
- **The forge is a user too.** Before lowering logged out below `write`, add a service account (say `forge`, role `write`), make it an API token, and put `Bearer <token>` in the webhook's Authorization Header field (Forgejo and Gitea have one). Its pushes then start runs like any other write; the push hook has no auth of its own.
- The frontend asks `/api/me` before rendering a page; with role none it goes to `/login?next=<page>` first, and comes back there once logged in. Every header shows who is logged in and Log Out, or Log In.

### HTTPS

Off by default: the boss speaks plain HTTP, and nobody deploying it meets a certificate. `boss --cert boss.crt --key boss.key` turns it on. bettertest is not a certificate authority and makes no certificates: whoever turns HTTPS on brings a PEM certificate (intermediates after it) and its key, usually from the site's own CA, which the site's browsers and workers already trust. The key stays in the file it was given; bettertest never copies it.

- At start the boss checks the pair: it refuses to start, saying which, for a key that is not the certificate's, an expired certificate, or a file that is not PEM. It logs the names the certificate covers and when it expires.
- The boss port (`--boss-port`, 7357 unless told otherwise) then speaks HTTPS only (OpenSSL, the one already compiled in, through `tokio-openssl`, as in axum's `low-level-openssl` example); a plain `http://` request gets no answer. Each handshake runs on its own task with a 10 s limit, so a stalled client blocks nobody.
- The settings page says whether HTTPS is on, for which names, until when, and warns from 30 days before expiry. A new certificate takes a restart.
- Workers: the certificate names a host, but a worker only knows the address the boss called it from, and that name may have no DNS record. So `start-task` carries `boss_name`, and the worker and the task's Python connect to that address while checking the certificate against the name, with the machine's trust store as everywhere else. A worker whose machine does not trust the certificate fails the task with the TLS error.

### Persistence

- sqlite at `/var/tmp/bettertest/bettertest.db`, rusqlite bundled, WAL, `synchronous=NORMAL`, foreign keys on.
- **No migrations, until release.** `schema.sql` is the schema, as if the db had never been any other way. A new db gets it; a db with any tables is used as it is. Every change to it is a breaking one: change `schema.sql` in place, and delete the old db. No version tracking, no upgrade code, no compatibility with older dbs or older workers: none of that until Iv says bettertest is released and backward compatibility starts to matter.
- Tables: `pl_runs`, `stage_runs`, `task_runs`, `log_lines` (one row per line, written as it arrives). Order is insertion order (`rowid`).
- **Heartbeats.** The boss writes its own heartbeat to the `heartbeats` table every 15 s, and a worker's whenever its `/events` stream says anything (a keep-alive every 15 s). Once one is gone, the db says when it went.
- Runs a dead boss left `running` are failed on the next start. Their running tasks end at the boss's last heartbeat, not at the restart, with a log line giving that time.
- A worker that goes quiet is down since its last heartbeat, not since the boss gave up on it 45 s later. A running task's stream from its worker carries keep-alives too: a task whose worker says nothing for 45 s fails ("lost http://…: it has said nothing for 45s"), so a frozen or unplugged worker cannot hang a run.
- **Retention.** The settings page (`settings` table) says how many days after a task ends its artifacts and its log are kept; empty keeps them forever, the default. The boss sweeps at startup, every hour, and when the settings are saved: in batches of 100 tasks, each its own short transaction, so running tasks keep logging. It deletes artifact files before their rows, so a failed sweep is redone rather than leaving files nothing refers to. A task records when retention dealt with it (`log_deleted_at`, `artifacts_deleted_at`), so its page and `get_artifact` say retention deleted them rather than that they never existed. Time is a parameter of the db functions, so tests check "30 days later" instantly (see the tests in `db.rs`); for end-to-end evidence, move tasks' `ended_at` back in sqlite and save the settings.
- Everything else on disk lives under `/var/tmp/bettertest` too: `artifacts/` (uploaded), `task-artifacts/<task>/` (published by tasks, listed in `task_artifacts`), `vm-images/` (worker cache), `vms/` (per-VM scratch), `repos/` (the boss's bare git mirrors), `tasks/<id>/` (a worker's checkout, inputs and outputs of one running task).

### Worker output

`start-task` unpacks the run's commit (when it carries one) into `tasks/<id>/src/` from the boss's `/api/run/<run>/source.tar`, spawns `python3 -u -c GLUE start <stage> <task>` in that directory, and streams stdout/stderr as SSE events `stdout` / `stderr`, then one `artifact` event (`<id>/<name>`) per file the task published, then `exit` with the code (-1 for a signal). When the form carries `pipeline` and `run`, the task gets them and the boss URL `http://<caller ip>:<boss-port>` to fetch artifacts from (`https://<boss_name>:<boss-port>`, reached at the caller's address, when the boss speaks HTTPS). `boss-port` is `--boss-port` (7357 unless told otherwise), sent in the form; the IP is still the peer address of the connection. The worker keeps `tasks/<id>/` until the caller hangs up, so the caller downloads each artifact (`GET /output/<id>/<name>`) as its event arrives; then the directory goes, so a worker keeps nothing of a task once it is over. Output is split on raw bytes, decoded lossily, and `\r` becomes a line break, because axum's SSE encoder panics on a carriage return. If the caller hangs up, the stream drops and the task is killed: python runs in a process group of its own, and the whole group gets SIGKILL (through `sh -c 'kill -KILL -<pgid>'`, since std cannot signal a group and kill(1) is not on every machine), so a subprocess it started dies with it.

### Frontend

Yew struct components, one per page (`PipelinePage`, `TaskPage`, …, in `pages/`), Elm architecture, no function components or hooks. The shell (the wasm, its JS and the CSS) loads once. Links use the History API, so moving between pages does not fetch the shell again, only that page's data; back and forward restore the URL and the page. A refresh still works, because the boss serves `index.html` for every page. `/` redirects to the first pipeline. Hashed shell files are cached forever; `index.html` is not, because it is what names them.

- Pipeline page: run grid (newest run on top, one column per task, grouped by stage; the header follows the current pipedef, so tasks a run predates show empty cells), "new run" button, spreadsheet-style pipeline tabs along the bottom. The grid is a CSS grid: each stage is √(its tasks) wide, so a stage of two is wider than a stage of one but each of its tasks narrower. Tasks sit 2px apart, stages 10px. Task names lie flat when they all fit. Otherwise they all tilt by one angle, the flattest that keeps neighbours a full line apart across (`label_layout`), measured after every render, on resize and once the fonts load. A stage whose columns are narrower than a line runs its tasks together into one bar, hides their names, and names the task in each cell's tooltip instead. Running cells pulse. A finished cell's tooltip says how long it took.
- Task page: path (`pipeline / run / stage / task`) plus a state badge, the worker it ran on, how long it took and rerun buttons (once the run is over) at the right (counting up while it runs, from the boss's clock in the snapshot), then the live log. The log container is `flex-direction: column-reverse` so the browser keeps the newest line in view while the reader is at the bottom.
- Settings page: artifact and log retention in days, with what saving deleted.
- Workers page: one row per worker, up (green edge) or down (red edge, with why), how many tasks it runs of how many it takes, for how long, and the tasks running on it, with a form to add one by url. Every page header links to it and to the settings page.
- Dark by default, light under `prefers-color-scheme: light`.
- Archivo is narrower than the system font it replaced, so UI text is sized for the old width rather than the guide's point sizes (copy 18px, grid labels 20px at width 62). Stage names keep their case. Buttons and links are in Title Case ("New Run").
- Looks follow `brand/style_guide.pdf`. Black ground, white type. Red is for primary buttons, orange for highlights. Pass green and fail red are the product's own colors. Fonts are Archivo (variable, moved along its width axis: 62 for labels and the grid, 80 for copy) and Martian Mono for code and logs, both self-hosted from `frontend/assets/` because the UI must work airgapped. Buttons and pop-up windows cut their top-left and bottom-right corners at 45° (10px, 6px small, 16px windows). The grid and logs stay square. Assets in `frontend/assets/` are served as immutable without a content hash, so a changed one needs a new `.vN` name.
- One override of the guide, by Iv: the gradient mark goes on white at any size, not only from 48px up.

## HTTP API

API names are verb-noun where a bare noun is ambiguous: `POST /api/run` could mean start or get, so it is `POST /api/start-run/{pipeline}`. Unambiguous reads stay nouns.

```
Boss :7357 (`--boss-port`)
  /, /pipeline/{pipeline}, /task/{task}   pages (/ redirects to the first pipeline)
  /workers, /settings, /users, /login     pages
  GET  /api/pipelines                     pipeline names
  GET  /api/pipeline/{pipeline}           stages and tasks, parsed from the pipedef now
  GET  /api/pipeline/{pipeline}/events    SSE: snapshot (pipedef + newest runs), then events
  POST /api/start-run/{pipeline}          create a run, returns it (422 if the pipedef is broken)
  POST /api/push-hook                     Forgejo/Gitea push webhook: runs every pipeline at the commit
  POST /api/cancel-run/{run}               cancels a running run, all of it (409 once it is over)
  POST /api/rerun/{run}[/{stage}[/{task}]]  rerun from there, carrying over what came before; returns the
                                          new run (409 while the run runs, or if an earlier stage failed)
  GET  /api/settings                      {"artifact_days": 30, "log_days": null}; null keeps forever
  PUT  /api/settings                      the same; applies them at once, returns {"logs": n, "artifacts": n}
                                          that it deleted
  GET  /api/me                            {"name", "role", "logged_in"}: who this browser is
  POST /api/login                         {"name", "password"}: sets the session cookie
  POST /api/logout                        ends the session
  GET  /api/users                         every user, logged out first, with counts of each kind of credential
  POST /api/add-user                      {"name", "role", "password"}
  POST /api/new-token                     {"id"}: a new API token for the user, returned this once
  POST /api/new-certificate               {"id"}: a client certificate for the user: {"label", "p12" (base64),
                                          "password"}, this once; 409 over plain HTTP or past 5
  POST /api/revoke-credential             {"id"} of a certificate or passkey credential
  POST /api/new-passkey-options           {"id"} or {"id": null} for oneself: {"ceremony", "publicKey"}, WebAuthn creation options;
                                          an admin's for others; 409 past 5
  POST /api/new-passkey                   {"ceremony", "credential"}: what navigator.credentials.create() made, as toJSON() gives it
  POST /api/passkey-login-options         {"ceremony", "publicKey"}: WebAuthn request options, for anyone
  POST /api/passkey-login                 {"ceremony", "credential"}: what navigator.credentials.get() signed; sets the session cookie
  POST /api/revoke-tokens                 {"id"}: removes all of the user's API tokens
  POST /api/set-role | set-password | remove-user   {"id", "role" | "password"}; 409 if it would lock everyone out
  GET  /api/https                         null over plain HTTP, else {"names", "expires", "days_left"}
  GET  /api/workers                       the workers the boss knows, up or down, and what runs on them
  GET  /api/workers/events                SSE: snapshot (all workers), then events
  POST /api/add-worker                    {"url": "http://worker-host:7358"}
  POST /api/forget-worker                 {"url": ...}; a pipedef that names it adds it back
  GET  /api/run/{run}                     {run} is a uuid
  GET  /api/run/{run}/events              SSE
  GET  /api/run/{run}/source.tar          git archive of the commit the run tests
  GET  /api/task/{task}/log               plain text
  GET  /api/task/{task}/events            SSE: snapshot (log and artifacts so far), then events
  GET  /api/task/{task}/artifact/{name}   download a file the task published
  GET  /api/run/{run}/artifact/{stage}/{task}/{name}   the same by name, for a later task of the run;
                                          404 says why there is none
  GET  /api/pipeline/{pipeline}/run/{run}/artifact/{stage}/{task}/{name}
                                          {run} is `latest` or a run id (prefix ok); header
                                          bettertest-run names the run; 404 says why there is none
  PUT  /artifact/{artifact}               upload; 409 if it exists
  GET  /artifact/{artifact}

  boss → worker, per task (the worker learns the boss's address from these calls):
  POST {worker}/start-container | start-vm  (when the task has an image | vm)
  POST {worker}/start-task                  multipart: pipedef, stage, task, target, pipeline, run, commit,
                                            boss_token (the task's, to fetch from a locked-down boss)
                                            boss_name (the certificate's name, when the boss speaks HTTPS)
                                            boss_port (where the boss listens; the IP comes from the connection)
  GET  {worker}/output/{id}/{name}          each artifact the task published, before hanging up
  GET  {worker}/events                      for as long as the boss knows the worker
  DELETE {worker}/container/{id} | vm/{id}

Worker :7358 (`--worker-port`)
  GET    /health
  GET    /events                          SSE: a `worker` event (hostname, arch, version, labels, capacity), then
                                          keep-alives
  POST   /start-container                 {"image": name or tarball URL} → {"id"}
  DELETE /container/{id}
  POST   /start-vm                        {"image": qcow2 URL} → {"id"}, once booted
  DELETE /vm/{id}
  POST   /start-task                      SSE: stdout / stderr lines, artifacts, then exit
  GET    /output/{id}/{name}              a file the running task published
  GET    /vm/{id}/{user-data,meta-data}   for cloud-init inside the vm
  POST   /vm/{id}/reboot                  reboot cleanly, return once booted again
  POST   /vm/{id}/ready                   the vm finished booting (from inside the vm)
```

## Deploying

### With root: the .deb or .rpm

```sh
sudo apt install ./bettertest_0.1.0_amd64.deb      # Debian, Ubuntu
sudo dnf install ./bettertest-0.1.0-1.x86_64.rpm   # Fedora, RHEL
```

Both packages do the same: each installs `/usr/bin/bettertest`, creates a `bettertest` system user (member of `docker` and `kvm`), and enables and starts `bettertest-boss` (port 7357 unless `--boss-port`, pipelines from `/etc/bettertest/pipedefs`, which ships `helloworld.py`) and `bettertest-worker` (port 7358 unless `--worker-port`). Both restart on failure and start on boot. A worker a pipedef names by url is enrolled when that pipeline is loaded. `helloworld.py` asks for the `x86` label instead, so enroll that worker once, on the workers page or with `POST /api/add-worker` and `{"url":"http://[::1]:7358"}` when it is on the same machine. Add a pipedef to `/etc/bettertest/pipedefs` and `systemctl restart bettertest-boss`. Depends only on `python3`, since the binary is static; recommends docker, qemu and openssh-client for sandboxes. Data stays in `/var/tmp/bettertest`, even on purge. The packages declare the `docker` group themselves (`g docker -` in sysusers.d), so rpm sees it provided and installs without docker present.

### Without root (the word doc)

Both machines: copy `bettertest` from the flash drive anywhere in your home directory, `chmod +x bettertest`.

```sh
# worker machine (needs python3, docker; qemu-system-x86_64, qemu-img, ssh, ssh-keygen for vm tasks)
# its labels are its architecture (x86 or arm) plus any --label
nohup ./bettertest worker > worker.log 2>&1 &

# boss machine (needs python3 and git). A pipedef names a worker url, or labels.
# The pipelines here ask for labels, so enroll the worker once.
echo http://ci:TOKEN@forge-host:3000/team/app.git > sources.txt && chmod 600 sources.txt
nohup ./bettertest boss --sources sources.txt > boss.log 2>&1 &
# (or pipedef files: nohup ./bettertest boss app.py ...)
# enroll the worker once. Tasks run on any enrolled worker whose labels cover what they ask for.
curl -X POST http://boss-host:7357/api/add-worker \
  -H 'content-type: application/json' \
  -d '{"url":"http://worker-host:7358"}'
# then in the forge, repo settings > webhooks: push events, POST, json, to
#   http://boss-host:7357/api/push-hook
#   (once logins are needed: Authorization Header "Bearer <the forge user's API token>", see Users)

# The pipedef names the image. The worker pulls a docker name, or downloads a vm from
# the qcow2's own URL. Neither is an address of the boss.
#   @task(image="debian:trixie", labels=[Label.x86])
#   @task(vm="https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2", labels=[Label.x86])
```

Open `http://boss-host:7357`. History lives in `/var/tmp/bettertest` and survives reboot. Starting boss and worker again after a reboot is the machine's job (systemd, as the .deb and .rpm set up, or whatever else it uses), not bettertest's.

## Development

```sh
./build.sh                                   # trunk release build of the frontend, then cargo release
./deb.sh                                     # after build.sh: target/debian/bettertest_<version>_<arch>.deb
./rpm.sh                                     # after build.sh: target/rpm/bettertest-<version>-1.<arch>.rpm
./release.sh 0.1.0                           # both archs, every package, into dist/0.1.0/
target/x86_64-unknown-linux-musl/release/bettertest unified pipedefs   # http://[::1]:7357
cd frontend && trunk serve                   # :8080, proxies /api/ to :7357, hot reload
cargo test                                   # core (SSE decoder) + frontend (exact HTML)
./prettify.sh                                # clippy --fix, fmt, one blank line between fns, ruff, ty
```

- Core embeds `frontend/dist` at compile time: rebuild the frontend (or run `build.sh`) before `cargo run`, or the binary serves a stale UI.
- `uv sync` at the repo root installs `pylib` editable so IDEs type-check pipedefs.
- The dev db is `/var/tmp/bettertest/bettertest.db`. After a change to `schema.sql`, move it aside (never delete someone's db without asking) and let the boss make a new one.
- Build machine needs: rust with the `<arch>-unknown-linux-musl` (`x86_64` or `aarch64`) and `wasm32-unknown-unknown` targets, trunk, `musl-gcc` (Fedora: `musl-gcc`), the Perl modules OpenSSL's build uses (Fedora: `perl-FindBin perl-IPC-Cmd perl-File-Compare perl-File-Copy perl-lib perl-Time-Piece`), `dpkg-deb` for `deb.sh` (Fedora: `dpkg`), and `rpmbuild` plus the systemd rpm macros for `rpm.sh` (Fedora: `rpm-build systemd-rpm-macros`).
- `build.sh` passes `--target $(uname -m)-unknown-linux-musl`, so it builds for the machine it runs on. Besides making the binary static, an explicit target keeps `-Crelocation-model=static` and `-Cforce-unwind-tables=no` off the proc-macro dylibs.
- Cross-building for another target, without root, uses zig as the C compiler: `RUSTFLAGS="-Crelocation-model=static -Cforce-unwind-tables=no" cargo zigbuild --release -p bettertest --target aarch64-unknown-linux-musl` on x86_64 (or `x86_64-unknown-linux-musl` on aarch64). Needs zig (a tarball from ziglang.org, unpacked anywhere on PATH), `cargo install cargo-zigbuild`, and `rustup target add <target>`.
- **bettertest tests itself** with `.bettertest/bettertest.py`, five stages, each a step: Prepare (the image the packages build in; artifacts made on the worker and in a container), Build (the .deb and .rpm, while a Debian 13, a Fedora 44 and an emulated aarch64 Debian 13 VM boot), Install (each package on its distro; copies to and from the arm VM; every check of artifacts, and of the errors for ones that are not there), Reboot (every VM), Check (the services came back, the web UI is up, a helloworld run passes; the arm VM really rebooted). The check enrolls the vm's own worker at `http://localhost:7358` before that helloworld run, because helloworld only asks for the `x86` label and a freshly installed boss knows no worker. The VMs are `Sandbox`es that live across those stages, three of them at once, each 2048 MiB: about 6 GiB before qemu overhead and the package build running beside them, so the worker needs roughly 8 GiB of RAM. A 4 GiB machine cannot run this pipeline. The worker runs with `--tasks 8`. Build starts four tasks at once, so `--tasks 4` does not queue there; fewer only queues, and the VMs still all stay up. The arm VM is emulated and needs `qemu-system-aarch64`, UEFI firmware (`/usr/share/edk2/aarch64/QEMU_EFI.fd` or `/usr/share/qemu-efi-aarch64/QEMU_EFI.fd`) and debugfs (e2fsprogs); boot gives up after 30 minutes. Run it with a copy of the binary, pointed at the repo: `cp target/x86_64-unknown-linux-musl/release/bettertest /tmp/bt && /tmp/bt unified http://ci:TOKEN@forge-host:3000/team/bettertest.git`. Enroll that worker once (`POST /api/add-worker` with `http://[::1]:7358`); the pipedef asks for the `x86` label. It names the Debian 13 genericcloud (amd64 and arm64) and Fedora 44 Cloud Base Generic images at their upstream URLs, and the worker downloads those. Push (or start a run). The packages build in a fresh container of `.bettertest/build.Dockerfile` (the build machine above, on Fedora), which the Prepare stage builds on the worker first (docker's cache makes that instant), so runs at the same time share nothing; a build starts from nothing, about two minutes. It fails unless the binary is fully static (`readelf` finds no interpreter or dynamic section).
- **Binary size.** The static release binary is about 10.4 MB on stable (8.7 MB before HTTPS, users and webauthn-rs; 3.5 MB when it was dynamically linked). An experiment on nightly (`-Zbuild-std`, `-Cpanic=immediate-abort`, trimmed axum features; the code is gone) reached 362 KB. Biggest single win: `-Cforce-unwind-tables=no`. **Unsound here, never use:** `-Zvirtual-function-elimination` (segfaults at startup) and lld `--icf=all` (breaks async waker vtable identity). `opt-level = "z"` beats `"s"`.

### Releases

Semantic versioning, from `core/Cargo.toml`'s version; a release is the tag `v<version>` on master plus a Forgejo release of that name with `release.sh`'s files attached. `./release.sh <version>` (on x86_64, zig on PATH) refuses a version Cargo.toml does not have, then builds each architecture once and packages it, so all of an architecture's files carry the same binary (rpm's post-install scripts are off so it does not re-strip it):

```
dist/0.1.0/  bettertest-0.1.0-x86_64-linux  bettertest-0.1.0-aarch64-linux
             bettertest_0.1.0_amd64.deb     bettertest_0.1.0_arm64.deb
             bettertest-0.1.0-1.x86_64.rpm  bettertest-0.1.0-1.aarch64.rpm   SHA256SUMS
```

To release: bump the version in `core/Cargo.toml` in a PR, merge it, tag the merge commit, run `release.sh`, and create the release on Forgejo with the files.

### Verification toolkit

What has worked for evidence on this machine:

- curl against the API, `curl -N` for SSE streams, `sqlite3` on the db, `ls -la` for permissions.
- Playwright + Chromium (Python package installed) for driving the UI, asserting DOM state and taking screenshots in both color schemes.
- `sudo -n -u nobody ./bettertest boss ...` to play intern 2 against intern 1's files.
- `kill -9` of boss or worker mid-run for crash recovery; `dispatch` + kill for hang-up.
- The gapless stress test above, plus its negative control, whenever the live-update code changes.
- Keep evidence outside `/tmp`: a reboot wipes it.

## Coding style

- Simple, readable, minimal. Write for the next person reading the file. Anti-code-golf, anti-slop.
- One approach that always works. No try-this-then-fallback. No "just for now" second path.
- **Crashes are catastrophic.** Five years, no intern, no restart. Pick the API that **cannot panic**. A `Result` you `unwrap` is a crash you chose. No `unwrap` / `expect` / `panic!` / indexing that can go out of bounds outside tests. The release profile is `panic = "abort"`: a panic anywhere, even in a spawned task, kills the whole process.
- Smallest working thing, then iterate. No speculative abstractions until there is a second real use.
- **Execution order is the order in the file.** Stages run top to bottom, one at a time, exactly as written. Never add ordering schemes: no dependency declarations, no `after` / `needs` / `inputs` lists, no graphs inferred from what a task reads or writes, nothing that runs a stage anywhere other than its place in the file. Concurrency comes only from tasks within one stage. Someone reading the pipedef top to bottom must know the order without thinking.
- Don't make simple fixes to complicated problems, or complicated fixes to simple ones.
- **Security code is a library's.** Cryptography and security protocols (TLS, password hashing, WebAuthn) go through a vetted library: OpenSSL, webauthn-rs. Never hand-roll their parsing or checks, even when it looks like a few lines; a dependency beats owning that code.
- Delete code whenever it can go without breaking anything.
- Comments say *why*, never *what*. No progress printouts in code ("🚀 downloading...").
- **Cargo.toml.** Pin minor versions only (`serde = "1"`, never `1.0.229`). Stay on the latest minor (`cargo update`). Never `default-features = false` — readable toml beats stripping features. Trim aggressively: no crate we do not need. Prefer deleting a dependency over adding one.
- Frontend HTML tests assert **exact** HTML strings, not `contains()`.
- **Tests live apart from the code**, in `core/test/` and `frontend/test/`, one file per module they test. Each is still that module's child, so it reaches what the module keeps private: the module ends with `#[cfg(test)] #[path = "../test/<module>.rs"] mod tests;`. Not `tests/`, which cargo would build as separate crates that see only public items.
- Validate by running the thing. Never claim something works without having run it.

## Git and PRs

- Forgejo at `http://van.local:3000/iv/bettertest` is the source of truth (git remote `origin`). The `github` remote (https://github.com/ivhacks/bettertest) is a mirror, not where PRs go. Never fork.
- Master is the only long-lived branch. Feature branches are short and literal (`trim-noise`, not `gork/trim-noise`, not a joke) and are deleted after merging.
- Claude works on Forgejo as **`clod`** (admin): pushes, opens PRs, answers review comments, merges. Credentials come from the user per session and are never stored in git config or files in the repo. Forgejo's API cannot reply inside an inline review thread; answer with one PR comment quoting each point.
- PRs land on master as **squash merges** only (the repo enforces it): one commit per PR, authored by whoever wrote the PR. Merge once Iv approves in a comment.
- Commits by Claude end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; PR descriptions end with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

### Verification evidence

Every PR must have verification evidence (VE) in its comments: collected material proving the change was run and works as intended — logs, captured API requests and responses, screenshots. It must be complete enough that the reviewer would gain nothing by pulling the PR and running it. Capture real data; never invent it.

## Known gaps

- Of the supported targets, only x86_64 and aarch64 Linux build and run today. The worker leans on Linux (`/var/tmp`, `/proc`, `kill`, docker, qemu), so Windows and macOS need their own ways to run tasks and sandboxes, and macOS allows no fully static binary (libSystem is always dynamic). Cross-builds are only set up between the two Linux targets.

- Forgejo refuses webhooks to private addresses unless its `app.ini` says `[webhook] ALLOWED_HOST_LIST = private` (or the boss's host).
- While logged out may `write`, anyone who can reach the boss can make the push hook run a commit the repo really has. Lock down, and give the forge a service account's token (see Users).
- A repo url given on the command line (not through `--sources`) shows its credentials in the process list.
- The static binary resolves names through `/etc/hosts` and DNS only, not mDNS: `forge.local` fails ("dns error") where git and curl, which use glibc's resolver plugins, succeed. Use an IP or a DNS name.
- Commit statuses on GitHub need a token in the repo url (a classic PAT with `repo:status`, or a fine-grained token that can write commit statuses). The request sends `User-Agent: bettertest`; without that header GitHub answers 403. Without a token the report is 401 and the run still finishes. The token is not written into the logged url. GitHub has no `warning` state, so a cancelled run is reported as `failure`.
- `dispatch` has no run or boss, so its task runs in an empty directory rather than a checkout, fetching artifacts fails there, and artifacts it publishes land in the current directory.
- Retention deletes logs and task artifacts, never runs: the grid keeps a row for every run forever.
- A broken worker→boss stream fails the task; the worker keeps no state to ask about it afterwards.
- A worker killed with SIGKILL leaves its python task processes running (containers and VMs are cleaned on the next start).
- A pipeline's `Sandbox` stays up on its worker between the stages that use it. A boss that dies meanwhile leaves it there until that worker restarts.
- Starting a sandbox and fetching a task's checkout have no time limit of their own: a worker that freezes during either holds the task until the run is cancelled. A running task's stream has one (45 s of silence).
- Containers run the worker's own architecture only. Sibling VMs cannot talk to each other yet (qemu multicast-socket networking is the rootless way).
- Passwords cross the network in the clear unless the boss speaks HTTPS.
- Passkeys need a browser that has WebAuthn's JSON helpers (`PublicKeyCredential.parseCreationOptionsFromJSON`, `toJSON`): Chrome and Edge 129, Firefox 119, recent Safari. Behind a reverse proxy that ends TLS, the boss sees plain HTTP and expects an `http://` origin, so passkeys fail there.
- Passkey ceremonies require user verification (a PIN, fingerprint or unlocked phone), as webauthn-rs does for passkeys: a security key without a PIN set cannot be one.
- Anyone can start passkey logins, so someone flooding them can fill the 10 000 ceremonies and keep passkey logins out for five minutes; passwords and certificates still work.
- The .p12 uses OpenSSL 3's default encryption (AES-256, PBKDF2), which current browsers and OSes import; some old ones (macOS before 10.15) only take the legacy 3DES kind.
- Short run ids can collide at scale; display only (links use the full uuid). Lengthen the prefix when ambiguous, like git, if it ever matters.

## Roadmap and ideas

- Warm sandbox pools, `max_parallel` per stage.
- A test that enforces the CA-store rule.
- A smaller static binary.
- Scheduled runs; multiple workers with worker selection; reserved dev workers the boss skips.
- Worker enrollment that needs only ssh; reaching workers through ssh port forwarding instead of exposing them.
- Interactive debugging: `ipdb` in a task turns the cell purple and offers a terminal in the browser (libghostty) or an ssh command into the worker's python process.
- Regex filters to hide noisy log lines. Hover tooltips on cells. Disabled tasks. A file watcher on pipedefs (all three were prototyped once; the code is gone).
- Builds and artifacts with caching, flake detection, GitHub integration, PR events.
- Retention for runs themselves (their grid rows and task rows), at five-year scale.
