using System.Text.Json;
using System.Text.Json.Nodes;

namespace ByteForge.Modules;

/// <summary>
/// A ByteForge module. Derive from it, override what the module reacts to, and start it from the entry point the
/// module host looks for:
/// <code>
/// public static class Entry
/// {
///     public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel)
///         =&gt; ModuleRunner.RunAsync&lt;MyModule&gt;(input, output, moduleDir, cancel);
/// }
/// </code>
/// The messages are those of docs/modules.md ("Protocol"). Handlers for commands, formatting and view actions
/// run concurrently; the others (start, activate, settings, events) one after the other, in order.
/// </summary>
public abstract class Module
{
    TextWriter? output;
    long nextRequest;
    readonly Dictionary<long, TaskCompletionSource<JsonObject>> pending = [];

    /// <summary>The module's folder (where its module.json is).</summary>
    public string ModuleDir { get; private set; } = "";

    /// <summary>The open folder, once the IDE has said (<see cref="OnActivateAsync"/>, folderOpened).</summary>
    public string? Workspace { get; private set; }

    /// <summary>The user's settings (settings.json), as of the last activate or settingsChanged.</summary>
    public JsonObject Settings { get; private set; } = [];

    /// <summary>Cancelled when the module is being turned off, uninstalled or updated.</summary>
    public CancellationToken Stopping { get; private set; }

    /// <summary>ByteForge's config folder (settings.json, themes/, modules/): two levels above the module's folder.</summary>
    public string ConfigDir => Path.GetFullPath(Path.Combine(ModuleDir, "..", ".."));

    // ───────────── what a module overrides ─────────────

    /// <summary>First thing after loading, before activate: register commands, formatters, status items.</summary>
    protected virtual Task OnStartAsync() => Task.CompletedTask;

    /// <summary>The IDE has sent the workspace and settings (<see cref="Workspace"/>, <see cref="Settings"/>).</summary>
    protected virtual Task OnActivateAsync() => Task.CompletedTask;

    /// <summary>The user changed settings (<see cref="Settings"/> has the new ones).</summary>
    protected virtual Task OnSettingsChangedAsync() => Task.CompletedTask;

    /// <summary>One of the module's commands was run.</summary>
    protected virtual Task OnCommandAsync(string id) => Task.CompletedTask;

    /// <summary>
    /// Formats <paramref name="text"/> (the content of <paramref name="path"/>) for a formatter registered with
    /// <see cref="RegisterFormatter"/>. Throw <see cref="ModuleException"/> to tell the user why it couldn't.
    /// </summary>
    protected virtual Task<string> FormatAsync(string? path, string text) => throw new ModuleException("This module doesn't format files.");

    /// <summary>An event: folderOpened, fileSaved, themeChanged, editorChanged (with path), filesChanged, viewShown, viewHidden.</summary>
    protected virtual Task OnEventAsync(string name, JsonObject message) => Task.CompletedTask;

    /// <summary>A row (<paramref name="action"/> "click") or button of one of the module's views was clicked.</summary>
    protected virtual Task OnTreeActionAsync(string view, string? item, string action) => Task.CompletedTask;

    // ───────────── messages to the IDE ─────────────

    /// <summary>Sends any message (a JSON object with "type").</summary>
    public void Send(JsonObject message)
    {
        var text = message.ToJsonString(Json.Compact);
        var writer = output ?? throw new InvalidOperationException("The module isn't running.");
        lock (writer)
        {
            writer.WriteLine(text);
            writer.Flush();
        }
    }

    public void Log(string text) => Send(new() { ["type"] = "log", ["text"] = text });

    public void RegisterCommand(string id, string title, string? category = null, string? keybinding = null)
    {
        var message = new JsonObject { ["type"] = "registerCommand", ["id"] = id, ["title"] = title };
        if (category != null)
        {
            message["category"] = category;
        }
        if (keybinding != null)
        {
            message["keybinding"] = keybinding;
        }
        Send(message);
    }

    /// <summary>Become a formatter for files with these extensions (without the dot); see <see cref="FormatAsync"/>.</summary>
    public void RegisterFormatter(string id, string name, params string[] extensions) =>
        Send(new() { ["type"] = "registerFormatter", ["id"] = id, ["name"] = name, ["extensions"] = new JsonArray([.. extensions.Select(e => (JsonNode)e)]) });

