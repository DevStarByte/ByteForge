"""Git Hooks module for ByteForge: the repository's git hooks and Husky.NET tasks in a Git Hooks view.

Lists the hooks in the folder git runs them from (core.hooksPath, else .git/hooks; Husky's .husky works the
same way), marks each active or turned off, and runs one in a terminal with the arguments git would give it.
"New Hook…" writes one from a template (dotnet format, build, test, a Conventional Commits check).
Turning a hook off renames it to <hook>.off, which git ignores; turning it on renames it back.
Husky.NET's tasks (.husky/task-runner.json) are listed under their hook and run with `dotnet husky run`.
"""

import json
import os
import re
import shlex
import stat
import subprocess
import sys
import tempfile

VIEW = "git-hooks"
OFF = ".off"

# The hooks git runs on a client, with what each is for, in the order a commit and push meet them.
HOOKS = {
    "pre-commit": "before a commit is made; fails the commit when it exits non-zero",
    "prepare-commit-msg": "fills in the commit message before the editor opens",
    "commit-msg": "checks the commit message; fails the commit when it exits non-zero",
    "post-commit": "after a commit",
    "pre-rebase": "before a rebase",
    "post-checkout": "after a checkout or switch",
    "post-merge": "after a merge (and a pull)",
    "pre-push": "before a push; fails the push when it exits non-zero",
    "pre-merge-commit": "before a merge commit",
    "post-rewrite": "after commit --amend and rebase",
    "pre-auto-gc": "before git gc --auto",
    "applypatch-msg": "checks the message of git am",
    "pre-applypatch": "before git am commits",
    "post-applypatch": "after git am commits",
    "reference-transaction": "when references change",
    "push-to-checkout": "on a push to a checked-out branch (server side)",
    "sendemail-validate": "before git send-email",
    "fsmonitor-watchman": "file system monitor",
    "post-index-change": "after the index changes",
}

TEMPLATES = [
    ("pre-commit", "dotnet format: fail when files aren't formatted",
     "dotnet format --verify-no-changes --verbosity quiet || {\n"
     "  echo \"Files aren't formatted. Run: dotnet format\" >&2\n  exit 1\n}\n"),
    ("pre-commit", "dotnet build: fail when the solution doesn't build",
     "dotnet build --nologo --verbosity quiet || exit 1\n"),
    ("pre-commit", "No conflict markers or Console.WriteLine debugging in staged C# files",
     "files=$(git diff --cached --name-only --diff-filter=ACM -- '*.cs')\n"
     "[ -z \"$files\" ] && exit 0\n"
     "if echo \"$files\" | xargs grep -nE '^(<<<<<<<|=======|>>>>>>>)( |$)' ; then\n"
     "  echo 'Conflict markers in staged files.' >&2\n  exit 1\nfi\n"
     "if echo \"$files\" | xargs grep -n 'Console.WriteLine(\"DEBUG' ; then\n"
     "  echo 'Debug output left in staged files.' >&2\n  exit 1\nfi\n"),
    ("pre-push", "dotnet test: fail the push when tests fail",
     "dotnet test --nologo --verbosity quiet || {\n  echo 'Tests failed; not pushing.' >&2\n  exit 1\n}\n"),
    ("commit-msg", "Conventional Commits: feat:, fix:, docs: … at the start of the message",
     "msg=$(head -n1 \"$1\")\n"
     "if ! echo \"$msg\" | grep -qE '^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\\([^)]+\\))?!?: .+'; then\n"
     "  echo \"Commit message doesn't follow Conventional Commits: $msg\" >&2\n"
     "  echo 'Example: feat(api): add order export' >&2\n  exit 1\nfi\n"),
    ("prepare-commit-msg", "Put the issue number from the branch name (feature/123-…) in front of the message",
     "case \"$2\" in merge|squash|commit) exit 0 ;; esac\n"
     "issue=$(git symbolic-ref --short HEAD 2>/dev/null | grep -oE '[0-9]+' | head -n1)\n"
     "[ -n \"$issue\" ] && ! grep -q \"#$issue\" \"$1\" && sed -i.bak \"1s/^/#$issue /\" \"$1\" && rm -f \"$1.bak\"\n"
     "exit 0\n"),
    ("post-merge", "dotnet restore when a project file or Directory.Packages.props changed",
     "if git diff --name-only HEAD@{1} HEAD | grep -qE '\\.(csproj|fsproj|props|targets)$'; then\n"
     "  echo 'Project files changed; restoring…'\n  dotnet restore --verbosity quiet\nfi\n"),
    (None, "Empty hook", "# Exit non-zero to stop the git command (for the pre-* hooks and commit-msg).\nexit 0\n"),
]


