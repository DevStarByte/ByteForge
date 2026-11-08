"""JSON formatter module for ByteForge: Format Document on .json files.

Indentation follows the file (tabs or the width of its first indented line, default 2), key order is kept
and the trailing newline too. JSON with comments (JSONC, e.g. some tsconfig/launch files) is left alone
rather than losing its comments.
"""

import json
import re
import sys


def send(message):
    print(json.dumps(message), flush=True)


def detect_indent(text):
    for line in text.splitlines()[1:]:
        if line.startswith("\t"):
            return "\t"
        stripped = len(line) - len(line.lstrip(" "))
        if stripped:
            return " " * stripped
    return "  "


def has_comments(text):
    # Strip strings first so "http://…" doesn't count as a comment.
    without_strings = re.sub(r'"(?:\\.|[^"\\])*"', '""', text)
    return "//" in without_strings or "/*" in without_strings


def format_json(text):
    if has_comments(text):
        raise ValueError("the file has comments (JSONC); left unchanged")
    data = json.loads(text)
    indent = detect_indent(text)
    out = json.dumps(data, indent=indent, ensure_ascii=False)
    return out + ("\n" if text.endswith("\n") or not text else "")


send({"type": "registerFormatter", "id": "json-format", "name": "JSON Formatter", "extensions": ["json"]})

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("type") == "format":
        request = message.get("requestId")
        try:
            send({"type": "formatResult", "requestId": request, "text": format_json(message.get("text", ""))})
        except ValueError as e:
            send({"type": "formatResult", "requestId": request, "error": f"JSON: {e}"})
