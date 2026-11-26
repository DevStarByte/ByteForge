using System.IO.Pipes;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.Discord;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<DiscordModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// Discord Rich Presence: talks to the Discord desktop app over its local IPC connection (no Discord library
/// needed) and sets your activity to what you're doing in ByteForge: the file, its language, the project and the
/// elapsed time. It always shows as ByteForge's Discord application with the ByteForge logo.
/// Settings: "discord.showFileName": false shows "Editing a C# file" instead of the file name;
/// "discord.showProject": false hides the project name (both default true).
/// </summary>
public class DiscordModule : Module
{
    public const string ClientId = "1569037494551322826"; // The ByteForge application on Discord.
    public const string Logo = "https://raw.githubusercontent.com/DevStarByte/ByteForge/packages/modules/discord/byteforge.png";

    public static readonly IReadOnlyDictionary<string, string> Languages = new Dictionary<string, string>
    {
        ["cs"] = "C#", ["csx"] = "C#", ["csproj"] = "MSBuild", ["props"] = "MSBuild", ["targets"] = "MSBuild", ["sln"] = "Visual Studio",
        ["slnx"] = "Visual Studio", ["razor"] = "Razor", ["cshtml"] = "Razor", ["xaml"] = "XAML", ["axaml"] = "XAML", ["fs"] = "F#",
        ["vb"] = "Visual Basic", ["json"] = "JSON", ["jsonc"] = "JSON", ["xml"] = "XML", ["config"] = "XML", ["resx"] = "XML",
        ["md"] = "Markdown", ["yml"] = "YAML", ["yaml"] = "YAML", ["toml"] = "TOML", ["rs"] = "Rust", ["py"] = "Python",
        ["js"] = "JavaScript", ["mjs"] = "JavaScript", ["ts"] = "TypeScript", ["tsx"] = "TypeScript", ["jsx"] = "JavaScript",
        ["html"] = "HTML", ["htm"] = "HTML", ["css"] = "CSS", ["scss"] = "SCSS", ["sql"] = "SQL", ["sh"] = "Shell", ["bash"] = "Shell",
        ["fish"] = "Shell", ["ps1"] = "PowerShell", ["lua"] = "Lua", ["go"] = "Go", ["java"] = "Java", ["kt"] = "Kotlin", ["c"] = "C",
        ["h"] = "C", ["cpp"] = "C++", ["hpp"] = "C++", ["cc"] = "C++", ["txt"] = "plain text", ["dockerfile"] = "Docker",
    };

    /// <summary>Discord allows 5 activity updates per 20 seconds.</summary>
    public TimeSpan MinInterval { get; init; } = TimeSpan.FromSeconds(4);

    /// <summary>How often to try connecting while Discord isn't there.</summary>
    public TimeSpan Retry { get; init; } = TimeSpan.FromSeconds(15);

    /// <summary>What the presence shows; guarded by its own lock.</summary>
    public sealed class PresenceState
    {
        public string? Workspace { get; set; }
        public string? Path { get; set; }
        public long Start { get; set; } = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        public bool Paused { get; set; }
        public bool Dirty { get; set; } = true;
        public bool RetryNow { get; set; } = true;
    }

    readonly PresenceState state = new();
    readonly SemaphoreSlim changed = new(0, 1);

    protected override Task OnStartAsync()
    {
        RegisterCommand("discord.toggle", "Pause or Resume Sharing", "Discord");
        RegisterCommand("discord.reconnect", "Reconnect", "Discord");
        _ = Task.Run(WorkerAsync);
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync()
    {
        Update(s =>
        {
            s.Workspace = Workspace;
            s.Start = Now;
        });
        return Task.CompletedTask;
    }

    protected override Task OnSettingsChangedAsync()
    {
        Update(_ => { });
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        switch (name)
        {
            case "editorChanged":
                var path = message["path"]?.GetValue<string>();
                bool same;
                lock (state)
                {
                    same = path == state.Path;
                }
                if (!same)
                {
                    Update(s => s.Path = path);
                }
                break;
            case "folderOpened":
                Update(s =>
                {
                    s.Workspace = message["path"]?.GetValue<string>();
                    s.Path = null;
                    s.Start = Now;
                });
                break;
            case "fileSaved":
                Update(s => s.RetryNow = true); // A cheap moment to retry.
                break;
        }
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "discord.toggle":
                bool paused;
                lock (state)
                {
                    paused = !state.Paused;
                }
                Update(s => s.Paused = paused);
                DiscordLog(paused ? "paused: your activity is hidden." : "sharing your activity again.");
                break;
            case "discord.reconnect":
                Update(s => s.RetryNow = true);
                break;
        }
        return Task.CompletedTask;
    }

