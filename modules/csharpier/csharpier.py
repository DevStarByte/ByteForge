"""CSharpier formatter module for ByteForge.

Registers CSharpier (https://csharpier.com) as the formatter for C# and MSBuild/XML files and formats text
the IDE sends with `csharpier format --write-stdout`. A version pinned in the workspace's dotnet tool
manifest (.config/dotnet-tools.json) wins over a global install, like in other editors.
"""

import json
import os
import shutil
import subprocess
import sys
import threading

EXTENSIONS = ["cs", "csx", "csproj", "props", "targets", "xml", "config"]
INSTALL_HINT = "CSharpier isn't installed. Run \"CSharpier: Install\" from the command palette (dotnet tool install -g csharpier)."

workspace = None
output_lock = threading.Lock()


def send(message):
    with output_lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def local_tool_dir(start):
    """The folder whose .config/dotnet-tools.json lists csharpier, searching upwards from `start`."""
    directory = os.path.abspath(start) if start else None
    while directory:
        manifest = os.path.join(directory, ".config", "dotnet-tools.json")
        try:
            with open(manifest, encoding="utf-8") as f:
                if "csharpier" in json.load(f).get("tools", {}):
                    return directory
        except (OSError, ValueError):
            pass
        parent = os.path.dirname(directory)
        directory = None if parent == directory else parent
    return None


def find_csharpier(near):
    """(command, working directory) to run CSharpier, or None."""
    local = local_tool_dir(near) or local_tool_dir(workspace)
    if local:
        return ["dotnet", "csharpier"], local
    found = shutil.which("csharpier")
    if not found:
        exe = "csharpier.exe" if os.name == "nt" else "csharpier"
        candidate = os.path.join(os.path.expanduser("~"), ".dotnet", "tools", exe)
        found = candidate if os.path.isfile(candidate) else None
    return ([found], None) if found else None


def update_status():
    if find_csharpier(workspace):
        send({"type": "status", "id": "csharpier.status", "text": "", "alignment": "right"})
    else:
        send({"type": "status", "id": "csharpier.status", "text": "CSharpier: not installed", "alignment": "right",
              "command": "csharpier.install", "tooltip": "Click to install CSharpier (dotnet tool install -g csharpier)"})


def format_text(request, path, text):
    tool = find_csharpier(os.path.dirname(path) if path else workspace)
    if not tool:
        send({"type": "formatResult", "requestId": request, "error": INSTALL_HINT})
        return
    command, cwd = tool
    try:
        done = subprocess.run(command + ["format", "--write-stdout", "--stdin-path", path or "Untitled.cs"],
                              input=text.encode("utf-8"), capture_output=True, cwd=cwd or workspace, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as error:
        send({"type": "formatResult", "requestId": request, "error": f"CSharpier failed: {error}"})
        return
    out = done.stdout.decode("utf-8", errors="replace")
    err = (done.stderr.decode("utf-8", errors="replace") or out).strip()
    if done.returncode != 0:
        send({"type": "formatResult", "requestId": request, "error": err or f"CSharpier exited with {done.returncode}"})
    elif not out and text.strip():
        # CSharpier prints nothing for file types it doesn't handle; never hand back an empty file.
        send({"type": "formatResult", "requestId": request, "error": "CSharpier can't format this file type."})
    else:
        send({"type": "formatResult", "requestId": request, "text": out})


def run_tool(arguments, success):
    def work():
        log("> dotnet " + " ".join(arguments))
        try:
            done = subprocess.run(["dotnet"] + arguments, capture_output=True, timeout=300)
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


send({"type": "registerFormatter", "id": "csharpier", "name": "CSharpier", "extensions": EXTENSIONS})
send({"type": "registerCommand", "id": "csharpier.install", "title": "Install (global dotnet tool)", "category": "CSharpier"})
send({"type": "registerCommand", "id": "csharpier.update", "title": "Update", "category": "CSharpier"})
send({"type": "registerCommand", "id": "csharpier.uninstall", "title": "Uninstall the dotnet tool", "category": "CSharpier"})
update_status()

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    kind = message.get("type")
    if kind == "activate":
        workspace = message.get("workspace")
        update_status()
    elif kind == "event" and message.get("name") == "folderOpened":
        workspace = message.get("path")
        update_status()
    elif kind == "format":
        threading.Thread(target=format_text, args=(message.get("requestId"), message.get("path"), message.get("text", "")),
                         daemon=True).start()
    elif kind == "executeCommand":
        command = message.get("id")
        if command == "csharpier.install":
            run_tool(["tool", "install", "-g", "csharpier"], "CSharpier is installed. Format a C# file with Shift+Alt+F.")
        elif command == "csharpier.update":
            run_tool(["tool", "update", "-g", "csharpier"], "CSharpier is up to date.")
        elif command == "csharpier.uninstall":
            run_tool(["tool", "uninstall", "-g", "csharpier"], "The CSharpier dotnet tool was uninstalled.")
