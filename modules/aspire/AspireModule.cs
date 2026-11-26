using System.Diagnostics;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.Aspire;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<AspireModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// A side bar view with the workspace's Aspire AppHosts and, while one runs, its resources (state, health,
/// endpoints) with start / stop / restart and logs, plus commands for the rest of the aspire CLI: new, init, add,
/// update, restore, publish, deploy and doctor.
/// Everything goes through the aspire CLI, the way the VS Code extension does it: <c>aspire ps --format json</c>
/// lists the running AppHosts, <c>aspire describe --format json --apphost &lt;path&gt;</c> their resources, and
/// <c>aspire resource &lt;name&gt; &lt;command&gt; --non-interactive --apphost &lt;path&gt;</c> runs a resource
/// command. <c>aspire run</c> and the other interactive commands run in an IDE terminal. Running AppHosts are polled
/// every few seconds, but only while the view is shown.
/// </summary>
public sealed class AspireModule : Module
{
    public const string View = "aspire";
    const string Purple = "#7455dd";
    const string RunningColor = "#3fb950";
    const string WarningColor = "#d29922";
    const string ErrorColor = "#f85149";
    const string IdleColor = "#8b949e";
    public const string InstallScript = "curl -sSL https://aspire.dev/install.sh | bash";
    public const string InstallScriptWindows = "powershell -NoProfile -Command \"iex \\\"& { $(irm https://aspire.dev/install.ps1) }\\\"\"";

    public static readonly (string Id, string Title)[] Commands =
    [
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
    ];

    static readonly Dictionary<string, string> StatusColors = new()
    {
        ["running"] = RunningColor, ["busy"] = WarningColor, ["warning"] = WarningColor, ["error"] = ErrorColor, ["idle"] = IdleColor,
    };

    static readonly Dictionary<string, string> StatusIcons = new()
    {
        ["running"] = "check", ["busy"] = "sync", ["warning"] = "warning", ["error"] = "error", ["idle"] = "stop",
    };

    readonly Func<JsonObject, string?> findCli;
    readonly CliRunner runCli;
    readonly Action<string> openUrl;
    readonly TimeSpan pollInterval;
    readonly SemaphoreSlim refreshWanted = new(0, 1);
    readonly Lock state = new();

    volatile bool visible;
    bool cliChecked;          // the CLI was looked for (cli null then: it isn't installed)
    string? cli;
    string? error;            // why `aspire ps` failed
    List<string> apphosts = [];
    Dictionary<string, RunningHost> running = [];
    string? busy;
    string? lastTree;

    public AspireModule() : this(null, null, null)
    {
    }

    /// <param name="findCli">Finds the aspire CLI for these settings (default: <see cref="AspireCli.FindCli"/>).</param>
    /// <param name="runCli">Runs it (default: <see cref="AspireCli.RunAsync"/>).</param>
    /// <param name="openUrl">Opens a URL in the browser.</param>
    /// <param name="pollInterval">How often the running AppHosts are polled while the view is shown (5 seconds).</param>
    public AspireModule(Func<JsonObject, string?>? findCli, CliRunner? runCli, Action<string>? openUrl, TimeSpan? pollInterval = null)
    {
        this.findCli = findCli ?? (settings => AspireCli.FindCli(settings));
        this.runCli = runCli ?? AspireCli.RunAsync;
        this.openUrl = openUrl ?? OpenInBrowser;
        this.pollInterval = pollInterval ?? TimeSpan.FromSeconds(5);
    }

    string? Ws => string.IsNullOrEmpty(Workspace) ? null : Workspace;

    // ───────────── messages from the IDE ─────────────

