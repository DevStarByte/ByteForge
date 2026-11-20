"""GitHub Actions module for ByteForge: the workflow runs of the folder's GitHub repository.

The Actions view shows the runs of the current branch and, per workflow, the latest runs of every branch.
A click on a run lists its jobs and steps; a click on a job opens its log in an editor (at the first error).
Right-click a run: Open in Browser, Re-run, Re-run Failed Jobs, Cancel. "Run Workflow…" starts a workflow
that has a workflow_dispatch trigger on the current branch. The status bar shows the latest run of the branch.

Talks to the GitHub REST API directly (no gh needed). Public repositories work without a token (60 requests
an hour); re-running, cancelling, starting workflows and private repositories need one: "github.token" in
settings, the GH_TOKEN or GITHUB_TOKEN environment variable, or a logged-in gh CLI.
"""

import datetime
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import webbrowser

VIEW = "github-actions"
API = "https://api.github.com"
GREEN, YELLOW, RED, GREY, BLUE = "#3fb950", "#d29922", "#f85149", "#8b949e", "#58a6ff"

lock = threading.Lock()


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


# --- repository ---------------------------------------------------------------------------------------------

def parse_remote(url):
    """(owner, repo) of a github.com remote URL (https, ssh or scp-like), else None."""
    m = re.match(r"^(?:https?://(?:[^@/]+@)?github\.com/|ssh://git@github\.com(?::\d+)?/|git@github\.com:)"
                 r"([\w.-]+)/([\w.-]+?)(?:\.git)?/?$", (url or "").strip())
    return (m.group(1), m.group(2)) if m else None


