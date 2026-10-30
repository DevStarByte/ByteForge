"""Example ByteForge module.

Modules are plain processes speaking newline-delimited JSON over stdin/stdout, so they can be written in
any language and can never crash the IDE. This one registers a command with a key binding and keeps a
clock in the status bar.
"""

import json
import sys
import threading
import time

workspace = None


def send(message):
    print(json.dumps(message), flush=True)


def clock():
    while True:
        send({"type": "status", "id": "hello.clock", "text": time.strftime("%H:%M:%S"),
              "alignment": "right", "command": "hello.sayHello", "tooltip": "Hello module — click me"})
        time.sleep(1)


send({"type": "registerCommand", "id": "hello.sayHello", "title": "Say Hello",
      "category": "Hello", "keybinding": "ctrl-alt-h"})
threading.Thread(target=clock, daemon=True).start()

for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "activate":
        workspace = message.get("workspace")
    elif kind == "executeCommand" and message.get("id") == "hello.sayHello":
        send({"type": "log", "text": f"Hello from Python! Workspace: {workspace or '(none)'}"})
        send({"type": "showView", "id": "output"})
    elif kind == "event":
        if message.get("name") == "folderOpened":
            workspace = message.get("path")
        send({"type": "log", "text": f"event {message.get('name')}: {message.get('path')}"})
