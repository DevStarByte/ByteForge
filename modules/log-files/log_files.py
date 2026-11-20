"""Log Files module for ByteForge: highlighting for .log files (declared in module.json) and commands to find
what matters in them.

"Log: List Errors" prints every error (with its exception and the first stack frame) to the Output as
path:line: text, so a click goes there. "Log: Filter…" opens a copy with only errors, warnings and up, without
debug and trace, or just the exceptions with their stack traces. "Log: Follow" runs tail -f (Get-Content -Wait
on Windows) in a terminal. Logs are read as saved.

Levels are recognized in the common formats: ASP.NET Core's console logger (fail: / warn: / info:), Serilog
([ERR] [WRN] [INF]), NLog, log4net and Microsoft.Extensions.Logging (Error, Warning, Information), plain
ERROR / WARN / INFO, syslog and GitHub Actions (##[error]).
"""

import datetime
import json
import os
import re
import sys

EXTENSIONS = (".log", ".out")
MAX_BYTES = 200_000_000
MAX_LISTED = 500

LEVEL_WORDS = {
    "fatal": ("fatal", "ftl", "crit", "critical", "emerg", "emergency", "alert", "panic"),
    "error": ("error", "err", "eror", "fail", "severe"),
    "warning": ("warn", "warning", "wrn"),
    "info": ("info", "inf", "information", "notice", "nfo"),
    "debug": ("debug", "dbg", "dbug", "fine"),
    "trace": ("trace", "trce", "trc", "verbose", "vrb", "finest", "finer"),
}
WORD_LEVEL = {w: level for level, words in LEVEL_WORDS.items() for w in words}
ORDER = ["trace", "debug", "info", "warning", "error", "fatal"]

# A level word as loggers write it: at the start of the line or after the timestamp, alone, in brackets, or
# followed by a colon / pipe; plus GitHub Actions' ##[error].
LEVEL = re.compile(
    r"^##\[(error|warning|notice|debug)\]"
    r"|(?:^|[\s\[|(<])(" + "|".join(sorted(WORD_LEVEL, key=len, reverse=True)) + r")(?=[\]\s:|)>]|$)",
    re.I)
STACK_FRAME = re.compile(r"^\s+(?:at |---|File \"|\.\.\. \d+ more|Caused by:)|^\s*--- End of")
EXCEPTION = re.compile(r"\b((?:[A-Za-z_]\w*\.)*[A-Z]\w*(?:Exception|Error))\b(?::|\s*\(|$)")
CONTINUATION = re.compile(r"^\s+\S|^\s*$")
TIMESTAMP_START = re.compile(r"^\s*(?:\[?\d{4}-\d\d-\d\d|\[?\d\d:\d\d:\d\d|\w{3} [ \d]\d \d\d:|\d{1,2}/\d{1,2}/\d{2,4})")


def send(message):
    print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def level_of(line):
    """The level a line is logged at, from the first level word near its start, or None."""
    m = LEVEL.search(line[:80])  # loggers put the level first, or after a timestamp and a category
    if not m:
        return None
    word = (m.group(1) or m.group(2)).lower()
    if m.group(1):
        return {"notice": "info"}.get(word, word)
    return WORD_LEVEL.get(word)


def entries(lines):
    """Groups lines into log entries: a line with a level (or a timestamp) starts one; indented lines, stack
    frames and blank lines after it belong to it. Yields (start index, level or None, [lines])."""
    current = None
    for i, line in enumerate(lines):
        level = level_of(line)
        starts = level is not None or TIMESTAMP_START.match(line)
        if current and not starts and (CONTINUATION.match(line) or STACK_FRAME.match(line)
                                       or EXCEPTION.search(line) and current[1] in ("error", "fatal", "warning")):
            current[2].append(line)
            continue
        if current:
            yield current
        current = (i, level, [line])
    if current:
        yield current


def at_least(level, minimum):
    return level is not None and ORDER.index(level) >= ORDER.index(minimum)


def exception_of(entry_lines):
    for line in entry_lines:
        m = EXCEPTION.search(line)
        if m:
            return m.group(1)
    return None


def first_frame(entry_lines):
    for line in entry_lines:
        if re.match(r"^\s+at ", line):
            return line.strip()
    return None


def list_errors(path, lines):
    """Output lines (path:line: text) for the errors and fatal errors, newest last."""
    out = []
    for start, level, block in entries(lines):
        if not at_least(level, "error"):
            continue
        text = block[0].strip()
        exception = exception_of(block[1:]) if not EXCEPTION.search(text) else None
        frame = first_frame(block)
        detail = " · ".join(x for x in (exception, frame) if x)
        out.append(f"{path}:{start + 1}:1: {text[:300]}" + (f"  [{detail[:200]}]" if detail else ""))
    return out


FILTERS = {
    "errors": ("Errors only", "error and fatal entries, with their stack traces"),
    "warnings": ("Warnings and errors", "warning, error and fatal entries"),
    "nodebug": ("Without debug and trace", "everything but debug and trace entries"),
    "exceptions": ("Exceptions", "entries with an exception or a stack trace"),
}


