"""Docker module for ByteForge.

A side bar view with the workspace's Dockerfiles and Compose files and the engine's containers (with their ports,
volumes and networks), images, volumes and networks, with the usual actions (build, compose up/down, start/stop/restart, logs, shell, run, remove), plus "Add Docker
Support" (a multi-stage Dockerfile and .dockerignore for a .NET project) and "Add Docker Compose File".

Everything goes through the `docker` CLI, so contexts, credentials and Docker Desktop or Podman's `docker` shim
all apply. Long-running and interactive commands run in an IDE terminal; quick ones run on a background thread
here. The engine is polled every 4 seconds, but only while the view is shown.

When Docker isn't usable, a SETUP section says why (engine stopped, not in the docker group, Compose or Buildx
missing) with a button that fixes it in a terminal.
"""

import json
import os
import posixpath
import queue
import re
import shutil
import subprocess
import sys
import threading

VIEW = "docker"
POLL_SECONDS = 4
MAX_FILES = 100_000
SKIP_DIRS = {".git", "bin", "obj", "node_modules", ".vs", ".idea"}
RUNNING_COLOR = "#3fb950"
PAUSED_COLOR = "#d29922"
SHELL_SCRIPT = "command -v bash >/dev/null && exec bash || exec sh"
BUILTIN_NETWORKS = {"bridge", "host", "none"}
# Material Design Icons (Apache 2.0): database and lan.
VOLUME_ICON = ("M12,3C7.58,3 4,4.79 4,7C4,9.21 7.58,11 12,11C16.42,11 20,9.21 20,7C20,4.79 16.42,3 12,3M4,9V12C4,14.21 "
               "7.58,16 12,16C16.42,16 20,14.21 20,12V9C20,11.21 16.42,13 12,13C7.58,13 4,11.21 4,9M4,14V17C4,19.21 7.58,21 "
               "12,21C16.42,21 20,19.21 20,17V14C20,16.21 16.42,18 12,18C7.58,18 4,16.21 4,14Z")
NETWORK_ICON = ("M10,2C8.89,2 8,2.89 8,4V7C8,8.11 8.89,9 10,9H11V11H2V13H6V15H5C3.89,15 3,15.89 3,17V20C3,21.11 3.89,22 "
                "5,22H9C10.11,22 11,21.11 11,20V17C11,15.89 10.11,15 9,15H8V13H16V15H15C13.89,15 13,15.89 13,17V20C13,21.11 "
                "13.89,22 15,22H19C20.11,22 21,21.11 21,20V17C21,15.89 20.11,15 19,15H18V13H22V11H13V9H14C15.11,9 16,8.11 "
                "16,7V4C16,2.89 15.11,2 14,2H10M10,4H14V7H10V4M5,17H9V20H5V17M15,17H19V20H15V17Z")

# ───────────── docker CLI ─────────────


class DockerError(Exception):
    pass


def run_docker(args, timeout=300):
    """Runs `docker <args>` and returns stdout; raises DockerError with stderr (or stdout) on failure."""
    kwargs = {}
    if os.name == "nt":
        kwargs["creationflags"] = 0x08000000  # CREATE_NO_WINDOW
    try:
        done = subprocess.run(docker_argv(args), stdin=subprocess.DEVNULL, capture_output=True,
                              timeout=timeout, **kwargs)
    except OSError as error:
        raise DockerError(f"could not run docker (is it installed?): {error}")
    except subprocess.TimeoutExpired:
        raise DockerError(f"docker {' '.join(args)} timed out")
    stdout = done.stdout.decode("utf-8", errors="replace")
    if done.returncode == 0:
        return stdout
    stderr = done.stderr.decode("utf-8", errors="replace")
    raise DockerError(stderr.strip() if stderr.strip() else stdout.strip())


# ───────────── setup diagnosis ─────────────

# True while the user is listed in the `docker` group but this session doesn't have it yet (it was added after
# logging in): docker then runs through `sg docker -c …`, so no logout is needed.
use_sg = False


def docker_group():
    """Linux: (listed, active) for the `docker` group, or None when there's no such group (or we're root)."""
    if not sys.platform.startswith("linux") or os.getuid() == 0:
        return None
    try:
        import grp
        import pwd
        group = grp.getgrnam("docker")
        user = pwd.getpwuid(os.getuid())
    except (ImportError, KeyError):
        return None
    listed = user.pw_name in group.gr_mem or user.pw_gid == group.gr_gid
    active = group.gr_gid in os.getgroups() or os.getgid() == group.gr_gid
    return listed, active


def update_sg():
    global use_sg
    state = docker_group()
    use_sg = bool(state and state[0] and not state[1] and shutil.which("sg"))


def docker_argv(args):
    """The argv that runs `docker <args>`, through `sg docker` when needed."""
    argv = ["docker"] + list(args)
    return ["sg", "docker", "-c", command_line(argv)] if use_sg else argv


def run_shell_args(reference):
    """Arguments that start an image's container in the background (ports published) and then open a shell at `/`
    in it, in the same terminal. Leaving the shell removes the container. When the container doesn't stay up (its
    command exits right away), its logs are shown instead."""
    script = "; ".join([
        'id=$(docker run --detach --interactive --tty --publish-all "$1") || exit',
        'docker port "$id"',
        'if [ "$(docker inspect --format \'{{.State.Running}}\' "$id")" = true ]',
        'then docker exec --interactive --tty --workdir / "$id" sh -c ' + shell_quote(SHELL_SCRIPT),
        'else echo "The container exited:"; docker logs "$id"',
        'fi',
        'docker rm --force "$id" >/dev/null'])
    return ["sh", "-c", script, "sh", reference]


def terminal_line(args):
    """A shell command line for a terminal, through `sg docker` when needed."""
    line = command_line(args)
    return command_line(["sg", "docker", "-c", line]) if use_sg else line


def classify_engine_error(text):
    """"permission" (no access to the socket), "stopped" (no daemon) or "other"."""
    t = (text or "").lower()
    if "permission denied" in t:
        return "permission"
    if any(s in t for s in ("no such file or directory", "is the docker daemon running", "cannot connect to the docker",
                            "connection refused", "failed to connect to the docker api")):
        return "stopped"
    return "other"


def package_manager():
    """The install command prefix for the system's package manager, or None."""
    for tool, prefix in (("pacman", "sudo pacman -S --needed"), ("apt-get", "sudo apt-get install -y"),
                         ("dnf", "sudo dnf install -y"), ("zypper", "sudo zypper install -y")):
        if shutil.which(tool):
            return tool, prefix
    return None


def plugin_packages(tool, compose, buildx):
    """Package names for the missing Compose and Buildx plugins."""
    names = {"pacman": ("docker-compose", "docker-buildx")}.get(tool, ("docker-compose-plugin", "docker-buildx-plugin"))
    return [n for n, missing in zip(names, (compose, buildx)) if missing]


def can_systemctl():
    return sys.platform.startswith("linux") and shutil.which("systemctl") is not None


def plugin_works(args):
    try:
        run_docker(args, timeout=30)
        return True
    except DockerError:
        return False


def cli_available():
    """Whether the `docker` CLI is installed (the daemon may still be down)."""
    try:
        run_docker(["--version"], timeout=30)
        return True
    except DockerError:
        return False


