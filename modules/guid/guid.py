"""GUID generator module for ByteForge: inserts fresh GUIDs at the cursor."""

import json
import sys
import uuid

FORMATS = {
    "guid.insert": ("Insert New GUID", lambda g: g, "ctrl-alt-g"),
    "guid.insertUpper": ("Insert New GUID (Uppercase)", lambda g: g.upper(), None),
    "guid.insertBraced": ("Insert New GUID ({…})", lambda g: "{" + g.upper() + "}", None),
    "guid.insertCSharp": ("Insert New GUID (C#: new Guid(\"…\"))", lambda g: f'new Guid("{g}")', None),
}


def send(message):
    print(json.dumps(message), flush=True)


for command, (title, _, key) in FORMATS.items():
    registration = {"type": "registerCommand", "id": command, "title": title, "category": "GUID"}
    if key:
        registration["keybinding"] = key
    send(registration)

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("type") == "executeCommand" and message.get("id") in FORMATS:
        _, fmt, _ = FORMATS[message["id"]]
        send({"type": "insertText", "text": fmt(str(uuid.uuid4()))})
