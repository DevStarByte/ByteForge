using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.Docker;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<DockerModule>(input, output, moduleDir, cancel);
}

/// <summary>A docker command that failed: its stderr (or stdout), or why it couldn't run.</summary>
public sealed class DockerException(string message) : Exception(message);

/// <summary>
/// Docker for ByteForge. A side bar view with the workspace's Dockerfiles and Compose files and the engine's
/// containers (with their ports, volumes and networks), images, volumes and networks, with the usual actions
/// (build, compose up/down, start/stop/restart, logs, shell, run, remove), plus "Add Docker Support" (a multi-stage
/// Dockerfile and .dockerignore for a .NET project) and "Add Docker Compose File".
/// Everything goes through the `docker` CLI, so contexts, credentials and Docker Desktop or Podman's `docker` shim
/// all apply. Long-running and interactive commands run in an IDE terminal; quick ones run in the background here.
/// The engine is polled every 4 seconds, but only while the view is shown. When Docker isn't usable, a SETUP
/// section says why (engine stopped, not in the docker group, Compose or Buildx missing) with a button that fixes
/// it in a terminal.
/// </summary>
public sealed class DockerModule : Module
{
    public const string View = "docker";
    const string RunningColor = "#3fb950";
    const string PausedColor = "#d29922";

    // Material Design Icons (Apache 2.0): database and lan.
    public const string VolumeIcon = "M12,3C7.58,3 4,4.79 4,7C4,9.21 7.58,11 12,11C16.42,11 20,9.21 20,7C20,4.79 16.42,3 12,3M4,9V12C4,14.21 " +
        "7.58,16 12,16C16.42,16 20,14.21 20,12V9C20,11.21 16.42,13 12,13C7.58,13 4,11.21 4,9M4,14V17C4,19.21 7.58,21 " +
        "12,21C16.42,21 20,19.21 20,17V14C20,16.21 16.42,18 12,18C7.58,18 4,16.21 4,14Z";
    public const string NetworkIcon = "M10,2C8.89,2 8,2.89 8,4V7C8,8.11 8.89,9 10,9H11V11H2V13H6V15H5C3.89,15 3,15.89 3,17V20C3,21.11 3.89,22 " +
        "5,22H9C10.11,22 11,21.11 11,20V17C11,15.89 10.11,15 9,15H8V13H16V15H15C13.89,15 13,15.89 13,17V20C13,21.11 " +
        "13.89,22 15,22H19C20.11,22 21,21.11 21,20V17C21,15.89 20.11,15 19,15H18V13H22V11H13V9H14C15.11,9 16,8.11 " +
        "16,7V4C16,2.89 15.11,2 14,2H10M10,4H14V7H10V4M5,17H9V20H5V17M15,17H19V20H15V17Z";

    public static readonly (string Id, string Title)[] Commands =
    [
        ("docker.refresh", "Refresh"),
        ("docker.build", "Build Image…"),
        ("docker.composeUp", "Compose Up"),
        ("docker.composeDown", "Compose Down"),
        ("docker.addDockerSupport", "Add Docker Support to Project…"),
        ("docker.addCompose", "Add Docker Compose File"),
        ("docker.prune", "System Prune…"),
    ];

    readonly DockerSystem system;
    readonly TimeSpan pollInterval;
    readonly Lock gate = new();
    readonly SemaphoreSlim refreshWanted = new(0, 1);

    volatile bool visible;
    volatile string? activePath;
    bool? cli;                 // null until the first check; false when the docker CLI isn't installed
    string? engineError;       // why the engine couldn't be queried (usually: the daemon isn't running)
    List<Container> containers = [];
    List<Image> images = [];
    List<Volume> volumes = [];
    List<Network> networks = [];
    (bool Compose, bool Buildx)? missingPlugins; // once known
    List<string> dockerfiles = [];
    List<string> composeFiles = [];
    string? busy;
    string? lastTree;
    // True while the user is listed in the `docker` group but this session doesn't have it yet (it was added after
    // logging in): docker then runs through `sg docker -c …`, so no logout is needed.
    volatile bool useSg;

    public DockerModule() : this(new DockerSystem(), TimeSpan.FromSeconds(4))
    {
    }

    /// <param name="system">Runs docker and the other programs, and knows the docker group.</param>
    /// <param name="pollInterval">How often the engine is polled while the view is shown.</param>
    public DockerModule(DockerSystem system, TimeSpan pollInterval)
    {
        this.system = system;
        this.pollInterval = pollInterval;
    }

    /// <summary>The open folder (null when none).</summary>
    string? Root => string.IsNullOrEmpty(Workspace) ? null : Workspace;

    // ───────────── lifecycle and messages ─────────────

