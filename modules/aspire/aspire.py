"""Aspire module for ByteForge.

A side bar view with the workspace's Aspire AppHosts and, while one runs, its resources (state, health, endpoints)
with start / stop / restart and logs, plus commands for the rest of the `aspire` CLI: new, init, add, update,
restore, publish, deploy and doctor.

Everything goes through the `aspire` CLI, the way the VS Code extension does it: `aspire ps --format json` lists the
running AppHosts, `aspire describe --format json --apphost <path>` their resources, and
`aspire resource <name> <command> --non-interactive --apphost <path>` runs a resource command. `aspire run` and the
other interactive commands run in an IDE terminal. Running AppHosts are polled every few seconds, but only while the
view is shown.
"""

import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import webbrowser

VIEW = "aspire"
POLL_SECONDS = 5
MAX_FILES = 100_000
SKIP_DIRS = {".git", "bin", "obj", "node_modules", ".vs", ".idea", "target", ".aspire"}
PURPLE = "#7455dd"
RUNNING_COLOR = "#3fb950"
WARNING_COLOR = "#d29922"
ERROR_COLOR = "#f85149"
IDLE_COLOR = "#8b949e"
INSTALL_SCRIPT = "curl -sSL https://aspire.dev/install.sh | bash"
INSTALL_SCRIPT_WINDOWS = 'powershell -NoProfile -Command "iex \\"& { $(irm https://aspire.dev/install.ps1) }\\""'

# AppHosts that aren't projects: a file of one of these names (TypeScript, JavaScript, Go, Rust, Java and
# file-based C# AppHosts).
FILE_APPHOSTS = {"apphost.cs", "apphost.ts", "apphost.mts", "apphost.cts", "apphost.js", "apphost.mjs",
                 "apphost.cjs", "apphost.go", "apphost.rs", "apphost.java"}

RUNNING_STATES = {"Running", "Active"}
BUSY_STATES = {"Starting", "Stopping", "Building", "Waiting"}
STOPPED_STATES = {"Finished", "Exited", "Stopped"}


class AspireError(Exception):
    pass


# ───────────── the CLI ─────────────

def find_cli(settings):
    """The aspire executable: the `aspire.cliPath` setting, PATH, or where the install script puts it."""
    configured = settings.get("aspire.cliPath") if isinstance(settings, dict) else None
    if isinstance(configured, str) and configured.strip():
        path = os.path.expanduser(configured.strip())
        return path if os.path.isfile(path) else None
    found = shutil.which("aspire")
    if found:
        return found
    home = os.path.expanduser("~")
    exe = "aspire.exe" if os.name == "nt" else "aspire"
    for candidate in (os.path.join(home, ".aspire", "bin", exe), os.path.join(home, ".dotnet", "tools", exe)):
        if os.path.isfile(candidate):
            return candidate
    return None


def run_cli(cli, args, timeout=60):
    try:
        done = subprocess.run([cli] + args, capture_output=True, text=True, timeout=timeout,
                              stdin=subprocess.DEVNULL, env=cli_environment())
    except FileNotFoundError:
        raise AspireError("the aspire CLI isn't installed")
    except subprocess.TimeoutExpired:
        raise AspireError(f"aspire {args[0]} timed out")
    if done.returncode != 0:
        text = (done.stderr.strip() or done.stdout.strip() or f"exit code {done.returncode}")
        raise AspireError(text.splitlines()[-1] if text else "failed")
    return done.stdout


def cli_environment():
    env = dict(os.environ)
    env.setdefault("ASPIRE_CLI_TELEMETRY_OPTOUT", "1")
    env["NO_COLOR"] = "1"
    return env


def find_json(text):
    """The JSON document in the CLI's output (it can print a banner or status lines around it)."""
    text = text.strip()
    for opener in ("[", "{"):
        start = text.find(opener)
        while start != -1:
            try:
                value, _ = json.JSONDecoder().raw_decode(text[start:])
                return value
            except ValueError:
                start = text.find(opener, start + 1)
    raise AspireError("couldn't read the aspire CLI's output")


