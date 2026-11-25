"""Lua module for ByteForge: run the current file in a terminal, check its syntax, and format it with StyLua.

Highlighting and the file icon are declared in module.json. The interpreter is "lua.interpreter" from the
settings, else the first of lua, luajit, lua5.4, lua5.3, lua5.2, lua5.1 on PATH. Syntax checks use luac -p
when it is there, else the interpreter compiling the file without running it.
"""

import json
import os
import re
import shlex
import shutil
import subprocess
import sys

# Windows looks for a program in the current folder first (this module's folder) and PATHEXT often has
# .PY, so shutil.which("lua") found this script instead of the program. Programs come from PATH only.
os.environ["NoDefaultCurrentDirectoryInExePath"] = "1"

INTERPRETERS = ["lua", "luajit", "lua5.4", "lua5.3", "lua5.2", "lua5.1"]
COMPILERS = ["luac", "luac5.4", "luac5.3", "luac5.2", "luac5.1"]


def send(message):
    print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def quote(arg):
    if os.name == "nt":
        return subprocess.list2cmdline([arg])
    return shlex.quote(arg)


def find_tool(configured, names):
    """The configured program (a name on PATH or a path), else the first of `names` on PATH."""
    if isinstance(configured, str) and configured.strip():
        return shutil.which(configured.strip()) or configured.strip()
    for name in names:
        found = shutil.which(name)
        if found:
            return found
    return None


class Module:
    def __init__(self):
        self.settings = {}
        self.workspace = None
        self.editor_path = None

    def interpreter(self):
        return find_tool(self.settings.get("lua.interpreter"), INTERPRETERS)

    def compiler(self):
        return find_tool(self.settings.get("lua.compiler"), COMPILERS)

    def current_lua_file(self):
        path = self.editor_path
        if not path or not path.lower().endswith((".lua", ".rockspec")):
            log("lua: open a .lua file first.")
            send({"type": "showView", "id": "output"})
            return None
        return path

    # ── commands ──

    def run_file(self):
        path = self.current_lua_file()
        if not path:
            return
        lua = self.interpreter()
        if not lua:
            log("lua: no Lua interpreter found (install lua or luajit, or set \"lua.interpreter\").")
            send({"type": "showView", "id": "output"})
            return
        send({
            "type": "runInTerminal",
            "title": f"Lua: {os.path.basename(path)}",
            "command": f"{quote(lua)} {quote(path)}",
            "cwd": os.path.dirname(path),
            "saveAll": True,
        })

    def check_syntax(self):
        path = self.current_lua_file()
        if not path:
            return
        error = check(path, self.compiler(), self.interpreter())
        if error is None:
            log(f"lua: {os.path.basename(path)}: no syntax errors.")
            return
        message, line = error
        log(f"lua: {message}")
        send({"type": "showView", "id": "output"})
        if line:
            # Lua reports 1-based lines; openFile takes 0-based ones.
            send({"type": "openFile", "path": path, "line": line - 1, "column": 0})

    def execute(self, command):
        if command == "lua.runFile":
            self.run_file()
        elif command == "lua.checkSyntax":
            self.check_syntax()

    # ── formatting ──

    def format(self, request, path, text):
        stylua = find_tool(self.settings.get("lua.styluaPath"), ["stylua"])
        if not stylua or not shutil.which(stylua):
            send({"type": "formatResult", "requestId": request,
                  "error": "StyLua isn't installed (cargo install stylua, or your package manager), or set \"lua.styluaPath\"."})
            return
        # From the file's folder, so its stylua.toml / .stylua.toml applies.
        args = [stylua, "--search-parent-directories"]
        if path:
            args += ["--stdin-filepath", path]
        args.append("-")
        try:
            result = subprocess.run(args, input=text, capture_output=True, text=True, encoding="utf-8",
                                    cwd=os.path.dirname(path) if path else None, timeout=30)
        except (OSError, subprocess.TimeoutExpired) as e:
            send({"type": "formatResult", "requestId": request, "error": f"StyLua: {e}"})
            return
        if result.returncode != 0:
            first = (result.stderr or result.stdout).strip().splitlines()
            send({"type": "formatResult", "requestId": request, "error": f"StyLua: {first[0] if first else 'failed'}"})
            return
        send({"type": "formatResult", "requestId": request, "text": result.stdout})

    # ── messages from the IDE ──

    def handle(self, message):
        kind = message.get("type")
        if kind == "activate":
            self.workspace = message.get("workspace") or None
            if isinstance(message.get("settings"), dict):
                self.settings = message["settings"]
        elif kind == "executeCommand":
            self.execute(message.get("id"))
        elif kind == "format":
            self.format(message.get("requestId"), message.get("path"), message.get("text") or "")
        elif kind == "event":
            name = message.get("name")
            if name == "editorChanged":
                self.editor_path = message.get("path")
            elif name == "folderOpened":
                self.workspace = message.get("path") or None
            elif name == "settingsChanged" and isinstance(message.get("settings"), dict):
                self.settings = message["settings"]


def parse_error(output, path):
    """(message, line) from luac's or lua's error output, e.g. "luac: a.lua:3: 'end' expected near <eof>"."""
    for raw in output.strip().splitlines():
        line = raw.strip()
        if not line or line.startswith("stack traceback"):
            continue
        # Drop the program name in front ("luac: ", "lua: ").
        line = re.sub(r"^[\w.\-/\\]*luac?[\w.\-]*:\s+", "", line)
        match = re.match(r"^(.*?):(\d+):\s*(.*)$", line)
        if match:
            name = os.path.basename(match.group(1)) or os.path.basename(path)
            return f"{name}:{match.group(2)}: {match.group(3)}", int(match.group(2))
        return line, None
    return "syntax check failed", None


def check(path, luac, lua):
    """None when the file compiles, else (message, line). Uses luac -p, else the interpreter's loadfile."""
    code = None
    if luac:
        args = [luac, "-p", path]
    elif lua:
        # Compile without running: the checking script comes on stdin ("-"), the file is its argument.
        args = [lua, "-", path]
        code = "local f, e = loadfile(...) if not f then io.stderr:write(e, '\\n') os.exit(1) end"
    else:
        return "no luac or Lua interpreter found (install lua, or set \"lua.compiler\" / \"lua.interpreter\")", None
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=30, input=code if code is not None else "")
    except (OSError, subprocess.TimeoutExpired) as e:
        return str(e), None
    if result.returncode == 0:
        return None
    return parse_error(result.stderr or result.stdout, path)


def main():
    module = Module()
    send({"type": "registerCommand", "id": "lua.runFile", "title": "Run File", "category": "Lua"})
    send({"type": "registerCommand", "id": "lua.checkSyntax", "title": "Check Syntax", "category": "Lua"})
    send({"type": "registerFormatter", "id": "lua.stylua", "name": "StyLua", "extensions": ["lua"]})
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if isinstance(message, dict):
            try:
                module.handle(message)
            except Exception as error:  # a bug here must never take the module down
                log(f"lua: {error}")


if __name__ == "__main__":
    main()
