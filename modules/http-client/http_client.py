"""HTTP Client module for ByteForge: sends the requests in .http / .rest files.

The file format is the one Visual Studio, Rider and VS Code's REST Client share:

    @host = https://localhost:5001

    ### Log in
    # @name login
    POST {{host}}/api/login
    Content-Type: application/json

    { "user": "admin", "password": "{{$processEnv API_PASSWORD}}" }

    ###
    GET {{host}}/api/orders?page=1
    Authorization: Bearer {{login.response.body.$.token}}

Requests are separated by ###; the text after ### or a "# @name" comment names a request. Variables:
file variables (@name = value), environments from http-client.env.json / http-client.private.env.json
(next to the .http file or in a folder above it, up to the workspace root; "$shared" applies to all),
the responses of named requests ({{login.response.body.$.token}}, {{login.response.headers.Location}})
and the dynamic ones: {{$guid}}, {{$randomInt 1 100}}, {{$timestamp}}, {{$isoTimestamp}},
{{$datetime iso8601|rfc1123|"%Y-%m-%d"}}, {{$localDatetime …}}, {{$processEnv NAME}}, {{$dotenv NAME}}.
A body of "< ./file.json" sends that file.

Each response opens in an editor (a new file in the cache folder), with its status line, headers and body
(JSON pretty-printed). Requests are read from the saved file.
"""

import datetime
import json
import os
import random
import re
import ssl
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

VIEW = "http"
EXTENSIONS = (".http", ".rest")
METHODS = ("GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT")
SKIP_DIRS = {"node_modules", "bin", "obj", ".git", "target", "dist", ".vs", ".idea", "TestResults"}
KEEP_RESPONSES = 100
MAX_PRETTY = 5_000_000

lock = threading.Lock()


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


# --- parsing ------------------------------------------------------------------------------------------------

REQUEST_LINE = re.compile(r"^(" + "|".join(METHODS) + r")\s+(\S.*?)(?:\s+HTTP/\d(?:\.\d)?)?\s*$", re.I)
URL_ONLY = re.compile(r"^(https?://\S.*?|\{\{[^}]+\}\}\S*)(?:\s+HTTP/\d(?:\.\d)?)?\s*$", re.I)
FILE_VARIABLE = re.compile(r"^@([A-Za-z_][\w.-]*)\s*=\s?(.*)$")
NAME_COMMENT = re.compile(r"^(?:#|//)\s*@name\s*=?\s*(\S+)")
HEADER = re.compile(r"^([!#$%&'*+.^_`|~0-9A-Za-z-]+)\s*:\s?(.*)$")


def is_comment(line):
    s = line.lstrip()
    return s.startswith("#") or s.startswith("//")


