"""bettertest's own pipeline: build the .deb and .rpm, install each on a fresh vm of its distro, and
check that it works. An emulated arm vm checks that vms of another architecture work too. The
intern test puts the bare binary on a fresh Debian, which has no qemu, and runs a pipeline whose
task is a vm, as a user without sudo who may use kvm: that vm is nested inside the Debian one, so
the worker needs nested virtualization. Tasks publish artifacts from the worker itself, from a container and from a vm, later tasks check that
they all arrive, and that asking for one that is not there says why.

Each stage is a step, and holds every task that can take that step now: the vms boot while the
packages build, and Debian, Fedora and arm go through install, reboot and check side by side, each
in a Sandbox that lives across those stages.

Each task runs in a checkout of the commit its run tests, and the packages travel to the install
tasks as artifacts. The worker runs many tasks at once (worker --tasks 8). Tasks ask for the
x86 label; enroll a worker that has it. The VMs are the cloud images as the distros publish them.
The worker downloads those URLs itself.

Three VMs are resident at once from Build through Check, and the intern's a fourth during
Install. Each qemu is given 2048 MiB, so that is 8 GiB before qemu's own overhead and the package
build running beside them. A 4 GiB worker cannot run this pipeline; give it about 10 GiB. Build starts four tasks at once, so
--tasks 4 is enough to not queue there; a smaller capacity only queues, and the VMs still all
stay up. The arm VM is emulated on x86, by the qemu and UEFI firmware the binary carries. Emulation
keeps the guest clock at wall speed, so Debian's 90s wait for /boot/efi used to drop the boot
into emergency mode; the worker lengthens that wait and allows 30 minutes for the ready callback.
That grub.cfg is written as a new file. A hard link debugfs does not count leaves the name on a
deleted inode, and the guest's journal takes it, so the next boot stops at the GRUB prompt.
"""

import os
import subprocess
import tempfile

import bettertest
from bettertest import (
    LATEST,
    Labels,
    Sandbox,
    Stage,
    artifact,
    copy,
    get_artifact,
    get_artifact_this_run,
    reboot,
    run,
    task,
)


# An enrolled worker with the x86 label and docker. The packages build in a container. A vm is the cloud image's own URL; the worker downloads it.
class Label(Labels):
    x86 = "x86"


IMAGE = "debian:trixie"
# Built by the Prepare stage from .bettertest/build.Dockerfile.
BUILD_IMAGE = "bettertest-build"

DEBIAN_IMAGE = (
    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"
)
DEBIAN = Sandbox(labels=[Label.x86], vm=DEBIAN_IMAGE)
FEDORA = Sandbox(
    labels=[Label.x86],
    vm="https://download.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2",
)
# Emulated on an x86_64 worker, so slow, but the same vm, ssh and cloud-init plumbing.
ARM = Sandbox(
    labels=[Label.x86],
    vm="https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-arm64.qcow2",
    arch="aarch64",
)


def in_vm_python(code: str) -> None:
    run(f"python3 - <<'EOF'\n{code}\nEOF")


def made_here(text: str) -> None:
    """Publishes made.txt, saying where it was made, from wherever the task runs."""
    with tempfile.TemporaryDirectory() as scratch:
        path = os.path.join(scratch, "made.txt")
        with open(path, "w") as f:
            f.write(text)
        artifact(path, name="made.txt")


class Prepare(Stage):
    """What needs nothing from this run: the image the packages build in, and artifacts made on the
    worker and in a container."""

    labels = (Label.x86,)

    @task()
    def build_image():
        # On the worker itself: docker's layer cache makes this instant unless the Dockerfile
        # changed.
        subprocess.run(
            ["docker", "build", "-t", BUILD_IMAGE, "-f", ".bettertest/build.Dockerfile", "."],
            check=True,
        )

    @task()
    def made_on_worker():
        made_here("made on the worker\n")

    @task(image=IMAGE)
    def made_in_container():
        run("echo 'made in a container' > /tmp/made.txt")
        artifact("/tmp/made.txt", name="made.txt")


