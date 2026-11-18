"""ReSharper formatter module for ByteForge.

Registers JetBrains' ReSharper command line tools (`jb cleanupcode`, the formatter of Rider and ReSharper) as a
formatter for C#. cleanupcode only works on files, so the text the IDE sends is written to a hidden file next to
the original (so the folder's .editorconfig applies), cleaned up there and read back. The solution's
<name>.DotSettings is passed along when there is one. A version pinned in the workspace's dotnet tool manifest
(.config/dotnet-tools.json) wins over a global install.
"""

import glob
import itertools
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading

PACKAGE = "jetbrains.resharper.globaltools"
EXTENSIONS = ["cs"]
DEFAULT_PROFILE = "Built-in: Reformat Code"
INSTALL_HINT = ("The ReSharper command line tools aren't installed. Run \"ReSharper: Install\" from the command palette "
                "(dotnet tool install -g JetBrains.ReSharper.GlobalTools).")

workspace = None
settings = {}
output_lock = threading.Lock()
counter = itertools.count()


def send(message):
    with output_lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def local_tool_dir(start):
    """The folder whose .config/dotnet-tools.json lists the ReSharper tools, searching upwards from `start`."""
    directory = os.path.abspath(start) if start else None
    while directory:
        manifest = os.path.join(directory, ".config", "dotnet-tools.json")
        try:
            with open(manifest, encoding="utf-8") as f:
                if PACKAGE in (name.lower() for name in json.load(f).get("tools", {})):
                    return directory
        except (OSError, ValueError, AttributeError):
            pass
        parent = os.path.dirname(directory)
        directory = None if parent == directory else parent
    return None


def find_jb(near):
    """(command, working directory) to run `jb`, or None."""
    local = local_tool_dir(near) or local_tool_dir(workspace)
    if local:
        return ["dotnet", "jb"], local
    found = shutil.which("jb")
    if not found:
        exe = "jb.exe" if os.name == "nt" else "jb"
        candidate = os.path.join(os.path.expanduser("~"), ".dotnet", "tools", exe)
        found = candidate if os.path.isfile(candidate) else None
    return ([found], None) if found else None


def solution_settings(start):
    """The <solution>.DotSettings of the nearest solution above `start`, or None."""
    directory = os.path.abspath(start) if start else None
    while directory:
        for solution in sorted(glob.glob(os.path.join(glob.escape(directory), "*.sln*"))):
            if solution.endswith((".sln", ".slnx")):
                dot_settings = solution + ".DotSettings"
                if os.path.isfile(dot_settings):
                    return dot_settings
                return None
        parent = os.path.dirname(directory)
        directory = None if parent == directory else parent
    return None


def update_status(text=None):
    if text:
        send({"type": "status", "id": "resharper.status", "text": text, "alignment": "right"})
    elif find_jb(workspace):
        send({"type": "status", "id": "resharper.status", "text": "", "alignment": "right"})
    else:
        send({"type": "status", "id": "resharper.status", "text": "ReSharper: not installed", "alignment": "right",
              "command": "resharper.install",
              "tooltip": "Click to install the ReSharper command line tools (dotnet tool install -g JetBrains.ReSharper.GlobalTools)"})


def scratch_file(path):
    """A new hidden file next to `path` (so .editorconfig applies), or in a temp folder for untitled files."""
    name = f".bf-resharper-{os.getpid()}-{next(counter)}.cs"
    folder = os.path.dirname(path) if path else None
    if folder and os.path.isdir(folder) and os.access(folder, os.W_OK):
        return os.path.join(folder, name), None
    temp = tempfile.mkdtemp(prefix="bf-resharper-")
    return os.path.join(temp, name), temp


def format_text(request, path, text):
    near = os.path.dirname(path) if path else workspace
    tool = find_jb(near)
    if not tool:
        send({"type": "formatResult", "requestId": request, "error": INSTALL_HINT})
        return
    command, cwd = tool
    profile = settings.get("resharper.profile") or DEFAULT_PROFILE
    scratch, temp_dir = scratch_file(path)
    arguments = command + ["cleanupcode", f"--profile={profile}", "--verbosity=WARN"]
    dot_settings = solution_settings(near)
    if dot_settings:
        arguments.append(f"--settings={dot_settings}")
    arguments.append(scratch)
    update_status("ReSharper: formatting…")
    try:
        # newline="" keeps the document's line endings as they are.
        with open(scratch, "w", encoding="utf-8", newline="") as f:
            f.write(text)
        done = subprocess.run(arguments, capture_output=True, cwd=cwd or near or None, timeout=300)
        with open(scratch, encoding="utf-8", newline="") as f:
            out = f.read()
    except (OSError, subprocess.TimeoutExpired) as error:
        send({"type": "formatResult", "requestId": request, "error": f"ReSharper failed: {error}"})
        return
    finally:
        try:
            os.remove(scratch)
        except OSError:
            pass
        if temp_dir:
            shutil.rmtree(temp_dir, ignore_errors=True)
        update_status()
    if done.returncode != 0:
        err = (done.stderr + done.stdout).decode("utf-8", errors="replace").strip()
        send({"type": "formatResult", "requestId": request, "error": err or f"jb cleanupcode exited with {done.returncode}"})
    elif not out and text.strip():
        send({"type": "formatResult", "requestId": request, "error": "ReSharper returned an empty file."})
    else:
        send({"type": "formatResult", "requestId": request, "text": out})


def run_tool(arguments, success):
    def work():
        log("> dotnet " + " ".join(arguments))
        try:
            done = subprocess.run(["dotnet"] + arguments, capture_output=True, timeout=600)
            output = (done.stdout + done.stderr).decode("utf-8", errors="replace").strip()
            if output:
                log(output)
            if done.returncode == 0:
                log(success)
        except (OSError, subprocess.TimeoutExpired) as error:
            log(f"dotnet failed: {error}")
        send({"type": "showView", "id": "output"})
        update_status()
    threading.Thread(target=work, daemon=True).start()


send({"type": "registerFormatter", "id": "resharper", "name": "ReSharper", "extensions": EXTENSIONS})
send({"type": "registerCommand", "id": "resharper.install", "title": "Install (global dotnet tool)", "category": "ReSharper"})
send({"type": "registerCommand", "id": "resharper.update", "title": "Update", "category": "ReSharper"})
send({"type": "registerCommand", "id": "resharper.uninstall", "title": "Uninstall the dotnet tool", "category": "ReSharper"})
update_status()

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    kind = message.get("type")
    if kind == "activate":
        workspace = message.get("workspace")
        settings = message.get("settings") or {}
        update_status()
    elif kind == "event" and message.get("name") == "folderOpened":
        workspace = message.get("path")
        update_status()
    elif kind == "event" and message.get("name") == "settingsChanged":
        settings = message.get("settings") or {}
    elif kind == "format":
        threading.Thread(target=format_text, args=(message.get("requestId"), message.get("path"), message.get("text", "")),
                         daemon=True).start()
    elif kind == "executeCommand":
        command = message.get("id")
        if command == "resharper.install":
            run_tool(["tool", "install", "-g", "JetBrains.ReSharper.GlobalTools"],
                     "The ReSharper command line tools are installed. Pick ReSharper with \"Format Document With…\".")
        elif command == "resharper.update":
            run_tool(["tool", "update", "-g", "JetBrains.ReSharper.GlobalTools"], "The ReSharper command line tools are up to date.")
        elif command == "resharper.uninstall":
            run_tool(["tool", "uninstall", "-g", "JetBrains.ReSharper.GlobalTools"], "The ReSharper command line tools were uninstalled.")