def parse(text):
    """Returns (file variables {name: raw value}, requests). A request: {name, title, method, url, headers
    [(name, value)], body (str or None), bodyFile (path or None), line (0-based line of the request line)}."""
    variables = {}
    requests = []
    lines = text.splitlines()
    blocks = []  # (title from ###, start index, lines)
    title, start, block = None, 0, []
    for i, line in enumerate(lines):
        if line.startswith("###"):
            blocks.append((title, start, block))
            title, start, block = line[3:].strip() or None, i + 1, []
        else:
            block.append(line)
    blocks.append((title, start, block))

    for title, start, block in blocks:
        name = None
        i = 0
        # Before the request line: blank lines, comments (# @name), file variables.
        while i < len(block):
            line = block[i].strip()
            m = FILE_VARIABLE.match(line)
            if not line:
                pass
            elif is_comment(line):
                n = NAME_COMMENT.match(line)
                if n:
                    name = n.group(1)
            elif m:
                variables[m.group(1)] = m.group(2).strip()
            else:
                break
            i += 1
        if i >= len(block):
            continue
        line = block[i].strip()
        m = REQUEST_LINE.match(line)
        if m:
            method, url = m.group(1).upper(), m.group(2)
        else:
            u = URL_ONLY.match(line)
            if not u:
                continue
            method, url = "GET", u.group(1)
        request_line = start + i
        i += 1
        # Query continuation lines (?a=1 / &b=2) on the lines below the URL.
        while i < len(block) and block[i].strip()[:1] in ("?", "&") and block[i][:1] in (" ", "\t", "?", "&"):
            url += block[i].strip()
            i += 1
        headers = []
        while i < len(block) and block[i].strip():
            h = block[i].strip()
            if not is_comment(h):
                hm = HEADER.match(h)
                if hm:
                    headers.append((hm.group(1), hm.group(2).strip()))
            i += 1
        body_text = "\n".join(block[i + 1:]) if i < len(block) else ""
        # Rider's response handlers (> {% … %}, > handler.js) and response references (<> file) aren't body.
        body_text = re.sub(r"^> \{%.*?%\}[ \t]*$", "", body_text, flags=re.S | re.M)
        body_text = re.sub(r"^(?:> |<> )\S.*$", "", body_text, flags=re.M).rstrip()
        body, body_file = (body_text if body_text.strip() else None), None
        if body and re.fullmatch(r"<@?\s+\S.*", body.strip()):
            body_file, body = body.strip().lstrip("<@").strip(), None
        requests.append({
            "name": name,
            "title": title if title and not title.startswith("#") else None,
            "method": method,
            "url": url,
            "headers": headers,
            "body": body,
            "bodyFile": body_file,
            "line": request_line,
        })
    return variables, requests


def label(request):
    """What a request is called in lists: its title, its @name, else METHOD /path."""
    if request.get("title"):
        return request["title"]
    if request.get("name"):
        return request["name"]
    url = request["url"]
    path = re.sub(r"^\{\{[^}]+\}\}", "", url)
    path = re.sub(r"^https?://[^/]+", "", path) or url
    return f"{request['method']} {path}"


# --- environments -------------------------------------------------------------------------------------------

ENV_FILES = ("http-client.env.json", "http-client.private.env.json", "http-client.env.json.user")


def env_dirs(http_path, root):
    """Folders searched for environment files: the .http file's folder and those above it, up to the root."""
    d = os.path.dirname(os.path.abspath(http_path))
    stop = os.path.abspath(root) if root else None
    dirs = []
    while True:
        dirs.append(d)
        if stop is None or os.path.normcase(d) == os.path.normcase(stop):
            break
        parent = os.path.dirname(d)
        if parent == d:
            break
        d = parent
    return dirs


def load_environments(http_path, root):
    """{environment: {variable: value}} from the nearest folder with an environment file ($shared merged in)."""
    for d in env_dirs(http_path, root):
        found = [os.path.join(d, n) for n in ENV_FILES if os.path.isfile(os.path.join(d, n))]
        if not found:
            continue
        envs = {}
        for path in found:
            try:
                with open(path, encoding="utf-8-sig") as f:
                    data = json.load(f)
            except (OSError, ValueError) as e:
                log(f"http: {path}: {e}")
                continue
            for env, values in (data.items() if isinstance(data, dict) else []):
                if isinstance(values, dict):
                    envs.setdefault(env, {}).update(values)
        shared = envs.pop("$shared", {})
        return {env: {**shared, **values} for env, values in envs.items()} or ({"$shared": shared} if shared else {})
    return {}


def read_dotenv(http_path, root):
    values = {}
    for d in reversed(env_dirs(http_path, root)):
        path = os.path.join(d, ".env")
        try:
            with open(path, encoding="utf-8-sig") as f:
                for line in f:
                    m = re.match(r"^\s*(?:export\s+)?([A-Za-z_]\w*)\s*=\s*(.*?)\s*$", line)
                    if m:
                        v = m.group(2)
                        if len(v) >= 2 and v[0] == v[-1] and v[0] in "\"'":
                            v = v[1:-1]
                        values[m.group(1)] = v
        except OSError:
            pass
    return values


# --- variables ----------------------------------------------------------------------------------------------

class VariableError(Exception):
    pass