def send(message):
    print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def git(root, *args):
    try:
        result = subprocess.run(["git", *args], cwd=root, capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def hooks_dir(root):
    """(folder git runs hooks from, its core.hooksPath or None), or (None, None) outside a repository."""
    path = git(root, "rev-parse", "--git-path", "hooks")
    if path is None:
        return None, None
    configured = git(root, "config", "core.hooksPath")
    return os.path.normpath(os.path.join(root, path)), configured


def is_executable(path):
    return os.name == "nt" or bool(os.stat(path).st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH))


def list_hooks(folder):
    """[{name, path, active, executable}] of the hooks in the folder (samples left out)."""
    hooks = []
    try:
        names = sorted(os.listdir(folder))
    except OSError:
        return hooks
    for name in names:
        path = os.path.join(folder, name)
        if not os.path.isfile(path) or name.endswith(".sample"):
            continue
        base, off = (name[:-len(OFF)], True) if name.endswith(OFF) else (name, False)
        if base not in HOOKS:
            continue
        hooks.append({"name": base, "path": path, "active": not off, "executable": is_executable(path)})
    order = list(HOOKS)
    return sorted(hooks, key=lambda h: order.index(h["name"]))


def strip_json_comments(text):
    text = re.sub(r'("(?:\\.|[^"\\])*")|//[^\n]*|/\*.*?\*/', lambda m: m.group(1) or "", text, flags=re.S)
    return re.sub(r'("(?:\\.|[^"\\])*")|,(\s*[}\]])', lambda m: m.group(1) or m.group(2), text)


def husky_tasks(root):
    """Husky.NET tasks from .husky/task-runner.json: [{name, group, command, args}]."""
    path = os.path.join(root, ".husky", "task-runner.json")
    try:
        with open(path, encoding="utf-8-sig") as f:
            data = json.loads(strip_json_comments(f.read()))
    except (OSError, ValueError):
        return None
    tasks = []
    for i, task in enumerate(data.get("tasks") or []):
        if not isinstance(task, dict) or not task.get("command"):
            continue
        tasks.append({"index": i, "name": task.get("name") or task["command"], "group": task.get("group") or "",
                      "command": task["command"], "args": [str(a) for a in task.get("args") or []]})
    return tasks


def quote(arg):
    if os.name == "nt":
        return '"' + arg.replace('"', '\\"') + '"' if re.search(r'[\s"]', arg) else arg
    return shlex.quote(arg)


def sample_message():
    fd, path = tempfile.mkstemp(prefix="byteforge-commit-msg-", suffix=".txt")
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write("feat: try the commit-msg hook from ByteForge\n")
    return path


def hook_command(hook, root):
    """The command line that runs a hook as git would: with git's arguments for it, from the work tree."""
    args = []
    name = hook["name"]
    if name in ("commit-msg", "prepare-commit-msg", "applypatch-msg"):
        args = [sample_message()]
        if name == "prepare-commit-msg":
            args.append("message")
    elif name == "pre-push":
        remote = "origin" if "origin" in (git(root, "remote") or "").split() else ""
        url = git(root, "remote", "get-url", remote) if remote else ""
        args = [remote or "origin", url or ""]
    elif name == "post-checkout":
        head = git(root, "rev-parse", "HEAD") or ""
        args = [head, head, "1"]
    elif name == "post-merge":
        args = ["0"]
    line = " ".join(quote(a) for a in [hook["path"]] + args)
    if os.name == "nt":
        return "sh " + line  # Git for Windows' sh runs hooks
    if name == "pre-push":
        line += " < /dev/null"  # git feeds the refs being pushed on stdin; nothing here
    return line