    /// <summary>A status bar item (empty text hides it).</summary>
    public void SetStatus(string id, string text, string? tooltip = null, string? command = null, bool right = false)
    {
        var message = new JsonObject { ["type"] = "status", ["id"] = id, ["text"] = text, ["alignment"] = right ? "right" : "left" };
        if (tooltip != null)
        {
            message["tooltip"] = tooltip;
        }
        if (command != null)
        {
            message["command"] = command;
        }
        Send(message);
    }

    /// <summary>Switches the IDE to the color theme with this name.</summary>
    public void SetTheme(string name) => Send(new() { ["type"] = "setTheme", ["name"] = name });

    /// <summary>Shows a view: "output", "problems", "explorer", "search" or one of the module's views.</summary>
    public void ShowView(string id) => Send(new() { ["type"] = "showView", ["id"] = id });

    /// <summary>Opens a file, at a line and column (from 0) when given.</summary>
    public void OpenFile(string path, int? line = null, int? column = null)
    {
        var message = new JsonObject { ["type"] = "openFile", ["path"] = path };
        if (line != null)
        {
            message["line"] = line;
        }
        if (column != null)
        {
            message["column"] = column;
        }
        Send(message);
    }

    /// <summary>Inserts text at the caret(s) of the active editor.</summary>
    public void InsertText(string text) => Send(new() { ["type"] = "insertText", ["text"] = text });

    /// <summary>Runs a command line in a new terminal tab.</summary>
    public void RunInTerminal(string title, string command, string? cwd = null, bool saveAll = false)
    {
        var message = new JsonObject { ["type"] = "runInTerminal", ["title"] = title, ["command"] = command, ["saveAll"] = saveAll };
        if (cwd != null)
        {
            message["cwd"] = cwd;
        }
        Send(message);
    }

    /// <summary>The content of one of the module's views (see setTree in docs/modules.md for the items).</summary>
    public void SetTree(string view, JsonArray items, string? message = null, string? error = null, int badge = 0, JsonArray? actions = null)
    {
        var tree = new JsonObject { ["type"] = "setTree", ["view"] = view, ["items"] = items, ["badge"] = badge };
        if (message != null)
        {
            tree["message"] = message;
        }
        if (error != null)
        {
            tree["error"] = error;
        }
        if (actions != null)
        {
            tree["actions"] = actions;
        }
        Send(tree);
    }

    /// <summary>A pick list; the value of the picked item, or null when dismissed.</summary>
    public async Task<string?> QuickPickAsync(string placeholder, IEnumerable<PickItem> items)
    {
        var list = new JsonArray();
        foreach (var item in items)
        {
            var node = new JsonObject { ["label"] = item.Label, ["value"] = item.Value ?? item.Label };
            if (item.Detail != null)
            {
                node["detail"] = item.Detail;
            }
            if (item.Hint != null)
            {
                node["hint"] = item.Hint;
            }
            list.Add(node);
        }
        // Only a pick is answered; a dismissed list never is, so this waits until the module stops.
        var answer = await Request(new() { ["type"] = "quickPick", ["placeholder"] = placeholder, ["items"] = list });
        return answer["value"]?.GetValue<string>();
    }

    /// <summary>A question with buttons; the index of the button pressed, or null when dismissed.</summary>
    public async Task<int?> PromptAsync(string message, string? detail, params string[] buttons)
    {
        var request = new JsonObject { ["type"] = "prompt", ["message"] = message, ["buttons"] = new JsonArray([.. buttons.Select(b => (JsonNode)b)]) };
        if (detail != null)
        {
            request["detail"] = detail;
        }
        var answer = await Request(request);
        return answer["index"]?.GetValue<int>();
    }

    // ───────────── settings ─────────────

    /// <summary>A setting's value, or <paramref name="fallback"/> when it's missing or of another type.</summary>
    public T Setting<T>(string key, T fallback)
    {
        if (Settings[key] is not { } node)
        {
            return fallback;
        }
        try
        {
            return node.Deserialize<T>() ?? fallback;
        }
        catch (Exception e) when (e is JsonException or InvalidOperationException or NotSupportedException)
        {
            return fallback;
        }
    }