    static long Now => DateTimeOffset.UtcNow.ToUnixTimeSeconds();

    void Update(Action<PresenceState> change)
    {
        lock (state)
        {
            change(state);
            state.Dirty = true;
            if (changed.CurrentCount == 0)
            {
                changed.Release();
            }
        }
    }

    void DiscordLog(string text) => Log($"Discord: {text}");

    void Status(string text, string? tooltip = null, string? command = null) => SetStatus("discord.status", text, tooltip, command, right: true);

    // ───────────── the connection ─────────────

    /// <summary>Where Discord (or a client like Vesktop, Flatpak, Snap) listens.</summary>
    protected virtual IEnumerable<string> SocketCandidates() => SocketCandidates(OperatingSystem.IsWindows(), Environment.GetEnvironmentVariable);

    public static IEnumerable<string> SocketCandidates(bool windows, Func<string, string?> environment)
    {
        if (windows)
        {
            return Enumerable.Range(0, 10).Select(i => $@"\\?\pipe\discord-ipc-{i}");
        }
        var runtime = environment("XDG_RUNTIME_DIR") is { Length: > 0 } xdg ? xdg : environment("TMPDIR") is { Length: > 0 } tmp ? tmp : "/tmp";
        string[] dirs = [runtime, $"{runtime}/app/com.discordapp.Discord", $"{runtime}/app/com.discordapp.DiscordCanary", $"{runtime}/snap.discord",
            $"{runtime}/app/dev.vencord.Vesktop", $"{runtime}/.flatpak/dev.vencord.Vesktop/xdg-run", "/tmp"];
        return dirs.SelectMany(d => Enumerable.Range(0, 10).Select(i => $"{d}/discord-ipc-{i}"));
    }