class Build(Stage):
    """The packages, while the vms boot."""

    labels = (Label.x86,)

    @task(image=BUILD_IMAGE)
    def packages():
        # A fresh container per run, so runs at the same time share nothing. It builds from
        # nothing each time, about two minutes.
        subprocess.run(["tar", "-czf", "../src.tgz", "."], check=True)
        copy("../src.tgz", "/src.tgz")
        run("mkdir /src && tar -xzf /src.tgz -C /src")
        run("mkdir /src/target && cp -r /tools/target/tools /src/target/")
        run("cd /src && ./build.sh && ./deb.sh && ./rpm.sh")
        # Fully static means no program interpreter (ld.so) and no dynamic section.
        binary = "/src/target/x86_64-unknown-linux-musl/release/bettertest"
        run(f"! readelf -lW {binary} | grep -E 'INTERP|DYNAMIC'")
        run(f"echo '{binary}: statically linked'")
        run("cp /src/target/debian/bettertest_*.deb /bettertest.deb")
        run("cp /src/target/rpm/bettertest-*.rpm /bettertest.rpm")
        artifact("/bettertest.deb", name="bettertest.deb")
        artifact("/bettertest.rpm", name="bettertest.rpm")
        artifact(binary, name="bettertest")

    @task(sandbox=DEBIAN)
    def debian():
        run("echo 'made in a vm' > /tmp/made.txt")
        artifact("/tmp/made.txt", name="made.txt")

    @task(sandbox=FEDORA)
    def fedora():
        run("cat /etc/fedora-release")

    @task(sandbox=ARM)
    def arm():
        run('uname -a && test "$(uname -m)" = aarch64')


