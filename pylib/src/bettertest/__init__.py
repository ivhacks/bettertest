import enum
import http.client
import json
import os
import re
import shutil
import socket
import ssl
import subprocess
import urllib.parse
import urllib.request
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any, Literal

Arch = Literal["x86_64", "aarch64"]


class Labels(enum.Enum):
    """The worker labels a pipeline picks its workers by. Declare them once per pipedef, as a
    subclass with one member per label:

        class Label(Labels):
            x86 = "x86"
            arm = "arm"

    Then `@task(labels=[Label.arm])` runs the task on whichever connected worker has every label
    it lists, and the IDE flags a label the pipedef did not declare (`Label.amr`). Each worker has
    the label of its architecture: x86 or arm.
    """


class Sandbox:
    """A container or VM that tasks in several stages share, declared once at the top of the
    pipedef:

        DEBIAN = Sandbox(worker=WORKER, vm="http://boss:7357/artifact/debian-13.qcow2")

    Then `@task(sandbox=DEBIAN)` runs a task in it. The boss starts it when the first stage that
    uses it comes up, and stops it once the last stage that uses it is over (or the run fails or is
    cancelled), so what one stage does to it, the next one finds: install a package in one stage,
    reboot in the next, check it in the one after. Tasks of one stage that share a sandbox run one
    at a time, in the order they are defined; different sandboxes run at the same time. It takes a
    `worker` or `labels`, and an `image` or a `vm` (with `arch`, as in task()).
    """

    def __init__(
        self,
        worker: str | None = None,
        image: str | None = None,
        vm: str | None = None,
        arch: Arch | None = None,
        labels: Sequence[Labels] = (),
    ):
        if (image is None) == (vm is None):
            raise ValueError("a Sandbox is an image or a vm: give it one of them")
        if (worker is None) == (not labels):
            raise ValueError(
                "a Sandbox names its worker or picks one by labels: give it one"
            )
        if arch is not None and vm is None:
            raise ValueError(
                "arch is the architecture of a vm: give the Sandbox a vm too"
            )
        self.worker = worker
        self.image = image
        self.vm = vm
        self.arch = arch
        self.labels = tuple(labels)


# Where run() and copy() send things. The glue script sets this before calling a task:
#   {"kind": "container", "id": "<docker container id>"}
#   {"kind": "vm", "port": <ssh port on 127.0.0.1>, "key": "<private key path>", "reboot": "<url>"}
#   None when the task has no sandbox
_target: dict | None = None

# This task's files, set by the glue script. None while parsing.
#   outputs: artifact() copies here, and the worker hands everything here to the boss at the end.
#   inputs: where get_artifact() and get_artifact_this_run() save what they fetch.
#   boss, pipeline, run: whose artifacts to fetch from where. None outside any run (dispatch).
_files: dict | None = None


class _Latest:
    def __repr__(self) -> str:
        return "LATEST"


LATEST = _Latest()
"""get_artifact(run=LATEST): the newest run in which the task passed and published the artifact."""

# Artifact names end up in file paths and URLs. The boss checks the same rule.
_ARTIFACT_NAME = re.compile(r"[A-Za-z0-9_\-][A-Za-z0-9_.\-]*")

_SSH_OPTIONS = [
    # Ignore ~/.ssh/config, agents and known_hosts: the VM is brand new and only our key works.
    "-F", "/dev/null",
    "-o", "IdentitiesOnly=yes",
    "-o", "IdentityAgent=none",
    "-o", "StrictHostKeyChecking=no",
    "-o", "UserKnownHostsFile=/dev/null",
    "-o", "LogLevel=ERROR",
]  # fmt: skip


