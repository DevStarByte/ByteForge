"""Discord Rich Presence module for ByteForge.

Talks to the Discord desktop app over its local IPC socket (no Discord library needed) and sets your activity
to what you're doing in ByteForge: the file, its language, the project and the elapsed time.

Settings (in ByteForge's settings.json):
  "discord.clientId":     optional: your own Discord application's ID, to use instead of ByteForge's.
  "discord.showFileName": false shows "Editing a C# file" instead of the file name (default true).
  "discord.showProject":  false hides the project name (default true).
  "discord.largeImage":   an art asset key of your application, or an image URL (default: the ByteForge logo).
"""

import json
import os
import re
import socket
import struct
import sys
import threading
import time
import uuid
from pathlib import Path

CLIENT_ID = "1569037494551322826"  # The ByteForge application on Discord.
LOGO = "https://raw.githubusercontent.com/DevStarByte/ByteForge/packages/modules/discord/byteforge.png"
MIN_INTERVAL = 4.0  # Discord allows 5 activity updates per 20 seconds.
RETRY = 15.0

LANGUAGES = {
    "cs": "C#", "csx": "C#", "csproj": "MSBuild", "props": "MSBuild", "targets": "MSBuild", "sln": "Visual Studio",
    "slnx": "Visual Studio", "razor": "Razor", "cshtml": "Razor", "xaml": "XAML", "axaml": "XAML", "fs": "F#",
    "vb": "Visual Basic", "json": "JSON", "jsonc": "JSON", "xml": "XML", "config": "XML", "resx": "XML",
    "md": "Markdown", "yml": "YAML", "yaml": "YAML", "toml": "TOML", "rs": "Rust", "py": "Python",
    "js": "JavaScript", "mjs": "JavaScript", "ts": "TypeScript", "tsx": "TypeScript", "jsx": "JavaScript",
    "html": "HTML", "htm": "HTML", "css": "CSS", "scss": "SCSS", "sql": "SQL", "sh": "Shell", "bash": "Shell",
    "fish": "Shell", "ps1": "PowerShell", "lua": "Lua", "go": "Go", "java": "Java", "kt": "Kotlin", "c": "C",
    "h": "C", "cpp": "C++", "hpp": "C++", "cc": "C++", "txt": "plain text", "dockerfile": "Docker",
}

out_lock = threading.Lock()


def send(message):
    with out_lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": f"Discord: {text}"})


def status(text, tooltip=None, command=None):
    send({"type": "status", "id": "discord.status", "text": text, "tooltip": tooltip, "command": command, "alignment": "right"})


# ───────────── settings ─────────────

def settings_path():
    here = Path(__file__).resolve().parent
    if here.parent.name == "modules":
        return here.parent.parent / "settings.json"
    config = os.environ.get("XDG_CONFIG_HOME") or os.environ.get("APPDATA") or os.path.join(Path.home(), ".config")
    return Path(config) / "byteforge" / "settings.json"


def read_settings():
    try:
        text = settings_path().read_text(encoding="utf-8")
    except OSError:
        return {}
    # settings.json may have comments and trailing commas.
    text = re.sub(r'("(?:\\.|[^"\\])*")|//[^\n]*|/\*.*?\*/', lambda m: m.group(1) or "", text, flags=re.S)
    text = re.sub(r",(\s*[}\]])", r"\1", text)
    try:
        value = json.loads(text)
    except ValueError:
        return {}
    return value if isinstance(value, dict) else {}


# ───────────── Discord IPC ─────────────

def socket_candidates():
    if os.name == "nt":
        return [rf"\\?\pipe\discord-ipc-{i}" for i in range(10)]
    base = os.environ.get("XDG_RUNTIME_DIR") or os.environ.get("TMPDIR") or "/tmp"
    dirs = [base, f"{base}/app/com.discordapp.Discord", f"{base}/app/com.discordapp.DiscordCanary",
            f"{base}/snap.discord", f"{base}/app/dev.vencord.Vesktop", f"{base}/.flatpak/dev.vencord.Vesktop/xdg-run", "/tmp"]
    return [f"{d}/discord-ipc-{i}" for d in dirs for i in range(10)]


class DiscordError(Exception):
    pass


class Ipc:
    def __init__(self, client_id):
        self.client_id = client_id
        self.conn = None
        self.pipe = None

    def connect(self):
        for path in socket_candidates():
            try:
                if os.name == "nt":
                    self.pipe = open(path, "r+b", buffering=0)
                else:
                    if not os.path.exists(path):
                        continue
                    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    s.settimeout(5)
                    s.connect(path)
                    self.conn = s
            except OSError:
                self.conn = self.pipe = None
                continue
            try:
                self._send(0, {"v": 1, "client_id": self.client_id})
                op, data = self._recv()
            except (OSError, DiscordError):
                self.close()
                continue
            if op == 2 or data.get("evt") == "ERROR":
                message = (data.get("data") or {}).get("message") or data.get("message") or "Discord refused the connection"
                self.close()
                raise DiscordError(message)
            return True
        return False

    def _write(self, data):
        if self.conn:
            self.conn.sendall(data)
        else:
            self.pipe.write(data)

    def _read(self, n):
        chunks = b""
        while len(chunks) < n:
            part = self.conn.recv(n - len(chunks)) if self.conn else self.pipe.read(n - len(chunks))
            if not part:
                raise DiscordError("Discord closed the connection")
            chunks += part
        return chunks

    def _send(self, op, payload):
        body = json.dumps(payload).encode("utf-8")
        self._write(struct.pack("<II", op, len(body)) + body)

    def _recv(self):
        while True:
            op, length = struct.unpack("<II", self._read(8))
            data = json.loads(self._read(length).decode("utf-8")) if length else {}
            if op == 3:  # ping
                self._send(4, data)
                continue
            return op, data

    def set_activity(self, activity):
        nonce = str(uuid.uuid4())
        # The pid is ByteForge's, so Discord clears the activity by itself when the IDE exits.
        self._send(1, {"cmd": "SET_ACTIVITY", "args": {"pid": os.getppid(), "activity": activity}, "nonce": nonce})
        while True:
            op, data = self._recv()
            if op == 2:
                raise DiscordError((data or {}).get("message") or "Discord closed the connection")
            if data.get("nonce") == nonce:
                if data.get("evt") == "ERROR":
                    raise DiscordError((data.get("data") or {}).get("message", "SET_ACTIVITY failed"))
                return

    def close(self):
        for handle in (self.conn, self.pipe):
            try:
                if handle:
                    handle.close()
            except OSError:
                pass
        self.conn = self.pipe = None


