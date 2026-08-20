import subprocess
from collections.abc import Callable

_current_image: str | None = None


def task(worker: str, image: str | None = None):
    def decorator(fn: Callable):
        def run_set_globals():
            global _current_image
            _current_image = image
            return fn()

        return staticmethod(run_set_globals)

    return decorator


class Stage:
    pass


def run(command: str) -> None:
    if _current_image is None:
        raise RuntimeError("run() only inside a @task with an image")
    subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--entrypoint",
            "/bin/sh",
            _current_image,
            "-c",
            command,
        ],
        check=True,
    )
