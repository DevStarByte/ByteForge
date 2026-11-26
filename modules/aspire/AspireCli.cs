using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.Aspire;

/// <summary>A failure of the aspire CLI, its message for the view or the Output.</summary>
public sealed class AspireException(string message) : Exception(message);

/// <summary>Runs the aspire CLI: <paramref name="args"/> without a shell; the output, or <see cref="AspireException"/>.</summary>
public delegate Task<string> CliRunner(string cli, IReadOnlyList<string> args, TimeSpan timeout, CancellationToken cancel);

/// <summary>A running AppHost, from <c>aspire ps</c>, with its resources from <c>aspire describe</c>.</summary>
public sealed class RunningHost
{
    public required string Path { get; init; }
    public JsonNode? Pid { get; init; }
    public string? Dashboard { get; init; }
    public string? Status { get; init; }
    public List<JsonObject> Resources { get; set; } = [];
    public string? Error { get; set; }
}

/// <summary>Finding and running the aspire CLI and reading what it prints (its JSON).</summary>
public static class AspireCli
{
    static readonly HashSet<string> RunningStates = ["Running", "Active"];
    static readonly HashSet<string> BusyStates = ["Starting", "Stopping", "Building", "Waiting"];
    static readonly HashSet<string> StoppedStates = ["Finished", "Exited", "Stopped"];

    /// <summary>
    /// The aspire executable: the <c>aspire.cliPath</c> setting (when set, only that), PATH, or where the install
    /// script puts it (~/.aspire/bin), or a dotnet tool (~/.dotnet/tools). Null when there's none.
    /// </summary>
    public static string? FindCli(JsonObject settings, string? home = null)
    {
        home ??= Programs.Home;
        if (settings["aspire.cliPath"] is JsonValue value && value.TryGetValue<string>(out var configured) && Python.Strip(configured).Length > 0)
        {
            var path = ExpandUser(Python.Strip(configured), home);
            return File.Exists(path) ? path : null;
        }
        if (Programs.Which("aspire") is { } found)
        {
            return found;
        }
        var exe = OperatingSystem.IsWindows() ? "aspire.exe" : "aspire";
        foreach (var candidate in new[] { Path.Combine(home, ".aspire", "bin", exe), Path.Combine(home, ".dotnet", "tools", exe) })
        {
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }
        return null;
    }

    /// <summary>~ and ~/… as the home folder.</summary>
    static string ExpandUser(string path, string home)
    {
        if (path == "~")
        {
            return home;
        }
        if (path.Length > 1 && path[0] == '~' && (path[1] == '/' || OperatingSystem.IsWindows() && path[1] == '\\'))
        {
            return home.TrimEnd('/', '\\') + path[1..];
        }
        return path;
    }

    /// <summary>Runs the CLI (stdin empty, no colors, telemetry off unless the user set it); its output.</summary>
    public static async Task<string> RunAsync(string cli, IReadOnlyList<string> args, TimeSpan timeout, CancellationToken cancel)
    {
        ProcessResult done;
        try
        {
            done = await Programs.RunAsync(cli, args, timeout: timeout, environment: Environment(), cancel: cancel);
        }
        catch (ModuleException)
        {
            throw new AspireException("the aspire CLI isn't installed");
        }
        catch (TimeoutException)
        {
            throw new AspireException($"aspire {args[0]} timed out");
        }
        if (done.ExitCode != 0)
        {
            var text = Python.Strip(done.Error) is { Length: > 0 } error ? error : Python.Strip(done.Output) is { Length: > 0 } output ? output : $"exit code {done.ExitCode}";
            throw new AspireException(Python.LastLine(text));
        }
        return done.Output;
    }

    /// <summary>What the CLI runs with on top of the inherited environment.</summary>
    public static Dictionary<string, string?> Environment()
    {
        var env = new Dictionary<string, string?> { ["NO_COLOR"] = "1" };
        if (System.Environment.GetEnvironmentVariable("ASPIRE_CLI_TELEMETRY_OPTOUT") == null)
        {
            env["ASPIRE_CLI_TELEMETRY_OPTOUT"] = "1";
        }
        return env;
    }