def make_executable(path):
    if os.name != "nt":
        mode = os.stat(path).st_mode
        os.chmod(path, mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def write_hook(folder, name, body):
    os.makedirs(folder, exist_ok=True)
    path = os.path.join(folder, name)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write("#!/bin/sh\n" + f"# git {name} hook: {HOOKS[name]}.\n# Written by ByteForge's Git Hooks module.\n\n" + body)
    make_executable(path)
    return path


class GitHooks:
    def __init__(self):
        self.root = None
        self.folder = None
        self.configured = None
        self.pending = {}
        self.next_request = 1

    def scan(self):
        self.folder, self.configured = hooks_dir(self.root) if self.root else (None, None)

    def ask(self, kind, data=None):
        request = self.next_request
        self.next_request += 1
        self.pending[request] = (kind, data)
        return request

    # view

    def tree(self):
        if not self.root:
            return {"type": "setTree", "view": VIEW, "items": [], "message": "Open a folder to see its git hooks."}
        if not self.folder:
            return {"type": "setTree", "view": VIEW, "items": [], "message": "This folder isn't a git repository."}
        hooks = list_hooks(self.folder)
        rows = []
        for h in hooks:
            notes = []
            if not h["active"]:
                notes.append("off")
            elif not h["executable"]:
                notes.append("not executable: git skips it")
            menu = [{"id": "open", "label": "Open"}, {"id": "run", "label": "Run"}, {"id": "-"},
                    {"id": "toggle", "label": "Turn Off" if h["active"] else "Turn On"}]
            if h["active"] and not h["executable"]:
                menu.append({"id": "chmod", "label": "Make Executable"})
            menu += [{"id": "-"}, {"id": "delete", "label": "Delete…"}]
            row = {
                "id": f"hook:{h['name']}", "label": h["name"], "description": ", ".join(notes) or HOOKS[h["name"]],
                "tooltip": f"{h['path']}\nRuns {HOOKS[h['name']]}.\nClick: open it.",
                "icon": "play" if h["active"] and h["executable"] else "pause",
                "iconColor": None if h["active"] and h["executable"] else "foreground.muted",
                "actions": [{"id": "run", "icon": "play", "tooltip": "Run (as git would)"}],
                "menu": menu,
            }
            rows.append({k: v for k, v in row.items() if v is not None})
        rel = os.path.relpath(self.folder, self.root).replace(os.sep, "/")
        where = f"{rel} (core.hooksPath)" if self.configured else rel
        items = [{"id": "hooks", "label": f"Hooks in {where}", "header": True, "expanded": True, "count": len(rows),
                  "children": rows or [{"id": "none", "label": "No hooks. + adds one.", "icon": "info"}]}]
        tasks = husky_tasks(self.root)
        if tasks is not None:
            groups = {}
            for t in tasks:
                groups.setdefault(t["group"] or "(no group: run by name)", []).append(t)
            children = []
            for group, ts in groups.items():
                children.append({"id": f"husky-group:{group}", "label": group, "icon": "layers", "expanded": True,
                                 "count": len(ts), "children": [{
                                     "id": f"husky:{t['index']}", "label": t["name"],
                                     "description": " ".join([t["command"]] + t["args"])[:80],
                                     "tooltip": "Click: run it with dotnet husky run --name …", "icon": "play",
                                     "actions": [{"id": "run", "icon": "play", "tooltip": "Run"}]} for t in ts]})
            items.append({"id": "husky", "label": "Husky.NET tasks", "header": True, "expanded": True,
                          "count": len(tasks), "children": children,
                          "actions": [{"id": "huskyOpen", "icon": "file", "tooltip": "Open task-runner.json"}]})
        message = None
        if self.configured and not os.path.isdir(self.folder):
            message = f"core.hooksPath is {self.configured}, which doesn't exist yet."
        return {"type": "setTree", "view": VIEW, "items": items, "message": message,
                "actions": [{"id": "new", "icon": "add", "tooltip": "New Hook…"},
                            {"id": "refresh", "icon": "refresh", "tooltip": "Refresh"}]}

    def refresh(self):
        self.scan()
        send(self.tree())

    # actions

    def find(self, name):
        return next((h for h in list_hooks(self.folder) if h["name"] == name), None) if self.folder else None

    def run(self, hook):
        send({"type": "runInTerminal", "title": f"hook: {hook['name']}", "command": hook_command(hook, self.root),
              "cwd": self.root, "saveAll": True})

    def new_hook(self):
        if not self.folder:
            log("git-hooks: open a git repository first.")
            return
        existing = {h["name"] for h in list_hooks(self.folder)}
        items = []
        for i, (name, title, _) in enumerate(TEMPLATES):
            if name is None:
                continue
            taken = name in existing
            items.append({"label": title, "detail": f"{name}" + ("  (replaces the current one)" if taken else ""),
                          "hint": name, "value": str(i)})
        for name in HOOKS:
            if name not in existing:
                items.append({"label": f"Empty {name} hook", "detail": HOOKS[name], "hint": name, "value": f"empty:{name}"})
        send({"type": "quickPick", "requestId": self.ask("new"), "placeholder": "New git hook", "items": items})

    def create(self, value):
        if value.startswith("empty:"):
            name, body = value[6:], TEMPLATES[-1][2]
        else:
            name, _, body = TEMPLATES[int(value)]
        target = os.path.join(self.folder, name)
        if os.path.exists(target) or os.path.exists(target + OFF):
            send({"type": "prompt", "requestId": self.ask("replace", (name, body)),
                  "message": f"Replace the {name} hook?", "detail": "The current one is overwritten.",
                  "buttons": ["Replace", "Cancel"]})
            return
        self.write(name, body)

    def write(self, name, body):
        off = os.path.join(self.folder, name + OFF)
        if os.path.exists(off):
            os.remove(off)
        path = write_hook(self.folder, name, body)
        log(f"git-hooks: wrote {path}")
        send({"type": "openFile", "path": path})
        self.refresh()

    def toggle(self, hook):
        if hook["active"]:
            os.replace(hook["path"], hook["path"] + OFF)
            log(f"git-hooks: {hook['name']} is off")
        else:
            target = hook["path"][:-len(OFF)]
            if os.path.exists(target):
                log(f"git-hooks: there's already an active {hook['name']} hook.")
                return
            os.replace(hook["path"], target)
            log(f"git-hooks: {hook['name']} is on")
        self.refresh()

    def handle(self, message):
        kind = message.get("type")
        if kind == "activate":
            self.root = message.get("workspace")
            self.refresh()
        elif kind == "event":
            name = message.get("name")
            if name == "folderOpened":
                self.root = message.get("path")
                self.refresh()
            elif name in ("filesChanged", "viewShown"):
                self.refresh()
            elif name == "fileSaved" and self.folder and (message.get("path") or "").startswith(self.folder):
                self.refresh()
        elif kind == "executeCommand":
            command = message.get("id")
            if command == "gitHooks.new":
                self.scan()
                self.new_hook()
            elif command == "gitHooks.run":
                self.scan()
                hooks = [h for h in list_hooks(self.folder or "") if h["active"]]
                if not hooks:
                    log("git-hooks: no active hooks in this repository.")
                    return
                send({"type": "quickPick", "requestId": self.ask("run"), "placeholder": "Run a git hook",
                      "items": [{"label": h["name"], "detail": HOOKS[h["name"]], "value": h["name"]} for h in hooks]})
            elif command == "gitHooks.show":
                send({"type": "showView", "id": VIEW})
        elif kind == "quickPickResult":
            what, _ = self.pending.pop(message.get("requestId"), (None, None))
            value = message.get("value")
            if what == "new" and value:
                self.create(value)
            elif what == "run" and value:
                hook = self.find(value)
                if hook:
                    self.run(hook)
        elif kind == "promptResult":
            what, data = self.pending.pop(message.get("requestId"), (None, None))
            if what == "replace" and message.get("index") == 0:
                self.write(*data)
            elif what == "delete" and message.get("index") == 0:
                hook = self.find(data)
                if hook:
                    os.remove(hook["path"])
                    log(f"git-hooks: deleted {hook['path']}")
                    self.refresh()
        elif kind == "treeAction":
            self.tree_action(message.get("item"), message.get("action"))

    def tree_action(self, item, action):
        if item is None:
            if action == "new":
                self.new_hook()
            elif action == "refresh":
                self.refresh()
            return
        prefix, _, rest = item.partition(":")
        if prefix == "hook":
            hook = self.find(rest)
            if not hook:
                self.refresh()
                return
            if action in ("click", "open"):
                send({"type": "openFile", "path": hook["path"]})
            elif action == "run":
                if not hook["active"]:
                    log(f"git-hooks: {hook['name']} is off; running it anyway.")
                self.run(hook)
            elif action == "toggle":
                self.toggle(hook)
            elif action == "chmod":
                make_executable(hook["path"])
                self.refresh()
            elif action == "delete":
                send({"type": "prompt", "requestId": self.ask("delete", hook["name"]),
                      "message": f"Delete the {hook['name']} hook?", "detail": hook["path"],
                      "buttons": ["Delete", "Cancel"]})
        elif prefix == "husky" and action in ("click", "run"):
            task = next((t for t in husky_tasks(self.root) or [] if str(t["index"]) == rest), None)
            if task:
                send({"type": "runInTerminal", "title": f"husky: {task['name']}",
                      "command": f"dotnet husky run --name {quote(task['name'])}", "cwd": self.root, "saveAll": True})
        elif item == "husky" and action == "huskyOpen":
            send({"type": "openFile", "path": os.path.join(self.root, ".husky", "task-runner.json")})


def main():
    for command, title in (("gitHooks.new", "New Hook…"), ("gitHooks.run", "Run Hook…"), ("gitHooks.show", "Show Hooks")):
        send({"type": "registerCommand", "id": command, "title": title, "category": "Git Hooks"})
    module = GitHooks()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        try:
            module.handle(message)
        except Exception as e:  # keep the module alive; say what went wrong
            log(f"git-hooks: {type(e).__name__}: {e}")


if __name__ == "__main__":
    main()