def snapshot():
    """All containers (not only running ones), images, volumes and networks. Raises DockerError when the daemon is
    down."""
    containers = parse_containers(run_docker(["ps", "--all", "--no-trunc", "--format", "{{json .}}"], timeout=60))
    images = parse_images(run_docker(["images", "--format", "{{json .}}"], timeout=60))
    volumes = parse_volumes(run_docker(["volume", "ls", "--format", "{{json .}}"], timeout=60))
    networks = parse_networks(run_docker(["network", "ls", "--no-trunc", "--format", "{{json .}}"], timeout=60))
    return containers, images, volumes, networks


def _field(raw, key, required):
    """A string field of a `{{json .}}` object: None (skip the object) when required and missing or not a string."""
    value = raw.get(key)
    if value is None:
        return None if required else ""
    return value if isinstance(value, str) else None


def state_from_status(status):
    """Older engines don't report `State`; derive it from the status text."""
    s = status.lower()
    if "(paused)" in s:
        return "paused"
    if s.startswith("up"):
        return "running"
    if s.startswith("restarting"):
        return "restarting"
    if s.startswith("created"):
        return "created"
    return "exited"


def is_running(container):
    return container["state"] in ("running", "restarting")


def is_paused(container):
    return container["state"] == "paused"


def parse_containers(text):
    """Parses `docker ps --format '{{json .}}'` (one object per line), sorted by Compose project, then name."""
    containers = []
    for line in text.splitlines():
        try:
            raw = json.loads(line.strip())
        except ValueError:
            continue
        if not isinstance(raw, dict):
            continue
        fields = {}
        for key, required in (("ID", True), ("Names", True), ("Image", True), ("State", False), ("Status", False),
                              ("Ports", False), ("Labels", False), ("Mounts", False),
                              ("Networks", False), ("Command", False), ("RunningFor", False)):
            fields[key] = _field(raw, key, required)
        if any(v is None for v in fields.values()):
            continue

        def label(key):
            for pair in fields["Labels"].split(","):
                if pair.startswith(key) and pair[len(key):].startswith("="):
                    value = pair[len(key) + 1:]
                    return value or None
            return None

        state = fields["State"].lower() if fields["State"] else state_from_status(fields["Status"])
        containers.append({
            "id": fields["ID"][:12],
            "name": fields["Names"].split(",")[0],
            "image": fields["Image"],
            "state": state,
            "status": fields["Status"],
            "ports": fields["Ports"],
            "compose_project": label("com.docker.compose.project"),
            "compose_service": label("com.docker.compose.service"),
            "mounts": split_list(fields["Mounts"]),
            "networks": split_list(fields["Networks"]),
            "command": fields["Command"].strip('"'),
            "age": fields["RunningFor"],
        })
    containers.sort(key=lambda c: (c["compose_project"] is None, c["compose_project"] or "", c["name"]))
    return containers


def split_list(text):
    return [part.strip() for part in text.split(",") if part.strip()]


def json_lines(text):
    for line in text.splitlines():
        try:
            raw = json.loads(line.strip())
        except ValueError:
            continue
        if isinstance(raw, dict):
            yield raw


def label_value(labels, key):
    for pair in (labels if isinstance(labels, str) else "").split(","):
        if pair.startswith(key + "="):
            return pair[len(key) + 1:] or None
    return None


def parse_volumes(text):
    """Parses `docker volume ls --format '{{json .}}'`: named volumes first, then anonymous ones."""
    volumes = []
    for raw in json_lines(text):
        name = raw.get("Name")
        if not isinstance(name, str) or not name:
            continue
        mountpoint = raw.get("Mountpoint")
        volumes.append({
            "name": name,
            "driver": raw.get("Driver") if isinstance(raw.get("Driver"), str) else "",
            "mountpoint": mountpoint if isinstance(mountpoint, str) else "",
            "compose_project": label_value(raw.get("Labels"), "com.docker.compose.project"),
            "anonymous": is_anonymous_volume(name),
        })
    volumes.sort(key=lambda v: (v["anonymous"], v["name"]))
    return volumes


def is_anonymous_volume(name):
    return len(name) == 64 and all(c in "0123456789abcdef" for c in name)


def parse_networks(text):
    """Parses `docker network ls --format '{{json .}}'`: the built-in ones (bridge, host, none) last."""
    networks = []
    for raw in json_lines(text):
        name, network_id = raw.get("Name"), raw.get("ID")
        if not isinstance(name, str) or not isinstance(network_id, str) or not name:
            continue
        networks.append({
            "id": network_id[:12],
            "name": name,
            "driver": raw.get("Driver") if isinstance(raw.get("Driver"), str) else "",
            "scope": raw.get("Scope") if isinstance(raw.get("Scope"), str) else "",
            "compose_project": label_value(raw.get("Labels"), "com.docker.compose.project"),
        })
    networks.sort(key=lambda n: (n["name"] in BUILTIN_NETWORKS, n["name"]))
    return networks


def parse_images(text):
    """Parses `docker images --format '{{json .}}'`."""
    images = []
    for line in text.splitlines():
        try:
            raw = json.loads(line.strip())
        except ValueError:
            continue
        if not isinstance(raw, dict):
            continue
        fields = {}
        for key, required in (("ID", True), ("Repository", True), ("Tag", True), ("Size", False), ("CreatedSince", False)):
            fields[key] = _field(raw, key, required)
        if any(v is None for v in fields.values()):
            continue
        image_id = fields["ID"]
        while image_id.startswith("sha256:"):
            image_id = image_id[len("sha256:"):]
        images.append({
            "id": image_id[:12],
            "repository": fields["Repository"],
            "tag": fields["Tag"],
            "size": fields["Size"],
            "created": fields["CreatedSince"],
        })
    images.sort(key=lambda i: (i["repository"] == "<none>", i["repository"], i["tag"]))
    return images


def image_reference(image):
    """`repository:tag`, or the id for dangling images (`<none>`)."""
    if image["repository"] == "<none>":
        return image["id"]
    if image["tag"] in ("<none>", ""):
        return image["repository"]
    return f"{image['repository']}:{image['tag']}"


# ───────────── workspace files ─────────────


def is_dockerfile_name(name):
    """`Dockerfile`, `Containerfile` and their variants (`Dockerfile.worker`, `api.dockerfile`)."""
    name = name.lower()
    for base in ("dockerfile", "containerfile"):
        if name == base:
            return True
        if name.startswith(base) and name[len(base):].startswith(".") and len(name) - len(base) > 1:
            return True
        if name.endswith(base) and name[:-len(base)].endswith(".") and len(name) - len(base) > 1:
            return True
    return False


def is_compose_file_name(name):
    """`compose.yaml`, `compose.yml`, `docker-compose.yml`, `docker-compose.yaml` and their variants
    (`docker-compose.override.yml`, `compose.prod.yaml`, …)."""
    name = name.lower()
    if name.endswith(".yml"):
        stem = name[:-4]
    elif name.endswith(".yaml"):
        stem = name[:-5]
    else:
        return False
    for base in ("compose", "docker-compose"):
        if stem == base:
            return True
        rest = stem[len(base):] if stem.startswith(base) else None
        if rest is not None and rest.startswith(".") and len(rest) > 1:
            return True
    return False


def docker_files(files, is_dockerfile=is_dockerfile_name):
    """Splits workspace files into (Dockerfiles, Compose files)."""
    dockerfiles = [p for p in files if is_dockerfile(os.path.basename(p))]
    compose = [p for p in files if is_compose_file_name(os.path.basename(p))]
    return dockerfiles, compose