def parse_ps(text):
    """Running AppHosts from `aspire ps --format json`: path, pid and dashboard URL."""
    if not text.strip():
        return []
    value = find_json(text)
    if isinstance(value, dict):
        value = [value]
    hosts = []
    for entry in value if isinstance(value, list) else []:
        if not isinstance(entry, dict) or not isinstance(entry.get("appHostPath"), str):
            continue
        hosts.append({"path": entry["appHostPath"], "pid": entry.get("appHostPid"),
                      "dashboard": entry.get("dashboardUrl") if isinstance(entry.get("dashboardUrl"), str) else None,
                      "status": entry.get("status") if isinstance(entry.get("status"), str) else None})
    return hosts


def parse_resources(text):
    """Resources from `aspire describe --format json`, sorted by name."""
    value = find_json(text)
    resources = value.get("resources") if isinstance(value, dict) else value
    result = [r for r in resources or [] if isinstance(r, dict) and isinstance(r.get("name"), str)]
    return sorted(result, key=lambda r: display_name(r).lower())


def display_name(resource):
    name = resource.get("displayName")
    return name if isinstance(name, str) and name else resource["name"]


def command_enabled(resource, name):
    """Whether the resource offers a command (`start`, `stop`, `restart`) that's visible and enabled now."""
    commands = resource.get("commands")
    if not isinstance(commands, dict):
        return False
    command = commands.get(name) or commands.get(f"resource-{name}")
    if not isinstance(command, dict):
        return False
    if (command.get("visibility") or "Visible") == "Hidden":
        return False
    return (command.get("state") or "Enabled") == "Enabled"


def resource_status(resource):
    """'running', 'busy', 'warning', 'error' or 'idle': how the resource's icon looks."""
    state = resource.get("state")
    style = resource.get("stateStyle")
    health = resource.get("healthStatus")
    exit_code = resource.get("exitCode")
    if state in RUNNING_STATES:
        if style == "error":
            return "error"
        if health in ("Unhealthy", "Degraded") or style == "warning":
            return "warning"
        return "running"
    if state in BUSY_STATES:
        return "busy"
    if state in STOPPED_STATES:
        return "error" if style == "error" or (exit_code not in (None, 0)) else "idle"
    if state == "FailedToStart":
        return "error" if exit_code not in (None, 0) else "warning"
    if state in ("RuntimeUnhealthy", "ValueMissing"):
        return "warning"
    return "idle"


def resource_urls(resource):
    """The resource's public endpoints as (label, url)."""
    urls = []
    for entry in resource.get("urls") or []:
        if not isinstance(entry, dict) or entry.get("isInternal") or not isinstance(entry.get("url"), str):
            continue
        label = entry.get("displayName") or entry.get("name") or entry["url"]
        urls.append((label, entry["url"]))
    return urls


# ───────────── the workspace ─────────────

def _read_text(path, limit=200_000):
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read(limit)


def is_apphost_project(text):
    """Whether a .csproj is an Aspire AppHost (the AppHost SDK, or the older IsAspireHost property)."""
    return "Aspire.AppHost.Sdk" in text or "<IsAspireHost>true</IsAspireHost>" in text.replace(" ", "")


def is_file_apphost(name, text, in_project_dir):
    """Whether a file is an AppHost on its own (`apphost.ts`, a file-based `apphost.cs`, …)."""
    lower = name.lower()
    if lower not in FILE_APPHOSTS and name != "AppHost.java":
        return False
    if lower == "apphost.cs":
        # In a project folder AppHost.cs is the project's source; a file-based one names the SDK itself.
        return not in_project_dir and "Aspire.AppHost.Sdk" in text
    return "aspire" in text.lower()