def json_path(value, path):
    """A small JSONPath: $.a.b[0].c, $['a'], $.items[*].id (a list), * for the whole value."""
    if path in ("*", "$"):
        return value
    if not path.startswith("$"):
        raise VariableError(f"not a JSONPath: {path}")
    tokens = re.findall(r"\.([^.\[\]]+)|\[\s*'([^']*)'\s*\]|\[\s*\"([^\"]*)\"\s*\]|\[\s*(-?\d+|\*)\s*\]", path[1:])
    current = [value]
    multi = False
    for dotted, single, double, index in tokens:
        key = dotted or single or double
        nxt = []
        for v in current:
            if index == "*" or key == "*":
                multi = True
                nxt += list(v.values()) if isinstance(v, dict) else list(v) if isinstance(v, list) else []
            elif index:
                if isinstance(v, list) and -len(v) <= int(index) < len(v):
                    nxt.append(v[int(index)])
            elif isinstance(v, dict) and key in v:
                nxt.append(v[key])
        current = nxt
    if not current:
        raise VariableError(f"{path} matches nothing in the response")
    return current if multi else current[0]


def as_text(value):
    if isinstance(value, str):
        return value
    if value is None:
        return ""
    return json.dumps(value)


def offset(amount, unit):
    seconds = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800, "ms": 0.001}.get(unit)
    if seconds is None:
        if unit == "M":
            return datetime.timedelta(days=30 * amount)
        if unit == "y":
            return datetime.timedelta(days=365 * amount)
        raise VariableError(f"unknown time unit {unit}")
    return datetime.timedelta(seconds=amount * seconds)


def format_datetime(moment, args):
    fmt = args[0] if args else "iso8601"
    rest = args[1:]
    if len(rest) >= 2:
        moment += offset(int(rest[0]), rest[1])
    if fmt == "iso8601":
        return moment.isoformat(timespec="seconds").replace("+00:00", "Z")
    if fmt == "rfc1123":
        return moment.astimezone(datetime.timezone.utc).strftime("%a, %d %b %Y %H:%M:%S GMT")
    fmt = fmt.strip("\"'")
    # .NET / Day.js tokens to strftime, for the common ones.
    for a, b in (("YYYY", "%Y"), ("yyyy", "%Y"), ("MM", "%m"), ("DD", "%d"), ("dd", "%d"), ("HH", "%H"),
                 ("mm", "%M"), ("ss", "%S")):
        fmt = fmt.replace(a, b)
    return moment.strftime(fmt)