def workspace_files(root):
    """All files below `root`, sorted, skipping build output, VCS and IDE folders and other dot-folders."""
    result = []
    if not root:
        return result
    stack = [root]
    while stack and len(result) < MAX_FILES:
        directory = stack.pop()
        try:
            entries = list(os.scandir(directory))
        except OSError:
            continue
        for entry in entries:
            try:
                if entry.is_dir(follow_symlinks=False):
                    if entry.name not in SKIP_DIRS and not entry.name.startswith("."):
                        stack.append(entry.path)
                elif entry.is_file():
                    result.append(entry.path)
            except OSError:
                continue
    result.sort(key=path_key)
    return result


def _parts(path):
    return [p for p in os.path.normpath(path).replace("\\", "/").split("/") if p not in ("", ".")], path.startswith(("/", "\\"))


def path_key(path):
    """Sorts paths component by component, like Rust's PathBuf ordering."""
    return _parts(path)[0]


def starts_with(path, base):
    """Component-wise prefix test (like Rust's Path::starts_with)."""
    p, p_abs = _parts(path)
    b, b_abs = _parts(base)
    return p_abs == b_abs and p[:len(b)] == b


def relative(path, base):
    """`path` relative to `base` with `/` separators ("" when equal), or `path` itself when it isn't below `base`."""
    if starts_with(path, base):
        p, _ = _parts(path)
        b, _ = _parts(base)
        return "/".join(p[len(b):])
    return path.replace("\\", "/")


def _extension(name):
    ext = os.path.splitext(name)[1]
    return ext[1:] if ext else ""


def _dir_has(directory, extensions):
    try:
        return any(_extension(name) in extensions for name in os.listdir(directory))
    except OSError:
        return False


def build_context(dockerfile, root):
    """The build context for a Dockerfile: its folder, unless it sits next to a .NET project in a workspace with a
    solution at `root` and no `.dockerignore` of its own. Visual Studio's Dockerfiles build from the solution folder
    (they copy `Project/Project.csproj`)."""
    directory = os.path.dirname(dockerfile)
    if not directory:
        return root
    if os.path.normpath(directory) == os.path.normpath(root) or os.path.exists(os.path.join(directory, ".dockerignore")):
        return directory
    solution_root = _dir_has(root, ("sln", "slnx")) or os.path.exists(os.path.join(root, ".dockerignore"))
    if starts_with(directory, root) and _dir_has(directory, ("csproj", "fsproj", "vbproj")) and solution_root:
        return root
    return directory


def image_name(name):
    """An image name from a folder or project name: lowercase, with only the characters Docker allows."""
    out = ""
    for c in name:
        c = c.lower() if c.isascii() else c
        separator_ok = bool(out) and not out.endswith((".", "_", "-"))
        if (c.isascii() and c.isalnum()) or (c in "._-" and separator_ok):
            out += c
        elif out and not out.endswith((".", "_", "-")):
            out += "-"
    out = out.rstrip("._-")
    return out or "app"


def image_name_for(dockerfile):
    """The image name for a Dockerfile: its folder's name (`Api/Dockerfile` → `api`), or the variant
    (`Dockerfile.worker` → `<folder>-worker`)."""
    folder = os.path.basename(os.path.dirname(dockerfile)) or "app"
    file = os.path.basename(dockerfile).lower()
    variant = None
    for prefix in ("dockerfile.", "containerfile."):
        if file.startswith(prefix):
            variant = file[len(prefix):]
            break
    if variant is None:
        for suffix in (".dockerfile", ".containerfile"):
            if file.endswith(suffix):
                variant = file[:-len(suffix)]
                break
    return image_name(f"{folder}-{variant}") if variant is not None else image_name(folder)


SAFE_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./:=@%+,")


def shell_quote(arg):
    """Quotes an argument for POSIX shells and fish (`'…'`, with `'` written as `'\\''`)."""
    if arg and all(c in SAFE_CHARS for c in arg):
        return arg
    return "'" + arg.replace("'", "'\\''") + "'"


def command_line(args):
    """A command line from arguments, quoted for the shell."""
    return " ".join(shell_quote(a) for a in args)


# ───────────── .NET projects ─────────────

DEFAULT_DOTNET_VERSION = "10.0"


def dotnet_projects(root):
    """The `.csproj` files below `root` (up to 4 folders deep), shallowest first."""
    out = []

    def collect(directory, depth):
        if depth > 4:
            return
        try:
            entries = list(os.scandir(directory))
        except OSError:
            return
        for entry in entries:
            try:
                if entry.is_dir():
                    if entry.name not in ("bin", "obj", ".git", "node_modules", "target"):
                        collect(entry.path, depth + 1)
                elif entry.name.endswith(".csproj"):
                    out.append(entry.path)
            except OSError:
                continue

    collect(root, 0)
    out.sort(key=lambda p: (len(_parts(p)[0]), path_key(p)))
    return out