def task(
    worker: str | None = None,
    image: str | None = None,
    vm: str | None = None,
    arch: Arch | None = None,
    timeout: float | None = None,
    labels: Sequence[Labels] = (),
    sandbox: Sandbox | None = None,
):
    """Mark a Stage method as a task.

    The task runs on `worker` (its url), or on whichever connected worker has every one of `labels`
    (see Labels). `image` gives the task its own docker container, `vm` its own virtual machine (a
    URL to a qcow2 image). run() executes commands inside it. With neither, the task body still
    runs on the worker, but run() is unavailable.

    `arch` is the vm's architecture: the worker's own when left out, which it runs at full speed
    (kvm), or another, which it emulates, slowly.

    In a stage that declares a sandbox (see Stage), leave out worker, labels, image, vm and arch:
    the task runs on the stage's worker, in the stage's sandbox. `sandbox` runs it in a Sandbox
    that tasks of other stages share too (see Sandbox), instead of all of those.

    `timeout` is how many seconds the task body may take. Once they are up, the task is killed,
    along with everything it started, and fails. Starting its sandbox does not count.
    """
    if image is not None and vm is not None:
        raise ValueError("a task gets an image or a vm, not both")
    if sandbox is not None and (worker, image, vm, arch, tuple(labels)) != (
        None,
        None,
        None,
        None,
        (),
    ):
        raise ValueError(
            "a task in a Sandbox runs where the Sandbox is: leave out worker, labels, image, vm"
            " and arch"
        )
    if sandbox is not None and not isinstance(sandbox, Sandbox):
        raise ValueError(f"sandbox is a Sandbox, not {sandbox!r}")
    if worker is not None and labels:
        raise ValueError("a task names its worker or picks one by labels, not both")
    if arch is not None and vm is None:
        raise ValueError("arch is the architecture of a vm: give the task a vm too")
    if timeout is not None and (
        isinstance(timeout, bool)
        or not isinstance(timeout, int | float)
        or not timeout > 0
    ):
        raise ValueError(f"timeout is a number of seconds above 0, not {timeout!r}")

    def decorator(fn: Callable[[], None]):
        fn.bettertest_task = True  # ty: ignore[unresolved-attribute]
        fn.bettertest_worker = worker  # ty: ignore[unresolved-attribute]
        fn.bettertest_image = image  # ty: ignore[unresolved-attribute]
        fn.bettertest_vm = vm  # ty: ignore[unresolved-attribute]
        fn.bettertest_arch = arch  # ty: ignore[unresolved-attribute]
        fn.bettertest_timeout = timeout  # ty: ignore[unresolved-attribute]
        fn.bettertest_labels = tuple(labels)  # ty: ignore[unresolved-attribute]
        fn.bettertest_sandbox = sandbox  # ty: ignore[unresolved-attribute]
        return staticmethod(fn)

    return decorator


class Stage:
    """A step of the pipeline: every task that can run at that point.

    Stages run one at a time, top to bottom, in the order they are defined. Once one fails, the
    ones below it are skipped. A stage is not a group of related tasks: put each task in the
    earliest stage after everything it needs, so that it runs as soon as it can.

    By default a stage's tasks run at the same time, each in its own sandbox. Set `worker` (or
    `labels`) and an `image` or `vm` on the stage instead, and its tasks share that one sandbox
    and run one at a time, in order, so each task can build on what the one before it did. Set
    `parallel = True` as well, and they share the sandbox but run at the same time: for small
    tasks, where starting a sandbox each would take longer than the tasks. A `worker` or `labels`
    without a sandbox is where the stage's tasks run unless they say otherwise.
    """

    worker: str | None = None
    labels: Sequence[Labels] = ()
    image: str | None = None
    vm: str | None = None
    arch: Arch | None = None
    parallel: bool = False


def _tasks(stage: type[Stage]) -> list[tuple[str, Any]]:
    """The tasks of `stage`, as (name, function). Any because @task gives the functions
    attributes of its own."""
    return [
        (name, obj.__func__)
        for name, obj in stage.__dict__.items()
        if isinstance(obj, staticmethod) and hasattr(obj.__func__, "bettertest_task")
    ]


def _label_values(owner: str, labels: Sequence[Labels]) -> list[str]:
    """The labels as workers know them. Only members of the pipedef's Labels class will do, so a
    label is always one the pipedef declared."""
    values = []
    for label in labels:
        if not isinstance(label, Labels):
            raise ValueError(
                f"{owner}: labels are members of the pipedef's Labels class, like Label.arm,"
                f" not {label!r}"
            )
        if not isinstance(label.value, str) or not _ARTIFACT_NAME.fullmatch(
            label.value
        ):
            raise ValueError(
                f"{label!r}: a label is a string of letters, digits, '.', '-' and '_'"
            )
        values.append(label.value)
    return values