    /// <summary>A string setting, trimmed; null when missing or empty.</summary>
    public string? SettingString(string key) => Setting<string?>(key, null) is { } s && s.Trim().Length > 0 ? s.Trim() : null;

    // ───────────── running (called by ModuleRunner) ─────────────

    async Task<JsonObject> Request(JsonObject message)
    {
        var id = Interlocked.Increment(ref nextRequest);
        var answer = new TaskCompletionSource<JsonObject>(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (pending)
        {
            pending[id] = answer;
        }
        message["requestId"] = id;
        Send(message);
        using (Stopping.Register(() => answer.TrySetCanceled()))
        {
            return await answer.Task;
        }
    }

    internal void Attach(TextWriter writer, string moduleDir, CancellationToken stopping)
    {
        output = writer;
        ModuleDir = Path.GetFullPath(moduleDir);
        Stopping = stopping;
    }

    internal Task StartAsync() => OnStartAsync();

    /// <summary>Handles one message from the IDE; returns work that may run alongside the next messages.</summary>
    internal async Task<Task?> HandleAsync(JsonObject message)
    {
        switch (message["type"]?.GetValue<string>())
        {
            case "activate":
                Workspace = message["workspace"]?.GetValue<string>();
                Settings = message["settings"] as JsonObject ?? [];
                await OnActivateAsync();
                return null;
            case "event":
                var name = message["name"]?.GetValue<string>() ?? "";
                if (name == "settingsChanged")
                {
                    Settings = message["settings"] as JsonObject ?? [];
                    await OnSettingsChangedAsync();
                    return null;
                }
                if (name == "folderOpened")
                {
                    Workspace = message["path"]?.GetValue<string>();
                }
                await OnEventAsync(name, message);
                return null;
            case "executeCommand":
                var command = message["id"]?.GetValue<string>() ?? "";
                return Guarded(() => OnCommandAsync(command), $"command {command}");
            case "format":
                return Format(message);
            case "treeAction":
                var view = message["view"]?.GetValue<string>() ?? "";
                var item = message["item"]?.GetValue<string>();
                var action = message["action"]?.GetValue<string>() ?? "";
                return Guarded(() => OnTreeActionAsync(view, item, action), $"{view} {action}");
            case "quickPickResult":
            case "promptResult":
                var id = message["requestId"]?.GetValue<long>() ?? -1;
                TaskCompletionSource<JsonObject>? waiting;
                lock (pending)
                {
                    pending.Remove(id, out waiting);
                }
                waiting?.TrySetResult(message);
                return null;
            default:
                return null;
        }
    }

    Task Format(JsonObject message)
    {
        var request = message["requestId"]?.GetValue<long>() ?? 0;
        var path = message["path"]?.GetValue<string>();
        var text = message["text"]?.GetValue<string>() ?? "";
        return Task.Run(async () =>
        {
            JsonObject result = new() { ["type"] = "formatResult", ["requestId"] = request };
            try
            {
                result["text"] = await FormatAsync(path, text);
            }
            catch (ModuleException e)
            {
                result["error"] = e.Message;
            }
            catch (Exception e) when (e is not OperationCanceledException)
            {
                result["error"] = $"{e.GetType().Name}: {e.Message}";
            }
            Send(result);
        });
    }

    Task Guarded(Func<Task> work, string what) => Task.Run(async () =>
    {
        try
        {
            await work();
        }
        catch (OperationCanceledException) when (Stopping.IsCancellationRequested)
        {
        }
        catch (Exception e)
        {
            Log($"{what} failed: {e}");
        }
    });
}

/// <summary>A pick list entry: <see cref="Value"/> comes back when it's picked (the label when null).</summary>
public sealed record PickItem(string Label, string? Value = null, string? Detail = null, string? Hint = null);

/// <summary>A failure whose message is for the user (shown as is, for example as a formatter's error).</summary>
public sealed class ModuleException(string message) : Exception(message);

static class Json
{
    public static readonly JsonSerializerOptions Compact = new() { WriteIndented = false };
}