def find_apphosts(root):
    """The AppHosts under `root`, as paths (project files, or the AppHost file itself)."""
    hosts = []
    if not root or not os.path.isdir(root):
        return hosts
    seen = 0
    for directory, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS and not d.startswith("."))
        seen += len(files)
        if seen > MAX_FILES:
            break
        projects = [f for f in files if f.endswith(".csproj")]
        for name in sorted(files):
            path = os.path.join(directory, name)
            try:
                if name.endswith(".csproj"):
                    if is_apphost_project(_read_text(path)):
                        hosts.append(path)
                elif name.lower() in FILE_APPHOSTS or name == "AppHost.java":
                    if is_file_apphost(name, _read_text(path), bool(projects)):
                        hosts.append(path)
            except OSError:
                continue
    return hosts


def path_key(path):
    path = os.path.normpath(os.path.abspath(path))
    return path.lower() if os.name == "nt" else path


def apphost_label(path):
    """What an AppHost is called in the view: the project name, or the folder of an AppHost file."""
    name = os.path.basename(path)
    if name.lower().endswith(".csproj"):
        return name[:-len(".csproj")]
    return os.path.basename(os.path.dirname(path)) or name


def source_file(path):
    """The file to open for an AppHost: AppHost.cs (or Program.cs) next to a project, else the file itself."""
    if not path.lower().endswith(".csproj"):
        return path
    directory = os.path.dirname(path)
    for name in ("AppHost.cs", "apphost.cs", "Program.cs"):
        candidate = os.path.join(directory, name)
        if os.path.isfile(candidate):
            return candidate
    return path


def relative(path, base):
    if base:
        try:
            rel = os.path.relpath(path, base)
            if not rel.startswith(".."):
                return rel
        except ValueError:
            pass
    return path


SAFE_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./:=@%+,")


def shell_quote(arg):
    if arg and all(c in SAFE_CHARS for c in arg):
        return arg
    if os.name == "nt":
        return '"' + arg.replace('"', '\\"') + '"'
    return "'" + arg.replace("'", "'\\''") + "'"


def command_line(args):
    return " ".join(shell_quote(a) for a in args)


# ───────────── protocol ─────────────

output_lock = threading.Lock()


def send(message):
    with output_lock:
        try:
            print(json.dumps(message), flush=True)
        except (OSError, ValueError):
            pass  # the IDE went away


def log(text):
    send({"type": "log", "text": text})


STATUS_COLORS = {"running": RUNNING_COLOR, "busy": WARNING_COLOR, "warning": WARNING_COLOR,
                 "error": ERROR_COLOR, "idle": IDLE_COLOR}
STATUS_ICONS = {"running": "check", "busy": "sync", "warning": "warning", "error": "error", "idle": "stop"}