# ───────────── presence ─────────────

state_lock = threading.Condition()
state = {"workspace": None, "path": None, "start": int(time.time()), "paused": False, "dirty": True, "retry": True}


def language_of(path):
    name = path.name.lower()
    if name == "dockerfile" or name.startswith("dockerfile."):
        return "Docker"
    return LANGUAGES.get(path.suffix.lower().lstrip("."), None)


def clip(text, limit=128):
    text = text.strip()
    if len(text) > limit:
        text = text[: limit - 1] + "…"
    return text if len(text) >= 2 else None


def build_activity(settings):
    workspace, path = state["workspace"], state["path"]
    show_file = settings.get("discord.showFileName", True) is not False
    show_project = settings.get("discord.showProject", True) is not False
    project = Path(workspace).name if workspace else None

    if path:
        p = Path(path)
        lang = language_of(p)
        if show_file:
            details = f"Editing {p.name}"
        else:
            details = f"Editing a {lang} file" if lang else "Editing a file"
    else:
        details = "Browsing the project" if project else "Idle"
        lang = None
    activity = {
        "details": clip(details),
        "timestamps": {"start": state["start"]},
        "assets": {"large_image": settings.get("discord.largeImage") or LOGO, "large_text": "ByteForge"},
    }
    if show_project and project:
        activity["state"] = clip(f"In {project}")
    elif lang:
        activity["state"] = clip(f"Writing {lang}")
    if lang and show_project and project:
        activity["assets"]["large_text"] = clip(f"ByteForge, writing {lang}")
    return {k: v for k, v in activity.items() if v is not None}


def worker():
    ipc, connected_id, last_sent, last_try, last_error = None, None, 0.0, 0.0, None
    while True:
        with state_lock:
            state_lock.wait(timeout=RETRY)
            wake_retry = state["retry"]
            state["retry"] = False
        settings = read_settings()
        client_id = str(settings.get("discord.clientId") or "").strip() or CLIENT_ID
        if ipc and connected_id != client_id:
            ipc.close()
            ipc = None
        if ipc is None:
            now = time.time()
            if not wake_retry and now - last_try < RETRY:
                continue
            last_try = now
            candidate = Ipc(client_id)
            try:
                if not candidate.connect():
                    status("Discord: offline", "The Discord app isn't running. ByteForge connects when it starts.", "discord.reconnect")
                    continue
            except DiscordError as e:
                if str(e) != last_error:
                    log(f"Discord refused the connection: {e}. Check \"discord.clientId\" in settings.json.")
                    last_error = str(e)
                status("Discord: error", f"Discord refused the connection: {e}. Click to retry.", "discord.reconnect")
                continue
            ipc, connected_id, last_error = candidate, client_id, None
            with state_lock:
                state["dirty"] = True
        with state_lock:
            if not state["dirty"]:
                continue
            wait = MIN_INTERVAL - (time.time() - last_sent)
        if wait > 0:
            time.sleep(wait)
        with state_lock:
            state["dirty"] = False
            paused = state["paused"]
            activity = None if paused else build_activity(settings)
        try:
            ipc.set_activity(activity)
            last_sent = time.time()
            if paused:
                status("Discord: paused", "Not sharing your activity. Click to resume.", "discord.toggle")
            else:
                status("", None)  # Connected and sharing: nothing to say in the status bar.
        except (OSError, DiscordError, ValueError) as e:
            log(f"lost the connection ({e}); retrying.")
            ipc.close()
            ipc = None
            with state_lock:
                state["dirty"] = True


def update(**changes):
    with state_lock:
        state.update(changes)
        state["dirty"] = True
        state_lock.notify()


# ───────────── commands ─────────────

def reconnect():
    update(retry=True)


def toggle():
    with state_lock:
        paused = not state["paused"]
    update(paused=paused)
    log("paused: your activity is hidden." if paused else "sharing your activity again.")


COMMANDS = {
    "discord.toggle": ("Pause or Resume Sharing", toggle),
    "discord.reconnect": ("Reconnect", reconnect),
}

if __name__ == "__main__":
    for command, (title, _) in COMMANDS.items():
        send({"type": "registerCommand", "id": command, "title": title, "category": "Discord"})
    threading.Thread(target=worker, daemon=True).start()

    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        kind = message.get("type")
        if kind == "activate":
            update(workspace=message.get("workspace"), start=int(time.time()))
        elif kind == "event":
            name = message.get("name")
            if name == "editorChanged":
                path = message.get("path")
                with state_lock:
                    same = path == state["path"]
                if not same:
                    update(path=path)
            elif name == "folderOpened":
                update(workspace=message.get("path"), path=None, start=int(time.time()))
            elif name == "fileSaved":
                update(retry=True)  # A cheap moment to pick up changed settings.
        elif kind == "executeCommand" and message.get("id") in COMMANDS:
            COMMANDS[message["id"]][1]()
