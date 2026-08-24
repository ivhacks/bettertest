# Language boundary: rust (the bettertest binary) spawns this script in a python
# process. It glues that rust side to the python side. This is how the binary
# asks python to:
# - parse a pipedef and dump its structure as JSON
# - start a task by actually calling the task function
#
# Nothing here is importable from disk. rust baked the library and this script
# into the binary, then stuffed library source + pipedef source into stdin JSON.
# pipedefs do `from bettertest import ...` — that name only works because we
# exec the library into sys.modules["bettertest"] *before* exec'ing the pipedef.
import json
import sys
import types

payload = json.loads(sys.stdin.read())
library_source = payload["bettertest"]
pipedef_source = payload["pipedef"]

library = types.ModuleType("bettertest")
exec(library_source, library.__dict__)
sys.modules["bettertest"] = library

pipedef = types.ModuleType("pipedef")
exec(pipedef_source, pipedef.__dict__)

command = sys.argv[1]
if command == "parse":
    print(library.dump_json())
elif command == "start":
    # python3 -c <glue> start <stage class> <task method>
    stage_name = sys.argv[2]
    task_name = sys.argv[3]
    stage = getattr(pipedef, stage_name)
    task = getattr(stage, task_name)
    task()
else:
    raise SystemExit(f"unknown command: {command}")