def _sandbox_names(pipedef: dict[str, Any]) -> dict[int, str]:
    """The name of each Sandbox the pipedef declares, by id: the name it is assigned to."""
    names: dict[int, str] = {}
    for name, value in pipedef.items():
        if isinstance(value, Sandbox):
            if id(value) in names:
                raise ValueError(
                    f"Sandbox {names[id(value)]} is also called {name}: give it one name"
                )
            names[id(value)] = name
    return names


def dump_json(pipedef: dict[str, Any]) -> str:
    """The pipeline the pipedef (its module's namespace) defines, as JSON for the boss."""
    sandbox_names = _sandbox_names(pipedef)
    used: dict[str, dict] = {}
    declared = Labels.__subclasses__()
    if len(declared) > 1:
        names = ", ".join(c.__name__ for c in declared)
        raise ValueError(
            f"a pipedef declares its labels in one Labels class, not in {names}"
        )
    stages = []
    for stage in Stage.__subclasses__():
        name = stage.__name__
        # A task may be called after; an `after = ...` attribute is an ordering scheme, refused.
        if "after" in stage.__dict__ and not isinstance(
            stage.__dict__["after"], staticmethod
        ):
            raise ValueError(
                f"stage {name} sets `after`: stages run in the order they are defined instead"
            )
        shared = stage.image is not None or stage.vm is not None
        stage_labels = _label_values(f"stage {name}", stage.labels)
        if stage.image is not None and stage.vm is not None:
            raise ValueError(f"stage {name} gets an image or a vm, not both")
        if stage.worker is not None and stage_labels:
            raise ValueError(
                f"stage {name} names its worker or picks one by labels, not both"
            )
        if shared and stage.worker is None and not stage_labels:
            raise ValueError(
                f"stage {name} has a sandbox, so it needs a worker or labels"
            )
        if stage.parallel and not shared:
            raise ValueError(
                f"stage {name} sets parallel, which is for tasks that share the stage's sandbox:"
                " give the stage an image or a vm (the tasks of other stages run at the same"
                " time anyway)"
            )
        if stage.arch is not None and stage.vm is None:
            raise ValueError(
                f"stage {name}: arch is the architecture of a vm: give it a vm too"
            )
        sandbox = (
            {
                "worker": stage.worker,
                "labels": stage_labels,
                "image": stage.image,
                "vm": stage.vm,
                "arch": stage.arch,
                "parallel": stage.parallel,
            }
            if shared
            else None
        )

        tasks = []
        for task_name, fn in _tasks(stage):
            own = (
                fn.bettertest_worker,
                fn.bettertest_image,
                fn.bettertest_vm,
                fn.bettertest_arch,
                fn.bettertest_labels,
            )
            spanning = fn.bettertest_sandbox
            if shared and (own != (None, None, None, None, ()) or spanning is not None):
                raise ValueError(
                    f"{name}.{task_name} runs in its stage's sandbox: use @task()"
                )
            sandbox_name = None
            if spanning is not None:
                sandbox_name = sandbox_names.get(id(spanning))
                if sandbox_name is None:
                    raise ValueError(
                        f"{name}.{task_name}: its Sandbox is not one the pipedef declares at its"
                        " top level, like DEBIAN = Sandbox(...)"
                    )
                used[sandbox_name] = {
                    "worker": spanning.worker,
                    "labels": _label_values(f"Sandbox {sandbox_name}", spanning.labels),
                    "image": spanning.image,
                    "vm": spanning.vm,
                    "arch": spanning.arch,
                    "parallel": False,
                }
            worker = fn.bettertest_worker
            labels = _label_values(f"{name}.{task_name}", fn.bettertest_labels)
            if worker is None and not labels and spanning is None:
                worker, labels = stage.worker, stage_labels
            if worker is None and not labels and spanning is None:
                raise ValueError(f"{name}.{task_name} needs a worker or labels")
            tasks.append(
                {
                    "name": task_name,
                    "worker": worker,
                    "labels": labels,
                    "image": fn.bettertest_image,
                    "vm": fn.bettertest_vm,
                    "arch": fn.bettertest_arch,
                    "timeout": fn.bettertest_timeout,
                    "sandbox": sandbox_name,
                }
            )
        stages.append({"name": name, "sandbox": sandbox, "tasks": tasks})
    unused = sorted(set(sandbox_names.values()) - set(used))
    if unused:
        raise ValueError(f"no task runs in Sandbox {', '.join(unused)}")
    return json.dumps({"stages": stages, "sandboxes": used})