def git(root, *args):
    try:
        result = subprocess.run(["git", *args], cwd=root, capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def find_repository(root):
    """(owner, repo, branch) of the folder: the remote of the branch's upstream, else origin, else any
    github.com remote."""
    if not root:
        return None
    branch = git(root, "symbolic-ref", "--quiet", "--short", "HEAD")  # None when detached
    remotes = (git(root, "remote") or "").split()
    upstream = git(root, "config", f"branch.{branch}.remote") if branch else None
    ordered = [r for r in (upstream, "origin") if r in remotes] + [r for r in remotes if r not in (upstream, "origin")]
    for remote in ordered:
        found = parse_remote(git(root, "remote", "get-url", remote))
        if found:
            return found[0], found[1], branch
    return None


def dispatchable_workflows(root):
    """File names in .github/workflows that have a workflow_dispatch trigger."""
    folder = os.path.join(root, ".github", "workflows")
    names = set()
    try:
        for name in os.listdir(folder):
            if name.endswith((".yml", ".yaml")):
                with open(os.path.join(folder, name), encoding="utf-8", errors="replace") as f:
                    if re.search(r"^\s*workflow_dispatch\s*:|[\[,\s]workflow_dispatch[\],\s]|^on:\s*workflow_dispatch",
                                 f.read(), re.M):
                        names.add(name)
    except OSError:
        pass
    return names


# --- GitHub API ---------------------------------------------------------------------------------------------

class ApiError(Exception):
    def __init__(self, status, message):
        super().__init__(message)
        self.status = status


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def find_token(settings):
    token = (settings.get("github.token") or "").strip()
    if token:
        return token, "the github.token setting"
    for name in ("GH_TOKEN", "GITHUB_TOKEN"):
        if os.environ.get(name):
            return os.environ[name].strip(), name
    if shutil.which("gh"):
        try:
            result = subprocess.run(["gh", "auth", "token", "--hostname", "github.com"], capture_output=True,
                                    text=True, timeout=10)
            if result.returncode == 0 and result.stdout.strip():
                return result.stdout.strip(), "gh"
        except (OSError, subprocess.TimeoutExpired):
            pass
    return None, None


class Api:
    def __init__(self, token=None):
        self.token = token
        self.etags = {}           # url -> (etag, data): 304 answers don't count against the rate limit
        self.remaining = None
        self.reset = None

    def headers(self):
        h = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
             "User-Agent": "ByteForge-GitHub-Actions"}
        if self.token:
            h["Authorization"] = f"Bearer {self.token}"
        return h

    def request(self, method, path, body=None):
        url = path if path.startswith("http") else API + path
        headers = self.headers()
        cached = self.etags.get(url) if method == "GET" else None
        if cached:
            headers["If-None-Match"] = cached[0]
        data = json.dumps(body).encode() if body is not None else None
        if data is not None:
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=20) as response:
                self.note_limits(response.headers)
                raw = response.read()
                result = json.loads(raw) if raw else None
                if method == "GET" and response.headers.get("ETag"):
                    self.etags[url] = (response.headers["ETag"], result)
                return result
        except urllib.error.HTTPError as e:
            with e:
                self.note_limits(e.headers)
                if e.code == 304 and cached:
                    return cached[1]
                raise ApiError(e.code, self.explain(e.code, e.read(), e.headers))
        except (urllib.error.URLError, OSError) as e:
            raise ApiError(0, f"can't reach GitHub: {getattr(e, 'reason', e)}")

    def note_limits(self, headers):
        if headers and headers.get("X-RateLimit-Remaining") is not None:
            self.remaining = int(headers["X-RateLimit-Remaining"])
            self.reset = int(headers.get("X-RateLimit-Reset") or 0)

    def explain(self, status, raw, headers):
        try:
            message = json.loads(raw).get("message") or ""
        except (ValueError, AttributeError):
            message = ""
        if status in (403, 429) and headers.get("X-RateLimit-Remaining") == "0":
            at = datetime.datetime.fromtimestamp(int(headers.get("X-RateLimit-Reset") or 0)).strftime("%H:%M")
            hint = "" if self.token else " Set \"github.token\" (or GH_TOKEN) for 5000 an hour."
            return f"GitHub's rate limit is used up until {at}.{hint}"
        if status == 401:
            return "GitHub didn't accept the token (expired or revoked?)."
        if status == 404 and not self.token:
            return "repository not found: private repositories need a token (\"github.token\", GH_TOKEN or gh)."
        if status == 403 and not self.token:
            return f"{message or 'forbidden'}: this needs a token (\"github.token\", GH_TOKEN or gh)."
        return message or f"HTTP {status}"

    def download(self, path):
        """A job log: the API answers with a redirect to a storage URL, which must be fetched without our token."""
        req = urllib.request.Request(API + path, headers=self.headers())
        opener = urllib.request.build_opener(NoRedirect())
        try:
            with opener.open(req, timeout=20) as response:
                return response.read().decode("utf-8", errors="replace")
        except urllib.error.HTTPError as e:
            with e:
                if e.code not in (301, 302, 303, 307, 308):
                    raise ApiError(e.code, self.explain(e.code, e.read(), e.headers))
                location = e.headers.get("Location")
        except (urllib.error.URLError, OSError) as e:
            raise ApiError(0, f"can't reach GitHub: {getattr(e, 'reason', e)}")
        try:
            with urllib.request.urlopen(urllib.request.Request(location, headers={"User-Agent": "ByteForge"}),
                                        timeout=60) as response:
                return response.read().decode("utf-8", errors="replace")
        except (urllib.error.URLError, OSError) as e:
            raise ApiError(0, f"can't download the log: {getattr(e, 'reason', e)}")


# --- presentation -------------------------------------------------------------------------------------------

def parse_time(value):
    if not value:
        return None
    return datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))


def ago(value, now=None):
    moment = parse_time(value)
    if not moment:
        return ""
    seconds = int(((now or datetime.datetime.now(datetime.timezone.utc)) - moment).total_seconds())
    for unit, size in (("d", 86400), ("h", 3600), ("m", 60)):
        if seconds >= size:
            return f"{seconds // size}{unit} ago"
    return "just now"


