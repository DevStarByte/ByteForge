"""TODO Finder module for ByteForge.

Scans the workspace for TODO / FIXME / HACK / BUG / XXX comments. "TODOs: List All" prints them to the
Output panel as `path:line:column: text` (click to jump); the status bar shows the count and refreshes
when a file is saved or a folder is opened.
"""

import json
import os
import re
import sys
import threading

TAGS = ("TODO", "FIXME", "HACK", "BUG", "XXX")
PATTERN = re.compile(r"(?://|#|/\*|<!--|@\*|\*)\s*(" + "|".join(TAGS) + r")\b[:\s]?(.*)")
EXTENSIONS = {".cs", ".csx", ".razor", ".cshtml", ".fs", ".xml", ".csproj", ".props", ".targets", ".json", ".yaml", ".yml",
              ".js", ".ts", ".css", ".scss", ".html", ".md", ".py", ".sh", ".ps1", ".sql", ".dockerfile"}
SKIP_DIRS = {"bin", "obj", ".git", "node_modules", ".vs", ".idea", "target", "TestResults", "wwwroot/lib"}
MAX_FILE = 2_000_000

workspace = None
lock = threading.Lock()


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def scan(root):
    found = []
    for directory, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS and not d.startswith(".")]
        for name in files:
            ext = os.path.splitext(name)[1].lower()
            if ext not in EXTENSIONS and name != "Dockerfile":
                continue
            path = os.path.join(directory, name)
            try:
                if os.path.getsize(path) > MAX_FILE:
                    continue
                with open(path, encoding="utf-8", errors="replace") as f:
                    for number, line in enumerate(f, 1):
                        m = PATTERN.search(line)
                        if m:
                            text = m.group(2).strip().rstrip("*/->").strip()
                            found.append((path, number, m.start(1) + 1, m.group(1), text))
            except OSError:
                pass
    return found


def update_status():
    if not workspace:
        send({"type": "status", "id": "todos.count", "text": ""})
        return
    items = scan(workspace)
    fixme = sum(1 for i in items if i[3] in ("FIXME", "BUG"))
    text = f"☐ {len(items)}" + (f" ({fixme} FIXME)" if fixme else "")
    send({"type": "status", "id": "todos.count", "text": text, "command": "todos.list",
          "tooltip": f"{len(items)} TODO comments in the workspace. Click to list them."})
    return items


def list_all():
    items = update_status() or []
    if not items:
        send({"type": "log", "text": "No TODO comments in the workspace."})
    else:
        send({"type": "log", "text": f"{len(items)} TODO comments:"})
        for path, line, column, tag, text in sorted(items, key=lambda i: (TAGS.index(i[3]) if i[3] != "BUG" else 1, i[0], i[1])):
            rel = os.path.relpath(path, workspace)
            send({"type": "log", "text": f"{rel}:{line}:{column}: {tag} {text}"})
    send({"type": "showView", "id": "output"})


send({"type": "registerCommand", "id": "todos.list", "title": "List All", "category": "TODOs"})
send({"type": "registerCommand", "id": "todos.refresh", "title": "Refresh Count", "category": "TODOs"})

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    kind = message.get("type")
    if kind == "activate":
        workspace = message.get("workspace")
        threading.Thread(target=update_status, daemon=True).start()
    elif kind == "executeCommand":
        if message.get("id") == "todos.list":
            threading.Thread(target=list_all, daemon=True).start()
        elif message.get("id") == "todos.refresh":
            threading.Thread(target=update_status, daemon=True).start()
    elif kind == "event" and message.get("name") in ("folderOpened", "fileSaved"):
        if message.get("name") == "folderOpened":
            workspace = message.get("path")
        threading.Thread(target=update_status, daemon=True).start()