def _read_text(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read()


def is_app(project):
    """Whether the project builds an app (console, ASP.NET Core, worker) rather than a library."""
    try:
        lower = _read_text(project).lower()
    except OSError:
        return False
    return any(s in lower for s in ("<outputtype>exe</outputtype>", "<outputtype>winexe</outputtype>",
                                    "microsoft.net.sdk.web", "microsoft.net.sdk.worker"))


def xml_value(text, name):
    """The text of the first `<name>…</name>` element (None when missing or empty)."""
    open_tag = f"<{name}>"
    start = text.find(open_tag)
    if start < 0:
        return None
    start += len(open_tag)
    end = text.find(f"</{name}>", start)
    if end < 0:
        return None
    value = text[start:end].strip()
    return value or None


def attribute_values(text, element, attribute):
    """Values of `attribute` on every `<element …>` tag."""
    out = []
    open_tag = f"<{element}"
    rest = text
    while True:
        at = rest.find(open_tag)
        if at < 0:
            break
        rest = rest[at + len(open_tag):]
        close = rest.find(">")
        tag = rest[:close if close >= 0 else len(rest)]
        key = f"{attribute}="
        k = tag.find(key)
        if k >= 0:
            value = tag[k + len(key):]
            if value[:1] in ('"', "'"):
                end = value[1:].find(value[0])
                if end >= 0:
                    out.append(value[1:1 + end])
    return out


def normalize(path):
    """Resolves `.` and `..` without touching the file system."""
    return os.path.normpath(path)


class DotnetProject:
    """What a Dockerfile for a .NET project needs to know about it."""

    def __init__(self, path, assembly, version, web):
        self.path = path
        self.assembly = assembly  # the assembly the container runs
        self.version = version    # the .NET version of the base images, e.g. "9.0"
        self.web = web            # ASP.NET Core: the `aspnet` base image and exposed ports

    @staticmethod
    def read(path):
        text = _read_text(path)  # raises OSError
        stem = os.path.splitext(os.path.basename(path))[0] or "App"
        assembly = xml_value(text, "AssemblyName")
        if not assembly or "$" in assembly:
            assembly = stem
        frameworks = xml_value(text, "TargetFramework") or xml_value(text, "TargetFrameworks") or ""
        # The highest modern `netX.Y` (OS-specific ones like net9.0-windows can't run in Linux containers).
        versions = []
        for framework in frameworks.split(";"):
            m = re.fullmatch(r"net\+?([0-9]+)\.\+?([0-9]+)", framework.strip())
            if m and int(m.group(1)) >= 5:
                versions.append((int(m.group(1)), int(m.group(2))))
        version = "%d.%d" % max(versions) if versions else DEFAULT_DOTNET_VERSION
        lower = text.lower()
        web = any(s in lower for s in ("microsoft.net.sdk.web", "microsoft.net.sdk.razor", "microsoft.net.sdk.blazorwebassembly"))
        return DotnetProject(path, assembly, version, web)

    def references(self):
        """The projects this one references, transitively (`<ProjectReference Include="…">`), excluding itself."""
        seen = [normalize(self.path)]
        queue = [self.path]
        out = []
        while queue:
            project = queue.pop()
            try:
                text = _read_text(project)
            except OSError:
                continue
            directory = os.path.dirname(project) or "."
            for include in attribute_values(text, "ProjectReference", "Include"):
                path = normalize(os.path.join(directory, include.replace("\\", "/")))
                if path not in seen:
                    seen.append(path)
                    out.append(path)
                    queue.append(path)
        out.sort(key=path_key)
        return out


def dotnet_dockerfile(project, context):
    """A multi-stage Dockerfile for `project`, built with `context` as the build context (Visual Studio's layout:
    restore with only the project files first so that layer is cached, then build and publish)."""
    v = project.version
    project_rel = relative(project.path, context)
    project_dir = posixpath.dirname(project_rel)
    project_file = posixpath.basename(project_rel)

    def dir_slash(d):
        return "./" if not d else f"{d}/"

    s = "# Build:  docker build -f "
    s += dir_slash(project_dir)
    s += "Dockerfile -t "
    s += image_name(project.assembly)
    s += " .\n# (run from the build context: the folder that contains "
    s += "this file" if not project_dir else "the solution"
    s += ")\n\n"
    if project.web:
        s += f"FROM mcr.microsoft.com/dotnet/aspnet:{v} AS base\nUSER $APP_UID\nWORKDIR /app\nEXPOSE 8080\nEXPOSE 8081\n\n"
    else:
        s += f"FROM mcr.microsoft.com/dotnet/runtime:{v} AS base\nUSER $APP_UID\nWORKDIR /app\n\n"
    s += f"FROM mcr.microsoft.com/dotnet/sdk:{v} AS build\nARG BUILD_CONFIGURATION=Release\nWORKDIR /src\n"
    # Restore inputs first, so the restore layer is reused until a project or package version changes.
    for shared in ("Directory.Build.props", "Directory.Build.targets", "Directory.Packages.props", "NuGet.config",
                   "nuget.config", "global.json"):
        if os.path.isfile(os.path.join(context, shared)):
            s += f"COPY [\"{shared}\", \".\"]\n"
    copies = [project.path] + [r for r in project.references() if starts_with(r, context)]
    for path in copies:
        rel = relative(path, context)
        s += f"COPY [\"{rel}\", \"{dir_slash(posixpath.dirname(rel))}\"]\n"
    s += f"RUN dotnet restore \"./{project_rel}\"\nCOPY . .\nWORKDIR \"/src/{project_dir}\"\n"
    s += f"RUN dotnet build \"./{project_file}\" -c $BUILD_CONFIGURATION -o /app/build\n\n"
    s += "FROM build AS publish\nARG BUILD_CONFIGURATION=Release\n"
    s += f"RUN dotnet publish \"./{project_file}\" -c $BUILD_CONFIGURATION -o /app/publish /p:UseAppHost=false\n\n"
    s += f"FROM base AS final\nWORKDIR /app\nCOPY --from=publish /app/publish .\nENTRYPOINT [\"dotnet\", \"{project.assembly}.dll\"]\n"
    return s.replace("WORKDIR \"/src/\"", "WORKDIR \"/src\"")


# A `.dockerignore` for .NET solutions (keeps build output, IDE files and secrets out of the context).
DOTNET_DOCKERIGNORE = """**/.classpath
**/.dockerignore
**/.env
**/.git
**/.gitignore
**/.project
**/.settings
**/.toolstarget
**/.vs
**/.vscode
**/.idea
**/*.*proj.user
**/*.dbmdl
**/*.jfm
**/azds.yaml
**/bin
**/charts
**/docker-compose*
**/compose.y*ml
**/Dockerfile*
**/node_modules
**/npm-debug.log
**/obj
**/secrets.dev.yaml
**/values.dev.yaml
LICENSE
README.md
!**/.gitignore
!.git/HEAD
!.git/config
!.git/packed-refs
!.git/refs/heads/**
"""


def compose_file(services, directory):
    """A `compose.yaml` (in `directory`) that builds and runs `services` (dicts with name, dockerfile, context,
    web). Web services get host ports 8080, 8090, …"""
    s = "# Start: docker compose up --build    Stop: docker compose down\nservices:\n"
    port = 8080
    for service in services:
        context = relative(service["context"], directory)
        if not context:
            context = "."
        elif not context.startswith(".") and not context.startswith("/"):
            context = f"./{context}"
        dockerfile = relative(service["dockerfile"], service["context"])
        name = service["name"]
        s += f"  {name}:\n    image: {name}\n    build:\n      context: {context}\n      dockerfile: {dockerfile}\n"
        if service["web"]:
            s += f"    ports:\n      - \"{port}:8080\"\n    environment:\n      - ASPNETCORE_ENVIRONMENT=Development\n"
            port += 10
    return s


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


def show_output():
    send({"type": "showView", "id": "output"})


class Module:
    def __init__(self):
        self.lock = threading.RLock()
        self.workspace = None
        self.settings = {}
        self.active_path = None
        self.visible = False
        self.cli = None           # None until the first check; False when the docker CLI isn't installed
        self.engine_error = None  # why the engine couldn't be queried (usually: the daemon isn't running)
        self.containers = []
        self.images = []
        self.volumes = []
        self.networks = []
        self.missing_plugins = None  # (compose, buildx) missing, once known
        self.dockerfiles = []
        self.compose_files = []
        self.busy = None
        self.next_request = 1
        self.pending = {}         # requestId -> callback(answer)
        self.refresh_wanted = threading.Event()
        self.last_tree = None
        self.work = queue.Queue()

    # ── state → tree ──

    def running_count(self):
        return sum(1 for c in self.containers if is_running(c))

    def engine_text(self):
        if self.cli is None:
            return "Looking for Docker…"
        if self.cli is False:
            return "Docker isn't installed (no docker command found)."
        if self.engine_error is not None:
            kind = classify_engine_error(self.engine_error)
            if kind == "stopped":
                return "The Docker engine isn't running. See SETUP below."
            if kind == "permission":
                return "No permission to use Docker. See SETUP below."
            return "The Docker engine isn't reachable. Check your Docker context."
        return (f"{self.running_count()} running · {len(self.containers)} containers · {len(self.images)} images · "
                f"{len(self.volumes)} volumes · {len(self.networks)} networks")

    def file_item(self, path, compose):
        name = os.path.basename(path)
        parent = os.path.dirname(path)
        directory = relative(parent, self.workspace) if self.workspace and starts_with(parent, self.workspace) else ""
        if compose:
            actions = [{"id": "up", "icon": "play", "tooltip": "Compose Up (build and start in the background)"},
                       {"id": "down", "icon": "stop", "tooltip": "Compose Down (stop and remove the containers)"}]
        else:
            actions = [{"id": "build", "icon": "build", "tooltip": "Build Image"}]
        return {"id": f"file:{path}", "label": name, "description": directory, "tooltip": path,
                "icon": f"file:{name}", "actions": actions}

    def container_item(self, c, grouped):
        if c["state"] == "running":
            color = RUNNING_COLOR
        elif c["state"] in ("paused", "restarting", "created"):
            color = PAUSED_COLOR
        else:
            color = "foreground.muted"
        label = c["compose_service"] if grouped and c["compose_service"] else c["name"]
        tip = f"{c['name']}\n{c['status']}\nimage: {c['image']}\nid: {c['id']}"
        if c["ports"]:
            tip += f"\nports: {c['ports']}"
        actions = [{"id": "logs", "icon": "file", "tooltip": "Logs (follow in a terminal)"}]
        if is_running(c):
            actions += [{"id": "shell", "icon": "terminal", "tooltip": "Open a Shell in the Container"},
                        {"id": "pause", "icon": "pause", "tooltip": "Pause"},
                        {"id": "restart", "icon": "refresh", "tooltip": "Restart"},
                        {"id": "stop", "icon": "stop", "tooltip": "Stop"}]
        elif is_paused(c):
            actions.append({"id": "unpause", "icon": "play", "tooltip": "Unpause"})
        else:
            actions.append({"id": "start", "icon": "play", "tooltip": "Start"})
        actions.append({"id": "inspect", "icon": "info", "tooltip": "Inspect (in a terminal)"})
        actions.append({"id": "remove", "icon": "trash", "tooltip": "Remove"})
        return {"id": f"container:{c['id']}", "label": label,
                "description": c["ports"] if is_running(c) and c["ports"] else c["status"],
                "tooltip": tip, "icon": "breakpoint", "iconColor": color, "actions": actions,
                "expanded": False, "children": self.container_details(c)}

    def container_details(self, c):
        """The rows under a container: what it runs, its ports, volumes and networks."""
        base = f"detail:{c['id']}"

        def row(key, label, kind, icon, color=None, tooltip=None):
            item = {"id": f"{base}:{key}", "label": label, "description": kind, "tooltip": tooltip or label, "icon": icon}
            if color:
                item["iconColor"] = color
            return item

        rows = [row("image", c["image"], "image", "layers", "icon.docker"),
                row("status", c["status"] or c["state"], "status", "info",
                    tooltip=f"{c['status']}\ncreated {c['age']}" if c.get("age") else None)]
        for i, port in enumerate(split_list(c["ports"])):
            rows.append(row(f"port{i}", port, "port", "arrowDown" if "->" in port else "remove"))
        volume_names = {v["name"] for v in self.volumes}
        for i, mount in enumerate(c.get("mounts", [])):
            named = mount in volume_names
            shown = mount[:12] + "…" if is_anonymous_volume(mount) else mount
            rows.append(row(f"mount{i}", shown, "volume" if named else "bind mount", VOLUME_ICON, None, mount))
        for i, network in enumerate(c.get("networks", [])):
            rows.append(row(f"net{i}", network, "network", NETWORK_ICON))
        if c.get("command"):
            rows.append(row("command", c["command"], "command", "terminal"))
        return rows

    def users_of_volume(self, name):
        return [c["name"] for c in self.containers if name in c.get("mounts", [])]

    def users_of_network(self, name):
        return [c["name"] for c in self.containers if name in c.get("networks", [])]

    def volume_item(self, v):
        users = self.users_of_volume(v["name"])
        label = v["name"][:12] + "… (anonymous)" if v["anonymous"] else v["name"]
        description = f"used by {', '.join(users)}" if users else "unused"
        tip = f"{v['name']}\ndriver: {v['driver']}\nmountpoint: {v['mountpoint']}"
        if v["compose_project"]:
            tip += f"\ncompose project: {v['compose_project']}"
        tip += f"\nused by: {', '.join(users)}" if users else "\nnot used by any container"
        return {"id": f"volume:{v['name']}", "label": label, "description": description, "tooltip": tip,
                "icon": VOLUME_ICON, "iconColor": "icon.docker" if users else "foreground.muted",
                "actions": [{"id": "inspect", "icon": "info", "tooltip": "Inspect (in a terminal)"},
                            {"id": "remove", "icon": "trash", "tooltip": "Remove Volume"}]}

    def network_item(self, n):
        users = self.users_of_network(n["name"])
        builtin = n["name"] in BUILTIN_NETWORKS
        description = n["driver"] + (f" · {len(users)} container{'s' if len(users) != 1 else ''}" if users else "")
        tip = f"{n['name']}\nid: {n['id']}\ndriver: {n['driver']}\nscope: {n['scope']}"
        if users:
            tip += f"\ncontainers: {', '.join(users)}"
        actions = [{"id": "inspect", "icon": "info", "tooltip": "Inspect (in a terminal)"}]
        if not builtin:
            actions.append({"id": "remove", "icon": "trash", "tooltip": "Remove Network"})
        return {"id": f"network:{n['name']}", "label": n["name"], "description": description, "tooltip": tip,
                "icon": NETWORK_ICON, "iconColor": "foreground.muted" if builtin else "icon.docker", "actions": actions}

    def setup_items(self):
        """Rows that explain what's keeping Docker from working, each with a button that fixes it in a terminal."""
        items = []
        if self.cli is False:
            items.append({"id": "setup:install", "label": "Docker isn't installed",
                          "description": "install the docker package", "icon": "error", "iconColor": "errorForeground",
                          "tooltip": "No docker command was found on PATH. Install Docker Engine (Linux) or Docker "
                                     "Desktop, then refresh."})
            return items
        if self.cli is not True:
            return items
        group = docker_group()
        kind = classify_engine_error(self.engine_error) if self.engine_error is not None else None
        if kind == "stopped":
            item = {"id": "setup:engine", "label": "The Docker engine isn't running", "icon": "error",
                    "iconColor": "errorForeground", "tooltip": self.engine_error}
            if can_systemctl():
                item["description"] = "▶ start it and enable it at boot"
                item["actions"] = [{"id": "start-engine", "icon": "play",
                                    "tooltip": "Start Docker now and at every boot (sudo systemctl enable --now docker)"}]
            else:
                item["description"] = "start Docker Desktop"
            items.append(item)
        if group and not group[0]:
            items.append({"id": "setup:group", "label": "You aren't in the docker group",
                          "description": "▶ add yourself (needs sudo)", "icon": "warning",
                          "iconColor": "editorWarning.foreground",
                          "tooltip": "Without it, only root can talk to the Docker engine. ByteForge picks up the "
                                     "change right away; other programs after you log in again.",
                          "actions": [{"id": "add-group", "icon": "add",
                                       "tooltip": "Add yourself to the docker group (sudo usermod -aG docker $USER)"}]})
        elif kind == "permission":
            items.append({"id": "setup:permission", "label": "No permission to use the Docker socket",
                          "description": "check the socket's group", "icon": "error", "iconColor": "errorForeground",
                          "tooltip": self.engine_error})
        if group and group[0] and not group[1]:
            items.append({"id": "setup:relogin", "label": "Using the docker group through sg",
                          "description": "log in again to make it permanent", "icon": "info",
                          "tooltip": "You were added to the docker group after logging in. ByteForge runs docker "
                                     "through `sg docker` until you log out and back in."})
        if kind == "other":
            items.append({"id": "setup:other", "label": "The Docker engine isn't reachable",
                          "description": "check your Docker context", "icon": "error", "iconColor": "errorForeground",
                          "tooltip": self.engine_error})
        if self.missing_plugins and any(self.missing_plugins):
            compose, buildx = self.missing_plugins
            names = " and ".join(n for n, m in (("Compose", compose), ("Buildx", buildx)) if m)
            item = {"id": "setup:plugins", "label": f"Docker {names} {'is' if compose != buildx else 'are'} missing",
                    "icon": "warning", "iconColor": "editorWarning.foreground",
                    "tooltip": ("Compose Up/Down needs the Compose plugin. " if compose else "")
                               + ("Builds use the legacy builder without Buildx." if buildx else "")}
            manager = package_manager()
            if manager:
                packages = plugin_packages(manager[0], compose, buildx)
                item["description"] = f"▶ install {' '.join(packages)}"
                item["actions"] = [{"id": "install-plugins", "icon": "add",
                                    "tooltip": f"{manager[1]} {' '.join(packages)}"}]
            else:
                item["description"] = "install them with your package manager"
            items.append(item)
        return items

    def setup_action(self, item, action):
        if item == "setup:engine" and action in ("start-engine", "click") and can_systemctl():
            self.setup_terminal("start Docker", "sudo systemctl enable --now docker")
        elif item == "setup:group" and action in ("add-group", "click"):
            self.setup_terminal("docker group", 'sudo usermod -aG docker "$USER"')
        elif item == "setup:plugins" and action in ("install-plugins", "click") and self.missing_plugins:
            manager = package_manager()
            if manager:
                packages = plugin_packages(manager[0], *self.missing_plugins)
                with self.lock:
                    self.missing_plugins = None  # checked again on the next refresh
                self.setup_terminal("install Docker plugins", f"{manager[1]} {' '.join(packages)}")

    def setup_terminal(self, title, command):
        """Runs a setup command in a terminal (it asks for the sudo password there); the view picks up the result."""
        log(f"> {command}")
        send({"type": "runInTerminal", "title": title, "command": command})
        self.request_refresh()

    def image_item(self, image):
        reference = image_reference(image)
        used = sum(1 for c in self.containers if c["image"] in (reference, image["repository"], image["id"]))
        return {"id": image_key(image), "label": "<none>" if image["repository"] == "<none>" else reference,
                "description": f"{image['size']} · {used} container{'s' if used != 1 else ''}" if used else image["size"],
                "tooltip": f"{reference}\nid: {image['id']}\nsize: {image['size']}\ncreated {image['created']}",
                "icon": "layers", "iconColor": "icon.docker",
                "actions": [{"id": "run", "icon": "play", "tooltip": "Run and open a shell in it (ports published, removed when the shell exits)"},
                            {"id": "inspect", "icon": "info", "tooltip": "Inspect (in a terminal)"},
                            {"id": "remove", "icon": "trash", "tooltip": "Remove Image"}]}

    def tree(self):
        with self.lock:
            files = [self.file_item(p, True) for p in self.compose_files]
            files += [self.file_item(p, False) for p in self.dockerfiles]
            count = len(files)
            if not files:
                files = [{"id": "note:files", "label": "No Dockerfile yet: run \"Docker: Add Docker Support to Project…\""}]
            items = []
            setup = self.setup_items()
            if setup:
                items.append({"id": "section:setup", "label": "SETUP", "header": True, "count": len(setup),
                              "children": setup})
            items.append({"id": "section:files", "label": "WORKSPACE FILES", "header": True, "count": count,
                          "children": files})
            if self.cli is True and self.engine_error is None:
                children = []
                groups = {}
                for c in self.containers:
                    project = c["compose_project"]
                    if project is None:
                        children.append(self.container_item(c, False))
                        continue
                    if project not in groups:
                        groups[project] = {"id": f"group:{project}", "label": f"{project} (compose)", "icon": "layers",
                                           "iconColor": "icon.docker", "children": []}
                        children.append(groups[project])
                    groups[project]["children"].append(self.container_item(c, True))
                items.append({"id": "section:containers", "label": "CONTAINERS", "header": True,
                              "count": len(self.containers), "children": children})
                items.append({"id": "section:images", "label": "IMAGES", "header": True, "count": len(self.images),
                              "children": [self.image_item(i) for i in self.images]})
                items.append({"id": "section:volumes", "label": "VOLUMES", "header": True, "count": len(self.volumes),
                              "children": [self.volume_item(v) for v in self.volumes] or
                              [{"id": "note:volumes", "label": "No volumes"}],
                              "actions": [{"id": "prune-volumes", "icon": "trash",
                                           "tooltip": "Remove Unused Anonymous Volumes…"}]})
                items.append({"id": "section:networks", "label": "NETWORKS", "header": True,
                              "count": len(self.networks), "expanded": False,
                              "children": [self.network_item(n) for n in self.networks]})
            return {"type": "setTree", "view": VIEW, "items": items,
                    "message": f"docker {self.busy}…" if self.busy else self.engine_text(),
                    "error": self.engine_error, "badge": self.running_count(),
                    "actions": [{"id": "refresh", "icon": "refresh", "tooltip": "Refresh"},
                                {"id": "prune", "icon": "trash", "tooltip": "System Prune…"}]}

    def publish(self, force=False):
        tree = self.tree()
        text = json.dumps(tree, sort_keys=True)
        with self.lock:
            if not force and text == self.last_tree:
                return
            self.last_tree = text
        send(tree)

    # ── refreshing ──

    def scan_files(self):
        dockerfiles, compose = docker_files(workspace_files(self.workspace))
        with self.lock:
            self.dockerfiles, self.compose_files = dockerfiles, compose

    def refresh_now(self):
        """Re-reads the workspace's Docker files and the engine's containers and images (on the refresh thread)."""
        with self.lock:
            if self.busy:
                return  # refreshed when the operation finishes
            check_cli = self.cli is not True
            check_plugins = self.missing_plugins is None or any(self.missing_plugins)
        self.scan_files()
        update_sg()
        cli = cli_available() if check_cli else True
        containers, images, volumes, networks, error = [], [], [], [], None
        plugins = None
        if cli:
            try:
                containers, images, volumes, networks = snapshot()
            except DockerError as e:
                error = str(e)
            if check_plugins:
                compose = self.compose_command()
                compose_ok = plugin_works(["compose", "version"]) if compose == "docker compose" \
                    else shutil.which(compose.split()[0]) is not None
                plugins = (not compose_ok, not plugin_works(["buildx", "version"]))
        with self.lock:
            self.cli = cli
            self.containers, self.images, self.engine_error = containers, images, error
            self.volumes, self.networks = volumes, networks
            if plugins is not None:
                self.missing_plugins = plugins
        self.publish()

    def refresher(self):
        """Refreshes when asked to, and every POLL_SECONDS while the view is shown."""
        while True:
            asked = self.refresh_wanted.wait(POLL_SECONDS)
            self.refresh_wanted.clear()
            if asked or self.visible:
                try:
                    self.refresh_now()
                except Exception as error:  # never let the thread die
                    log(f"docker: refresh failed: {error}")

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

    def prompt(self, message, detail, buttons, on_first):
        """A confirmation prompt; `on_first` runs when the first button is chosen."""
        self.ask({"type": "prompt", "message": message, "detail": detail, "buttons": buttons},
                 lambda index: on_first() if index == 0 else None)

    def quick_pick(self, placeholder, items, on_pick):
        self.ask({"type": "quickPick", "placeholder": placeholder, "items": items},
                 lambda value: on_pick(value) if value is not None else None)

    def answer(self, request, value):
        with self.lock:
            callback = self.pending.pop(request, None)
        if callback:
            callback(value)

    # ── quick operations ──

    def run_op(self, label, args):
        """Runs a quick docker command in the background, then logs the result and refreshes."""
        with self.lock:
            if self.busy:
                return
            self.busy = label
        self.publish()

        def work():
            try:
                out = run_docker(args).strip()
                log(f"docker {label}: {out}" if out else f"docker {label}: done")
            except DockerError as error:
                log(f"docker {label} failed: {error}")
            finally:
                with self.lock:
                    self.busy = None
                self.publish()
                self.request_refresh()

        threading.Thread(target=work, daemon=True).start()

    def container_op(self, verb, c):
        args = {"rm": ["rm", "--force", c["id"]]}.get(verb, [verb, c["id"]])
        self.run_op(f"{verb} {c['name']}", args)

    def terminal(self, title, args, cwd=None, save_all=False):
        message = {"type": "runInTerminal", "title": title, "command": terminal_line(args)}
        if cwd:
            message["cwd"] = cwd
        if save_all:
            message["saveAll"] = True
        send(message)

    # ── tree actions ──

    def find_container(self, container_id):
        with self.lock:
            return next((c for c in self.containers if c["id"] == container_id), None)

    def find_image(self, key):
        with self.lock:
            return next((i for i in self.images if image_key(i) == key), None)

    def tree_action(self, item, action):
        if item is None:
            if action == "refresh":
                self.request_refresh()
            elif action == "prune":
                self.prune()
            return
        if not isinstance(item, str):
            return
        if item.startswith("setup:"):
            self.setup_action(item, action)
        elif item == "section:volumes" and action == "prune-volumes":
            self.prompt("Remove all anonymous volumes no container uses?",
                        "Named volumes are kept. Their data is lost; this cannot be undone.", ["Prune", "Cancel"],
                        lambda: self.run_op("volume prune", ["volume", "prune", "--force"]))
        elif item.startswith("volume:"):
            self.volume_action(item[len("volume:"):], action)
        elif item.startswith("network:"):
            self.network_action(item[len("network:"):], action)
        elif item.startswith("file:"):
            path = item[len("file:"):]
            if action == "click":
                send({"type": "openFile", "path": path})
            elif action == "build":
                self.build(path)
            elif action in ("up", "down"):
                self.compose(path, action == "up")
        elif item.startswith("container:"):
            c = self.find_container(item[len("container:"):])
            if c:
                self.container_action(c, action)
        elif item.startswith("image:"):
            image = self.find_image(item)
            if image:
                reference = image_reference(image)
                if action == "run":
                    self.terminal(f"run: {reference}", run_shell_args(reference))
                elif action == "inspect":
                    self.terminal(f"inspect: {reference}", ["docker", "image", "inspect", reference])
                elif action == "remove":
                    self.prompt(f"Remove image '{reference}'?", "Images used by a container can't be removed.",
                                ["Remove", "Cancel"], lambda: self.run_op(f"rmi {reference}", ["rmi", reference]))

    def volume_action(self, name, action):
        if action == "inspect":
            self.terminal(f"volume: {name[:12]}", ["docker", "volume", "inspect", name])
        elif action == "remove":
            with self.lock:
                users = self.users_of_volume(name)
            if users:
                log(f"Volume {name} is used by {', '.join(users)}; remove those containers first.")
                show_output()
                return
            self.prompt(f"Remove volume '{name}'?", "Its data is lost. This cannot be undone.", ["Remove", "Cancel"],
                        lambda: self.run_op(f"volume rm {name[:12]}", ["volume", "rm", name]))

    def network_action(self, name, action):
        if action == "inspect":
            self.terminal(f"network: {name}", ["docker", "network", "inspect", name])
        elif action == "remove" and name not in BUILTIN_NETWORKS:
            self.prompt(f"Remove network '{name}'?", "Containers still attached to it keep it from being removed.",
                        ["Remove", "Cancel"], lambda: self.run_op(f"network rm {name}", ["network", "rm", name]))

    def container_action(self, c, action):
        if action == "inspect":
            self.terminal(f"inspect: {c['name']}", ["docker", "inspect", c["id"]])
        elif action == "logs":
            self.terminal(f"logs: {c['name']}", ["docker", "logs", "--follow", "--tail", "500", c["id"]])
        elif action == "shell":
            # bash when the image has it, else sh (Alpine, distroless debug images, …).
            self.terminal(f"shell: {c['name']}", ["docker", "exec", "-it", c["id"], "sh", "-c", SHELL_SCRIPT])
        elif action in ("start", "stop", "restart", "pause", "unpause"):
            self.container_op(action, c)
        elif action == "remove":
            detail = ("The container is running; it will be stopped first." if is_running(c)
                      else "Its file system changes are lost (volumes are kept).")
            self.prompt(f"Remove container '{c['name']}'?", detail, ["Remove", "Cancel"],
                        lambda: self.container_op("rm", c))

    def prune(self):
        """`docker system prune` after confirmation (stopped containers, dangling images, unused networks)."""
        self.prompt("Remove all stopped containers, unused networks, dangling images and the build cache?",
                    "Volumes are kept. This cannot be undone.", ["Prune", "Cancel"],
                    lambda: self.run_op("system prune", ["system", "prune", "--force"]))

    # ── build and compose ──

    def compose_command(self):
        value = self.settings.get("docker.composeCommand") if isinstance(self.settings, dict) else None
        if value is None and isinstance(self.settings, dict) and isinstance(self.settings.get("docker"), dict):
            value = self.settings["docker"].get("composeCommand")
        return value if isinstance(value, str) and value.strip() else "docker compose"

    def build(self, dockerfile):
        """`docker build` of a Dockerfile in a terminal, tagged after its folder, from its build context."""
        root = self.workspace or os.path.dirname(dockerfile)
        context = build_context(dockerfile, root)
        tag = image_name_for(dockerfile)
        command = terminal_line(["docker", "build", "--file", dockerfile, "--tag", tag, context])
        log(f"> {command}")
        send({"type": "runInTerminal", "title": f"build: {tag}", "command": command, "saveAll": True})

    def compose(self, file, up):
        """`<compose> up --build --detach` (or `down`) for a Compose file in a terminal."""
        args = self.compose_command().split() + ["--file", file]
        args += ["up", "--build", "--detach"] if up else ["down"]
        command = terminal_line(args)
        log(f"> {command}")
        name = os.path.basename(os.path.dirname(file))
        message = {"type": "runInTerminal", "title": f"compose {'up' if up else 'down'}: {name}", "command": command}
        if up:
            message["saveAll"] = True
        send(message)
        # The containers show up (or go away) in the Docker view a moment later.
        send({"type": "showView", "id": VIEW})

    def pick_file(self, compose, up=True):
        """Build or Compose up/down: the active editor's file when it is one, the only one in the workspace, or one
        picked from a list."""
        matches = is_compose_file_name if compose else is_dockerfile_name
        run = (lambda p: self.compose(p, up)) if compose else self.build
        active = self.active_path
        if active and matches(os.path.basename(active)):
            run(active)
            return
        dockerfiles, compose_files = docker_files(workspace_files(self.workspace))
        files = compose_files if compose else dockerfiles
        if not files:
            log("No Compose file (compose.yaml, docker-compose.yml) in the workspace. Create one with \"Docker: Add Docker Compose File\"."
                if compose else "No Dockerfile in the workspace. Create one with \"Docker: Add Docker Support to Project…\".")
            show_output()
        elif len(files) == 1:
            run(files[0])
        else:
            if not compose:
                placeholder = "Select the Dockerfile to build"
            else:
                placeholder = "Select the Compose file to start" if up else "Select the Compose file to stop"
            items = [{"label": relative(p, self.workspace) if self.workspace else p, "value": p} for p in files]
            self.quick_pick(placeholder, items, run)

    # ── .NET Docker support ──

    def add_support(self):
        """Picks the .NET project to add a Dockerfile to (apps first), or uses the only one."""
        root = self.workspace
        if not root:
            log("Open a folder containing a .NET project first.")
            show_output()
            return
        projects = dotnet_projects(root)
        if not projects:
            log(f"No .csproj found in {root}.")
            show_output()
            return
        if len(projects) == 1:
            self.add_support_to(projects[0])
            return
        items = []
        for path in projects:
            app = is_app(path)
            try:
                web = DotnetProject.read(path).web
            except OSError:
                web = False
            has_dockerfile = os.path.exists(os.path.join(os.path.dirname(path), "Dockerfile"))
            kind = "web app" if web else "app" if app else "library"
            item = {"label": os.path.splitext(os.path.basename(path))[0], "value": path,
                    "hint": f"{kind} · has a Dockerfile" if has_dockerfile else kind}
            detail = relative(os.path.dirname(path), root)
            if detail:
                item["detail"] = detail
            items.append((not app, item))
        items.sort(key=lambda pair: pair[0])
        self.quick_pick("Select the project to add Docker support to", [item for _, item in items], self.add_support_to)

    def add_support_to(self, project):
        """Writes a Dockerfile next to `project` (asking before replacing one) and a `.dockerignore` in the build
        context: the workspace folder for solutions and projects that reference others, else the project's folder."""
        try:
            info = DotnetProject.read(project)
        except OSError as error:
            log(f"Could not read {project}: {error}")
            return
        project_dir = os.path.dirname(project)
        root = self.workspace if self.workspace and starts_with(project, self.workspace) else project_dir
        has_solution = _dir_has(root, ("sln", "slnx"))
        context = root if has_solution or info.references() else project_dir
        dockerfile = os.path.join(project_dir, "Dockerfile")
        text = dotnet_dockerfile(info, context)

        def write():
            ignore = os.path.join(context, ".dockerignore")
            try:
                with open(dockerfile, "w", encoding="utf-8", newline="\n") as f:
                    f.write(text)
                if not os.path.exists(ignore):
                    with open(ignore, "w", encoding="utf-8", newline="\n") as f:
                        f.write(DOTNET_DOCKERIGNORE)
            except OSError as error:
                log(f"Could not write {dockerfile}: {error}")
                show_output()
                return
            log(f"Added {dockerfile} ({'ASP.NET Core' if info.web else 'console app'} on .NET {info.version}, "
                f"build context {context}). Build it with \"Docker: Build Image…\".")
            self.request_refresh()
            send({"type": "openFile", "path": dockerfile})

        if not os.path.exists(dockerfile):
            write()
        else:
            self.prompt("This project already has a Dockerfile.", "Replace it with a generated one?",
                        ["Replace", "Cancel"], write)

    def add_compose(self):
        """Writes a `compose.yaml` in the workspace folder with a service per Dockerfile, or opens the existing one."""
        root = self.workspace
        if not root:
            log("Open a folder first.")
            show_output()
            return
        dockerfiles, compose_files = docker_files(workspace_files(root))
        existing = next((p for p in compose_files if os.path.normpath(os.path.dirname(p)) == os.path.normpath(root)), None)
        if existing:
            log(f"{existing} already exists.")
            send({"type": "openFile", "path": existing})
            return
        if not dockerfiles:
            log("The workspace has no Dockerfile yet. Run \"Docker: Add Docker Support to Project…\" first.")
            show_output()
            return
        services = compose_services(dockerfiles, root)
        path = os.path.join(root, "compose.yaml")
        try:
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                f.write(compose_file(services, root))
        except OSError as error:
            log(f"Could not write {path}: {error}")
            show_output()
            return
        log(f"Added {path} with {len(services)} service(s). Start it with \"Docker: Compose Up\".")
        self.request_refresh()
        send({"type": "openFile", "path": path})

    # ── messages ──

    def execute(self, command):
        if command == "docker.refresh":
            self.request_refresh()
        elif command == "docker.build":
            self.pick_file(False)
        elif command == "docker.composeUp":
            self.pick_file(True, True)
        elif command == "docker.composeDown":
            self.pick_file(True, False)
        elif command == "docker.addDockerSupport":
            self.add_support()
        elif command == "docker.addCompose":
            self.add_compose()
        elif command == "docker.prune":
            self.prune()

    def handle(self, message):
        kind = message.get("type")
        if kind == "activate":
            self.workspace = message.get("workspace") or None
            if isinstance(message.get("settings"), dict):
                self.settings = message["settings"]
            self.publish(force=True)
            if self.visible:
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
        elif kind == "promptResult":
            self.later(self.answer, message.get("requestId"), message.get("index"))
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
                self.dockerfiles, self.compose_files = [], []
            self.request_refresh() if self.visible else self.publish()
        elif name == "filesChanged":
            if self.visible:
                self.request_refresh()  # showing the view rescans anyway
        elif name == "settingsChanged":
            if isinstance(message.get("settings"), dict):
                self.settings = message["settings"]
        elif name == "editorChanged":
            path = message.get("path")
            self.active_path = path if isinstance(path, str) else None

    def later(self, function, *args):
        """Runs `function` on the worker thread: commands touch the file system, and the reader must stay
        responsive. One worker keeps them in the order they arrived."""
        self.work.put((function, args))

    def worker(self):
        while True:
            function, args = self.work.get()
            self.safely(function, *args)

    def safely(self, function, *args):
        try:
            function(*args)
        except Exception as error:  # a bug here must never take the module down
            log(f"docker: {error}")


def image_key(image):
    """A stable item id for an image (one image id can have several tags)."""
    return f"image:{image['id']}:{image_reference(image)}"


def compose_services(dockerfiles, root):
    """A Compose service per Dockerfile, with unique names, its build context and whether it's a web app."""
    names = set()
    services = []
    for dockerfile in dockerfiles:
        try:
            text = _read_text(dockerfile)
        except OSError:
            text = ""
        base = image_name_for(dockerfile)
        name, n = base, 2
        while name in names:
            name = f"{base}-{n}"
            n += 1
        names.add(name)
        services.append({"name": name, "dockerfile": dockerfile, "context": build_context(dockerfile, root),
                         "web": "EXPOSE" in text or "dotnet/aspnet" in text})
    return services


COMMANDS = [
    ("docker.refresh", "Refresh"),
    ("docker.build", "Build Image…"),
    ("docker.composeUp", "Compose Up"),
    ("docker.composeDown", "Compose Down"),
    ("docker.addDockerSupport", "Add Docker Support to Project…"),
    ("docker.addCompose", "Add Docker Compose File"),
    ("docker.prune", "System Prune…"),
]


def main():
    module = Module()
    for command, title in COMMANDS:
        send({"type": "registerCommand", "id": command, "title": title, "category": "Docker"})
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