def duration(start, end):
    a, b = parse_time(start), parse_time(end)
    if not a:
        return ""
    seconds = int(((b or datetime.datetime.now(datetime.timezone.utc)) - a).total_seconds())
    if seconds < 0:
        return ""
    if seconds >= 3600:
        return f"{seconds // 3600}h {seconds % 3600 // 60}m"
    if seconds >= 60:
        return f"{seconds // 60}m {seconds % 60}s"
    return f"{seconds}s"


def state(item):
    """One of success, failure, cancelled, skipped, running, queued, neutral."""
    status, conclusion = item.get("status"), item.get("conclusion")
    if status in ("queued", "waiting", "pending", "requested"):
        return "queued"
    if status == "in_progress":
        return "running"
    if conclusion in ("success",):
        return "success"
    if conclusion in ("failure", "timed_out", "startup_failure", "action_required"):
        return "failure"
    if conclusion == "cancelled":
        return "cancelled"
    if conclusion == "skipped":
        return "skipped"
    return "neutral"


STATE_ICONS = {"success": ("check", GREEN), "failure": ("error", RED), "cancelled": ("stop", GREY),
               "skipped": ("chevronRight", GREY), "running": ("sync", YELLOW), "queued": ("pause", YELLOW),
               "neutral": ("info", GREY)}
STATE_SYMBOLS = {"success": "✓", "failure": "✗", "cancelled": "⊘", "skipped": "↷", "running": "●", "queued": "○",
                 "neutral": "–"}
STATE_WORDS = {"success": "succeeded", "failure": "failed", "cancelled": "cancelled", "skipped": "skipped",
               "running": "running", "queued": "queued", "neutral": "finished"}


def run_item(run, jobs=None, expanded=False, with_workflow=True):
    s = state(run)
    icon, color = STATE_ICONS[s]
    title = run.get("display_title") or run.get("head_commit", {}).get("message", "").split("\n")[0]
    label = f"{run.get('name')} #{run.get('run_number')}" if with_workflow else f"#{run.get('run_number')} {title}"
    took = duration(run.get("run_started_at") or run.get("created_at"),
                    None if s in ("running", "queued") else run.get("updated_at"))
    parts = [] if with_workflow else [run.get("head_branch") or ""]
    parts += [run.get("event") or "", ago(run.get("created_at")), took]
    actor = (run.get("actor") or {}).get("login")
    tooltip = "\n".join(x for x in [
        f"{run.get('name')} #{run.get('run_number')}: {STATE_WORDS[s]}",
        title,
        f"{run.get('head_branch')} @ {(run.get('head_sha') or '')[:7]}" + (f" by {actor}" if actor else ""),
        "Click: jobs and steps.",
    ] if x)
    menu = [{"id": "open", "label": "Open in Browser"}, {"id": "-"}]
    if s in ("running", "queued"):
        menu.append({"id": "cancel", "label": "Cancel Run"})
    else:
        menu.append({"id": "rerun", "label": "Re-run All Jobs"})
        if s == "failure":
            menu.append({"id": "rerunFailed", "label": "Re-run Failed Jobs"})
    actions = [{"id": "cancel", "icon": "stop", "tooltip": "Cancel Run"}] if s in ("running", "queued") else \
        [{"id": "rerun", "icon": "refresh", "tooltip": "Re-run All Jobs"}]
    item = {"id": f"run:{run['id']}", "label": label, "description": " · ".join(p for p in parts if p),
            "tooltip": tooltip, "icon": icon, "iconColor": color, "actions": actions, "menu": menu}
    if jobs is not None:
        item["children"] = [job_item(j) for j in jobs] or [{"id": f"nojobs:{run['id']}", "label": "No jobs yet",
                                                            "icon": "info", "iconColor": GREY}]
        item["expanded"] = expanded
    return item