def _sandbox() -> dict:
    if _target is None:
        raise RuntimeError(
            "this task has no sandbox: give it (or its stage) an image or a vm"
        )
    return _target


def _call(argv: list[str | Path]) -> None:
    result = subprocess.run(argv, check=False)
    if result.returncode != 0:
        raise SystemExit(result.returncode)


def run(command: str) -> None:
    """Run a shell command inside this task's container or vm. A non-zero exit fails the task."""
    sandbox = _sandbox()
    print(f"$ {command}", flush=True)
    if sandbox["kind"] == "container":
        _call(["docker", "exec", sandbox["id"], "/bin/sh", "-c", command])
    else:
        port = str(sandbox["port"])
        _call(
            [
                "ssh",
                *_SSH_OPTIONS,
                "-i",
                sandbox["key"],
                "-p",
                port,
                "bt@127.0.0.1",
                command,
            ]
        )


def copy(local: str | Path, remote: str) -> None:
    """Copy a file from the worker into this task's container or vm."""
    sandbox = _sandbox()
    print(f"$ copy {local} {remote}", flush=True)
    if sandbox["kind"] == "container":
        _call(["docker", "cp", local, f"{sandbox['id']}:{remote}"])
    else:
        _scp(sandbox, local, f"bt@127.0.0.1:{remote}")


def _scp(sandbox: dict, source: str | Path, destination: str | Path) -> None:
    port = str(sandbox["port"])
    _call(["scp", *_SSH_OPTIONS, "-i", sandbox["key"], "-P", port, source, destination])


def reboot() -> None:
    """Reboot this task's vm. Returns once it is back up and ssh works again."""
    sandbox = _sandbox()
    if sandbox["kind"] != "vm":
        raise RuntimeError("reboot() needs the task to run in a vm")
    print("$ reboot", flush=True)
    urllib.request.urlopen(urllib.request.Request(sandbox["reboot"], method="POST"))


def artifact(path: str | Path, *, name: str) -> None:
    """Publish a file this task made, like a build output or a screenshot, as `name`. Like run(),
    `path` is inside this task's container or vm, or on the worker when the task has neither. Once
    the task ends the boss keeps the file, shows it on the task page, and gives it to later tasks
    that ask for it with get_artifact(). Names are unique within a task.
    """
    if _files is None:
        raise RuntimeError("artifact() only works inside a task")
    if not _ARTIFACT_NAME.fullmatch(name):
        raise ValueError(
            f"artifact {name!r}: names are letters, digits, '.', '-' and '_', not starting with '.'"
        )
    published = os.path.join(_files["outputs"], name)
    if os.path.exists(published):
        raise ValueError(f"this task already published an artifact named {name}")
    print(f"$ artifact {path}", flush=True)
    if _target is None:
        shutil.copyfile(path, published)
    elif _target["kind"] == "container":
        _call(["docker", "cp", f"{_target['id']}:{path}", published])
    else:
        _scp(_target, f"bt@127.0.0.1:{path}", published)
    # docker cp and scp -r would bring a whole directory along. An artifact is one file.
    if not os.path.isfile(published):
        shutil.rmtree(published, ignore_errors=True)
        raise ValueError(f"{path} is not a file")


