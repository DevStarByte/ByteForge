#!/usr/bin/env python3
"""Builds modules/index.json (the ByteForge module registry) from the module folders.

Every folder in modules/ with a module.json is a package; all its files (except caches and tests) are listed so the
IDE can download them. A .NET module ("runtime": "dotnet") ships only module.json and its build in bin/ (built by
the workflow first); its sources and tests stay in the repository. Run from the root of the packages branch:
python3 tools/mkindex.py
"""

import json
import os
import re
import sys

root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "modules")


def load_json5(path):
    # module.json allows // comments.
    with open(path, encoding="utf-8") as f:
        text = f.read()
    text = re.sub(r'("(?:\\.|[^"\\])*")|//[^\n]*', lambda m: m.group(1) or "", text)
    return json.loads(text)


REQUIRES = {"python3": "python3", "python": "python3", "node": "node", "dotnet": "dotnet"}

modules = []
for name in sorted(os.listdir(root)):
    folder = os.path.join(root, name)
    manifest_path = os.path.join(folder, "module.json")
    if not os.path.isfile(manifest_path):
        continue
    m = load_json5(manifest_path)
    # "command" may be empty: such a module only declares things (languages, file icons).
    for key in ("id", "name", "version"):
        if not m.get(key):
            sys.exit(f"{manifest_path}: missing {key}")
    dotnet = str(m.get("runtime", "")).lower() == "dotnet"
    if dotnet:
        assembly = m.get("assembly") or ""
        if not os.path.isfile(os.path.join(folder, assembly)):
            sys.exit(f"{manifest_path}: {assembly or 'the assembly'} isn't built (dotnet build {name})")
    files = []
    for directory, dirs, names in os.walk(folder):
        if dotnet:
            # module.json and bin/ only
            rel = os.path.relpath(directory, folder).replace(os.sep, "/")
            dirs[:] = [d for d in dirs if rel != "." or d == "bin"]
            names = [n for n in names if rel != "." or n == "module.json"]
        dirs[:] = [d for d in dirs if d not in ("__pycache__", "node_modules") and not d.startswith(".")]
        for n in names:
            # tests (test_*.py) stay in the repository; users don't download them
            if n.endswith((".pyc", ".pyo")) or n.startswith(".") or (n.startswith("test_") and n.endswith(".py")):
                continue
            files.append(os.path.relpath(os.path.join(directory, n), folder).replace(os.sep, "/"))
    requires = ["dotnet"] if dotnet else [REQUIRES[m["command"]]] if m.get("command") in REQUIRES else []
    requires += [r for r in m.get("requires", []) if r not in requires]
    modules.append({
        "id": m["id"],
        "name": m["name"],
        "version": m["version"],
        "description": m.get("description"),
        "author": m.get("author"),
        "authorUrl": m.get("authorUrl"),
        "category": m.get("category"),
        "dir": name,
        "files": sorted(files),
        "requires": requires,
    })

with open(os.path.join(root, "index.json"), "w", encoding="utf-8") as f:
    json.dump({"modules": modules}, f, indent=2, ensure_ascii=False)
    f.write("\n")
print(f"modules/index.json: {len(modules)} modules")