class Resolver:
    def __init__(self, file_variables, environment, responses, http_path, root):
        self.file_variables = file_variables
        self.environment = environment
        self.responses = responses
        self.http_path = http_path
        self.root = root
        self.dotenv = None
        self.missing = []

    def dynamic(self, name, args):
        if name in ("$guid", "$uuid", "$random.uuid"):
            return str(uuid.uuid4())
        if name in ("$randomInt", "$random.integer"):
            low, high = (int(args[0]), int(args[1])) if len(args) >= 2 else (0, 1000)
            return str(random.randrange(low, high) if high > low else low)
        if name == "$timestamp":
            moment = time.time()
            if len(args) >= 2:
                moment += offset(int(args[0]), args[1]).total_seconds()
            return str(int(moment))
        if name == "$isoTimestamp":
            return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
        if name == "$datetime":
            return format_datetime(datetime.datetime.now(datetime.timezone.utc), args)
        if name == "$localDatetime":
            return format_datetime(datetime.datetime.now().astimezone(), args)
        if name == "$processEnv":
            if not args:
                raise VariableError("$processEnv needs a variable name")
            key = args[0]
            if key.startswith("%"):  # %name: the name is itself a variable (REST Client)
                key = self.lookup(key[1:], 0)
            if key not in os.environ:
                raise VariableError(f"environment variable {key} isn't set")
            return os.environ[key]
        if name == "$dotenv":
            if self.dotenv is None:
                self.dotenv = read_dotenv(self.http_path, self.root)
            key = args[0] if args else ""
            if key.startswith("%"):
                key = self.lookup(key[1:], 0)
            if key not in self.dotenv:
                raise VariableError(f"{key} isn't in a .env file")
            return self.dotenv[key]
        raise VariableError(f"unknown variable {name}")

    def request_variable(self, expression):
        # name.response.body.<path> | name.response.headers.<Header>
        m = re.match(r"^([\w-]+)\.(response|request)\.(body|headers)(?:\.(.+))?$", expression)
        if not m:
            return None
        name, part, kind, rest = m.groups()
        response = self.responses.get(name)
        if response is None:
            raise VariableError(f"send the request named {name} first")
        if part == "request":
            source = response["request"]
        else:
            source = response
        if kind == "headers":
            wanted = (rest or "").lower()
            for k, v in source["headers"]:
                if k.lower() == wanted:
                    return v
            raise VariableError(f"{name}'s {part} has no header {rest}")
        body = source.get("text") if part == "response" else source.get("body")
        if not rest or rest == "*":
            return body or ""
        try:
            data = json.loads(body or "")
        except ValueError:
            raise VariableError(f"{name}'s {part} body isn't JSON")
        return as_text(json_path(data, rest))

    def lookup(self, expression, depth):
        expression = expression.strip()
        if depth > 20:
            raise VariableError(f"{{{{{expression}}}}} refers to itself")
        if expression.startswith("$"):
            parts = re.findall(r'"[^"]*"|\'[^\']*\'|\S+', expression)
            return self.dynamic(parts[0], [p.strip("\"'") if p[0] in "\"'" else p for p in parts[1:]])
        if expression in self.file_variables:
            return self.resolve(self.file_variables[expression], depth + 1)
        if expression in self.environment:
            return self.resolve(as_text(self.environment[expression]), depth + 1)
        value = self.request_variable(expression)
        if value is not None:
            return value
        raise VariableError(f"{{{{{expression}}}}} isn't defined")

    def resolve(self, text, depth=0):
        if text is None:
            return None
        return re.sub(r"\{\{(.+?)\}\}", lambda m: self.lookup(m.group(1), depth), text)


# --- sending ------------------------------------------------------------------------------------------------

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def is_local(host):
    return host in ("localhost", "127.0.0.1", "::1", "[::1]") or (host or "").endswith(".localhost")


def build(request, resolver, http_path):
    """The request with variables filled in: (method, url, headers, body bytes)."""
    url = resolver.resolve(request["url"]).strip()
    if not re.match(r"^https?://", url, re.I):
        url = ("http://" if url.startswith(("localhost", "127.")) else "https://") + url.lstrip("/")
    headers = [(k, resolver.resolve(v)) for k, v in request["headers"]]
    body = None
    if request.get("bodyFile"):
        path = resolver.resolve(request["bodyFile"])
        if not os.path.isabs(path):
            path = os.path.join(os.path.dirname(http_path), path)
        with open(path, "rb") as f:
            body = f.read()
    elif request.get("body") is not None:
        text = resolver.resolve(request["body"])
        content_type = next((v for k, v in headers if k.lower() == "content-type"), "")
        if "x-www-form-urlencoded" in content_type:
            # Form bodies may be written on several lines (a=1\n&b=2).
            text = "".join(line.strip() for line in text.splitlines())
        body = text.encode("utf-8")
    return request["method"], url, headers, body


def perform(method, url, headers, body, settings):
    timeout = float(settings.get("http.timeout") or 30)
    handlers = []
    host = urllib.parse.urlsplit(url).hostname
    if url.lower().startswith("https") and (
            settings.get("http.verifyCertificates") is False
            or (is_local(host) and settings.get("http.insecureLocalhost", True) is not False)):
        context = ssl.create_default_context()
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE
        handlers.append(urllib.request.HTTPSHandler(context=context))
    if settings.get("http.followRedirects") is False:
        handlers.append(NoRedirect())
    opener = urllib.request.build_opener(*handlers)
    req = urllib.request.Request(url, data=body, method=method)
    for k, v in headers:
        req.add_header(k, v)
    if not any(k.lower() == "user-agent" for k, _ in headers):
        req.add_header("User-Agent", "ByteForge-HTTP-Client")
    started = time.monotonic()
    try:
        response = opener.open(req, timeout=timeout)
    except urllib.error.HTTPError as e:  # 4xx / 5xx still are responses
        response = e
    with response:
        data = response.read()
        elapsed = (time.monotonic() - started) * 1000
        version = getattr(response, "version", None) or getattr(getattr(response, "fp", None), "version", 11)
        return {
            "status": getattr(response, "status", None) or response.code,
            "reason": getattr(response, "reason", "") or "",
            "version": "HTTP/1.0" if version == 10 else "HTTP/1.1",
            "headers": list(response.headers.items()),
            "data": data,
            "elapsed": elapsed,
            "url": response.geturl(),
        }