def get_artifact_this_run(task: Callable[[], None], *, name: str) -> Path:
    """Where artifact `name`, which `task` published earlier in this run, is on this worker. A path
    to the file, not its contents: open it, or copy() it into the sandbox. `task` is a task of
    this pipedef, like Build.packages, so the IDE checks it.
    """
    # "Render.frame" for a task defined in class Render.
    qualname: str = getattr(task, "__qualname__", "")
    stage, _, task_name = qualname.rpartition(".")
    files = _task_files(f"Can't get artifact {name} of {qualname}")
    url = f"{files['boss']}/api/run/{files['run']}/artifact"
    return _fetch(url, ["this-run", stage, task_name, name], None)


def get_artifact(
    *, pipeline: str, stage: str, task: str, name: str, run: str | _Latest
) -> Path:
    """Where artifact `name` of task `stage.task` in a run of `pipeline` is on this worker: the
    newest run in which the task passed and published it (run=LATEST), or a run picked by its
    id, like "6dfde7". Any pipeline, this one included. Earlier runs may have had other tasks, so
    everything is named by strings; for this run, get_artifact_this_run() can check the task.
    """
    which = "latest" if isinstance(run, _Latest) else run
    files = _task_files(f"Can't get artifact {name} of {pipeline} {stage}.{task}")
    url = f"{files['boss']}/api/pipeline/{urllib.parse.quote(pipeline)}/run/{which}/artifact"
    return _fetch(
        url,
        [pipeline, which, stage, task, name],
        f"{name} of {pipeline} {stage}.{task}",
    )


def _task_files(cant: str) -> dict:
    if _files is None:
        raise RuntimeError("artifacts can only be fetched inside a task")
    if _files["boss"] is None:
        raise LookupError(
            f"{cant}: this task runs alone, outside any run (dispatch), so there is no boss to"
            " ask."
        )
    return _files


def _get(url: str) -> http.client.HTTPResponse:
    """GETs `url` from the boss. Over HTTPS its certificate is for a name DNS may not know, so this
    connects to the address the boss called the worker from and checks the certificate against the
    name in the url."""
    address = _files.get("boss_address") if _files else None
    parsed = urllib.parse.urlsplit(url)
    path = parsed.path + (f"?{parsed.query}" if parsed.query else "")
    if parsed.scheme == "https" and address:
        host, _, port = address.rpartition(":")
        connection = _BossConnection(
            parsed.hostname or "", (host.strip("[]"), int(port))
        )
    else:
        connection = http.client.HTTPConnection(parsed.netloc)
    # A locked-down boss lets a running task read what its run needs, by this token.
    token = _files.get("boss_token") if _files else None
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    connection.request("GET", path, headers=headers)
    return connection.getresponse()


class _BossConnection(http.client.HTTPSConnection):
    """HTTPS to `address`, checking the certificate against `name` with the machine's trust store."""

    def __init__(self, name: str, address: tuple[str, int]):
        super().__init__(name, address[1], context=ssl.create_default_context())
        self._address = address

    def connect(self) -> None:
        self.sock = self._context.wrap_socket(  # ty: ignore[unresolved-attribute]
            socket.create_connection(self._address, self.timeout),
            server_hostname=self.host,
        )


def _fetch(url: str, parts: list[str], announce: str | None) -> Path:
    """Downloads `url` + /stage/task/name into this task's inputs, once. `parts` name the copy;
    `announce` says what it is, to log which run it came from."""
    for part in parts:
        if not _ARTIFACT_NAME.fullmatch(part):
            raise ValueError(
                f"{part!r} is not a pipeline, run, stage, task or artifact name"
            )
    path = Path(_files["inputs"] if _files else "", ".".join(parts))
    if path.is_file():
        return path
    stage, task, name = parts[-3:]
    response = _get(f"{url}/{stage}/{task}/{name}")
    if response.status != 200:
        # The boss says why it has no such file. Its message is the whole story.
        raise LookupError(response.read().decode())
    # The file appears under its name only once complete, so a half download is never used.
    partial = path.with_name(f".{path.name}.partial")
    with response, open(partial, "wb") as f:
        shutil.copyfileobj(response, f)
    partial.rename(path)
    if announce:
        run = response.headers.get("bettertest-run", "").replace("-", "")[:6]
        print(f"got {announce} from run {run}", flush=True)
    return path