    /// <summary>The JSON document in the CLI's output (it can print a banner or status lines around it).</summary>
    public static JsonNode FindJson(string text)
    {
        text = Python.Strip(text);
        foreach (var opener in "[{")
        {
            for (var start = text.IndexOf(opener); start != -1; start = text.IndexOf(opener, start + 1))
            {
                if (Decode(text[start..]) is { } value)
                {
                    return value;
                }
            }
        }
        throw new AspireException("couldn't read the aspire CLI's output");
    }

    /// <summary>The JSON value at the start of <paramref name="text"/> (whatever follows it), or null.</summary>
    static JsonNode? Decode(string text)
    {
        var reader = new Utf8JsonReader(Encoding.UTF8.GetBytes(text), new JsonReaderOptions { AllowMultipleValues = true });
        try
        {
            using var document = JsonDocument.ParseValue(ref reader);
            return ToNode(document.RootElement);
        }
        catch (JsonException)
        {
            return null;
        }
    }

    /// <summary>A node from an element; a key that's there twice keeps its first place and its last value (as in Python).</summary>
    static JsonNode? ToNode(JsonElement element)
    {
        switch (element.ValueKind)
        {
            case JsonValueKind.Object:
                var obj = new JsonObject();
                foreach (var property in element.EnumerateObject())
                {
                    obj[property.Name] = ToNode(property.Value);
                }
                return obj;
            case JsonValueKind.Array:
                var array = new JsonArray();
                foreach (var item in element.EnumerateArray())
                {
                    array.Add(ToNode(item));
                }
                return array;
            case JsonValueKind.Null:
                return null;
            default:
                return JsonValue.Create(element.Clone());
        }
    }

    /// <summary>Running AppHosts from <c>aspire ps --format json</c>: path, pid and dashboard URL.</summary>
    public static List<RunningHost> ParsePs(string text)
    {
        if (Python.Strip(text).Length == 0)
        {
            return [];
        }
        var value = FindJson(text);
        IEnumerable<JsonNode?> entries = value switch
        {
            JsonObject obj => [obj],
            JsonArray array => array,
            _ => [],
        };
        var hosts = new List<RunningHost>();
        foreach (var entry in entries)
        {
            if (entry is not JsonObject e || Str(e["appHostPath"]) is not { } path)
            {
                continue;
            }
            hosts.Add(new RunningHost { Path = path, Pid = e["appHostPid"]?.DeepClone(), Dashboard = Str(e["dashboardUrl"]), Status = Str(e["status"]) });
        }
        return hosts;
    }

    /// <summary>Resources from <c>aspire describe --format json</c>, sorted by name.</summary>
    public static List<JsonObject> ParseResources(string text)
    {
        var value = FindJson(text);
        var resources = value is JsonObject obj ? obj["resources"] : value;
        var result = resources is JsonArray array
            ? array.OfType<JsonObject>().Where(r => Str(r["name"]) != null).Select(r => (JsonObject)r.DeepClone()).ToList()
            : [];
        return [.. result.OrderBy(r => DisplayName(r).ToLowerInvariant(), StringComparer.Ordinal)];
    }

    public static string DisplayName(JsonObject resource) => Str(resource["displayName"]) is { Length: > 0 } name ? name : Str(resource["name"])!;

    /// <summary>Whether the resource offers a command (start, stop, restart) that's visible and enabled now.</summary>
    public static bool CommandEnabled(JsonObject resource, string name)
    {
        if (resource["commands"] is not JsonObject commands)
        {
            return false;
        }
        var command = Python.Truthy(commands[name]) ? commands[name] : commands[$"resource-{name}"];
        if (command is not JsonObject c)
        {
            return false;
        }
        if (Python.Or(c["visibility"], "Visible") == "Hidden")
        {
            return false;
        }
        return Python.Or(c["state"], "Enabled") == "Enabled";
    }

