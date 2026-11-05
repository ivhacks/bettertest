<img src="brand/logo_assets/bettertest-gradient.svg" alt="" width="96">

# bettertest

CI that doesn't suck. One binary, pipelines written in real Python, a web UI that shows green and red
instantly. No YAML.

- **One file.** The binary carries everything: the Python library, the web UI, the database. It is
  fully static, so it runs on any x86-64 or ARM64 Linux, whatever the distro.
- **Root and internet are optional.** The usual install is a package, with root, and most pipelines
  use the internet. But it can also run as an ordinary user, on ports above 1024, keeping its history
  in `/var/tmp/bettertest`, and be stood up in an airgapped room from a flash drive.
- **Pipelines are Python.** Real code, real IDE support. A task can run in a container, in a VM
  (x86 or ARM, emulated or not), or straight on a machine, to toggle a GPIO pin or talk to a rig.

## Five minutes

Two machines: a **worker** that runs tasks (it needs python3 and docker; qemu for VMs) and a **boss**
that serves the web UI and decides what runs where (it needs python3, and git for repos). Copy the
binary for your machines (x86_64 or aarch64, from the [releases page](https://github.com/ivhacks/bettertest/releases), or built as below) onto both.

```sh
# on the worker
nohup ./bettertest worker > worker.log 2>&1 &

# on the boss
nohup ./bettertest boss pipelines/ > boss.log 2>&1 &
```

Open `http://boss-host:7357`. Starting them again after a reboot is the machine's job: the packages
below install systemd units that do it, or use whatever your machines already use.

To run the pipelines of a git repo on every push, give the boss the repo instead of a directory, and
point the forge's push webhook (Forgejo or Gitea) at `http://boss-host:7357/api/push-hook`. Put the
repo url, with the credentials the boss fetches with, in a file only you can read, so they never show
in the process list:

```sh
echo http://ci:TOKEN@forge-host:3000/team/app.git > sources.txt && chmod 600 sources.txt
nohup ./bettertest boss --sources sources.txt > boss.log 2>&1 &
```

The repo's pipelines are its `.bettertest/*.py` files. Each run tests one commit and reports its
result back to the forge as a commit status.

## A pipeline

`.bettertest/app.py` is the pipeline `app`:

```python
import subprocess

from bettertest import Stage, artifact, copy, get_artifact_this_run, run, task

WORKER = "http://worker-host:7358"
IMAGE = "debian:trixie"


class Build(Stage):
    # Each task gets its own container; the tasks of a stage run at the same time. The task's
    # Python runs on the worker, in a checkout of the commit: copy() into the container what the
    # commands there need.
    @task(WORKER, IMAGE, timeout=1800)
    def package():
        subprocess.run(["tar", "-czf", "../src.tgz", "."], check=True)
        copy("../src.tgz", "/src.tgz")
        run("mkdir /src && tar -xzf /src.tgz -C /src && cd /src && make dist")
        artifact("/src/dist/app.tar", name="app.tar")


class Test(Stage):
    # Stages run one at a time, top to bottom; a failed one skips the rest.
    @task(WORKER, IMAGE)
    def smoke():
        copy(get_artifact_this_run(Build.package, name="app.tar"), "/tmp/app.tar")
        run("tar -xf /tmp/app.tar -C /opt && /opt/app/bin/app --version")
```

A stage is a step, not a group: put each task in the first stage after everything it needs, and
it runs as soon as it can.

- `@task(vm="http://boss-host:7357/artifact/debian-13.qcow2")` runs a task in a VM instead.
- `@task(labels=[Label.arm])` picks any connected worker that is an ARM machine, with the labels
  declared once in the pipedef: `class Label(Labels): arm = "arm"`. Workers take more labels when
  started: `./bettertest worker --label gpu`.
- A task with neither runs straight on the worker.
- A worker runs one task at a time, since it may have hardware on it that two tests at once would
  break. Plain compute takes more: `./bettertest worker --tasks 8`.

A VM or container can outlive a stage. Declare it once, and tasks in several stages use it, each
finding it as the stage before left it:

```python
from bettertest import Sandbox, Stage, reboot, run, task

DEBIAN = Sandbox(worker=WORKER, vm="http://boss-host:7357/artifact/debian-13.qcow2")


class Install(Stage):
    @task(sandbox=DEBIAN)
    def debian():
        run("sudo apt-get install --yes /tmp/app.deb")


class Reboot(Stage):
    @task(sandbox=DEBIAN)
    def debian():
        reboot()
```

A stage can also share one container among its tasks, for tasks so small that starting a
container each would take longer: set `worker`, `image` and `parallel = True` on the stage.

`bettertest dispatch app.py --stage Build --task package` runs one task from your laptop, with no
boss.

## The web UI

- **The grid:** one row per run and one column per task, with every pipeline as a tab. The red X
  before a running run's id cancels all of it.
- **The task log:** live as it runs, with rerun buttons for the run, the stage or the task.
- **Workers:** which ones are up, how busy, and what they are running.
- **Settings:** how long to keep logs and artifacts, and whether the boss speaks HTTPS.
- **Users:** who may do what, and how each logs in.

## Users

Until you lock it down, everyone is the **logged out** user, which starts as admin: a fresh boss works
with no accounts at all. To lock it down, add an admin with a password on the users page, then lower
logged out (to `read`, or to `none` so that everything needs a login). Roles are `read`, `write` (start
runs, add workers, upload artifacts) and `admin` (settings and users).

People log in with a password, or without one:

- **Client certificates** (HTTPS only): New Certificate on a user's row gives a file to import into
  the browser, which is then that user on every visit.
- **Passkeys** (HTTPS, or at localhost): Add Passkey in the header, then Log In With Passkey.

Scripts use an API token (New Token on the user's row) as `Authorization: Bearer <token>`. So does
the forge: once logged out is below `write`, add a user for it with `write`, make it a token, and put
`Bearer <token>` in the webhook's Authorization Header field. Running tasks get a token of their own,
so a locked-down boss still runs its pipelines.

## HTTPS

Off by default. Give the boss a certificate your machines already trust, usually from your site's own
CA, and it speaks HTTPS, and only HTTPS, on the same port:

```sh
nohup ./bettertest boss --cert boss.crt --key boss.key pipelines/ > boss.log 2>&1 &
```

## Packages

With root, `bettertest_<version>_amd64.deb` and `bettertest-<version>-1.x86_64.rpm` (and their ARM64
twins) install the binary and systemd units for boss and worker.

## Building

```sh
./build.sh       # the static binary for this machine, in target/<arch>-unknown-linux-musl/release/
./deb.sh         # the .deb of that binary
./rpm.sh         # the .rpm of that binary
./release.sh     # everything a release ships, x86_64 and aarch64, in dist/<version>/
cargo test
```

Building needs Rust with the musl and wasm32 targets, trunk, musl-gcc, and the Perl modules
OpenSSL's build uses; see "Development" in [AGENTS.md](AGENTS.md). AGENTS.md is the whole design:
how runs, sandboxes, live updates and the API work, and why.

## License

MIT. See [LICENSE](LICENSE).
