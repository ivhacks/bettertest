import sys, importlib.util, pathlib, os

# args: bettertest_lib_dir, pipedef_path, stage_class_name, task_method_name
sys.path.insert(0, sys.argv[1])
pipedef_path = sys.argv[2]
sys.path.insert(0, str(pathlib.Path(pipedef_path).parent))

if not os.path.isfile(pipedef_path):
    print(f"ERROR: pipedef not found: {pipedef_path}", file=sys.stderr)
    sys.exit(1)

spec = importlib.util.spec_from_file_location("pipedef", pipedef_path)
if spec is None:
    print(f"ERROR: could not load pipedef: {pipedef_path}", file=sys.stderr)
    sys.exit(1)

mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
getattr(getattr(mod, sys.argv[3]), sys.argv[4])()