    /// <summary>Opens one candidate; null when there's nothing there.</summary>
    protected virtual async Task<Stream?> OpenAsync(string path, CancellationToken cancel)
    {
        if (OperatingSystem.IsWindows())
        {
            var name = path[(path.LastIndexOf('\\') + 1)..];
            var pipe = new NamedPipeClientStream(".", name, PipeDirection.InOut, PipeOptions.Asynchronous);
            try
            {
                // A single attempt, as opening the pipe file did: a missing or busy pipe fails at once.
                await pipe.ConnectAsync(0, cancel);
                return pipe;
            }
            catch
            {
                await pipe.DisposeAsync();
                throw;
            }
        }
        if (!File.Exists(path))
        {
            return null;
        }
        var socket = new Socket(AddressFamily.Unix, SocketType.Stream, ProtocolType.Unspecified);
        try
        {
            using var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel);
            limit.CancelAfter(TimeSpan.FromSeconds(5));
            await socket.ConnectAsync(new UnixDomainSocketEndPoint(path), limit.Token);
            return new NetworkStream(socket, ownsSocket: true);
        }
        catch (OperationCanceledException) when (!cancel.IsCancellationRequested)
        {
            socket.Dispose();
            throw new TimeoutException("connecting timed out");
        }
        catch
        {
            socket.Dispose();
            throw;
        }
    }

    /// <summary>The process whose activity it is: Discord clears it by itself when that process ends.</summary>
    protected virtual int ProcessId => Environment.ProcessId;

    async Task WorkerAsync()
    {
        DiscordIpc? ipc = null;
        var lastSent = DateTime.MinValue;
        var lastTry = DateTime.MinValue;
        string? lastError = null;
        try
        {
            while (!Stopping.IsCancellationRequested)
            {
                await changed.WaitAsync(Retry, Stopping);
                bool wakeRetry;
                lock (state)
                {
                    wakeRetry = state.RetryNow;
                    state.RetryNow = false;
                }
                if (ipc == null)
                {
                    var now = DateTime.UtcNow;
                    if (!wakeRetry && now - lastTry < Retry)
                    {
                        continue;
                    }
                    lastTry = now;
                    try
                    {
                        ipc = await DiscordIpc.ConnectAsync(SocketCandidates(), OpenAsync, ClientId, Stopping);
                    }
                    catch (DiscordException e)
                    {
                        if (e.Message != lastError)
                        {
                            DiscordLog($"Discord refused the connection: {e.Message}.");
                            lastError = e.Message;
                        }
                        Status("Discord: error", $"Discord refused the connection: {e.Message}. Click to retry.", "discord.reconnect");
                        continue;
                    }
                    if (ipc == null)
                    {
                        Status("Discord: offline", "The Discord app isn't running. ByteForge connects when it starts.", "discord.reconnect");
                        continue;
                    }
                    lastError = null;
                    lock (state)
                    {
                        state.Dirty = true;
                    }
                }
                TimeSpan wait;
                lock (state)
                {
                    if (!state.Dirty)
                    {
                        continue;
                    }
                    wait = MinInterval - (DateTime.UtcNow - lastSent);
                }
                if (wait > TimeSpan.Zero)
                {
                    await Task.Delay(wait, Stopping);
                }
                bool paused;
                JsonObject? activity;
                lock (state)
                {
                    state.Dirty = false;
                    paused = state.Paused;
                    activity = paused ? null : BuildActivity(state.Workspace, state.Path, state.Start, Settings);
                }
                try
                {
                    await ipc.SetActivityAsync(activity, ProcessId, Stopping);
                    lastSent = DateTime.UtcNow;
                    if (paused)
                    {
                        Status("Discord: paused", "Not sharing your activity. Click to resume.", "discord.toggle");
                    }
                    else
                    {
                        Status(""); // Connected and sharing: nothing to say in the status bar.
                    }
                }
                catch (Exception e) when (DiscordIpc.IsConnectionError(e))
                {
                    DiscordLog($"lost the connection ({e.Message}); retrying.");
                    ipc.Dispose();
                    ipc = null;
                    lock (state)
                    {
                        state.Dirty = true;
                    }
                }
            }
        }
        catch (OperationCanceledException) when (Stopping.IsCancellationRequested)
        {
        }
        catch (Exception e)
        {
            DiscordLog($"stopped: {e}");
        }
        finally
        {
            // Closing the connection clears the activity in Discord.
            ipc?.Dispose();
        }
    }

    // ───────────── presence ─────────────

    /// <summary>The language shown for a file, or null.</summary>
    public static string? LanguageOf(string path)
    {
        var fileName = System.IO.Path.GetFileName(path);
        var name = fileName.ToLowerInvariant();
        if (name == "dockerfile" || name.StartsWith("dockerfile.", StringComparison.Ordinal))
        {
            return "Docker";
        }
        // The suffix as Python's pathlib has it: none for ".bashrc" or "name.".
        var dot = fileName.LastIndexOf('.');
        var suffix = dot > 0 && dot < fileName.Length - 1 ? fileName[(dot + 1)..].ToLowerInvariant() : "";
        return Languages.GetValueOrDefault(suffix);
    }

    /// <summary>Trimmed and at most <paramref name="limit"/> characters; null when shorter than 2 (Discord's limits).</summary>
    public static string? Clip(string text, int limit = 128)
    {
        text = text.Trim();
        var runes = text.EnumerateRunes().ToArray();
        if (runes.Length > limit)
        {
            var clipped = new StringBuilder();
            foreach (var rune in runes[..(limit - 1)])
            {
                clipped.Append(rune.ToString());
            }
            text = clipped.Append('…').ToString();
            runes = text.EnumerateRunes().ToArray();
        }
        return runes.Length >= 2 ? text : null;
    }

    static bool IsFalse(JsonObject settings, string key) => settings[key] is JsonValue value && value.GetValueKind() == JsonValueKind.False;

    /// <summary>The activity sent to Discord for this state and settings.</summary>
    public static JsonObject BuildActivity(string? workspace, string? path, long start, JsonObject settings)
    {
        var showFile = !IsFalse(settings, "discord.showFileName");
        var showProject = !IsFalse(settings, "discord.showProject");
        var project = !string.IsNullOrEmpty(workspace) && System.IO.Path.GetFileName(System.IO.Path.TrimEndingDirectorySeparator(workspace)) is { Length: > 0 } folder
            ? folder
            : null;

        string details;
        string? lang = null;
        if (!string.IsNullOrEmpty(path))
        {
            lang = LanguageOf(path);
            details = showFile ? $"Editing {System.IO.Path.GetFileName(path)}" : lang != null ? $"Editing a {lang} file" : "Editing a file";
        }
        else
        {
            details = project != null ? "Browsing the project" : "Idle";
        }
        var activity = new JsonObject();
        if (Clip(details) is { } clippedDetails)
        {
            activity["details"] = clippedDetails;
        }
        activity["timestamps"] = new JsonObject { ["start"] = start };
        var assets = new JsonObject { ["large_image"] = Logo, ["large_text"] = "ByteForge" };
        activity["assets"] = assets;
        string? stateText = null;
        if (showProject && project != null)
        {
            stateText = Clip($"In {project}");
        }
        else if (lang != null)
        {
            stateText = Clip($"Writing {lang}");
        }
        if (stateText != null)
        {
            activity["state"] = stateText;
        }
        if (lang != null && showProject && project != null)
        {
            assets["large_text"] = Clip($"ByteForge, writing {lang}");
        }
        return activity;
    }
}
