"""Protocol Buffers module for ByteForge: Format Document for .proto files with buf or clang-format.

"protobuf.formatter" picks one ("auto": buf if it's installed, else clang-format). Highlighting and the file
icon are declared in module.json.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile

settings = {}


def send(message):
    print(json.dumps(message), flush=True)


def formatter():
    choice = settings.get("protobuf.formatter") or "auto"
    candidates = ["buf", "clang-format"] if choice == "auto" else [choice]
    for name in candidates:
        if shutil.which(name):
            return name
    return None


def format_proto(path, text):
    tool = formatter()
    if tool is None:
        return None, "install buf (buf.build) or clang-format to format .proto files"
    folder = os.path.dirname(path) if path else None
    if tool == "buf":
        # buf format reads a file or directory, not stdin: give it a temporary file next to the original, so
        # buf.yaml settings around it apply.
        fd, temp = tempfile.mkstemp(suffix=".proto", dir=folder if folder and os.path.isdir(folder) else None)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(text)
            result = subprocess.run(["buf", "format", temp], capture_output=True, text=True, timeout=30)
        finally:
            os.remove(temp)
    else:
        args = ["clang-format", "--assume-filename=" + (path or "file.proto")]
        result = subprocess.run(args, input=text, capture_output=True, text=True, timeout=30, cwd=folder or None)
    if result.returncode != 0:
        return None, f"{tool}: {(result.stderr or result.stdout).strip()}"
    return result.stdout, None


def main():
    global settings
    send({"type": "registerFormatter", "id": "protobuf", "name": "Protocol Buffers (buf / clang-format)",
          "extensions": ["proto"]})
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        kind = message.get("type")
        if kind == "activate" or (kind == "event" and message.get("name") == "settingsChanged"):
            settings = message.get("settings") or {}
        elif kind == "format":
            try:
                text, error = format_proto(message.get("path"), message.get("text") or "")
            except (OSError, subprocess.SubprocessError) as e:
                text, error = None, str(e)
            reply = {"type": "formatResult", "requestId": message.get("requestId")}
            reply.update({"error": error} if error else {"text": text})
            send(reply)


if __name__ == "__main__":
    main()