    protected override Task OnStartAsync()
    {
        foreach (var (id, title) in Commands)
        {
            RegisterCommand(id, title, "Docker");
        }
        _ = Task.Run(RefresherAsync);
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync()
    {
        Publish(force: true);
        if (visible)
        {
            RequestRefresh();
        }
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        var view = message["view"] is JsonValue v && v.TryGetValue<string>(out var s) ? s : null;
        switch (name)
        {
            case "viewShown" when view == View:
                visible = true;
                RequestRefresh();
                break;
            case "viewHidden" when view == View:
                visible = false;
                break;
            case "folderOpened":
                lock (gate)
                {
                    dockerfiles = [];
                    composeFiles = [];
                }
                if (visible)
                {
                    RequestRefresh();
                }
                else
                {
                    Publish();
                }
                break;
            case "filesChanged":
                if (visible)
                {
                    RequestRefresh(); // showing the view rescans anyway
                }
                break;
            case "editorChanged":
                activePath = message["path"] is JsonValue p && p.TryGetValue<string>(out var path) ? path : null;
                break;
        }
        return Task.CompletedTask;
    }

    protected override async Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "docker.refresh":
                RequestRefresh();
                break;
            case "docker.build":
                await PickFile(false, true);
                break;
            case "docker.composeUp":
                await PickFile(true, true);
                break;
            case "docker.composeDown":
                await PickFile(true, false);
                break;
            case "docker.addDockerSupport":
                await AddSupport();
                break;
            case "docker.addCompose":
                AddCompose();
                break;
            case "docker.prune":
                await Prune();
                break;
        }
    }

    // ───────────── docker CLI ─────────────

    /// <summary>The argv that runs `docker &lt;args&gt;`, through `sg docker` when needed.</summary>
    public string[] DockerArgv(IEnumerable<string> args)
    {
        string[] argv = ["docker", .. args];
        return useSg ? ["sg", "docker", "-c", DockerData.CommandLine(argv)] : argv;
    }

    /// <summary>A shell command line for a terminal, through `sg docker` when needed.</summary>
    public string TerminalLine(IEnumerable<string> args)
    {
        var line = DockerData.CommandLine(args);
        return useSg ? DockerData.CommandLine(["sg", "docker", "-c", line]) : line;
    }

    /// <summary>Runs `docker &lt;args&gt;` and returns stdout; throws <see cref="DockerException"/> with stderr (or stdout) on failure.</summary>
    public async Task<string> RunDocker(string[] args, int timeoutSeconds = 300)
    {
        ProcessResult done;
        try
        {
            done = await system.RunAsync(DockerArgv(args), TimeSpan.FromSeconds(timeoutSeconds), Stopping);
        }
        catch (ModuleException e)
        {
            throw new DockerException($"could not run docker (is it installed?): {e.Message}");
        }
        catch (TimeoutException)
        {
            throw new DockerException($"docker {string.Join(" ", args)} timed out");
        }
        if (done.Success)
        {
            return done.Output;
        }
        var error = done.Error.Trim();
        throw new DockerException(error.Length > 0 ? error : done.Output.Trim());
    }

    async Task<bool> Works(string[] args)
    {
        try
        {
            await RunDocker(args, 30);
            return true;
        }
        catch (DockerException)
        {
            return false;
        }
    }

    void UpdateSg()
    {
        var group = system.DockerGroup();
        useSg = group is { Listed: true, Active: false } && system.Which("sg") != null;
    }

    bool CanSystemctl() => system.IsLinux && system.Which("systemctl") != null;

    /// <summary>The install command prefix for the system's package manager, or null.</summary>
    (string Tool, string Prefix)? PackageManager()
    {
        foreach (var (tool, prefix) in new[] { ("pacman", "sudo pacman -S --needed"), ("apt-get", "sudo apt-get install -y"), ("dnf", "sudo dnf install -y"), ("zypper", "sudo zypper install -y") })
        {
            if (system.Which(tool) != null)
            {
                return (tool, prefix);
            }
        }
        return null;
    }

    /// <summary>The command that runs Compose: the docker.composeCommand setting, else `docker compose`.</summary>
    public string ComposeCommand()
    {
        var value = Settings["docker.composeCommand"];
        if (value == null && Settings["docker"] is JsonObject docker)
        {
            value = docker["composeCommand"];
        }
        return value is JsonValue v && v.TryGetValue<string>(out var text) && text.Trim().Length > 0 ? text : "docker compose";
    }

    static string[] SplitWords(string text) => text.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);

    // ───────────── state → tree ─────────────

    int RunningCount() => containers.Count(c => c.Running);

    string EngineText()
    {
        if (cli == null)
        {
            return "Looking for Docker…";
        }
        if (cli == false)
        {
            return "Docker isn't installed (no docker command found).";
        }
        if (engineError != null)
        {
            return DockerData.ClassifyEngineError(engineError) switch
            {
                "stopped" => "The Docker engine isn't running. See SETUP below.",
                "permission" => "No permission to use Docker. See SETUP below.",
                _ => "The Docker engine isn't reachable. Check your Docker context.",
            };
        }
        return $"{RunningCount()} running · {containers.Count} containers · {images.Count} images · {volumes.Count} volumes · {networks.Count} networks";
    }

    static JsonObject Action(string id, string icon, string tooltip, string? label = null)
    {
        var action = new JsonObject { ["id"] = id, ["icon"] = icon, ["tooltip"] = tooltip };
        if (label != null)
        {
            action["label"] = label;
        }
        return action;
    }

    static JsonArray Array(IEnumerable<JsonNode> items) => [.. items];

    JsonObject FileItem(string path, bool compose)
    {
        var name = PyPath.Basename(path);
        var parent = PyPath.Dirname(path);
        var directory = Root is { } root && PyPath.StartsWith(parent, root) ? PyPath.Relative(parent, root) : "";
        JsonArray actions = compose
            ? [Action("up", "play", "Compose Up (build and start in the background)", "Compose Up"), Action("down", "stop", "Compose Down (stop and remove the containers)", "Compose Down")]
            : [Action("build", "build", "Build Image")];
        return new() { ["id"] = $"file:{path}", ["label"] = name, ["description"] = directory, ["tooltip"] = path, ["icon"] = $"file:{name}", ["actions"] = actions };
    }

    JsonObject ContainerItem(Container c, bool grouped)
    {
        var color = c.State == "running" ? RunningColor : c.State is "paused" or "restarting" or "created" ? PausedColor : "foreground.muted";
        var label = grouped && !string.IsNullOrEmpty(c.ComposeService) ? c.ComposeService : c.Name;
        var tip = $"{c.Name}\n{c.Status}\nimage: {c.Image}\nid: {c.Id}";
        if (c.Ports.Length > 0)
        {
            tip += $"\nports: {c.Ports}";
        }
        JsonArray actions = [Action("logs", "file", "Logs (follow in a terminal)", "View Logs")];
        if (c.Running)
        {
            actions.Add(Action("shell", "terminal", "Open a Shell in the Container", "Open Shell"));
            actions.Add(Action("pause", "pause", "Pause"));
            actions.Add(Action("restart", "refresh", "Restart"));
            actions.Add(Action("stop", "stop", "Stop"));
        }
        else if (c.Paused)
        {
            actions.Add(Action("unpause", "play", "Unpause"));
        }
        else
        {
            actions.Add(Action("start", "play", "Start"));
        }
        actions.Add(Action("inspect", "info", "Inspect (in a terminal)", "Inspect"));
        actions.Add(Action("remove", "trash", "Remove", "Remove…"));
        return new()
        {
            ["id"] = $"container:{c.Id}",
            ["label"] = label,
            ["description"] = c.Running && c.Ports.Length > 0 ? c.Ports : c.Status,
            ["tooltip"] = tip,
            ["icon"] = "breakpoint",
            ["iconColor"] = color,
            ["actions"] = actions,
            ["expanded"] = false,
            ["children"] = ContainerDetails(c),
        };
    }

    /// <summary>The rows under a container: what it runs, its ports, volumes and networks.</summary>
    JsonArray ContainerDetails(Container c)
    {
        var @base = $"detail:{c.Id}";
        JsonObject Row(string key, string label, string kind, string icon, string? color = null, string? tooltip = null)
        {
            var item = new JsonObject { ["id"] = $"{@base}:{key}", ["label"] = label, ["description"] = kind, ["tooltip"] = string.IsNullOrEmpty(tooltip) ? label : tooltip, ["icon"] = icon };
            if (!string.IsNullOrEmpty(color))
            {
                item["iconColor"] = color;
            }
            return item;
        }
        JsonArray rows =
        [
            Row("image", c.Image, "image", "layers", "icon.docker"),
            Row("status", c.Status.Length > 0 ? c.Status : c.State, "status", "info", tooltip: c.Age.Length > 0 ? $"{c.Status}\ncreated {c.Age}" : null),
        ];
        var ports = DockerData.SplitList(c.Ports);
        for (var i = 0; i < ports.Count; i++)
        {
            rows.Add(Row($"port{i}", ports[i], "port", ports[i].Contains("->") ? "arrowDown" : "remove"));
        }
        var volumeNames = volumes.Select(v => v.Name).ToHashSet();
        for (var i = 0; i < c.Mounts.Count; i++)
        {
            var mount = c.Mounts[i];
            var shown = DockerData.IsAnonymousVolume(mount) ? mount[..12] + "…" : mount;
            rows.Add(Row($"mount{i}", shown, volumeNames.Contains(mount) ? "volume" : "bind mount", VolumeIcon, null, mount));
        }
        for (var i = 0; i < c.Networks.Count; i++)
        {
            rows.Add(Row($"net{i}", c.Networks[i], "network", NetworkIcon));
        }
        if (c.Command.Length > 0)
        {
            rows.Add(Row("command", c.Command, "command", "terminal"));
        }
        return rows;
    }

    List<string> UsersOfVolume(string name) => [.. containers.Where(c => c.Mounts.Contains(name)).Select(c => c.Name)];

    List<string> UsersOfNetwork(string name) => [.. containers.Where(c => c.Networks.Contains(name)).Select(c => c.Name)];

    JsonObject VolumeItem(Volume v)
    {
        var users = UsersOfVolume(v.Name);
        var label = v.Anonymous ? v.Name[..12] + "… (anonymous)" : v.Name;
        var tip = $"{v.Name}\ndriver: {v.Driver}\nmountpoint: {v.Mountpoint}";
        if (!string.IsNullOrEmpty(v.ComposeProject))
        {
            tip += $"\ncompose project: {v.ComposeProject}";
        }
        tip += users.Count > 0 ? $"\nused by: {string.Join(", ", users)}" : "\nnot used by any container";
        return new()
        {
            ["id"] = $"volume:{v.Name}",
            ["label"] = label,
            ["description"] = users.Count > 0 ? $"used by {string.Join(", ", users)}" : "unused",
            ["tooltip"] = tip,
            ["icon"] = VolumeIcon,
            ["iconColor"] = users.Count > 0 ? "icon.docker" : "foreground.muted",
            ["actions"] = new JsonArray(Action("inspect", "info", "Inspect (in a terminal)", "Inspect"), Action("remove", "trash", "Remove Volume", "Remove…")),
        };
    }

    JsonObject NetworkItem(Network n)
    {
        var users = UsersOfNetwork(n.Name);
        var builtin = DockerData.BuiltinNetworks.Contains(n.Name);
        var description = n.Driver + (users.Count > 0 ? $" · {users.Count} container{(users.Count != 1 ? "s" : "")}" : "");
        var tip = $"{n.Name}\nid: {n.Id}\ndriver: {n.Driver}\nscope: {n.Scope}";
        if (users.Count > 0)
        {
            tip += $"\ncontainers: {string.Join(", ", users)}";
        }
        JsonArray actions = [Action("inspect", "info", "Inspect (in a terminal)", "Inspect")];
        if (!builtin)
        {
            actions.Add(Action("remove", "trash", "Remove Network", "Remove…"));
        }
        return new()
        {
            ["id"] = $"network:{n.Name}",
            ["label"] = n.Name,
            ["description"] = description,
            ["tooltip"] = tip,
            ["icon"] = NetworkIcon,
            ["iconColor"] = builtin ? "foreground.muted" : "icon.docker",
            ["actions"] = actions,
        };
    }

    JsonObject ImageItem(Image image)
    {
        var reference = image.Reference;
        var used = containers.Count(c => c.Image == reference || c.Image == image.Repository || c.Image == image.Id);
        return new()
        {
            ["id"] = image.Key,
            ["label"] = image.Repository == "<none>" ? "<none>" : reference,
            ["description"] = used > 0 ? $"{image.Size} · {used} container{(used != 1 ? "s" : "")}" : image.Size,
            ["tooltip"] = $"{reference}\nid: {image.Id}\nsize: {image.Size}\ncreated {image.Created}",
            ["icon"] = "layers",
            ["iconColor"] = "icon.docker",
            ["actions"] = new JsonArray(
                Action("run", "play", "Run and open a shell in it (ports published, removed when the shell exits)", "Run Interactive"),
                Action("inspect", "info", "Inspect (in a terminal)", "Inspect"),
                Action("remove", "trash", "Remove Image", "Remove…")),
        };
    }

    /// <summary>Rows that explain what's keeping Docker from working, each with a button that fixes it in a terminal.</summary>
    List<JsonObject> SetupItems()
    {
        var items = new List<JsonObject>();
        if (cli == false)
        {
            items.Add(new()
            {
                ["id"] = "setup:install",
                ["label"] = "Docker isn't installed",
                ["description"] = "install the docker package",
                ["icon"] = "error",
                ["iconColor"] = "errorForeground",
                ["tooltip"] = "No docker command was found on PATH. Install Docker Engine (Linux) or Docker Desktop, then refresh.",
            });
            return items;
        }
        if (cli != true)
        {
            return items;
        }
        var group = system.DockerGroup();
        var kind = engineError != null ? DockerData.ClassifyEngineError(engineError) : null;
        if (kind == "stopped")
        {
            var item = new JsonObject { ["id"] = "setup:engine", ["label"] = "The Docker engine isn't running", ["icon"] = "error", ["iconColor"] = "errorForeground", ["tooltip"] = engineError };
            if (CanSystemctl())
            {
                item["description"] = "▶ start it and enable it at boot";
                item["actions"] = new JsonArray(Action("start-engine", "play", "Start Docker now and at every boot (sudo systemctl enable --now docker)"));
            }
            else
            {
                item["description"] = "start Docker Desktop";
            }
            items.Add(item);
        }
        if (group is { Listed: false })
        {
            items.Add(new()
            {
                ["id"] = "setup:group",
                ["label"] = "You aren't in the docker group",
                ["description"] = "▶ add yourself (needs sudo)",
                ["icon"] = "warning",
                ["iconColor"] = "editorWarning.foreground",
                ["tooltip"] = "Without it, only root can talk to the Docker engine. ByteForge picks up the change right away; other programs after you log in again.",
                ["actions"] = new JsonArray(Action("add-group", "add", "Add yourself to the docker group (sudo usermod -aG docker $USER)")),
            });
        }
        else if (kind == "permission")
        {
            items.Add(new()
            {
                ["id"] = "setup:permission",
                ["label"] = "No permission to use the Docker socket",
                ["description"] = "check the socket's group",
                ["icon"] = "error",
                ["iconColor"] = "errorForeground",
                ["tooltip"] = engineError,
            });
        }
        if (group is { Listed: true, Active: false })
        {
            items.Add(new()
            {
                ["id"] = "setup:relogin",
                ["label"] = "Using the docker group through sg",
                ["description"] = "log in again to make it permanent",
                ["icon"] = "info",
                ["tooltip"] = "You were added to the docker group after logging in. ByteForge runs docker through `sg docker` until you log out and back in.",
            });
        }
        if (kind == "other")
        {
            items.Add(new()
            {
                ["id"] = "setup:other",
                ["label"] = "The Docker engine isn't reachable",
                ["description"] = "check your Docker context",
                ["icon"] = "error",
                ["iconColor"] = "errorForeground",
                ["tooltip"] = engineError,
            });
        }
        if (missingPlugins is { } missing && (missing.Compose || missing.Buildx))
        {
            var names = string.Join(" and ", new[] { ("Compose", missing.Compose), ("Buildx", missing.Buildx) }.Where(p => p.Item2).Select(p => p.Item1));
            var item = new JsonObject
            {
                ["id"] = "setup:plugins",
                ["label"] = $"Docker {names} {(missing.Compose != missing.Buildx ? "is" : "are")} missing",
                ["icon"] = "warning",
                ["iconColor"] = "editorWarning.foreground",
                ["tooltip"] = (missing.Compose ? "Compose Up/Down needs the Compose plugin. " : "") + (missing.Buildx ? "Builds use the legacy builder without Buildx." : ""),
            };
            if (PackageManager() is { } manager)
            {
                var packages = string.Join(" ", DockerData.PluginPackages(manager.Tool, missing.Compose, missing.Buildx));
                item["description"] = $"▶ install {packages}";
                item["actions"] = new JsonArray(Action("install-plugins", "add", $"{manager.Prefix} {packages}"));
            }
            else
            {
                item["description"] = "install them with your package manager";
            }
            items.Add(item);
        }
        return items;
    }

    /// <summary>The view's content (a setTree message).</summary>
    public JsonObject Tree()
    {
        lock (gate)
        {
            var files = composeFiles.Select(p => FileItem(p, true)).Concat(dockerfiles.Select(p => FileItem(p, false))).ToList();
            var count = files.Count;
            if (files.Count == 0)
            {
                files.Add(new() { ["id"] = "note:files", ["label"] = "No Dockerfile yet: run \"Docker: Add Docker Support to Project…\"" });
            }
            var items = new JsonArray();
            var setup = SetupItems();
            if (setup.Count > 0)
            {
                items.Add(new JsonObject { ["id"] = "section:setup", ["label"] = "SETUP", ["header"] = true, ["count"] = setup.Count, ["children"] = Array(setup) });
            }
            items.Add(new JsonObject { ["id"] = "section:files", ["label"] = "WORKSPACE FILES", ["header"] = true, ["count"] = count, ["children"] = Array(files) });
            if (cli == true && engineError == null)
            {
                var children = new JsonArray();
                var groups = new Dictionary<string, JsonArray>();
                foreach (var c in containers)
                {
                    if (c.ComposeProject is not { } project)
                    {
                        children.Add(ContainerItem(c, false));
                        continue;
                    }
                    if (!groups.TryGetValue(project, out var members))
                    {
                        members = [];
                        groups[project] = members;
                        children.Add(new JsonObject { ["id"] = $"group:{project}", ["label"] = $"{project} (compose)", ["icon"] = "layers", ["iconColor"] = "icon.docker", ["children"] = members });
                    }
                    members.Add(ContainerItem(c, true));
                }
                items.Add(new JsonObject { ["id"] = "section:containers", ["label"] = "CONTAINERS", ["header"] = true, ["count"] = containers.Count, ["children"] = children });
                items.Add(new JsonObject { ["id"] = "section:images", ["label"] = "IMAGES", ["header"] = true, ["count"] = images.Count, ["children"] = Array(images.Select(ImageItem)) });
                items.Add(new JsonObject
                {
                    ["id"] = "section:volumes",
                    ["label"] = "VOLUMES",
                    ["header"] = true,
                    ["count"] = volumes.Count,
                    ["children"] = volumes.Count > 0 ? Array(volumes.Select(VolumeItem)) : new JsonArray(new JsonObject { ["id"] = "note:volumes", ["label"] = "No volumes" }),
                    ["actions"] = new JsonArray(Action("prune-volumes", "trash", "Remove Unused Anonymous Volumes…")),
                });
                items.Add(new JsonObject { ["id"] = "section:networks", ["label"] = "NETWORKS", ["header"] = true, ["count"] = networks.Count, ["expanded"] = false, ["children"] = Array(networks.Select(NetworkItem)) });
            }
            return new()
            {
                ["type"] = "setTree",
                ["view"] = View,
                ["items"] = items,
                ["message"] = busy != null ? $"docker {busy}…" : EngineText(),
                ["error"] = engineError,
                ["badge"] = RunningCount(),
                ["actions"] = new JsonArray(Action("refresh", "refresh", "Refresh"), Action("prune", "trash", "System Prune…")),
            };
        }
    }

    /// <summary>Sends the tree when it changed (or always, with <paramref name="force"/>).</summary>
    void Publish(bool force = false)
    {
        var tree = Tree();
        var text = tree.ToJsonString();
        lock (gate)
        {
            if (!force && text == lastTree)
            {
                return;
            }
            lastTree = text;
        }
        Send(tree);
    }

    // ───────────── refreshing ─────────────

    void RequestRefresh()
    {
        try
        {
            refreshWanted.Release();
        }
        catch (SemaphoreFullException)
        {
            // already asked for
        }
    }

    /// <summary>Re-reads the workspace's Docker files and the engine's containers, images, volumes and networks.</summary>
    public async Task RefreshNowAsync()
    {
        bool checkCli, checkPlugins;
        lock (gate)
        {
            if (busy != null)
            {
                return; // refreshed when the operation finishes
            }
            checkCli = cli != true;
            checkPlugins = missingPlugins is not { } m || m.Compose || m.Buildx;
        }
        var (foundDockerfiles, foundCompose) = DockerData.DockerFiles(DockerData.WorkspaceFiles(Root));
        lock (gate)
        {
            dockerfiles = foundDockerfiles;
            composeFiles = foundCompose;
        }
        UpdateSg();
        var available = !checkCli || await Works(["--version"]);
        List<Container> newContainers = [];
        List<Image> newImages = [];
        List<Volume> newVolumes = [];
        List<Network> newNetworks = [];
        string? error = null;
        (bool, bool)? plugins = null;
        if (available)
        {
            try
            {
                newContainers = DockerData.ParseContainers(await RunDocker(["ps", "--all", "--no-trunc", "--format", "{{json .}}"], 60));
                newImages = DockerData.ParseImages(await RunDocker(["images", "--format", "{{json .}}"], 60));
                newVolumes = DockerData.ParseVolumes(await RunDocker(["volume", "ls", "--format", "{{json .}}"], 60));
                newNetworks = DockerData.ParseNetworks(await RunDocker(["network", "ls", "--no-trunc", "--format", "{{json .}}"], 60));
            }
            catch (DockerException e)
            {
                newContainers = [];
                newImages = [];
                newVolumes = [];
                newNetworks = [];
                error = e.Message;
            }
            if (checkPlugins)
            {
                var compose = ComposeCommand();
                var composeOk = compose == "docker compose" ? await Works(["compose", "version"]) : system.Which(SplitWords(compose)[0]) != null;
                plugins = (!composeOk, !await Works(["buildx", "version"]));
            }
        }
        lock (gate)
        {
            cli = available;
            containers = newContainers;
            images = newImages;
            volumes = newVolumes;
            networks = newNetworks;
            engineError = error;
            if (plugins != null)
            {
                missingPlugins = plugins;
            }
        }
        Publish();
    }

    /// <summary>Refreshes when asked to, and every poll interval while the view is shown.</summary>
    async Task RefresherAsync()
    {
        try
        {
            while (true)
            {
                var asked = await refreshWanted.WaitAsync(pollInterval, Stopping);
                if (asked || visible)
                {
                    try
                    {
                        await RefreshNowAsync();
                    }
                    catch (Exception e) when (e is not OperationCanceledException)
                    {
                        Log($"docker: refresh failed: {e.Message}"); // never let the refresher die
                    }
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    // ───────────── quick operations ─────────────

    /// <summary>Runs a quick docker command, then logs the result and refreshes.</summary>
    async Task RunOp(string label, string[] args)
    {
        lock (gate)
        {
            if (busy != null)
            {
                return;
            }
            busy = label;
        }
        Publish();
        try
        {
            var output = (await RunDocker(args)).Trim();
            Log(output.Length > 0 ? $"docker {label}: {output}" : $"docker {label}: done");
        }
        catch (DockerException e)
        {
            Log($"docker {label} failed: {e.Message}");
        }
        finally
        {
            lock (gate)
            {
                busy = null;
            }
            Publish();
            RequestRefresh();
        }
    }

    Task ContainerOp(string verb, Container c) => RunOp($"{verb} {c.Name}", verb == "rm" ? ["rm", "--force", c.Id] : [verb, c.Id]);

    void Terminal(string title, IEnumerable<string> args) => RunInTerminal(title, TerminalLine(args));

    /// <summary>A confirmation prompt; true when the first button was chosen.</summary>
    async Task<bool> Confirm(string message, string detail, params string[] buttons) => await PromptAsync(message, detail, buttons) == 0;

    // ───────────── tree actions ─────────────

    protected override async Task OnTreeActionAsync(string view, string? item, string action)
    {
        if (view != View)
        {
            return;
        }
        if (item == null)
        {
            if (action == "refresh")
            {
                RequestRefresh();
            }
            else if (action == "prune")
            {
                await Prune();
            }
            return;
        }
        if (item.StartsWith("setup:", StringComparison.Ordinal))
        {
            SetupAction(item, action);
        }
        else if (item == "section:volumes" && action == "prune-volumes")
        {
            if (await Confirm("Remove all anonymous volumes no container uses?", "Named volumes are kept. Their data is lost; this cannot be undone.", "Prune", "Cancel"))
            {
                await RunOp("volume prune", ["volume", "prune", "--force"]);
            }
        }
        else if (item.StartsWith("volume:", StringComparison.Ordinal))
        {
            await VolumeAction(item["volume:".Length..], action);
        }
        else if (item.StartsWith("network:", StringComparison.Ordinal))
        {
            await NetworkAction(item["network:".Length..], action);
        }
        else if (item.StartsWith("file:", StringComparison.Ordinal))
        {
            var path = item["file:".Length..];
            if (action == "click")
            {
                OpenFile(path);
            }
            else if (action == "build")
            {
                Build(path);
            }
            else if (action is "up" or "down")
            {
                Compose(path, action == "up");
            }
        }
        else if (item.StartsWith("container:", StringComparison.Ordinal))
        {
            Container? c;
            lock (gate)
            {
                c = containers.FirstOrDefault(c => c.Id == item["container:".Length..]);
            }
            if (c != null)
            {
                await ContainerAction(c, action);
            }
        }
        else if (item.StartsWith("image:", StringComparison.Ordinal))
        {
            Image? image;
            lock (gate)
            {
                image = images.FirstOrDefault(i => i.Key == item);
            }
            if (image != null)
            {
                var reference = image.Reference;
                if (action == "run")
                {
                    Terminal($"run: {reference}", DockerData.RunShellArgs(reference));
                }
                else if (action == "inspect")
                {
                    Terminal($"inspect: {reference}", ["docker", "image", "inspect", reference]);
                }
                else if (action == "remove" && await Confirm($"Remove image '{reference}'?", "Images used by a container can't be removed.", "Remove", "Cancel"))
                {
                    await RunOp($"rmi {reference}", ["rmi", reference]);
                }
            }
        }
    }

    void SetupAction(string item, string action)
    {
        if (item == "setup:engine" && action is "start-engine" or "click" && CanSystemctl())
        {
            SetupTerminal("start Docker", "sudo systemctl enable --now docker");
        }
        else if (item == "setup:group" && action is "add-group" or "click")
        {
            SetupTerminal("docker group", "sudo usermod -aG docker \"$USER\"");
        }
        else if (item == "setup:plugins" && action is "install-plugins" or "click" && missingPlugins is { } missing)
        {
            if (PackageManager() is { } manager)
            {
                var packages = DockerData.PluginPackages(manager.Tool, missing.Compose, missing.Buildx);
                lock (gate)
                {
                    missingPlugins = null; // checked again on the next refresh
                }
                SetupTerminal("install Docker plugins", $"{manager.Prefix} {string.Join(" ", packages)}");
            }
        }
    }

    /// <summary>Runs a setup command in a terminal (it asks for the sudo password there); the view picks up the result.</summary>
    void SetupTerminal(string title, string command)
    {
        Log($"> {command}");
        RunInTerminal(title, command);
        RequestRefresh();
    }

    async Task VolumeAction(string name, string action)
    {
        if (action == "inspect")
        {
            Terminal($"volume: {Short(name)}", ["docker", "volume", "inspect", name]);
        }
        else if (action == "remove")
        {
            List<string> users;
            lock (gate)
            {
                users = UsersOfVolume(name);
            }
            if (users.Count > 0)
            {
                Log($"Volume {name} is used by {string.Join(", ", users)}; remove those containers first.");
                ShowView("output");
                return;
            }
            if (await Confirm($"Remove volume '{name}'?", "Its data is lost. This cannot be undone.", "Remove", "Cancel"))
            {
                await RunOp($"volume rm {Short(name)}", ["volume", "rm", name]);
            }
        }
    }

    static string Short(string name) => name.Length > 12 ? name[..12] : name;

    async Task NetworkAction(string name, string action)
    {
        if (action == "inspect")
        {
            Terminal($"network: {name}", ["docker", "network", "inspect", name]);
        }
        else if (action == "remove" && !DockerData.BuiltinNetworks.Contains(name)
            && await Confirm($"Remove network '{name}'?", "Containers still attached to it keep it from being removed.", "Remove", "Cancel"))
        {
            await RunOp($"network rm {name}", ["network", "rm", name]);
        }
    }

    async Task ContainerAction(Container c, string action)
    {
        switch (action)
        {
            case "inspect":
                Terminal($"inspect: {c.Name}", ["docker", "inspect", c.Id]);
                break;
            case "logs":
                Terminal($"logs: {c.Name}", ["docker", "logs", "--follow", "--tail", "500", c.Id]);
                break;
            case "shell":
                // bash when the image has it, else sh (Alpine, distroless debug images, …).
                Terminal($"shell: {c.Name}", ["docker", "exec", "-it", c.Id, "sh", "-c", DockerData.ShellScript]);
                break;
            case "start" or "stop" or "restart" or "pause" or "unpause":
                await ContainerOp(action, c);
                break;
            case "remove":
                var detail = c.Running ? "The container is running; it will be stopped first." : "Its file system changes are lost (volumes are kept).";
                if (await Confirm($"Remove container '{c.Name}'?", detail, "Remove", "Cancel"))
                {
                    await ContainerOp("rm", c);
                }
                break;
        }
    }

    /// <summary>`docker system prune` after confirmation (stopped containers, dangling images, unused networks).</summary>
    async Task Prune()
    {
        if (await Confirm("Remove all stopped containers, unused networks, dangling images and the build cache?", "Volumes are kept. This cannot be undone.", "Prune", "Cancel"))
        {
            await RunOp("system prune", ["system", "prune", "--force"]);
        }
    }

    // ───────────── build and compose ─────────────

    /// <summary>`docker build` of a Dockerfile in a terminal, tagged after its folder, from its build context.</summary>
    void Build(string dockerfile)
    {
        var root = Root ?? PyPath.Dirname(dockerfile);
        var context = DockerData.BuildContext(dockerfile, root);
        var tag = DockerData.ImageNameFor(dockerfile);
        var command = TerminalLine(["docker", "build", "--file", dockerfile, "--tag", tag, context]);
        Log($"> {command}");
        RunInTerminal($"build: {tag}", command, saveAll: true);
    }

    /// <summary>`&lt;compose&gt; up --build --detach` (or `down`) for a Compose file in a terminal.</summary>
    void Compose(string file, bool up)
    {
        string[] args = [.. SplitWords(ComposeCommand()), "--file", file, .. up ? new[] { "up", "--build", "--detach" } : new[] { "down" }];
        var command = TerminalLine(args);
        Log($"> {command}");
        var name = PyPath.Basename(PyPath.Dirname(file));
        RunInTerminal($"compose {(up ? "up" : "down")}: {name}", command, saveAll: up);
        // The containers show up (or go away) in the Docker view a moment later.
        ShowView(View);
    }

    /// <summary>Build or Compose up/down: the active editor's file when it is one, the only one in the workspace, or one picked from a list.</summary>
    async Task PickFile(bool compose, bool up)
    {
        Func<string, bool> matches = compose ? DockerData.IsComposeFileName : DockerData.IsDockerfileName;
        void Run(string path)
        {
            if (compose)
            {
                Compose(path, up);
            }
            else
            {
                Build(path);
            }
        }
        if (activePath is { Length: > 0 } active && matches(PyPath.Basename(active)))
        {
            Run(active);
            return;
        }
        var (dockerfileList, composeList) = DockerData.DockerFiles(DockerData.WorkspaceFiles(Root));
        var files = compose ? composeList : dockerfileList;
        if (files.Count == 0)
        {
            Log(compose
                ? "No Compose file (compose.yaml, docker-compose.yml) in the workspace. Create one with \"Docker: Add Docker Compose File\"."
                : "No Dockerfile in the workspace. Create one with \"Docker: Add Docker Support to Project…\".");
            ShowView("output");
        }
        else if (files.Count == 1)
        {
            Run(files[0]);
        }
        else
        {
            var placeholder = !compose ? "Select the Dockerfile to build" : up ? "Select the Compose file to start" : "Select the Compose file to stop";
            var picked = await QuickPickAsync(placeholder, files.Select(p => new PickItem(Root is { } root ? PyPath.Relative(p, root) : p, p)));
            if (picked != null)
            {
                Run(picked);
            }
        }
    }

    // ───────────── .NET Docker support ─────────────

    /// <summary>Picks the .NET project to add a Dockerfile to (apps first), or uses the only one.</summary>
    async Task AddSupport()
    {
        if (Root is not { } root)
        {
            Log("Open a folder containing a .NET project first.");
            ShowView("output");
            return;
        }
        var projects = DockerData.DotnetProjects(root);
        if (projects.Count == 0)
        {
            Log($"No .csproj found in {root}.");
            ShowView("output");
            return;
        }
        if (projects.Count == 1)
        {
            await AddSupportTo(projects[0]);
            return;
        }
        var items = new List<(bool Library, PickItem Item)>();
        foreach (var path in projects)
        {
            var app = DockerData.IsApp(path);
            bool web;
            try
            {
                web = DotnetProject.Read(path).Web;
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                web = false;
            }
            var hasDockerfile = File.Exists(PyPath.Join(PyPath.Dirname(path), "Dockerfile")) || Directory.Exists(PyPath.Join(PyPath.Dirname(path), "Dockerfile"));
            var kind = web ? "web app" : app ? "app" : "library";
            var detail = PyPath.Relative(PyPath.Dirname(path), root);
            items.Add((!app, new PickItem(PyPath.Stem(PyPath.Basename(path)), path, detail.Length > 0 ? detail : null, hasDockerfile ? $"{kind} · has a Dockerfile" : kind)));
        }
        var picked = await QuickPickAsync("Select the project to add Docker support to", items.OrderBy(i => i.Library).Select(i => i.Item));
        if (picked != null)
        {
            await AddSupportTo(picked);
        }
    }

    /// <summary>
    /// Writes a Dockerfile next to <paramref name="project"/> (asking before replacing one) and a `.dockerignore` in
    /// the build context: the workspace folder for solutions and projects that reference others, else the project's folder.
    /// </summary>
    async Task AddSupportTo(string project)
    {
        DotnetProject info;
        try
        {
            info = DotnetProject.Read(project);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"Could not read {project}: {e.Message}");
            return;
        }
        var projectDir = PyPath.Dirname(project);
        var root = Root is { } workspace && PyPath.StartsWith(project, workspace) ? workspace : projectDir;
        var context = DockerData.HasSolution(root) || info.References().Count > 0 ? root : projectDir;
        var dockerfile = PyPath.Join(projectDir, "Dockerfile");
        var text = DockerData.DotnetDockerfile(info, context);
        if (File.Exists(dockerfile) || Directory.Exists(dockerfile))
        {
            if (!await Confirm("This project already has a Dockerfile.", "Replace it with a generated one?", "Replace", "Cancel"))
            {
                return;
            }
        }
        var ignore = PyPath.Join(context, ".dockerignore");
        try
        {
            File.WriteAllText(dockerfile, text);
            if (!File.Exists(ignore) && !Directory.Exists(ignore))
            {
                File.WriteAllText(ignore, DockerData.DotnetDockerignore);
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"Could not write {dockerfile}: {e.Message}");
            ShowView("output");
            return;
        }
        Log($"Added {dockerfile} ({(info.Web ? "ASP.NET Core" : "console app")} on .NET {info.Version}, build context {context}). Build it with \"Docker: Build Image…\".");
        RequestRefresh();
        OpenFile(dockerfile);
    }

    /// <summary>Writes a `compose.yaml` in the workspace folder with a service per Dockerfile, or opens the existing one.</summary>
    void AddCompose()
    {
        if (Root is not { } root)
        {
            Log("Open a folder first.");
            ShowView("output");
            return;
        }
        var (dockerfileList, composeList) = DockerData.DockerFiles(DockerData.WorkspaceFiles(root));
        var existing = composeList.FirstOrDefault(p => PyPath.Same(PyPath.Dirname(p), root));
        if (existing != null)
        {
            Log($"{existing} already exists.");
            OpenFile(existing);
            return;
        }
        if (dockerfileList.Count == 0)
        {
            Log("The workspace has no Dockerfile yet. Run \"Docker: Add Docker Support to Project…\" first.");
            ShowView("output");
            return;
        }
        var services = DockerData.ComposeServices(dockerfileList, root);
        var path = PyPath.Join(root, "compose.yaml");
        try
        {
            File.WriteAllText(path, DockerData.ComposeFile(services, root));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"Could not write {path}: {e.Message}");
            ShowView("output");
            return;
        }
        Log($"Added {path} with {services.Count} service(s). Start it with \"Docker: Compose Up\".");
        RequestRefresh();
        OpenFile(path);
    }
}
