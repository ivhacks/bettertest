# Python's half of the language boundary. The binary spawns this with the library, then the
# pipedef, on stdin (nothing is importable from disk), to parse the pipedef or call one of its
# tasks. A pipedef's `from bettertest import ...` works because the library is planted first.
import json
import os
import sys
import types

# The library comes first, on a line of its own: a worker starts python before it has a task for
# it, and importing and compiling the library is most of what starting a task takes.
library = types.ModuleType("bettertest")
exec(json.loads(sys.stdin.readline()), library.__dict__)
sys.modules["bettertest"] = library

payload = json.loads(sys.stdin.read())
if payload["cwd"]:
    os.chdir(payload["cwd"])
library._target = payload["target"]
library._files = payload["files"]

pipedef = types.ModuleType("pipedef")
# Compiling under a file name makes tracebacks point at the pipedef rather than at this script.
exec(compile(payload["pipedef"], "pipedef.py", "exec"), pipedef.__dict__)

command = sys.argv[1]
if command == "parse":
    print(library.dump_json(pipedef.__dict__))
elif command == "start":
    stage, task = payload["task"]
    getattr(getattr(pipedef, stage), task)()
elif command == "view":
    view_name = payload["view"]
    results = payload["results"]
    view_fn = getattr(pipedef, view_name)
    figure = view_fn(results)
    if hasattr(figure, "to_json"):
        print(figure.to_json())
    elif hasattr(figure, "to_plotly_json"):
        print(json.dumps(figure.to_plotly_json()))
    else:
        raise SystemExit(f"view {view_name} did not return a Plotly figure")
else:
    raise SystemExit(f"unknown command: {command}")