def charset(headers):
    for k, v in headers:
        if k.lower() == "content-type":
            m = re.search(r"charset=([\w.-]+)", v, re.I)
            if m:
                return m.group(1)
    return "utf-8"


def content_type(headers):
    return next((v.split(";")[0].strip().lower() for k, v in headers if k.lower() == "content-type"), "")


def is_text(ctype, data):
    if ctype.startswith("text/") or any(t in ctype for t in ("json", "xml", "javascript", "html", "yaml", "csv",
                                                               "x-www-form-urlencoded", "problem")):
        return True
    if ctype:
        return False
    return b"\0" not in data[:2048]


def body_text(response, pretty=True):
    """The body as text (pretty JSON when it is JSON), or None for binary content."""
    data = response["data"]
    ctype = content_type(response["headers"])
    if not is_text(ctype, data):
        return None
    try:
        text = data.decode(charset(response["headers"]), errors="replace")
    except LookupError:
        text = data.decode("utf-8", errors="replace")
    if pretty and len(data) <= MAX_PRETTY and ("json" in ctype or (not ctype and text.lstrip()[:1] in "{[")):
        try:
            return json.dumps(json.loads(text), indent=2, ensure_ascii=False)
        except ValueError:
            pass
    return text


def size(n):
    for unit in ("B", "KB", "MB"):
        if n < 1024 or unit == "MB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024


def summary(response):
    return f"{response['status']} {response['reason']} · {response['elapsed']:.0f} ms · {size(len(response['data']))}".replace("  ", " ")


def response_document(method, url, request_headers, response, text):
    lines = [f"{method} {url}", f"# {summary(response)}"]
    if response["url"] != url:
        lines.append(f"# redirected to {response['url']}")
    lines += ["", f"{response['version']} {response['status']} {response['reason']}".rstrip()]
    lines += [f"{k}: {v}" for k, v in response["headers"]]
    lines.append("")
    if text is None:
        lines.append(f"# {size(len(response['data']))} of {content_type(response['headers']) or 'binary data'}, "
                     "saved next to this file")
    else:
        lines.append(text)
    return "\n".join(lines) + "\n"


def cache_dir():
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    else:
        base = os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache")
    return os.path.join(base, "byteforge", "http-responses")


def slug(text):
    return re.sub(r"[^\w.-]+", "-", text).strip("-")[:60] or "request"


EXTENSION_BY_TYPE = {"image/png": ".png", "image/jpeg": ".jpg", "image/gif": ".gif", "image/webp": ".webp",
                     "application/pdf": ".pdf", "application/zip": ".zip", "image/svg+xml": ".svg"}


def save_response(name, document, response, binary):
    folder = cache_dir()
    os.makedirs(folder, exist_ok=True)
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    base = f"{stamp}-{slug(name)}"
    n = 1
    while os.path.exists(os.path.join(folder, base + ".http")):
        n += 1
        base = f"{stamp}-{slug(name)}-{n}"
    path = os.path.join(folder, base + ".http")
    with open(path, "w", encoding="utf-8") as f:
        f.write(document)
    if binary:
        ext = EXTENSION_BY_TYPE.get(content_type(response["headers"]), ".bin")
        with open(os.path.join(folder, base + ext), "wb") as f:
            f.write(response["data"])
    prune(folder)
    return path


