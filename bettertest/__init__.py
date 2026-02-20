import sys

import requests


class _StageMeta(type):
    def __new__(cls, name, bases, namespace):
        for key, value in namespace.items():
            if callable(value) and not key.startswith("_"):
                namespace[key] = staticmethod(value)
        return super().__new__(cls, name, bases, namespace)


class Stage(metaclass=_StageMeta):
    pass


def _check_worker(worker: str):
    try:
        requests.get(f"{worker}/health", timeout=3)
    except (requests.ConnectionError, requests.Timeout):
        print(f"error: worker at {worker} is not reachable — is it running?", file=sys.stderr)
        sys.exit(1)


def run(worker: str, image: str, command: str) -> int:
    _check_worker(worker)
    print(f"running: {command}")
    resp = requests.post(
        f"{worker}/run", json={"image": image, "command": command}, stream=True
    )
    resp.raise_for_status()
    event = None
    for line in resp.iter_lines(decode_unicode=True):
        if line.startswith("event: "):
            event = line[7:]
        elif line.startswith("data: "):
            data = line[6:]
            if event == "error":
                raise Exception(f"worker error: {data}")
            if event == "log":
                print(data)
            if event == "done":
                exit_code = int(data)
                print(f"\nexit code: {exit_code}")
                return exit_code
    raise Exception("stream ended without done event")