class Module:
    def __init__(self):
        self.lock = threading.RLock()
        self.workspace = None
        self.settings = {}
        self.visible = False
        self.cli = None          # path, False when it isn't installed, None before the first check
        self.error = None        # why `aspire ps` failed
        self.apphosts = []       # paths found in the workspace
        self.running = {}        # path_key -> {"path", "pid", "dashboard", "resources", "error"}
        self.busy = None
        self.next_request = 1
        self.pending = {}
        self.refresh_wanted = threading.Event()
        self.last_tree = None
        self.work = queue.Queue()

    # ── state → tree ──

    def all_apphosts(self):
        """Workspace AppHosts first, then running ones from elsewhere, as (path, running info or None)."""
        with self.lock:
            hosts = [(p, self.running.get(path_key(p))) for p in self.apphosts]
            known = {path_key(p) for p in self.apphosts}
            hosts += [(r["path"], r) for k, r in sorted(self.running.items()) if k not in known]
            return hosts

    def resource_item(self, host, resource):
        status = resource_status(resource)
        name = display_name(resource)
        state = resource.get("state") or "Unknown"
        description = state if not resource.get("resourceType") else f"{state} · {resource['resourceType']}"
        health = resource.get("healthStatus")
        tooltip = [f"{name} ({resource.get('resourceType') or 'resource'})", f"State: {state}"]
        if health:
            tooltip.append(f"Health: {health}")
        if resource.get("exitCode") not in (None, 0):
            tooltip.append(f"Exit code: {resource['exitCode']}")
        key = f"res:{host}\n{resource['name']}"
        actions = []
        if command_enabled(resource, "start"):
            actions.append({"id": "start", "icon": "play", "tooltip": "Start"})
        if command_enabled(resource, "restart"):
            actions.append({"id": "restart", "icon": "refresh", "tooltip": "Restart"})
        if command_enabled(resource, "stop"):
            actions.append({"id": "stop", "icon": "stop", "tooltip": "Stop"})
        actions.append({"id": "logs", "icon": "terminal", "tooltip": "View Logs"})
        children = [{"id": f"url:{url}\n{key}", "label": label, "description": url, "tooltip": f"Open {url}",
                     "icon": "openPreview"} for label, url in resource_urls(resource)]
        item = {"id": key, "label": name, "description": description, "tooltip": "\n".join(tooltip),
                "icon": STATUS_ICONS[status], "iconColor": STATUS_COLORS[status], "actions": actions}
        if children:
            item["children"] = children
            item["expanded"] = False
        return item

    def apphost_item(self, path, info):
        label = apphost_label(path)
        rel = relative(path, self.workspace)
        if info is None:
            return {"id": f"host:{path}", "label": label, "description": rel, "tooltip": f"{path}\nNot running",
                    "icon": "layers", "iconColor": PURPLE,
                    "actions": [{"id": "run", "icon": "play", "tooltip": "Run AppHost"}]}
        children = []
        if info.get("dashboard"):
            children.append({"id": f"dash:{path}", "label": "Dashboard", "description": info["dashboard"],
                             "tooltip": "Open the Aspire Dashboard", "icon": "openPreview", "iconColor": PURPLE})
        resources = info.get("resources") or []
        if info.get("error"):
            children.append({"id": f"err:{path}", "label": "Couldn't read the resources", "icon": "warning",
                             "iconColor": WARNING_COLOR, "tooltip": info["error"]})
        elif not resources:
            children.append({"id": f"wait:{path}", "label": "Waiting for resources…", "icon": "sync"})
        children += [self.resource_item(path, r) for r in resources]
        running = sum(1 for r in resources if resource_status(r) == "running")
        actions = []
        if info.get("dashboard"):
            actions.append({"id": "dashboard", "icon": "openPreview", "tooltip": "Open Dashboard"})
        actions.append({"id": "stop", "icon": "stop", "tooltip": "Stop AppHost"})
        return {"id": f"host:{path}", "label": label,
                "description": f"running · {running}/{len(resources)} up" if resources else "running",
                "tooltip": f"{path}\nRunning (pid {info.get('pid')})", "icon": "layers",
                "iconColor": RUNNING_COLOR, "count": len(resources), "expanded": True,
                "children": children, "actions": actions}

    def tree(self):
        with self.lock:
            items = []
            if self.cli is False:
                items.append({"id": "section:setup", "label": "SETUP", "header": True, "count": 1, "children": [
                    {"id": "setup:install", "label": "Install the Aspire CLI", "icon": "add", "iconColor": PURPLE,
                     "tooltip": "Runs the official install script (aspire.dev) in a terminal",
                     "actions": [{"id": "install", "icon": "play", "tooltip": "Install"}]}]})
            hosts = self.all_apphosts()
            children = [self.apphost_item(p, info) for p, info in hosts] or [
                {"id": "note:none", "label": "No AppHost here: run \"Aspire: New Aspire Project…\" or "
                                            "\"Aspire: Initialize Aspire in This Folder\""}]
            items.append({"id": "section:apphosts", "label": "APP HOSTS", "header": True, "count": len(hosts),
                          "children": children})
            running = len(self.running)
            if self.busy:
                message = f"aspire {self.busy}…"
            elif self.cli is None:
                message = "Looking for the Aspire CLI…"
            elif self.cli is False:
                message = "The Aspire CLI isn't installed. See SETUP below."
            elif self.error:
                message = "Couldn't list the running AppHosts."
            else:
                message = None
            tree = {"type": "setTree", "view": VIEW, "items": items, "badge": running,
                    "actions": [{"id": "new", "icon": "add", "tooltip": "New Aspire Project…"},
                                {"id": "refresh", "icon": "refresh", "tooltip": "Refresh"}]}
            if message:
                tree["message"] = message
            if self.error:
                tree["error"] = self.error
            return tree

    def publish(self, force=False):
        tree = self.tree()
        text = json.dumps(tree, sort_keys=True)
        with self.lock:
            if not force and text == self.last_tree:
                return
            self.last_tree = text
        send(tree)

    # ── refreshing ──

    def refresh_now(self, rescan=True):
        with self.lock:
            if self.busy:
                return
        if rescan:
            hosts = find_apphosts(self.workspace)
            with self.lock:
                self.apphosts = hosts
        cli = find_cli(self.settings) or False
        running, error = {}, None
        if cli:
            try:
                for host in parse_ps(run_cli(cli, ["ps", "--format", "json"], timeout=30)):
                    try:
                        host["resources"] = parse_resources(
                            run_cli(cli, ["describe", "--format", "json", "--apphost", host["path"]], timeout=30))
                        host["error"] = None
                    except AspireError as e:
                        host["resources"], host["error"] = [], str(e)
                    running[path_key(host["path"])] = host
            except AspireError as e:
                error = str(e)
        with self.lock:
            self.cli, self.running, self.error = cli, running, error
        self.publish()

    def refresher(self):
        rescans = 0
        while True:
            asked = self.refresh_wanted.wait(POLL_SECONDS)
            self.refresh_wanted.clear()
            if asked or self.visible:
                try:
                    # The workspace scan is cheap but not free: on a timer only every sixth poll.
                    self.refresh_now(rescan=asked or rescans % 6 == 0)
                    rescans += 1
                except Exception as error:  # never let the thread die
                    log(f"aspire: refresh failed: {error}")

    def request_refresh(self):
        self.refresh_wanted.set()

    # ── requests to the IDE ──

    def ask(self, message, callback):
        with self.lock:
            request = self.next_request
            self.next_request += 1
            self.pending[request] = callback
        message["requestId"] = request
        send(message)

    def quick_pick(self, placeholder, items, on_pick):
        self.ask({"type": "quickPick", "placeholder": placeholder, "items": items},
                 lambda value: on_pick(value) if value is not None else None)

    def answer(self, request, value):
        with self.lock:
            callback = self.pending.pop(request, None)
        if callback:
            callback(value)

    def terminal(self, title, args, cwd=None, save_all=False):
        cli = self.require_cli()
        if not cli:
            return
        message = {"type": "runInTerminal", "title": title, "command": command_line([cli] + args),
                   "cwd": cwd or self.workspace or os.path.expanduser("~")}
        if save_all:
            message["saveAll"] = True
        send(message)

    def require_cli(self):
        cli = find_cli(self.settings)
        if not cli:
            log("aspire: the Aspire CLI isn't installed. Run \"Aspire: Install the Aspire CLI\" "
                "or set \"aspire.cliPath\".")
            send({"type": "showView", "id": "output"})
            with self.lock:
                self.cli = False
            self.publish()
        return cli

    # ── operations ──

    def run_op(self, label, args, then=None):
        """Runs a quick aspire command in the background, logs the result and refreshes."""
        cli = self.require_cli()
        if not cli:
            return
        with self.lock:
            if self.busy:
                return
            self.busy = label
        self.publish()

        def work():
            try:
                out = run_cli(cli, args, timeout=120).strip()
                log(f"aspire {label}: done" if not out else f"aspire {label}: {out.splitlines()[-1]}")
            except AspireError as error:
                log(f"aspire {label} failed: {error}")
            finally:
                with self.lock:
                    self.busy = None
                self.publish()
                self.request_refresh()

        threading.Thread(target=work, daemon=True).start()

    def run_apphost(self, path):
        self.terminal(f"aspire run: {apphost_label(path)}", ["run", "--apphost", path],
                      cwd=os.path.dirname(path), save_all=True)
        self.request_refresh()

    def stop_apphost(self, path):
        self.run_op(f"stop {apphost_label(path)}", ["stop", "--apphost", path])

    def resource_command(self, host, resource, command):
        self.run_op(f"{command} {resource}",
                    ["resource", resource, command, "--non-interactive", "--apphost", host])

    def open_url(self, url):
        threading.Thread(target=webbrowser.open, args=(url,), daemon=True).start()

    def pick_apphost(self, placeholder, on_pick, running_only=False):
        hosts = [(p, info) for p, info in self.all_apphosts() if info or not running_only]
        if not hosts:
            log("aspire: no running AppHost." if running_only else
                "aspire: no AppHost in this folder. Run \"Aspire: New Aspire Project…\" or "
                "\"Aspire: Initialize Aspire in This Folder\".")
            send({"type": "showView", "id": "output"})
            return
        if len(hosts) == 1:
            on_pick(hosts[0][0])
            return
        self.quick_pick(placeholder, [{"label": apphost_label(p), "detail": relative(p, self.workspace),
                                       "hint": "running" if info else "", "value": p} for p, info in hosts], on_pick)

    def ensure_known(self):
        """Makes sure the AppHosts are known before a command that needs one (the view may never have shown)."""
        with self.lock:
            known = self.cli is not None
        if not known:
            self.refresh_now()

    # ── tree actions ──

    def tree_action(self, item, action):
        if item is None:
            if action == "refresh":
                self.request_refresh()
            elif action == "new":
                self.execute("aspire.new")
            return
        if not isinstance(item, str):
            return
        kind, _, rest = item.partition(":")
        if kind == "setup":
            self.install_cli()
        elif kind == "host":
            with self.lock:
                info = self.running.get(path_key(rest))
            if action == "run":
                self.run_apphost(rest)
            elif action == "stop":
                self.stop_apphost(rest)
            elif action == "dashboard" and info and info.get("dashboard"):
                self.open_url(info["dashboard"])
            elif action == "click":
                send({"type": "openFile", "path": source_file(rest)})
        elif kind == "dash":
            with self.lock:
                info = self.running.get(path_key(rest))
            if info and info.get("dashboard"):
                self.open_url(info["dashboard"])
        elif kind == "res":
            host, _, resource = rest.partition("\n")
            if action in ("start", "stop", "restart"):
                self.resource_command(host, resource, action)
            elif action == "logs":
                self.terminal(f"aspire logs: {resource}", ["logs", resource, "--apphost", host],
                              cwd=os.path.dirname(host))
        elif kind == "url":
            url, _, _ = rest.partition("\n")
            self.open_url(url)

    # ── commands ──

    def install_cli(self):
        script = INSTALL_SCRIPT_WINDOWS if os.name == "nt" else INSTALL_SCRIPT
        send({"type": "runInTerminal", "title": "Install the Aspire CLI", "command": script,
              "cwd": os.path.expanduser("~")})
        log("aspire: after the install finishes, open a new terminal or restart ByteForge if `aspire` "
            "isn't on PATH yet (it goes to ~/.aspire/bin, which this module also checks).")

    def with_apphost(self, args, title):
        """A terminal command against one AppHost (`--apphost` when there's one to pick, else the CLI asks)."""
        self.ensure_known()
        if not self.all_apphosts():
            self.terminal(title, args)
            return
        self.pick_apphost("Choose an AppHost", lambda p: self.terminal(
            f"{title}: {apphost_label(p)}", args + ["--apphost", p], cwd=os.path.dirname(p), save_all=True))

    def execute(self, command):
        if command == "aspire.refresh":
            self.request_refresh()
        elif command == "aspire.run":
            self.ensure_known()
            self.pick_apphost("Choose the AppHost to run", self.run_apphost)
        elif command == "aspire.stop":
            self.ensure_known()
            self.pick_apphost("Choose the AppHost to stop", self.stop_apphost, running_only=True)
        elif command == "aspire.openDashboard":
            self.ensure_known()

            def open_dashboard(path):
                with self.lock:
                    info = self.running.get(path_key(path))
                if info and info.get("dashboard"):
                    self.open_url(info["dashboard"])
                else:
                    log("aspire: that AppHost has no dashboard URL yet.")
            self.pick_apphost("Choose the AppHost", open_dashboard, running_only=True)
        elif command == "aspire.new":
            self.terminal("aspire new", ["new"])
        elif command == "aspire.init":
            self.terminal("aspire init", ["init"])
        elif command == "aspire.add":
            self.with_apphost(["add"], "aspire add")
        elif command == "aspire.update":
            self.with_apphost(["update"], "aspire update")
        elif command == "aspire.restore":
            self.with_apphost(["restore"], "aspire restore")
        elif command == "aspire.publish":
            self.with_apphost(["publish"], "aspire publish")
        elif command == "aspire.deploy":
            self.with_apphost(["deploy"], "aspire deploy")
        elif command == "aspire.doctor":
            self.terminal("aspire doctor", ["doctor"])
        elif command == "aspire.installCli":
            self.install_cli()
        elif command == "aspire.updateCli":
            self.terminal("aspire update --self", ["update", "--self"])

    # ── messages from the IDE ──

    def handle(self, message):
        kind = message.get("type")
        if kind == "activate":
            self.workspace = message.get("workspace") or None
            if isinstance(message.get("settings"), dict):
                self.settings = message["settings"]
            self.publish(force=True)
            self.request_refresh()
        elif kind == "executeCommand":
            self.later(self.execute, message.get("id"))
        elif kind == "treeAction":
            if message.get("view") == VIEW:
                self.later(self.tree_action, message.get("item"), message.get("action"))
        elif kind == "quickPickResult":
            value = message.get("value")
            if isinstance(value, str):
                self.later(self.answer, message.get("requestId"), value)
        elif kind == "event":
            self.event(message)

    def event(self, message):
        name = message.get("name")
        if name == "viewShown" and message.get("view") == VIEW:
            self.visible = True
            self.request_refresh()
        elif name == "viewHidden" and message.get("view") == VIEW:
            self.visible = False
        elif name == "folderOpened":
            self.workspace = message.get("path") or None
            with self.lock:
                self.apphosts = []
            self.request_refresh()
        elif name == "filesChanged":
            if self.visible:
                self.request_refresh()
        elif name == "settingsChanged":
            if isinstance(message.get("settings"), dict):
                self.settings = message["settings"]
                self.request_refresh()

    def later(self, function, *args):
        self.work.put((function, args))

    def worker(self):
        while True:
            function, args = self.work.get()
            self.safely(function, *args)

    def safely(self, function, *args):
        try:
            function(*args)
        except Exception as error:  # a bug here must never take the module down
            log(f"aspire: {error}")


COMMANDS = [
    ("aspire.run", "Run AppHost…"),
    ("aspire.stop", "Stop AppHost…"),
    ("aspire.openDashboard", "Open Dashboard"),
    ("aspire.new", "New Aspire Project…"),
    ("aspire.init", "Initialize Aspire in This Folder"),
    ("aspire.add", "Add an Integration…"),
    ("aspire.update", "Update Integrations"),
    ("aspire.restore", "Restore Packages"),
    ("aspire.publish", "Publish Deployment Artifacts"),
    ("aspire.deploy", "Deploy App"),
    ("aspire.doctor", "Check Environment (aspire doctor)"),
    ("aspire.installCli", "Install the Aspire CLI"),
    ("aspire.updateCli", "Update the Aspire CLI"),
    ("aspire.refresh", "Refresh"),
]


def main():
    module = Module()
    for command, title in COMMANDS:
        send({"type": "registerCommand", "id": command, "title": title, "category": "Aspire"})
    threading.Thread(target=module.refresher, daemon=True).start()
    threading.Thread(target=module.worker, daemon=True).start()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if isinstance(message, dict):
            module.safely(module.handle, message)


if __name__ == "__main__":
    main()