    protected override Task OnStartAsync()
    {
        foreach (var (id, title) in Commands)
        {
            RegisterCommand(id, title, "Aspire");
        }
        _ = Task.Run(RefresherAsync);
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync()
    {
        Publish(force: true);
        RequestRefresh();
        return Task.CompletedTask;
    }

    protected override Task OnSettingsChangedAsync()
    {
        RequestRefresh();
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        var view = AspireCli.Str(message["view"]);
        if (name == "viewShown" && view == View)
        {
            visible = true;
            RequestRefresh();
        }
        else if (name == "viewHidden" && view == View)
        {
            visible = false;
        }
        else if (name == "folderOpened")
        {
            lock (state)
            {
                apphosts = [];
            }
            RequestRefresh();
        }
        else if (name == "filesChanged" && visible)
        {
            RequestRefresh();
        }
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id) => Safely(() => Execute(id));

    protected override Task OnTreeActionAsync(string view, string? item, string action) =>
        view == View ? Safely(() => TreeAction(item, action)) : Task.CompletedTask;

    /// <summary>A bug here must never take the module down: it's logged.</summary>
    async Task Safely(Func<Task> work)
    {
        try
        {
            await work();
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            Log($"aspire: {e.Message}");
        }
    }

    // ───────────── state → tree ─────────────

    /// <summary>Workspace AppHosts first, then running ones from elsewhere, as (path, running info or null).</summary>
    List<(string Path, RunningHost? Info)> AllApphosts()
    {
        lock (state)
        {
            var hosts = apphosts.Select(p => (p, running.GetValueOrDefault(AspireWorkspace.PathKey(p)))).ToList();
            var known = apphosts.Select(AspireWorkspace.PathKey).ToHashSet();
            hosts.AddRange(running.Where(r => !known.Contains(r.Key)).OrderBy(r => r.Key, StringComparer.Ordinal).Select(r => (r.Value.Path, (RunningHost?)r.Value)));
            return hosts;
        }
    }

    static JsonObject Action(string id, string icon, string tooltip) => new() { ["id"] = id, ["icon"] = icon, ["tooltip"] = tooltip };

    static JsonObject ResourceItem(string host, JsonObject resource)
    {
        var status = AspireCli.ResourceStatus(resource);
        var name = AspireCli.DisplayName(resource);
        var state = Python.Truthy(resource["state"]) ? Python.Str(resource["state"]) : "Unknown";
        var type = resource["resourceType"];
        // Without a type the state is the description as the CLI gave it (even when it isn't text).
        JsonNode description = Python.Truthy(type) ? $"{state} · {Python.Str(type)}" : Python.Truthy(resource["state"]) ? resource["state"]!.DeepClone() : "Unknown";
        var tooltip = new List<string> { $"{name} ({(Python.Truthy(type) ? Python.Str(type) : "resource")})", $"State: {state}" };
        if (Python.Truthy(resource["healthStatus"]))
        {
            tooltip.Add($"Health: {Python.Str(resource["healthStatus"])}");
        }
        if (Python.NonZero(resource["exitCode"]))
        {
            tooltip.Add($"Exit code: {Python.Str(resource["exitCode"])}");
        }
        var key = $"res:{host}\n{AspireCli.Str(resource["name"])}";
        var actions = new JsonArray();
        if (AspireCli.CommandEnabled(resource, "start"))
        {
            actions.Add(Action("start", "play", "Start"));
        }
        if (AspireCli.CommandEnabled(resource, "restart"))
        {
            actions.Add(Action("restart", "refresh", "Restart"));
        }
        if (AspireCli.CommandEnabled(resource, "stop"))
        {
            actions.Add(Action("stop", "stop", "Stop"));
        }
        actions.Add(Action("logs", "terminal", "View Logs"));
        var children = new JsonArray();
        foreach (var (label, url) in AspireCli.ResourceUrls(resource))
        {
            children.Add(new JsonObject
            {
                ["id"] = $"url:{url}\n{key}", ["label"] = label.DeepClone(), ["description"] = url, ["tooltip"] = $"Open {url}", ["icon"] = "openPreview",
            });
        }
        var item = new JsonObject
        {
            ["id"] = key,
            ["label"] = name,
            ["description"] = description,
            ["tooltip"] = string.Join("\n", tooltip),
            ["icon"] = StatusIcons[status],
            ["iconColor"] = StatusColors[status],
            ["actions"] = actions,
        };
        if (children.Count > 0)
        {
            item["children"] = children;
            item["expanded"] = false;
        }
        return item;
    }

    JsonObject ApphostItem(string path, RunningHost? info)
    {
        var label = AspireWorkspace.ApphostLabel(path);
        var rel = AspireWorkspace.Relative(path, Ws);
        if (info == null)
        {
            return new JsonObject
            {
                ["id"] = $"host:{path}",
                ["label"] = label,
                ["description"] = rel,
                ["tooltip"] = $"{path}\nNot running",
                ["icon"] = "layers",
                ["iconColor"] = Purple,
                ["actions"] = new JsonArray(Action("run", "play", "Run AppHost")),
            };
        }
        var children = new JsonArray();
        var hasDashboard = !string.IsNullOrEmpty(info.Dashboard);
        if (hasDashboard)
        {
            children.Add(new JsonObject
            {
                ["id"] = $"dash:{path}", ["label"] = "Dashboard", ["description"] = info.Dashboard, ["tooltip"] = "Open the Aspire Dashboard",
                ["icon"] = "openPreview", ["iconColor"] = Purple,
            });
        }
        var resources = info.Resources;
        if (!string.IsNullOrEmpty(info.Error))
        {
            children.Add(new JsonObject
            {
                ["id"] = $"err:{path}", ["label"] = "Couldn't read the resources", ["icon"] = "warning", ["iconColor"] = WarningColor, ["tooltip"] = info.Error,
            });
        }
        else if (resources.Count == 0)
        {
            children.Add(new JsonObject { ["id"] = $"wait:{path}", ["label"] = "Waiting for resources…", ["icon"] = "sync" });
        }
        foreach (var resource in resources)
        {
            children.Add(ResourceItem(path, resource));
        }
        var up = resources.Count(r => AspireCli.ResourceStatus(r) == "running");
        var actions = new JsonArray();
        if (hasDashboard)
        {
            actions.Add(Action("dashboard", "openPreview", "Open Dashboard"));
        }
        actions.Add(Action("stop", "stop", "Stop AppHost"));
        return new JsonObject
        {
            ["id"] = $"host:{path}",
            ["label"] = label,
            ["description"] = resources.Count > 0 ? $"running · {up}/{resources.Count} up" : "running",
            ["tooltip"] = $"{path}\nRunning (pid {Python.Str(info.Pid)})",
            ["icon"] = "layers",
            ["iconColor"] = RunningColor,
            ["count"] = resources.Count,
            ["expanded"] = true,
            ["children"] = children,
            ["actions"] = actions,
        };
    }

    /// <summary>The view's setTree message for the current state.</summary>
    public JsonObject Tree()
    {
        lock (state)
        {
            var items = new JsonArray();
            if (cliChecked && cli == null)
            {
                items.Add(new JsonObject
                {
                    ["id"] = "section:setup", ["label"] = "SETUP", ["header"] = true, ["count"] = 1,
                    ["children"] = new JsonArray(new JsonObject
                    {
                        ["id"] = "setup:install", ["label"] = "Install the Aspire CLI", ["icon"] = "add", ["iconColor"] = Purple,
                        ["tooltip"] = "Runs the official install script (aspire.dev) in a terminal",
                        ["actions"] = new JsonArray(Action("install", "play", "Install")),
                    }),
                });
            }
            var hosts = AllApphosts();
            var children = new JsonArray([.. hosts.Select(h => (JsonNode)ApphostItem(h.Path, h.Info))]);
            if (children.Count == 0)
            {
                children.Add(new JsonObject { ["id"] = "note:none", ["label"] = "No AppHost here: run \"Aspire: New Aspire Project…\" or \"Aspire: Initialize Aspire in This Folder\"" });
            }
            items.Add(new JsonObject { ["id"] = "section:apphosts", ["label"] = "APP HOSTS", ["header"] = true, ["count"] = hosts.Count, ["children"] = children });
            string? message = null;
            if (!string.IsNullOrEmpty(busy))
            {
                message = $"aspire {busy}…";
            }
            else if (!cliChecked)
            {
                message = "Looking for the Aspire CLI…";
            }
            else if (cli == null)
            {
                message = "The Aspire CLI isn't installed. See SETUP below.";
            }
            else if (!string.IsNullOrEmpty(error))
            {
                message = "Couldn't list the running AppHosts.";
            }
            var tree = new JsonObject
            {
                ["type"] = "setTree",
                ["view"] = View,
                ["items"] = items,
                ["badge"] = running.Count,
                ["actions"] = new JsonArray(Action("new", "add", "New Aspire Project…"), Action("refresh", "refresh", "Refresh")),
            };
            if (message != null)
            {
                tree["message"] = message;
            }
            if (!string.IsNullOrEmpty(error))
            {
                tree["error"] = error;
            }
            return tree;
        }
    }

    void Publish(bool force = false)
    {
        var tree = Tree();
        var text = tree.ToJsonString();
        lock (state)
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

    async Task RefreshNowAsync(bool rescan = true)
    {
        lock (state)
        {
            if (busy != null)
            {
                return;
            }
        }
        if (rescan)
        {
            var hosts = AspireWorkspace.FindApphosts(Ws);
            lock (state)
            {
                apphosts = hosts;
            }
        }
        var found = findCli(Settings);
        var now = new Dictionary<string, RunningHost>();
        string? failure = null;
        if (found != null)
        {
            try
            {
                foreach (var host in AspireCli.ParsePs(await runCli(found, ["ps", "--format", "json"], TimeSpan.FromSeconds(30), Stopping)))
                {
                    try
                    {
                        host.Resources = AspireCli.ParseResources(await runCli(found, ["describe", "--format", "json", "--apphost", host.Path], TimeSpan.FromSeconds(30), Stopping));
                        host.Error = null;
                    }
                    catch (AspireException e)
                    {
                        (host.Resources, host.Error) = ([], e.Message);
                    }
                    now[AspireWorkspace.PathKey(host.Path)] = host;
                }
            }
            catch (AspireException e)
            {
                failure = e.Message;
            }
        }
        lock (state)
        {
            (cliChecked, cli, running, error) = (true, found, now, failure);
        }
        Publish();
    }

    async Task RefresherAsync()
    {
        var rescans = 0;
        try
        {
            while (true)
            {
                var asked = await refreshWanted.WaitAsync(pollInterval, Stopping);
                if (asked || visible)
                {
                    try
                    {
                        // The workspace scan is cheap but not free: on a timer only every sixth poll.
                        await RefreshNowAsync(rescan: asked || rescans % 6 == 0);
                        rescans++;
                    }
                    catch (Exception e) when (e is not OperationCanceledException)
                    {
                        Log($"aspire: refresh failed: {e.Message}");
                    }
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    void RequestRefresh()
    {
        try
        {
            refreshWanted.Release();
        }
        catch (SemaphoreFullException)
        {
            // Already asked for.
        }
    }

    // ───────────── requests to the IDE ─────────────

    void Terminal(string title, IEnumerable<string> args, string? cwd = null, bool saveAll = false)
    {
        if (RequireCli() is not { } found)
        {
            return;
        }
        var message = new JsonObject
        {
            ["type"] = "runInTerminal",
            ["title"] = title,
            ["command"] = AspireWorkspace.CommandLine([found, .. args]),
            ["cwd"] = !string.IsNullOrEmpty(cwd) ? cwd : Ws ?? Programs.Home,
        };
        if (saveAll)
        {
            message["saveAll"] = true;
        }
        Send(message);
    }

    string? RequireCli()
    {
        var found = findCli(Settings);
        if (found == null)
        {
            Log("aspire: the Aspire CLI isn't installed. Run \"Aspire: Install the Aspire CLI\" or set \"aspire.cliPath\".");
            ShowView("output");
            lock (state)
            {
                (cliChecked, cli) = (true, null);
            }
            Publish();
        }
        return found;
    }

    // ───────────── operations ─────────────

    /// <summary>Runs a quick aspire command, logs the result and refreshes.</summary>
    async Task RunOp(string label, IReadOnlyList<string> args)
    {
        if (RequireCli() is not { } found)
        {
            return;
        }
        lock (state)
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
            var output = Python.Strip(await runCli(found, args, TimeSpan.FromSeconds(120), Stopping));
            Log(output.Length == 0 ? $"aspire {label}: done" : $"aspire {label}: {Python.LastLine(output)}");
        }
        catch (AspireException e)
        {
            Log($"aspire {label} failed: {e.Message}");
        }
        finally
        {
            lock (state)
            {
                busy = null;
            }
            Publish();
            RequestRefresh();
        }
    }

    Task RunApphost(string path)
    {
        Terminal($"aspire run: {AspireWorkspace.ApphostLabel(path)}", ["run", "--apphost", path], AspireWorkspace.Dirname(path), saveAll: true);
        RequestRefresh();
        return Task.CompletedTask;
    }

    Task StopApphost(string path) => RunOp($"stop {AspireWorkspace.ApphostLabel(path)}", ["stop", "--apphost", path]);

    Task ResourceCommand(string host, string resource, string command) =>
        RunOp($"{command} {resource}", ["resource", resource, command, "--non-interactive", "--apphost", host]);

    async Task PickApphost(string placeholder, Func<string, Task> onPick, bool runningOnly = false)
    {
        var hosts = AllApphosts().Where(h => h.Info != null || !runningOnly).ToList();
        if (hosts.Count == 0)
        {
            Log(runningOnly ? "aspire: no running AppHost." :
                "aspire: no AppHost in this folder. Run \"Aspire: New Aspire Project…\" or \"Aspire: Initialize Aspire in This Folder\".");
            ShowView("output");
            return;
        }
        if (hosts.Count == 1)
        {
            await onPick(hosts[0].Path);
            return;
        }
        var picked = await QuickPickAsync(placeholder,
            hosts.Select(h => new PickItem(AspireWorkspace.ApphostLabel(h.Path), h.Path, AspireWorkspace.Relative(h.Path, Ws), h.Info != null ? "running" : "")));
        if (picked != null)
        {
            await onPick(picked);
        }
    }

    /// <summary>Makes sure the AppHosts are known before a command that needs one (the view may never have shown).</summary>
    async Task EnsureKnown()
    {
        bool known;
        lock (state)
        {
            known = cliChecked;
        }
        if (!known)
        {
            await RefreshNowAsync();
        }
    }

    RunningHost? RunningInfo(string path)
    {
        lock (state)
        {
            return running.GetValueOrDefault(AspireWorkspace.PathKey(path));
        }
    }

    void OpenDashboardOf(string path)
    {
        if (RunningInfo(path) is { Dashboard: { Length: > 0 } dashboard })
        {
            openUrl(dashboard);
        }
        else
        {
            Log("aspire: that AppHost has no dashboard URL yet.");
        }
    }

    // ───────────── tree actions ─────────────

    async Task TreeAction(string? item, string action)
    {
        if (item == null)
        {
            if (action == "refresh")
            {
                RequestRefresh();
            }
            else if (action == "new")
            {
                await Execute("aspire.new");
            }
            return;
        }
        var colon = item.IndexOf(':');
        var (kind, rest) = colon < 0 ? (item, "") : (item[..colon], item[(colon + 1)..]);
        switch (kind)
        {
            case "setup":
                InstallCli();
                break;
            case "host":
                if (action == "run")
                {
                    await RunApphost(rest);
                }
                else if (action == "stop")
                {
                    await StopApphost(rest);
                }
                else if (action == "dashboard")
                {
                    if (RunningInfo(rest) is { Dashboard: { Length: > 0 } dashboard })
                    {
                        openUrl(dashboard);
                    }
                }
                else if (action == "click")
                {
                    OpenFile(AspireWorkspace.SourceFile(rest));
                }
                break;
            case "dash":
                if (RunningInfo(rest) is { Dashboard: { Length: > 0 } url })
                {
                    openUrl(url);
                }
                break;
            case "res":
                var newline = rest.IndexOf('\n');
                var (host, resource) = newline < 0 ? (rest, "") : (rest[..newline], rest[(newline + 1)..]);
                if (action is "start" or "stop" or "restart")
                {
                    await ResourceCommand(host, resource, action);
                }
                else if (action == "logs")
                {
                    Terminal($"aspire logs: {resource}", ["logs", resource, "--apphost", host], AspireWorkspace.Dirname(host));
                }
                break;
            case "url":
                var end = rest.IndexOf('\n');
                openUrl(end < 0 ? rest : rest[..end]);
                break;
        }
    }

    // ───────────── commands ─────────────

    void InstallCli()
    {
        Send(new JsonObject
        {
            ["type"] = "runInTerminal",
            ["title"] = "Install the Aspire CLI",
            ["command"] = OperatingSystem.IsWindows() ? InstallScriptWindows : InstallScript,
            ["cwd"] = Programs.Home,
        });
        Log("aspire: after the install finishes, open a new terminal or restart ByteForge if `aspire` isn't on PATH yet (it goes to ~/.aspire/bin, which this module also checks).");
    }

    /// <summary>A terminal command against one AppHost (--apphost when there's one to pick, else the CLI asks).</summary>
    async Task WithApphost(string[] args, string title)
    {
        await EnsureKnown();
        if (AllApphosts().Count == 0)
        {
            Terminal(title, args);
            return;
        }
        await PickApphost("Choose an AppHost", p =>
        {
            Terminal($"{title}: {AspireWorkspace.ApphostLabel(p)}", [.. args, "--apphost", p], AspireWorkspace.Dirname(p), saveAll: true);
            return Task.CompletedTask;
        });
    }

    async Task Execute(string command)
    {
        switch (command)
        {
            case "aspire.refresh":
                RequestRefresh();
                break;
            case "aspire.run":
                await EnsureKnown();
                await PickApphost("Choose the AppHost to run", RunApphost);
                break;
            case "aspire.stop":
                await EnsureKnown();
                await PickApphost("Choose the AppHost to stop", StopApphost, runningOnly: true);
                break;
            case "aspire.openDashboard":
                await EnsureKnown();
                await PickApphost("Choose the AppHost", p =>
                {
                    OpenDashboardOf(p);
                    return Task.CompletedTask;
                }, runningOnly: true);
                break;
            case "aspire.new":
                Terminal("aspire new", ["new"]);
                break;
            case "aspire.init":
                Terminal("aspire init", ["init"]);
                break;
            case "aspire.add":
                await WithApphost(["add"], "aspire add");
                break;
            case "aspire.update":
                await WithApphost(["update"], "aspire update");
                break;
            case "aspire.restore":
                await WithApphost(["restore"], "aspire restore");
                break;
            case "aspire.publish":
                await WithApphost(["publish"], "aspire publish");
                break;
            case "aspire.deploy":
                await WithApphost(["deploy"], "aspire deploy");
                break;
            case "aspire.doctor":
                Terminal("aspire doctor", ["doctor"]);
                break;
            case "aspire.installCli":
                InstallCli();
                break;
            case "aspire.updateCli":
                Terminal("aspire update --self", ["update", "--self"]);
                break;
        }
    }

    /// <summary>Opens a URL in the default browser, in the background; a failure is ignored (as before).</summary>
    public static void OpenInBrowser(string url) => _ = Task.Run(() =>
    {
        try
        {
            if (OperatingSystem.IsWindows())
            {
                using var _ = Process.Start(new ProcessStartInfo(url) { UseShellExecute = true });
            }
            else if ((OperatingSystem.IsMacOS() ? Programs.Which("open") : Programs.Which("xdg-open")) is { } opener)
            {
                using var _ = Process.Start(new ProcessStartInfo(opener, [url]) { UseShellExecute = false });
            }
        }
        catch (Exception e) when (e is System.ComponentModel.Win32Exception or InvalidOperationException or IOException)
        {
        }
    });
}