def prune(folder):
    try:
        names = sorted(os.listdir(folder))
    except OSError:
        return
    responses = [n for n in names if n.endswith(".http")]
    for old in responses[:-KEEP_RESPONSES]:
        stem = old[:-5]
        for n in names:
            if n == old or (n.startswith(stem + ".") and not n.endswith(".http")):
                try:
                    os.remove(os.path.join(folder, n))
                except OSError:
                    pass


def curl_command(method, url, headers, body):
    def q(s):
        return "'" + s.replace("'", "'\\''") + "'"
    parts = ["curl", "-X", method, q(url)]
    for k, v in headers:
        parts += ["-H", q(f"{k}: {v}")]
    if body is not None:
        parts += ["--data-binary", q(body.decode("utf-8", errors="replace"))]
    return " ".join(parts)


# --- module -------------------------------------------------------------------------------------------------

def find_http_files(root):
    found = []
    for directory, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS and not d.startswith("."))
        for name in sorted(files):
            if name.lower().endswith(EXTENSIONS):
                found.append(os.path.join(directory, name))
    return found


def read(path):
    with open(path, encoding="utf-8-sig", errors="replace") as f:
        return f.read()


class HttpClient:
    def __init__(self):
        self.root = None
        self.settings = {}
        self.active = None           # path of the active editor, when it's an .http file
        self.environment = None      # chosen environment name
        self.responses = {}          # @name -> {"headers", "text", "request": {"headers", "body"}}
        self.results = {}            # (path, line) -> summary of the last response
        self.last = None             # (path, line) of the last request sent
        self.pending = {}            # quickPick requestId -> kind
        self.next_request = 1
        self.sending = set()

    # environments

    def environments(self, path=None):
        path = path or self.active or (self.root and os.path.join(self.root, "x.http"))
        return load_environments(path, self.root) if path else {}

    def current_environment(self, envs):
        if self.environment in envs:
            return self.environment
        wanted = self.settings.get("http.environment")
        if wanted in envs:
            return wanted
        for preferred in ("dev", "development", "local", "Development", "Local"):
            if preferred in envs:
                return preferred
        return next(iter(envs), None)

    def update_status(self):
        envs = self.environments()
        env = self.current_environment(envs)
        if not envs:
            send({"type": "status", "id": "http.env", "text": ""})
            return
        send({"type": "status", "id": "http.env", "text": f"⇄ {env}", "command": "http.selectEnvironment",
              "tooltip": "HTTP Client environment (http-client.env.json). Click to change it."})

    def pick_environment(self):
        envs = self.environments()
        if not envs:
            log("http: no http-client.env.json next to the .http file or above it.")
            return
        current = self.current_environment(envs)
        request = self.ask("environment")
        send({"type": "quickPick", "requestId": request, "placeholder": "HTTP Client environment",
              "items": [{"label": e, "hint": "current" if e == current else "",
                         "detail": ", ".join(sorted(v))[:120], "value": e} for e, v in envs.items()]})

    # view

    def refresh(self):
        if not self.root:
            send({"type": "setTree", "view": VIEW, "items": [], "message": "Open a folder to see its .http files."})
            return
        items = []
        for path in find_http_files(self.root):
            try:
                _, requests = parse(read(path))
            except OSError:
                continue
            children = []
            for r in requests:
                key = f"{path}\n{r['line']}"
                children.append({
                    "id": key,
                    "label": label(r),
                    "description": self.results.get(key, ""),
                    "tooltip": f"{r['method']} {r['url']}",
                    "icon": "play",
                    "actions": [{"id": "send", "icon": "play", "tooltip": "Send Request", "label": "Send Request"}],
                    "menu": [{"id": "goto", "label": "Go to Request"}, {"id": "curl", "label": "Show as curl"}],
                })
            rel = os.path.relpath(path, self.root).replace(os.sep, "/")
            items.append({"id": path, "label": rel, "icon": f"file:{os.path.basename(path)}", "expanded": True,
                          "count": len(children), "children": children,
                          "actions": [{"id": "sendAll", "icon": "play", "tooltip": "Send All Requests"}],
                          "menu": [{"id": "open", "label": "Open File"}]})
        envs = self.environments()
        env = self.current_environment(envs)
        message = None if items else "No .http or .rest files. Create one (Ctrl+N, then save it as api.http)."
        actions = [{"id": "refresh", "icon": "refresh", "tooltip": "Refresh"},
                   {"id": "responses", "icon": "file", "tooltip": "Show the responses folder in the Output"}]
        if envs:
            actions.insert(0, {"id": "environment", "icon": "layers", "tooltip": f"Environment: {env} (click to change)"})
        send({"type": "setTree", "view": VIEW, "items": items, "message": message, "actions": actions})

    # sending

    def ask(self, kind, data=None):
        request = self.next_request
        self.next_request += 1
        self.pending[request] = (kind, data)
        return request

    def send_request(self, path, line):
        key = (path, line)
        if key in self.sending:
            log("http: that request is still running.")
            return
        self.sending.add(key)
        self.last = key
        threading.Thread(target=self._send, args=(path, line), daemon=True).start()

    def _send(self, path, line):
        try:
            self._send_inner(path, line)
        finally:
            self.sending.discard((path, line))

    def _send_inner(self, path, line, open_response=True):
        try:
            variables, requests = parse(read(path))
        except OSError as e:
            log(f"http: {e}")
            return None
        request = next((r for r in requests if r["line"] == line), None)
        if request is None:
            log(f"http: no request at line {line + 1} of {os.path.basename(path)} any more (save the file?)")
            return None
        envs = load_environments(path, self.root)
        env = self.current_environment(envs)
        resolver = Resolver(variables, envs.get(env, {}), self.responses, path, self.root)
        name = label(request)
        key = f"{path}\n{line}"
        try:
            method, url, headers, body = build(request, resolver, path)
        except (VariableError, OSError) as e:
            log(f"http: {name}: {e}")
            send({"type": "showView", "id": "output"})
            self.results[key] = "variable error"
            self.refresh()
            return None
        self.results[key] = "sending…"
        self.refresh()
        try:
            response = perform(method, url, headers, body, self.settings)
        except Exception as e:  # connection refused, timeouts, TLS, bad URLs
            reason = getattr(e, "reason", None) or e
            log(f"http: {method} {url}: {reason}")
            send({"type": "showView", "id": "output"})
            self.results[key] = "failed"
            self.refresh()
            return None
        text = body_text(response, self.settings.get("http.prettyPrint", True) is not False)
        if request.get("name"):
            raw = response["data"].decode(charset(response["headers"]), errors="replace") if text is not None else ""
            self.responses[request["name"]] = {
                "headers": response["headers"], "text": raw,
                "request": {"headers": headers, "body": body.decode("utf-8", errors="replace") if body else ""},
            }
        document = response_document(method, url, headers, response, text)
        saved = save_response(name, document, response, text is None)
        self.results[key] = summary(response)
        log(f"http: {method} {url} → {summary(response)}")
        if open_response:
            send({"type": "openFile", "path": saved})
        self.refresh()
        return response

    def send_all(self, path):
        def run():
            try:
                _, requests = parse(read(path))
            except OSError as e:
                log(f"http: {e}")
                return
            # In order, so later requests can use the responses of earlier named ones.
            for r in requests:
                self._send_inner(path, r["line"], open_response=False)
            log(f"http: sent {len(requests)} requests from {os.path.basename(path)}; responses in {cache_dir()}")
            send({"type": "showView", "id": "output"})
        threading.Thread(target=run, daemon=True).start()

    def pick_request(self):
        files = [self.active] if self.active else (find_http_files(self.root) if self.root else [])
        items = []
        for path in files:
            try:
                _, requests = parse(read(path))
            except OSError:
                continue
            for r in requests:
                where = os.path.basename(path) if len(files) > 1 else f"line {r['line'] + 1}"
                items.append({"label": label(r), "detail": f"{r['method']} {r['url']}", "hint": where,
                              "value": f"{path}\n{r['line']}"})
        if not items:
            log("http: no requests here. Open an .http file (requests are read from the saved file).")
            return
        send({"type": "quickPick", "requestId": self.ask("send"), "placeholder": "Send a request", "items": items})

    def show_curl(self, path, line):
        try:
            variables, requests = parse(read(path))
            request = next(r for r in requests if r["line"] == line)
            envs = load_environments(path, self.root)
            resolver = Resolver(variables, envs.get(self.current_environment(envs), {}), self.responses, path, self.root)
            log(curl_command(*build(request, resolver, path)))
        except (OSError, StopIteration, VariableError) as e:
            log(f"http: {e}")
        send({"type": "showView", "id": "output"})

    # messages

    def handle(self, message):
        kind = message.get("type")
        if kind == "activate":
            self.root = message.get("workspace")
            self.settings = message.get("settings") or {}
            self.refresh()
            self.update_status()
        elif kind == "event":
            name = message.get("name")
            if name == "settingsChanged":
                self.settings = message.get("settings") or {}
                self.update_status()
            elif name == "folderOpened":
                self.root, self.results, self.responses, self.environment = message.get("path"), {}, {}, None
                self.refresh()
                self.update_status()
            elif name == "editorChanged":
                path = message.get("path")
                self.active = path if path and path.lower().endswith(EXTENSIONS) else None
                self.update_status()
            elif name == "fileSaved" and (message.get("path") or "").lower().endswith(EXTENSIONS + (".json",)):
                self.refresh()
                self.update_status()
            elif name in ("filesChanged", "viewShown"):
                self.refresh()
        elif kind == "executeCommand":
            command = message.get("id")
            if command == "http.send":
                self.pick_request()
            elif command == "http.sendAll":
                if self.active:
                    self.send_all(self.active)
                else:
                    log("http: open an .http file first.")
            elif command == "http.rerun":
                if self.last:
                    self.send_request(*self.last)
                else:
                    self.pick_request()
            elif command == "http.selectEnvironment":
                self.pick_environment()
            elif command == "http.showResponses":
                log(f"http: responses are saved in {cache_dir()}")
                send({"type": "showView", "id": "output"})
        elif kind == "quickPickResult":
            kind_, _ = self.pending.pop(message.get("requestId"), (None, None))
            value = message.get("value")
            if kind_ == "send" and value:
                path, line = value.rsplit("\n", 1)
                self.send_request(path, int(line))
            elif kind_ == "environment" and value:
                self.environment = value
                log(f"http: environment {value}")
                self.update_status()
                self.refresh()
        elif kind == "treeAction":
            item, action = message.get("item"), message.get("action")
            if item is None:
                if action == "refresh":
                    self.refresh()
                elif action == "environment":
                    self.pick_environment()
                elif action == "responses":
                    log(f"http: responses are saved in {cache_dir()}")
                    send({"type": "showView", "id": "output"})
                return
            if "\n" in item:
                path, line = item.rsplit("\n", 1)
                line = int(line)
                if action in ("click", "send"):
                    self.send_request(path, line)
                elif action == "goto":
                    send({"type": "openFile", "path": path, "line": line, "column": 0})
                elif action == "curl":
                    self.show_curl(path, line)
            elif action == "sendAll":
                self.send_all(item)
            elif action in ("open", "click"):
                send({"type": "openFile", "path": item})


def main():
    for command, title, key in (("http.send", "Send Request…", "ctrl-alt-enter"),
                                ("http.sendAll", "Send All Requests in File", None),
                                ("http.rerun", "Rerun Last Request", None),
                                ("http.selectEnvironment", "Select Environment", None),
                                ("http.showResponses", "Where Are the Responses?", None)):
        registration = {"type": "registerCommand", "id": command, "title": title, "category": "HTTP"}
        if key:
            registration["keybinding"] = key
        send(registration)
    client = HttpClient()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        try:
            client.handle(message)
        except Exception as e:  # keep the module alive; say what went wrong
            log(f"http: {type(e).__name__}: {e}")


if __name__ == "__main__":
    main()
