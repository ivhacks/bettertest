# Language boundary: rust (the bettertest binary) spawns this script in a python
# process. It glues that rust side to the python side. This is how the binary
# asks python to:
# - parse a pipedef and dump its structure as JSON
# - start a task by actually calling the task function
#
# Nothing here is importable from disk. rust baked the library and this script
# into the binary, then stuffed library source + pipedef source into stdin, as JSON.
# pipedefs do `from bettertest import ...` — that name only works because we
# exec the library into sys.modules["bettertest"] *before* exec'ing the pipedef.
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
else:
    raise SystemExit(f"unknown command: {command}")
