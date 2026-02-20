import ast, json, sys

tree = ast.parse(open(sys.argv[1]).read())
stages = []
for node in tree.body:
    if isinstance(node, ast.ClassDef):
        bases = [b.id if isinstance(b, ast.Name) else '' for b in node.bases]
        if 'Stage' in bases:
            tasks = [n.name for n in node.body
                     if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
                     and n.name.startswith('task_')]
            stages.append({'name': node.name, 'tasks': tasks})
print(json.dumps(stages))
