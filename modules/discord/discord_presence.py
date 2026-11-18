"""Discord Rich Presence module for ByteForge.

Talks to the Discord desktop app over its local IPC socket (no Discord library needed) and sets your activity
to what you're doing in ByteForge: the file, its language, the project and the elapsed time.

Discord in a browser has no local socket. With "discord.webBridge" on, the module starts arRPC
(github.com/OpenAsar/arrpc, an open-source stand-in for the desktop app's socket) when no desktop app is
running; the Vencord or Equicord browser extension, with its "WebRichPresence (arRPC)" plugin, passes the
activity on to discord.com. arRPC is installed once into this module's folder (needs Node.js) and stops
with the module.

Settings (in ByteForge's settings.json):
  "discord.clientId":     optional: your own Discord application's ID, to use instead of ByteForge's.
  "discord.showFileName": false shows "Editing a C# file" instead of the file name (default true).
  "discord.showProject":  false hides the project name (default true).
  "discord.largeImage":   an art asset key of your application, or an image URL (default: the ByteForge logo).
  "discord.webBridge":    true: Discord in the browser via arRPC, when the desktop app isn't running (default false).
"""

import json
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path

CLIENT_ID = "1569037494551322826"  # The ByteForge application on Discord.
LOGO = "https://raw.githubusercontent.com/DevStarByte/ByteForge/packages/modules/discord/byteforge.png"
MIN_INTERVAL = 4.0  # Discord allows 5 activity updates per 20 seconds.
RETRY = 15.0
ARRPC_VERSION = "3.6.0"  # Pinned: the bridge runs code from npm, so only this reviewed version.
ARRPC_USER_ID = "1045800378228281345"  # The user arRPC reports in READY (the desktop app reports yours).
WEB_HINT = ("Open Discord in your browser with the Vencord or Equicord extension and its "
            "\"WebRichPresence (arRPC)\" plugin turned on.")

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
        self.is_arrpc = False  # connected to arRPC (Discord in the browser), not the desktop app

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
            user = ((data.get("data") or {}).get("user") or {}) if isinstance(data, dict) else {}
            self.is_arrpc = user.get("id") == ARRPC_USER_ID
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


# ───────────── the web bridge (arRPC) ─────────────

class BridgeError(Exception):
    pass


ANSI = re.compile(r"\x1b\[[0-9;]*m")
# Runs arRPC and exits when its stdin closes, i.e. when this module ends, even if it is killed: no stray
# arRPC keeps the socket and port 1337 after ByteForge is gone.
WRAPPER = ("process.stdin.on('end',()=>process.exit(0));process.stdin.on('error',()=>process.exit(0));"
           "process.stdin.resume();import(require('url').pathToFileURL(process.argv[1]).href);")