class Install(Stage):
    """The packages on their distros, and every check of artifacts made so far."""

    labels = (Label.x86,)

    @task(sandbox=DEBIAN)
    def debian():
        copy(
            get_artifact_this_run(Build.packages, name="bettertest.deb"),
            "/tmp/bettertest.deb",
        )
        run(
            "sudo DEBIAN_FRONTEND=noninteractive"
            " apt-get install --yes --no-install-recommends /tmp/bettertest.deb"
        )

    @task(sandbox=FEDORA)
    def fedora():
        copy(
            get_artifact_this_run(Build.packages, name="bettertest.rpm"),
            "/tmp/bettertest.rpm",
        )
        # Everything it requires is already on the image, so no repository needs to be reached.
        run(
            "sudo dnf install --assumeyes --disablerepo='*' --setopt=install_weak_deps=False"
            " /tmp/bettertest.rpm"
        )

    @task(sandbox=ARM)
    def arm():
        # Both ways: a file from the worker in, and one from the vm out, as an artifact.
        with tempfile.TemporaryDirectory() as scratch:
            path = os.path.join(scratch, "there.txt")
            with open(path, "w") as f:
                f.write("from the worker\n")
            copy(path, "/tmp/there.txt")
        run(
            "test \"$(cat /tmp/there.txt)\" = 'from the worker' && echo 'from arm' > /tmp/back.txt"
        )
        artifact("/tmp/back.txt", name="back.txt")
        # Not /tmp: Debian empties it on boot. Check compares it with the boot after Reboot.
        run("cat /proc/sys/kernel/random/boot_id > ~/before")

    @task(vm=DEBIAN_IMAGE)
    def intern():
        # The intern test (AGENTS.md): a machine as its distro ships it, an account without sudo
        # that may use kvm, and nothing but the binary and a pipedef whose task runs in a vm.
        copy(get_artifact_this_run(Build.packages, name="bettertest"), "/tmp/bettertest")
        copy("pipedefs/vm.py", "/tmp/vm.py")
        run(INTERN_MACHINE)
        run(INTERN_STARTS)
        check_tests_run("vm")
        run("cat /tmp/intern.log")
        run(TOOLS_ARE_STATIC)

    @task()
    def artifacts_arrive():
        expected = {
            Prepare.made_on_worker: "made on the worker\n",
            Prepare.made_in_container: "made in a container\n",
            Build.debian: "made in a vm\n",
        }
        for producer, text in expected.items():
            got = get_artifact_this_run(producer, name="made.txt").read_text()
            print(f"{producer.__qualname__}: {got!r}")
            assert got == text, f"expected {text!r}"

    @task()
    def latest_artifact():
        # Prepare.made_on_worker passed in this run, so this run is the newest that has it.
        made = get_artifact(
            pipeline="bettertest",
            stage="Prepare",
            task="made_on_worker",
            name="made.txt",
            run=LATEST,
        )
        assert made.read_text() == "made on the worker\n"

    @task()
    def pinned_artifact():
        # A real pipedef pins an id copied from the run grid. Only a test reaches inside for its
        # own run's.
        for run_id in [this_run()[:6], this_run()]:
            made = get_artifact(
                pipeline="bettertest",
                stage="Build",
                task="debian",
                name="made.txt",
                run=run_id,
            )
            assert made.read_text() == "made in a vm\n", run_id

    @task()
    def artifact_mistakes():
        # Asking for an artifact that is not there must fail with a message that says why.
        cases = [
            (
                lambda: get_artifact(
                    pipeline="nope",
                    stage="Prepare",
                    task="made_on_worker",
                    name="made.txt",
                    run=LATEST,
                ),
                "Can't get artifact made.txt of nope Prepare.made_on_worker: there is no"
                " pipeline named 'nope'.",
            ),
            (
                lambda: get_artifact(
                    pipeline="bettertest",
                    stage="Prepare",
                    task="made_on_worker",
                    name="made.txt",
                    run="xyz",
                ),
                "Can't get artifact made.txt of bettertest Prepare.made_on_worker: 'xyz' is not"
                " a run id. Use LATEST, or a run's id like 6dfde7.",
            ),
            (
                lambda: get_artifact(
                    pipeline="bettertest",
                    stage="Prepare",
                    task="made_on_worker",
                    name="made.txt",
                    run="0" * 32,
                ),
                "Can't get artifact made.txt of bettertest Prepare.made_on_worker: bettertest"
                f" has no run {'0' * 32}.",
            ),
            (
                lambda: get_artifact(
                    pipeline="bettertest",
                    stage="Prepare",
                    task="nope",
                    name="made.txt",
                    run=LATEST,
                ),
                "Can't get artifact made.txt of bettertest Prepare.nope: no run of bettertest"
                " has Prepare.nope passing with an artifact named 'made.txt'.",
            ),
            (
                lambda: get_artifact(
                    pipeline="bettertest",
                    stage="Reboot",
                    task="made_later",
                    name="note.txt",
                    run=this_run(),
                ),
                "Can't get artifact note.txt of Reboot.made_later because Reboot.made_later"
                f" hasn't run yet in run {this_run()[:6]} of bettertest.",
            ),
            (
                lambda: get_artifact_this_run(Reboot.made_later, name="note.txt"),
                "Can't get artifact note.txt of Reboot.made_later because Reboot.made_later"
                " hasn't run yet. Move Reboot.made_later to a stage that runs before this one"
                " for the artifact to be available.",
            ),
        ]
        for fetch, expected in cases:
            try:
                fetch()
            except LookupError as e:
                print(e)
                assert str(e) == expected, f"expected: {expected}"
            else:
                raise AssertionError(
                    f"got an artifact that is not there, expected: {expected}"
                )
        try:
            get_artifact_this_run(Prepare.made_on_worker, name="mdae.txt")
        except LookupError as e:
            print(e)
            assert "published no artifact named 'mdae.txt'" in str(e)
            assert "It published: made.txt." in str(e)
        else:
            raise AssertionError("got an artifact that was never published")


def this_run() -> str:
    """This run's id, without dashes. A test reaches inside bettertest for it."""
    files = bettertest._files
    assert files is not None
    return files["run"].replace("-", "")


class Reboot(Stage):
    """Every vm reboots, cleanly; the packages must come back up by themselves."""

    labels = (Label.x86,)

    @task(sandbox=DEBIAN)
    def debian():
        reboot()

    @task(sandbox=FEDORA)
    def fedora():
        reboot()

    @task(sandbox=ARM)
    def arm():
        reboot()

    @task()
    def made_later():
        # Install.artifact_mistakes asked for this before it existed.
        with tempfile.TemporaryDirectory() as scratch:
            path = os.path.join(scratch, "note.txt")
            with open(path, "w") as f:
                f.write("too late\n")
            artifact(path, name="note.txt")


