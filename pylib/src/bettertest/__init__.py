import subprocess
from collections.abc import Callable
from typing import Literal


def task[S](fn: Callable[[S], None]) -> Callable[[S], None]:
    return fn


class Stage:
    max_parallel_tasks: int = 1
    image: str | None = None
    execution_mode: Literal["isolated", "shared"] = "isolated"
    vm: str | None = None

    def shell(self, command: str) -> None:
        if self.image is None and self.vm is None:
            subprocess.run(["/bin/sh", "-c", command], check=False)