def apply_filter(lines, which):
    kept = []
    for start, level, block in entries(lines):
        if which == "errors":
            keep = at_least(level, "error")
        elif which == "warnings":
            keep = at_least(level, "warning")
        elif which == "nodebug":
            keep = level not in ("debug", "trace")
        else:
            keep = any(EXCEPTION.search(l) for l in block) or any(STACK_FRAME.match(l) for l in block)
        if keep:
            kept.extend(block)
    return kept


def counts(lines):
    result = {level: 0 for level in ORDER}
    for _, level, _ in entries(lines):
        if level:
            result[level] += 1
    return result


def read_lines(path):
    size = os.path.getsize(path)
    with open(path, "rb") as f:
        if size > MAX_BYTES:  # the end of a huge log is what matters
            f.seek(size - MAX_BYTES)
            f.readline()
        data = f.read()
    return data.decode("utf-8", errors="replace").replace("\r\n", "\n").split("\n")


def cache_dir():
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    else:
        base = os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache")
    return os.path.join(base, "byteforge", "logs")


def quote(path):
    if os.name == "nt":
        return "'" + path.replace("'", "''") + "'"
    return "'" + path.replace("'", "'\\''") + "'"


class LogModule:
    def __init__(self):
        self.active = None
        self.pending = {}
        self.next_request = 1

    def target(self):
        if not self.active:
            log("log: open a .log file first (it's read as saved).")
            send({"type": "showView", "id": "output"})
        return self.active

    def list_errors(self):
        path = self.target()
        if not path:
            return
        lines = read_lines(path)
        found = list_errors(path, lines)
        c = counts(lines)
        log(f"log: {os.path.basename(path)}: {c['fatal']} fatal, {c['error']} errors, {c['warning']} warnings")
        for line in found[-MAX_LISTED:]:
            log(line)
        if len(found) > MAX_LISTED:
            log(f"log: … and {len(found) - MAX_LISTED} older errors (\"Log: Filter…\" shows them all)")
        send({"type": "showView", "id": "output"})

    def pick_filter(self):
        if not self.target():
            return
        request = self.next_request
        self.next_request += 1
        self.pending[request] = "filter"
        send({"type": "quickPick", "requestId": request, "placeholder": "Show only…",
              "items": [{"label": title, "detail": detail, "value": key} for key, (title, detail) in FILTERS.items()]})

    def filter(self, which):
        path = self.active
        kept = apply_filter(read_lines(path), which)
        if not kept:
            log(f"log: nothing in {os.path.basename(path)} for \"{FILTERS[which][0]}\".")
            send({"type": "showView", "id": "output"})
            return
        folder = cache_dir()
        os.makedirs(folder, exist_ok=True)
        stamp = datetime.datetime.now().strftime("%H%M%S")
        out = os.path.join(folder, f"{os.path.splitext(os.path.basename(path))[0]}.{which}-{stamp}.log")
        with open(out, "w", encoding="utf-8") as f:
            f.write("\n".join(kept) + "\n")
        send({"type": "openFile", "path": out})

    def follow(self):
        path = self.target()
        if not path:
            return
        command = (f"Get-Content -Wait -Tail 200 -Path {quote(path)}" if os.name == "nt"
                   else f"tail -n 200 -F {quote(path)}")
        send({"type": "runInTerminal", "title": f"follow {os.path.basename(path)}", "command": command,
              "cwd": os.path.dirname(path)})

    def update_status(self):
        if not self.active:
            send({"type": "status", "id": "log.counts", "text": ""})
            return
        try:
            c = counts(read_lines(self.active))
        except OSError:
            return
        errors = c["error"] + c["fatal"]
        send({"type": "status", "id": "log.counts", "text": f"✗ {errors}  ⚠ {c['warning']}", "command": "log.listErrors",
              "tooltip": f"{os.path.basename(self.active)}: {c['fatal']} fatal, {c['error']} errors, "
                         f"{c['warning']} warnings, {c['info']} info. Click: list the errors."})

    def handle(self, message):
        kind = message.get("type")
        if kind == "event":
            name = message.get("name")
            if name == "editorChanged":
                path = message.get("path")
                self.active = path if path and is_log(path) else None
                self.update_status()
            elif name == "fileSaved" and message.get("path") == self.active:
                self.update_status()
        elif kind == "executeCommand":
            command = message.get("id")
            if command == "log.listErrors":
                self.list_errors()
            elif command == "log.filter":
                self.pick_filter()
            elif command == "log.follow":
                self.follow()
        elif kind == "quickPickResult":
            if self.pending.pop(message.get("requestId"), None) == "filter" and message.get("value") in FILTERS:
                self.filter(message["value"])


def is_log(path):
    name = os.path.basename(path).lower()
    return name.endswith(EXTENSIONS) or re.search(r"\.log\.\d+$|\.log\.\d{4}-\d\d-\d\d$", name) is not None


def main():
    for command, title in (("log.listErrors", "List Errors"), ("log.filter", "Filter…"), ("log.follow", "Follow (tail -f)")):
        send({"type": "registerCommand", "id": command, "title": title, "category": "Log"})
    module = LogModule()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        try:
            module.handle(message)
        except Exception as e:  # keep the module alive; say what went wrong
            log(f"log: {type(e).__name__}: {e}")


if __name__ == "__main__":
    main()