INTERN_MACHINE = """set -ex
# As Debian's cloud image comes, less the qemu-img it carries: no qemu, no docker.
sudo dpkg --purge --force-depends qemu-utils
! command -v qemu-system-x86_64 qemu-system-aarch64 qemu-img docker
# What the machine's admin gives the intern: an account without sudo, which may use kvm.
test -c /dev/kvm
# This machine is itself a vm of 2 GiB, as much as the vm inside it asks for. The kernel would
# refuse to promise that much at once; the guest only ever touches part of it.
sudo sysctl vm.overcommit_memory=1
sudo useradd --create-home --groups kvm intern
sudo install -o intern -m 755 -t /home/intern /tmp/bettertest /tmp/vm.py
# Fully static: nothing from the machine, not even libc.
sudo -u intern ldd /home/intern/bettertest 2>&1 | grep 'not a dynamic executable'
"""

# Its PATH holds python3, which pipedefs run in, and sh: bettertest fails to find any other
# program it tries to run from the machine.
INTERN_STARTS = """set -ex
sudo -u intern sh -c 'mkdir /home/intern/bin && ln -s /usr/bin/python3 /bin/sh /home/intern/bin/'
cd /tmp
sudo -u intern env -i HOME=/home/intern PATH=/home/intern/bin \\
    /home/intern/bettertest unified /home/intern/vm.py > /tmp/intern.log 2>&1 < /dev/null &
"""

TOOLS_ARE_STATIC = """set -e
for tool in qemu-system-x86_64 qemu-system-aarch64 qemu-img debugfs e2fsck ssh scp ssh-keygen; do
    path=$(echo /var/tmp/bettertest/tools/*/$tool)
    ldd "$path" 2>&1 | grep -q 'not a dynamic executable' || { echo "$path is not static"; exit 1; }
    echo "$path: static"
done
"""


def check_services_running() -> None:
    run("systemctl is-active bettertest-boss bettertest-worker")


def check_web_ui_visible() -> None:
    in_vm_python("""
import re, urllib.request
page = urllib.request.urlopen("http://localhost:7357/").read().decode()
assert "<title>bettertest</title>" in page, page
wasm = re.search(r'href="(/[^"]+_bg\\.wasm)"', page).group(1)
size = len(urllib.request.urlopen("http://localhost:7357" + wasm).read())
print(f"index.html and {wasm} ({size} bytes) served")
""")


def check_tests_run(pipeline: str = "helloworld") -> None:
    # The package starts a worker on this machine. helloworld only names the x86 label, so the
    # boss it just installed has no worker until this enrolls that one, the same as the workers page.
    in_vm_python(f"pipeline = {pipeline!r}\n" + """
import json, time, urllib.request
boss = "http://localhost:7357"
# A boss started a moment ago may not be listening yet.
for _ in range(300):
    try:
        urllib.request.urlopen(f"{boss}/api/pipelines")
        break
    except OSError:
        time.sleep(0.1)
enroll = urllib.request.Request(
    f"{boss}/api/add-worker",
    data=json.dumps({"url": "http://localhost:7358"}).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
urllib.request.urlopen(enroll)
start = urllib.request.Request(f"{boss}/api/start-run/{pipeline}", method="POST")
run = json.load(urllib.request.urlopen(start))
for line in urllib.request.urlopen(f"{boss}/api/run/{run['id']}/events"):
    if b"RunFinished" in line:
        break
run = json.load(urllib.request.urlopen(f"{boss}/api/run/{run['id']}"))
for stage in run["stages"]:
    for task in stage["tasks"]:
        print(f"--- {stage['name']}.{task['name']}: {task['state']}")
        print(urllib.request.urlopen(f"{boss}/api/task/{task['id']}/log").read().decode(), end="")
assert run["state"] == "pass", run["state"]
""")


class Check(Stage):
    """After the reboot: the services came back, the web ui is up, and a pipeline runs. The checks
    of one distro run one after another, in its vm; the distros side by side."""

    @task(sandbox=DEBIAN)
    def debian_services():
        check_services_running()

    @task(sandbox=DEBIAN)
    def debian_web_ui():
        check_web_ui_visible()

    @task(sandbox=DEBIAN)
    def debian_tests_run():
        check_tests_run()

    @task(sandbox=FEDORA)
    def fedora_services():
        check_services_running()

    @task(sandbox=FEDORA)
    def fedora_web_ui():
        check_web_ui_visible()

    @task(sandbox=FEDORA)
    def fedora_tests_run():
        check_tests_run()

    @task(sandbox=ARM)
    def arm_rebooted():
        run(
            'test -s ~/before && test "$(cat ~/before)" != "$(cat /proc/sys/kernel/random/boot_id)"'
        )