def job_item(job):
    s = state(job)
    icon, color = STATE_ICONS[s]
    steps = job.get("steps") or []
    children = []
    for step in steps:
        ss = state(step)
        si, sc = STATE_ICONS[ss]
        children.append({"id": f"step:{job['id']}:{step.get('number')}", "label": step.get("name") or "",
                         "description": duration(step.get("started_at"), step.get("completed_at")),
                         "icon": si, "iconColor": sc, "tooltip": f"{step.get('name')}: {STATE_WORDS[ss]}. Click: the job's log."})
    return {
        "id": f"job:{job['id']}", "label": job.get("name") or "",
        "description": duration(job.get("started_at"), job.get("completed_at")),
        "tooltip": f"{job.get('name')}: {STATE_WORDS[s]}. Click: its log.", "icon": icon, "iconColor": color,
        "expanded": s == "failure", "children": children,
        "actions": [{"id": "log", "icon": "file", "tooltip": "Open Log"}],
        "menu": [{"id": "log", "label": "Open Log"}, {"id": "openJob", "label": "Open in Browser"}],
    }


TIMESTAMP = re.compile(r"^\ufeff?\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z ?", re.M)


def clean_log(text):
    """The log without the timestamp in front of every line; and the 0-based line of its first error."""
    text = TIMESTAMP.sub("", text.replace("\r\n", "\n"))
    first_error = None
    for number, line in enumerate(text.split("\n")):
        if line.startswith("##[error]"):
            first_error = number
            break
    return text, first_error


def cache_dir():
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    else:
        base = os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache")
    return os.path.join(base, "byteforge", "github-actions")


def slug(text):
    return re.sub(r"[^\w.-]+", "-", text).strip("-")[:60] or "job"


# --- module -------------------------------------------------------------------------------------------------