class WebBridge:
    def __init__(self, folder):
        self.folder = folder
        self.proc = None
        self.browser = False  # a browser tab with the arRPC plugin is connected
        self.lock = threading.Lock()

    def script(self):
        return self.folder / "node_modules" / "arrpc" / "src" / "index.js"

    def installed_version(self):
        try:
            return json.loads((self.folder / "node_modules" / "arrpc" / "package.json").read_text(encoding="utf-8")).get("version")
        except (OSError, ValueError):
            return None

    def install(self):
        """Installs the pinned arRPC into the module's folder (once; again after a version change)."""
        if self.installed_version() == ARRPC_VERSION and self.script().is_file():
            return
        npm = shutil.which("npm")
        if not npm:
            raise BridgeError("the web bridge needs Node.js (npm wasn't found on PATH; nodejs.org)")
        log(f"installing arRPC {ARRPC_VERSION} for Discord in the browser (once)…")
        self.folder.mkdir(parents=True, exist_ok=True)
        args = [npm, "install", "--prefix", str(self.folder), f"arrpc@{ARRPC_VERSION}", "--ignore-scripts",
                "--no-audit", "--no-fund", "--no-update-notifier", "--omit=dev", "--loglevel=error"]
        # npm's output goes to a file, not a pipe: on Windows a process npm leaves behind can hold a pipe
        # open, and reading it would wait long after the install is done.
        output = self.folder / "install.log"
        try:
            with open(output, "w", encoding="utf-8") as out:
                proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, **hidden_window())
                code = proc.wait(timeout=300)
        except subprocess.TimeoutExpired:
            proc.kill()
            raise BridgeError("installing arRPC took too long (is npm's registry reachable?)")
        except OSError as e:
            raise BridgeError(f"installing arRPC failed: {e}")
        if code != 0 or not self.script().is_file():
            text = output.read_text(encoding="utf-8", errors="replace").strip()
            raise BridgeError(f"installing arRPC failed: {text[-400:] or f'npm exited with {code}'}")
        log("arRPC is installed.")

    def running(self):
        return self.proc is not None and self.proc.poll() is None

    def start(self):
        with self.lock:
            if self.running():
                return
            self.install()
            node = shutil.which("node")
            if not node:
                raise BridgeError("the web bridge needs Node.js (node wasn't found on PATH; nodejs.org)")
            env = dict(os.environ, ARRPC_NO_PROCESS_SCANNING="1")  # it would look for running games otherwise
            try:
                self.proc = subprocess.Popen([node, "-e", WRAPPER, str(self.script())], cwd=str(self.folder), env=env,
                                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                             text=True, encoding="utf-8", errors="replace", **hidden_window())
            except OSError as e:
                raise BridgeError(f"could not start arRPC: {e}")
            self.browser = False
            threading.Thread(target=self.follow, args=(self.proc,), daemon=True).start()
            log("started arRPC, the bridge to Discord in the browser.")

    def follow(self, proc):
        """Reads arRPC's log: whether a browser is connected, and why it failed if it does."""
        for line in proc.stdout:
            text = ANSI.sub("", line).strip()
            if "web connected" in text:
                self.browser = True
                update()
            elif "web disconnected" in text:
                self.browser = False
                update()
            elif "EADDRINUSE" in text or "Error" in text:
                log(f"arRPC: {text}")
        if proc is self.proc:
            self.browser = False
            log(f"arRPC stopped (exit code {proc.wait()}).")
            update(retry=True)

    def stop(self):
        with self.lock:
            if self.running():
                try:
                    self.proc.stdin.close()  # the wrapper exits on this
                    self.proc.wait(timeout=3)
                except (OSError, subprocess.TimeoutExpired):
                    self.proc.kill()
            self.proc = None


def hidden_window():
    """No console window flashing up on Windows for npm and node."""
    return {"creationflags": 0x08000000} if os.name == "nt" else {}


bridge = WebBridge(Path(__file__).resolve().parent / ".arrpc")


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
        web = settings.get("discord.webBridge") is True
        if ipc and connected_id != client_id:
            ipc.close()
            ipc = None
        if not web and bridge.running():
            # Turned off: stop the bridge (and leave it, if that's what we're connected to).
            if ipc and ipc.is_arrpc:
                ipc.close()
                ipc = None
            bridge.stop()
            log("stopped arRPC (\"discord.webBridge\" is off).")
        if ipc is None:
            now = time.time()
            if not wake_retry and now - last_try < RETRY:
                continue
            last_try = now
            candidate = Ipc(client_id)
            try:
                connected = candidate.connect()
                if not connected and web:
                    connected = connect_through_bridge(candidate)
                if not connected:
                    if not web:
                        status("Discord: offline", "The Discord app isn't running. ByteForge connects when it starts. "
                               "Discord in the browser: turn on \"discord.webBridge\" in the settings.", "discord.reconnect")
                    continue
            except BridgeError as e:
                if str(e) != last_error:
                    log(f"{e}.")
                    last_error = str(e)
                status("Discord: web bridge failed", f"{e}. Click to retry.", "discord.reconnect")
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
            elif ipc.is_arrpc and bridge.running() and not bridge.browser:
                status("Discord: open it in the browser", f"The web bridge (arRPC) is running, but no browser is connected. {WEB_HINT}", "discord.reconnect")
            else:
                status("", None)  # Connected and sharing: nothing to say in the status bar.
        except (OSError, DiscordError, ValueError) as e:
            log(f"lost the connection ({e}); retrying.")
            ipc.close()
            ipc = None
            with state_lock:
                state["dirty"] = True


def connect_through_bridge(candidate):
    """No desktop app: start arRPC (installing it the first time) and connect to its socket."""
    status("Discord: starting the web bridge…", "Starting arRPC for Discord in the browser.", None)
    bridge.start()
    for _ in range(40):  # arRPC opens its socket within a second or two
        if candidate.connect():
            return True
        if not bridge.running():
            break
        time.sleep(0.25)
    raise BridgeError("arRPC started but its socket didn't open")


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
            elif name in ("fileSaved", "settingsChanged"):
                update(retry=True)  # Picks up changed settings (the web bridge turned on or off, …).
        elif kind == "executeCommand" and message.get("id") in COMMANDS:
            COMMANDS[message["id"]][1]()

    # ByteForge closed the module: the bridge goes too (it would also exit on its own once we're gone).
    bridge.stop()
