using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.GitHooks;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<GitHooksModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// The repository's git hooks and Husky.NET tasks in a Git Hooks view. Lists the hooks in the folder git runs them
/// from (core.hooksPath, else .git/hooks; Husky's .husky works the same way), marks each active or turned off, and
/// runs one in a terminal with the arguments git would give it. "New Hook…" writes one from a template. Turning a
/// hook off renames it to &lt;hook&gt;.off, which git ignores; turning it on renames it back. Husky.NET's tasks
/// (.husky/task-runner.json) are listed by group and run with <c>dotnet husky run</c>.
/// </summary>
public sealed class GitHooksModule : Module
{
    readonly GitRunner git;
    string? folder;
    string? configured;

    public GitHooksModule() : this(Hooks.RunGit)
    {
    }

    /// <param name="git">How git is run (tests pass a fake).</param>
    public GitHooksModule(GitRunner git) => this.git = git;

    protected override Task OnStartAsync()
    {
        RegisterCommand("gitHooks.new", "New Hook…", "Git Hooks");
        RegisterCommand("gitHooks.run", "Run Hook…", "Git Hooks");
        RegisterCommand("gitHooks.show", "Show Hooks", "Git Hooks");
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync() => Guard(Refresh);

    protected override Task OnEventAsync(string name, JsonObject message) => Guard(async () =>
    {
        switch (name)
        {
            case "folderOpened" or "filesChanged" or "viewShown":
                await Refresh();
                break;
            case "fileSaved":
                var path = message["path"] is JsonValue v && v.GetValueKind() == JsonValueKind.String ? v.GetValue<string>() : "";
                if (!string.IsNullOrEmpty(folder) && path.StartsWith(folder, StringComparison.Ordinal))
                {
                    await Refresh();
                }
                break;
        }
    });

    /// <summary>Errors are logged as the Python's message loop logged them, and the module goes on.</summary>
    async Task Guard(Func<Task> work)
    {
        try
        {
            await work();
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            Log($"git-hooks: {e.GetType().Name}: {e.Message}");
        }
    }

    async Task Scan() =>
        (folder, configured) = !string.IsNullOrEmpty(Workspace) ? await Hooks.HooksDir(Workspace, git) : (null, null);

    async Task Refresh()
    {
        await Scan();
        Send(Hooks.Tree(Workspace, folder, configured));
    }

    Hook? Find(string name) => string.IsNullOrEmpty(folder) ? null : Hooks.List(folder).FirstOrDefault(h => h.Name == name);

    async Task Run(Hook hook) =>
        RunInTerminal($"hook: {hook.Name}", await Hooks.Command(hook, Workspace ?? "", git), Workspace, saveAll: true);

    async Task NewHook()
    {
        if (string.IsNullOrEmpty(folder))
        {
            Log("git-hooks: open a git repository first.");
            return;
        }
        var existing = Hooks.List(folder).Select(h => h.Name).ToHashSet();
        var items = new List<PickItem>();
        for (var i = 0; i < Hooks.Templates.Length; i++)
        {
            var (name, title, _) = Hooks.Templates[i];
            if (name == null)
            {
                continue;
            }
            items.Add(new PickItem(title, i.ToString(System.Globalization.CultureInfo.InvariantCulture),
                name + (existing.Contains(name) ? "  (replaces the current one)" : ""), name));
        }
        foreach (var (name, description) in Hooks.All)
        {
            if (!existing.Contains(name))
            {
                items.Add(new PickItem($"Empty {name} hook", $"empty:{name}", description, name));
            }
        }
        var value = await QuickPickAsync("New git hook", items);
        if (!string.IsNullOrEmpty(value))
        {
            await Create(value);
        }
    }

    async Task Create(string value)
    {
        string name, body;
        if (value.StartsWith("empty:", StringComparison.Ordinal))
        {
            (name, body) = (value[6..], Hooks.Templates[^1].Body);
        }
        else
        {
            var index = int.Parse(value, System.Globalization.CultureInfo.InvariantCulture);
            var template = Hooks.Templates[index < 0 ? index + Hooks.Templates.Length : index];
            (name, body) = (template.Name!, template.Body);
        }
        var target = Path.Combine(folder!, name);
        if (Path.Exists(target) || Path.Exists(target + Hooks.Off))
        {
            if (await PromptAsync($"Replace the {name} hook?", "The current one is overwritten.", "Replace", "Cancel") == 0)
            {
                await Write(name, body);
            }
            return;
        }
        await Write(name, body);
    }

    async Task Write(string name, string body)
    {
        var off = Path.Combine(folder!, name + Hooks.Off);
        if (Path.Exists(off))
        {
            File.Delete(off);
        }
        var path = Hooks.Write(folder!, name, body);
        Log($"git-hooks: wrote {path}");
        OpenFile(path);
        await Refresh();
    }

    async Task Toggle(Hook hook)
    {
        if (hook.Active)
        {
            File.Move(hook.Path, hook.Path + Hooks.Off, overwrite: true);
            Log($"git-hooks: {hook.Name} is off");
        }
        else
        {
            var target = hook.Path[..^Hooks.Off.Length];
            if (Path.Exists(target))
            {
                Log($"git-hooks: there's already an active {hook.Name} hook.");
                return;
            }
            File.Move(hook.Path, target, overwrite: true);
            Log($"git-hooks: {hook.Name} is on");
        }
        await Refresh();
    }

    protected override Task OnCommandAsync(string id) => Guard(async () =>
    {
        switch (id)
        {
            case "gitHooks.new":
                await Scan();
                await NewHook();
                break;
            case "gitHooks.run":
                await Scan();
                var hooks = Hooks.List(folder).Where(h => h.Active).ToList();
                if (hooks.Count == 0)
                {
                    Log("git-hooks: no active hooks in this repository.");
                    return;
                }
                var value = await QuickPickAsync("Run a git hook", hooks.Select(h => new PickItem(h.Name, h.Name, Hooks.Describe(h.Name))));
                if (!string.IsNullOrEmpty(value) && Find(value) is { } hook)
                {
                    await Run(hook);
                }
                break;
            case "gitHooks.show":
                ShowView(Hooks.View);
                break;
        }
    });

    protected override Task OnTreeActionAsync(string view, string? item, string action) => Guard(async () =>
    {
        if (item == null)
        {
            if (action == "new")
            {
                await NewHook();
            }
            else if (action == "refresh")
            {
                await Refresh();
            }
            return;
        }
        var colon = item.IndexOf(':');
        var (prefix, rest) = colon >= 0 ? (item[..colon], item[(colon + 1)..]) : (item, "");
        if (prefix == "hook")
        {
            if (Find(rest) is not { } hook)
            {
                await Refresh();
                return;
            }
            switch (action)
            {
                case "click" or "open":
                    OpenFile(hook.Path);
                    break;
                case "run":
                    if (!hook.Active)
                    {
                        Log($"git-hooks: {hook.Name} is off; running it anyway.");
                    }
                    await Run(hook);
                    break;
                case "toggle":
                    await Toggle(hook);
                    break;
                case "chmod":
                    Hooks.MakeExecutable(hook.Path);
                    await Refresh();
                    break;
                case "delete":
                    if (await PromptAsync($"Delete the {hook.Name} hook?", hook.Path, "Delete", "Cancel") == 0 && Find(hook.Name) is { } current)
                    {
                        File.Delete(current.Path);
                        Log($"git-hooks: deleted {current.Path}");
                        await Refresh();
                    }
                    break;
            }
        }
        else if (prefix == "husky" && action is "click" or "run")
        {
            var root = Workspace ?? "";
            if ((Hooks.HuskyTasks(root) ?? []).FirstOrDefault(t => t.Index.ToString(System.Globalization.CultureInfo.InvariantCulture) == rest) is { } task)
            {
                RunInTerminal($"husky: {task.Name}", $"dotnet husky run --name {Hooks.Quote(task.Name)}", Workspace, saveAll: true);
            }
        }
        else if (item == "husky" && action == "huskyOpen")
        {
            OpenFile(Path.Combine(Workspace ?? "", ".husky", "task-runner.json"));
        }
    });
}