class Actions:
    def __init__(self):
        self.root = None
        self.settings = {}
        self.repo = None            # (owner, repo, branch)
        self.api = Api()
        self.token_source = None
        self.branch_runs = []
        self.all_runs = []
        self.jobs = {}              # run id -> (run updated_at, jobs)
        self.open_runs = set()
        self.error = None
        self.visible = False
        self.pending = {}
        self.next_request = 1
        self.refresh_lock = threading.Lock()
        self.wake = threading.Event()

    # data

    def configure(self):
        token, source = find_token(self.settings)
        if token != self.api.token:
            self.api = Api(token)
        self.token_source = source
        self.repo = find_repository(self.root)

    def slug(self):
        return f"{self.repo[0]}/{self.repo[1]}"

    def fetch(self):
        if not self.repo:
            return
        try:
            base = f"/repos/{self.slug()}/actions/runs"
            self.all_runs = (self.api.request("GET", f"{base}?per_page=40") or {}).get("workflow_runs", [])
            branch = self.repo[2]
            if branch:
                quoted = urllib.request.quote(branch, safe="")
                self.branch_runs = (self.api.request("GET", f"{base}?per_page=10&branch={quoted}") or {}).get("workflow_runs", [])
            else:
                self.branch_runs = []
            self.error = None
            for run_id in list(self.open_runs):
                self.fetch_jobs(run_id)
        except ApiError as e:
            self.error = e.args[0]

    def fetch_jobs(self, run_id):
        run = self.find_run(run_id)
        cached = self.jobs.get(run_id)
        if cached and run and cached[0] == run.get("updated_at") and state(run) not in ("running", "queued"):
            return cached[1]
        data = self.api.request("GET", f"/repos/{self.slug()}/actions/runs/{run_id}/jobs?per_page=100") or {}
        jobs = data.get("jobs", [])
        self.jobs[run_id] = (run.get("updated_at") if run else None, jobs)
        return jobs

    def find_run(self, run_id):
        return next((r for r in self.branch_runs + self.all_runs if r["id"] == run_id), None)

    def find_job(self, job_id):
        for _, jobs in self.jobs.values():
            for job in jobs:
                if job["id"] == job_id:
                    return job
        return None

    def active(self):
        return [r for r in {r["id"]: r for r in self.branch_runs + self.all_runs}.values()
                if state(r) in ("running", "queued")]

    # view

    def tree(self):
        if not self.root:
            return {"type": "setTree", "view": VIEW, "items": [], "message": "Open a folder to see its GitHub Actions."}
        if not self.repo:
            return {"type": "setTree", "view": VIEW, "items": [],
                    "message": "This folder isn't a GitHub repository (no github.com remote)."}
        items = []
        if self.repo[2]:
            children = [self.run_row(r, with_workflow=True) for r in self.branch_runs]
            items.append({"id": "branch", "label": f"Branch {self.repo[2]}", "header": True, "expanded": True,
                          "count": len(children), "children": children or ([] if self.error else [
                              {"id": "none", "label": "No runs on this branch", "icon": "info", "iconColor": GREY}])})
        workflows = {}
        for run in self.all_runs:
            workflows.setdefault(run.get("name") or run.get("path") or "?", []).append(run)
        rows = []
        for name in sorted(workflows, key=str.lower):
            runs = workflows[name]
            s = state(runs[0])
            icon, color = STATE_ICONS[s]
            rows.append({"id": f"wf:{name}", "label": name, "icon": icon, "iconColor": color,
                         "description": f"{STATE_WORDS[s]} {ago(runs[0].get('created_at'))}",
                         "count": len(runs), "children": [self.run_row(r, with_workflow=False) for r in runs[:10]],
                         "menu": [{"id": "openWorkflow", "label": "Open in Browser"}]})
        if rows:
            items.append({"id": "workflows", "label": "Workflows", "header": True, "expanded": True,
                          "count": len(rows), "children": rows})
        actions = [{"id": "dispatch", "icon": "play", "tooltip": "Run Workflow…"},
                   {"id": "browser", "icon": "openPreview", "tooltip": "Open Actions in the Browser"},
                   {"id": "refresh", "icon": "refresh", "tooltip": "Refresh"}]
        message = None
        if not items and not self.error:
            message = f"No workflow runs in {self.slug()} yet."
        error = self.error
        if not error and self.api.remaining is not None and self.api.remaining < 10 and not self.api.token:
            error = (f"{self.api.remaining} GitHub requests left this hour. "
                     "Set \"github.token\" (or GH_TOKEN, or log in with gh) for more.")
        return {"type": "setTree", "view": VIEW, "items": items, "message": message, "error": error,
                "badge": len(self.active()), "actions": actions}

    def run_row(self, run, with_workflow):
        jobs = self.jobs.get(run["id"], (None, None))[1] if run["id"] in self.open_runs else None
        return run_item(run, jobs, expanded=run["id"] in self.open_runs, with_workflow=with_workflow)

    def update_status(self):
        if self.settings.get("github.showStatus") is False or not self.repo or not self.branch_runs:
            send({"type": "status", "id": "github.run", "text": ""})
            return
        latest = self.branch_runs[0]
        s = state(latest)
        send({"type": "status", "id": "github.run", "text": f"{STATE_SYMBOLS[s]} {latest.get('name')}",
              "command": "github.show",
              "tooltip": f"GitHub Actions: {latest.get('name')} #{latest.get('run_number')} on {self.repo[2]} "
                         f"{STATE_WORDS[s]} ({ago(latest.get('created_at'))}). Click: the Actions view."})

    def refresh(self, reconfigure=False):
        with self.refresh_lock:
            if reconfigure:
                self.configure()
            self.fetch()
            send(self.tree())
            self.update_status()

    def refresh_async(self, reconfigure=False):
        threading.Thread(target=self.refresh, args=(reconfigure,), daemon=True).start()

    def poll(self):
        """Refreshes on a timer: often while runs are going, rarely otherwise (or never, with the view hidden
        and the status bar item off)."""
        while True:
            interval = float(self.settings.get("github.refreshSeconds") or 60)
            if not self.api.token:
                interval = max(interval, 180)  # 60 requests an hour without a token; a refresh is two
            if not self.active():
                interval *= 5
            self.wake.wait(interval)
            self.wake.clear()
            if self.repo and (self.visible or self.active() or self.settings.get("github.showStatus") is not False):
                try:
                    self.refresh()
                except Exception as e:  # keep polling
                    log(f"github-actions: {type(e).__name__}: {e}")

    # actions

    def needs_token(self, what):
        if self.api.token:
            return False
        log(f"github-actions: {what} needs a GitHub token: set \"github.token\" in settings (a fine-grained token "
            "with Actions: read and write), or GH_TOKEN, or log in with gh.")
        send({"type": "showView", "id": "output"})
        return True

    def run_action(self, run_id, action):
        run = self.find_run(run_id)
        base = f"/repos/{self.slug()}/actions/runs/{run_id}"
        if action == "open":
            webbrowser.open(run["html_url"] if run else f"https://github.com/{self.slug()}/actions/runs/{run_id}")
            return
        if action in ("click", "expand"):
            if run_id in self.open_runs and action == "click":
                self.open_runs.discard(run_id)
            else:
                self.open_runs.add(run_id)
                try:
                    self.fetch_jobs(run_id)
                except ApiError as e:
                    self.open_runs.discard(run_id)
                    log(f"github-actions: {e}")
            send(self.tree())
            return
        paths = {"rerun": "/rerun", "rerunFailed": "/rerun-failed-jobs", "cancel": "/cancel"}
        words = {"rerun": "Re-running", "rerunFailed": "Re-running the failed jobs of", "cancel": "Cancelling"}
        if action not in paths or self.needs_token(words[action].split()[0].lower()):
            return
        try:
            self.api.request("POST", base + paths[action], {})
            log(f"github-actions: {words[action]} {run.get('name') if run else ''} #{run.get('run_number') if run else run_id}")
        except ApiError as e:
            log(f"github-actions: {e}")
            send({"type": "showView", "id": "output"})
        time.sleep(2)
        self.refresh()

    def open_log(self, job_id):
        job = self.find_job(job_id)
        try:
            text = self.api.download(f"/repos/{self.slug()}/actions/jobs/{job_id}/logs")
        except ApiError as e:
            if e.status in (403, 404) and not self.api.token:
                self.needs_token("Reading logs")
                return
            if e.status == 404:
                log("github-actions: the log isn't there (yet, or any more: logs are kept for 90 days).")
            else:
                log(f"github-actions: {e}")
            send({"type": "showView", "id": "output"})
            return
        text, first_error = clean_log(text)
        folder = cache_dir()
        os.makedirs(folder, exist_ok=True)
        name = f"{job_id}-{slug(job.get('name') if job else 'job')}.log"
        path = os.path.join(folder, name)
        # A new file when the job is still running (an open tab isn't reloaded from disk).
        if job and state(job) in ("running", "queued"):
            path = os.path.join(folder, f"{job_id}-{int(time.time())}-{slug(job.get('name'))}.log")
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        message = {"type": "openFile", "path": path}
        if first_error is not None:
            message.update({"line": first_error, "column": 0})
        send(message)

    def dispatch(self):
        if not self.repo or self.needs_token("Running a workflow"):
            return
        try:
            data = self.api.request("GET", f"/repos/{self.slug()}/actions/workflows?per_page=100") or {}
        except ApiError as e:
            log(f"github-actions: {e}")
            return
        local = dispatchable_workflows(self.root)
        workflows = [w for w in data.get("workflows", []) if w.get("state") == "active"
                     and os.path.basename(w.get("path", "")) in local]
        if not workflows:
            log("github-actions: no active workflow here has a workflow_dispatch trigger.")
            send({"type": "showView", "id": "output"})
            return
        request = self.next_request
        self.next_request += 1
        self.pending[request] = "dispatch"
        send({"type": "quickPick", "requestId": request, "placeholder": f"Run a workflow on {self.repo[2]}",
              "items": [{"label": w["name"], "detail": w["path"], "value": str(w["id"])} for w in workflows]})

    def do_dispatch(self, workflow_id):
        try:
            self.api.request("POST", f"/repos/{self.slug()}/actions/workflows/{workflow_id}/dispatches",
                             {"ref": self.repo[2]})
            log(f"github-actions: started the workflow on {self.repo[2]} (inputs take their defaults)")
        except ApiError as e:
            log(f"github-actions: {e}")
            send({"type": "showView", "id": "output"})
            return
        time.sleep(3)  # the run shows up in the API a moment later
        self.refresh()

    # messages

    def handle(self, message):
        kind = message.get("type")
        if kind == "activate":
            self.root = message.get("workspace")
            self.settings = message.get("settings") or {}
            self.refresh_async(reconfigure=True)
        elif kind == "event":
            name = message.get("name")
            if name == "folderOpened":
                self.root = message.get("path")
                self.branch_runs, self.all_runs, self.jobs, self.open_runs = [], [], {}, set()
                self.refresh_async(reconfigure=True)
            elif name == "settingsChanged":
                self.settings = message.get("settings") or {}
                self.refresh_async(reconfigure=True)
            elif name == "viewShown" and message.get("view") == VIEW:
                self.visible = True
                self.refresh_async(reconfigure=True)
            elif name == "viewHidden" and message.get("view") == VIEW:
                self.visible = False
            elif name == "filesChanged" and self.repo:
                # A checkout changes the branch; check it cheaply (git only) before asking GitHub.
                found = find_repository(self.root)
                if found != self.repo:
                    self.refresh_async(reconfigure=True)
        elif kind == "executeCommand":
            command = message.get("id")
            if command == "github.show":
                send({"type": "showView", "id": VIEW})
            elif command == "github.refresh":
                self.refresh_async(reconfigure=True)
            elif command == "github.dispatch":
                threading.Thread(target=self.dispatch, daemon=True).start()
            elif command == "github.openInBrowser" and self.repo:
                webbrowser.open(f"https://github.com/{self.slug()}/actions")
        elif kind == "quickPickResult":
            if self.pending.pop(message.get("requestId"), None) == "dispatch" and message.get("value"):
                threading.Thread(target=self.do_dispatch, args=(message["value"],), daemon=True).start()
        elif kind == "treeAction":
            item, action = message.get("item"), message.get("action")
            if item is None:
                if action == "refresh":
                    self.refresh_async(reconfigure=True)
                elif action == "dispatch":
                    threading.Thread(target=self.dispatch, daemon=True).start()
                elif action == "browser" and self.repo:
                    webbrowser.open(f"https://github.com/{self.slug()}/actions")
                return
            prefix, _, rest = item.partition(":")
            if prefix == "run":
                threading.Thread(target=self.run_action, args=(int(rest), action), daemon=True).start()
            elif prefix == "job" and action in ("click", "log"):
                threading.Thread(target=self.open_log, args=(int(rest),), daemon=True).start()
            elif prefix == "job" and action == "openJob":
                job = self.find_job(int(rest))
                if job and job.get("html_url"):
                    webbrowser.open(job["html_url"])
            elif prefix == "step" and action == "click":
                threading.Thread(target=self.open_log, args=(int(rest.split(":")[0]),), daemon=True).start()
            elif prefix == "wf" and action == "openWorkflow":
                runs = [r for r in self.all_runs if (r.get("name") or r.get("path")) == rest]
                if runs:
                    webbrowser.open(f"https://github.com/{self.slug()}/actions/workflows/"
                                    f"{os.path.basename(runs[0].get('path', ''))}")


def main():
    for command, title in (("github.show", "Show Actions"), ("github.refresh", "Refresh Actions"),
                           ("github.dispatch", "Run Workflow…"), ("github.openInBrowser", "Open Actions in the Browser")):
        send({"type": "registerCommand", "id": command, "title": title, "category": "GitHub"})
    actions = Actions()
    threading.Thread(target=actions.poll, daemon=True).start()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        try:
            actions.handle(message)
        except Exception as e:  # keep the module alive; say what went wrong
            log(f"github-actions: {type(e).__name__}: {e}")


if __name__ == "__main__":
    main()
