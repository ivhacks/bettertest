import json
import subprocess
from collections.abc import Callable

_current_image: str | None = None


def task(worker: str, image: str | None = None):
    def decorator(fn: Callable):
        def wrapped():
            global _current_image
            _current_image = image
            return fn()

        wrapped.bettertest_task = True  # ty: ignore[unresolved-attribute]
        wrapped.bettertest_worker = worker  # ty: ignore[unresolved-attribute]
        wrapped.bettertest_image = image  # ty: ignore[unresolved-attribute]
        return staticmethod(wrapped)

    return decorator


class Stage:
    @classmethod
    def dump_json(cls) -> str:
        tasks = []
        for name, obj in cls.__dict__.items():
            fn = obj.__func__ if isinstance(obj, staticmethod) else obj
            if not getattr(fn, "bettertest_task", False):
                continue
            tasks.append(
                {
                    "name": name,
                    "worker": fn.bettertest_worker,
                    "image": fn.bettertest_image,
                }
            )
        return json.dumps({"name": cls.__name__, "tasks": tasks})


def dump_json() -> str:
    return json.dumps(
        {
            "stages": [json.loads(cls.dump_json()) for cls in Stage.__subclasses__()],
        }
    )


def run(command: str) -> None:
    if _current_image is None:
        raise RuntimeError("run() only inside a @task with an image")
    result = subprocess.run(
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
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(result.returncode)