    /// <summary>"running", "busy", "warning", "error" or "idle": how the resource's icon looks.</summary>
    public static string ResourceStatus(JsonObject resource)
    {
        var state = Str(resource["state"]);
        var style = Str(resource["stateStyle"]);
        var health = Str(resource["healthStatus"]);
        var failed = Python.NonZero(resource["exitCode"]);
        if (state != null && RunningStates.Contains(state))
        {
            if (style == "error")
            {
                return "error";
            }
            return health is "Unhealthy" or "Degraded" || style == "warning" ? "warning" : "running";
        }
        if (state != null && BusyStates.Contains(state))
        {
            return "busy";
        }
        if (state != null && StoppedStates.Contains(state))
        {
            return style == "error" || failed ? "error" : "idle";
        }
        if (state == "FailedToStart")
        {
            return failed ? "error" : "warning";
        }
        return state is "RuntimeUnhealthy" or "ValueMissing" ? "warning" : "idle";
    }

    /// <summary>The resource's public endpoints as (label, url); the label is the endpoint's display name, name or URL.</summary>
    public static List<(JsonNode Label, string Url)> ResourceUrls(JsonObject resource)
    {
        var urls = new List<(JsonNode, string)>();
        if (resource["urls"] is not JsonArray entries)
        {
            return urls;
        }
        foreach (var node in entries)
        {
            if (node is not JsonObject entry || Python.Truthy(entry["isInternal"]) || Str(entry["url"]) is not { } url)
            {
                continue;
            }
            var label = new[] { entry["displayName"], entry["name"] }.FirstOrDefault(Python.Truthy)?.DeepClone() ?? JsonValue.Create(url);
            urls.Add((label, url));
        }
        return urls;
    }

    /// <summary>A string value, else null.</summary>
    public static string? Str(JsonNode? node) => node is JsonValue value && value.TryGetValue<string>(out var text) ? text : null;
}

/// <summary>Python's way with values and text, where the old module's output depended on it.</summary>
public static class Python
{
    /// <summary>Whether a JSON value counts as true (null, false, 0, "", [] and {} don't).</summary>
    public static bool Truthy(JsonNode? node) => node switch
    {
        null => false,
        JsonArray array => array.Count > 0,
        JsonObject obj => obj.Count > 0,
        JsonValue value when value.TryGetValue<string>(out var text) => text.Length > 0,
        JsonValue value when value.TryGetValue<bool>(out var flag) => flag,
        JsonValue value when value.TryGetValue<double>(out var number) => number != 0,
        _ => true,
    };

    /// <summary><c>value or fallback</c>, as text (a value that isn't a string never equals one).</summary>
    public static string? Or(JsonNode? node, string fallback) => Truthy(node) ? AspireCli.Str(node) : fallback;

    /// <summary>Whether an exit code isn't None or 0.</summary>
    public static bool NonZero(JsonNode? node) => node switch
    {
        null => false,
        JsonValue value when value.TryGetValue<bool>(out var flag) => flag,
        JsonValue value when value.TryGetValue<double>(out var number) => number != 0,
        _ => true,
    };

    /// <summary>A value as Python's str() wrote it into a text (None, True, numbers as written).</summary>
    public static string Str(JsonNode? node) => node switch
    {
        null => "None",
        JsonValue value when value.TryGetValue<string>(out var text) => text,
        JsonValue value when value.TryGetValue<bool>(out var flag) => flag ? "True" : "False",
        _ => node.ToJsonString(),
    };

    /// <summary>Trims whitespace as str.strip() does (which also counts \x1c-\x1f).</summary>
    public static string Strip(string text) => text.Trim(Whitespace);

    /// <summary>The last of str.splitlines().</summary>
    public static string LastLine(string text)
    {
        var end = text.Length;
        if (end > 0 && LineBreaks.Contains(text[end - 1]))
        {
            // The last line's own line break (\r\n counts as one).
            end--;
            if (text[end] == '\n' && end > 0 && text[end - 1] == '\r')
            {
                end--;
            }
        }
        var start = end == 0 ? 0 : text.LastIndexOfAny(LineBreaks, end - 1) + 1;
        return text[start..end];
    }

    static readonly char[] LineBreaks = ['\n', '\r', '\v', '\f', '\x1c', '\x1d', '\x1e', '\x85', (char)0x2028, (char)0x2029];

    static readonly char[] Whitespace =
    [
        .. new[] { 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x85, 0xa0, 0x1680 }.Select(c => (char)c),
        .. Enumerable.Range(0x2000, 11).Select(c => (char)c),
        .. new[] { 0x2028, 0x2029, 0x202f, 0x205f, 0x3000 }.Select(c => (char)c),
    ];
}
