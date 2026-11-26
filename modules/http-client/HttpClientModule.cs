using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.HttpClientModule;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<HttpClientModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// Sends the requests in .http / .rest files (the format Visual Studio, Rider and VS Code's REST Client share):
/// an HTTP view lists every request in the workspace, each response opens in an editor (a file in the cache
/// folder) with its status line, headers and body. Requests are read from the saved file. See
/// <see cref="HttpFiles"/> for the format and the variables.
/// </summary>
public sealed class HttpClientModule : Module
{
    public const string View = "http";

    readonly string? cacheDir;
    readonly Func<bool, TimeSpan, HttpMessageHandler>? handlers;
    readonly Lock gate = new();
    string? active;                                          // the active editor, when it's an .http file
    string? environment;                                     // the environment picked for the session
    Dictionary<string, NamedResponse> responses = [];        // @name -> what it left for {{name.response…}}
    Dictionary<string, string> results = [];                 // "path\nline" -> summary of the last response
    (string Path, int Line)? last;                           // the last request sent
    readonly HashSet<(string, int)> sending = [];

    public HttpClientModule() : this(null, null)
    {
    }

    /// <param name="cacheDir">Where responses are saved (null: the module's responses/ folder).</param>
    /// <param name="handlers">Makes the handler a request goes through (accept any certificate, connect timeout); null: <see cref="Sender.DefaultHandler"/>.</param>
    public HttpClientModule(string? cacheDir, Func<bool, TimeSpan, HttpMessageHandler>? handlers)
    {
        this.cacheDir = cacheDir;
        this.handlers = handlers;
    }

    // In the module's own folder, like every module's files (it goes with the module).
    string CacheDir => cacheDir ?? Path.Combine(ModuleDir, "responses");

    protected override Task OnStartAsync()
    {
        RegisterCommand("http.send", "Send Request…", "HTTP", "ctrl-alt-enter");
        RegisterCommand("http.sendAll", "Send All Requests in File", "HTTP");
        RegisterCommand("http.rerun", "Rerun Last Request", "HTTP");
        RegisterCommand("http.selectEnvironment", "Select Environment", "HTTP");
        RegisterCommand("http.showResponses", "Where Are the Responses?", "HTTP");
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync()
    {
        Refresh();
        UpdateStatus();
        return Task.CompletedTask;
    }

    protected override Task OnSettingsChangedAsync()
    {
        UpdateStatus();
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        var path = message["path"] is JsonValue v && v.GetValueKind() == JsonValueKind.String ? v.GetValue<string>() : null;
        switch (name)
        {
            case "folderOpened":
                lock (gate)
                {
                    (results, responses, environment) = ([], [], null);
                }
                Refresh();
                UpdateStatus();
                break;
            case "editorChanged":
                active = HttpFiles.IsHttpFile(path) ? path : null;
                UpdateStatus();
                break;
            case "fileSaved" when (path ?? "").ToLowerInvariant() is var lower && (HttpFiles.IsHttpFile(lower) || lower.EndsWith(".json", StringComparison.Ordinal)):
                Refresh();
                UpdateStatus();
                break;
            case "filesChanged" or "viewShown":
                Refresh();
                break;
        }
        return Task.CompletedTask;
    }

    // ───────────── environments ─────────────

    OrderedDictionary<string, OrderedDictionary<string, object?>> Environments(string? path = null)
    {
        path ??= active ?? (!string.IsNullOrEmpty(Workspace) ? Path.Combine(Workspace, "x.http") : null);
        return path != null ? HttpFiles.LoadEnvironments(path, Workspace, Log) : [];
    }

    /// <summary>The environment in use: the one picked, the setting's, dev / development / local, else the first.</summary>
    public string? CurrentEnvironment(IReadOnlyDictionary<string, OrderedDictionary<string, object?>> envs)
    {
        if (environment != null && envs.ContainsKey(environment))
        {
            return environment;
        }
        if (Settings["http.environment"] is JsonValue v && v.GetValueKind() == JsonValueKind.String && envs.ContainsKey(v.GetValue<string>()))
        {
            return v.GetValue<string>();
        }
        foreach (var preferred in new[] { "dev", "development", "local", "Development", "Local" })
        {
            if (envs.ContainsKey(preferred))
            {
                return preferred;
            }
        }
        return envs.Keys.FirstOrDefault();
    }

    void UpdateStatus()
    {
        var envs = Environments();
        if (envs.Count == 0)
        {
            SetStatus("http.env", "");
            return;
        }
        SetStatus("http.env", $"⇄ {CurrentEnvironment(envs)}", "HTTP Client environment (http-client.env.json). Click to change it.", "http.selectEnvironment");
    }

    async Task PickEnvironment()
    {
        var envs = Environments();
        if (envs.Count == 0)
        {
            Log("http: no http-client.env.json next to the .http file or above it.");
            return;
        }
        var current = CurrentEnvironment(envs);
        var items = envs.Select(e =>
        {
            var detail = string.Join(", ", e.Value.Keys.Order(StringComparer.Ordinal));
            return new PickItem(e.Key, e.Key, detail.Length > 120 ? detail[..120] : detail, e.Key == current ? "current" : "");
        });
        var value = await QuickPickAsync("HTTP Client environment", items);
        if (!string.IsNullOrEmpty(value))
        {
            environment = value;
            Log($"http: environment {value}");
            UpdateStatus();
            Refresh();
        }
    }

    // ───────────── view ─────────────

    static (OrderedDictionary<string, string>, List<HttpRequestEntry>)? ParseFile(string path)
    {
        try
        {
            return HttpFiles.Parse(Py.ReadText(path));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    public void Refresh()
    {
        var root = Workspace;
        if (string.IsNullOrEmpty(root))
        {
            SetTree(View, [], "Open a folder to see its .http files.");
            return;
        }
        var items = new JsonArray();
        foreach (var path in HttpFiles.FindHttpFiles(root))
        {
            if (ParseFile(path) is not ({ } _, { } requests))
            {
                continue;
            }
            var children = new JsonArray();
            foreach (var r in requests)
            {
                var key = $"{path}\n{r.Line}";
                string description;
                lock (gate)
                {
                    description = results.GetValueOrDefault(key, "");
                }
                children.Add(new JsonObject
                {
                    ["id"] = key,
                    ["label"] = HttpFiles.Label(r),
                    ["description"] = description,
                    ["tooltip"] = $"{r.Method} {r.Url}",
                    ["icon"] = "play",
                    ["actions"] = new JsonArray(new JsonObject { ["id"] = "send", ["icon"] = "play", ["tooltip"] = "Send Request", ["label"] = "Send Request" }),
                    ["menu"] = new JsonArray(new JsonObject { ["id"] = "goto", ["label"] = "Go to Request" }, new JsonObject { ["id"] = "curl", ["label"] = "Show as curl" }),
                });
            }
            items.Add(new JsonObject
            {
                ["id"] = path,
                ["label"] = Path.GetRelativePath(root, path).Replace(Path.DirectorySeparatorChar, '/'),
                ["icon"] = $"file:{Path.GetFileName(path)}",
                ["expanded"] = true,
                ["count"] = children.Count,
                ["children"] = children,
                ["actions"] = new JsonArray(new JsonObject { ["id"] = "sendAll", ["icon"] = "play", ["tooltip"] = "Send All Requests" }),
                ["menu"] = new JsonArray(new JsonObject { ["id"] = "open", ["label"] = "Open File" }),
            });
        }
        var envs = Environments();
        var actions = new JsonArray(
            new JsonObject { ["id"] = "refresh", ["icon"] = "refresh", ["tooltip"] = "Refresh" },
            new JsonObject { ["id"] = "responses", ["icon"] = "file", ["tooltip"] = "Show the responses folder in the Output" });
        if (envs.Count > 0)
        {
            actions.Insert(0, new JsonObject { ["id"] = "environment", ["icon"] = "layers", ["tooltip"] = $"Environment: {CurrentEnvironment(envs)} (click to change)" });
        }
        SetTree(View, items, items.Count > 0 ? null : "No .http or .rest files. Create one (Ctrl+N, then save it as api.http).", actions: actions);
    }

    // ───────────── sending ─────────────

    async Task SendRequest(string path, int line)
    {
        lock (gate)
        {
            if (!sending.Add((path, line)))
            {
                Log("http: that request is still running.");
                return;
            }
            last = (path, line);
        }
        try
        {
            await SendOne(path, line);
        }
        finally
        {
            lock (gate)
            {
                sending.Remove((path, line));
            }
        }
    }

    HttpFiles.Resolver MakeResolver(string path, OrderedDictionary<string, string> variables)
    {
        var envs = HttpFiles.LoadEnvironments(path, Workspace, Log);
        var env = CurrentEnvironment(envs);
        IReadOnlyDictionary<string, object?> values = env != null && envs.TryGetValue(env, out var found) ? found : new Dictionary<string, object?>();
        Dictionary<string, NamedResponse> named;
        lock (gate)
        {
            named = new(responses);
        }
        return new HttpFiles.Resolver(variables, values, named, path, Workspace);
    }

    void SetResult(string key, string text)
    {
        lock (gate)
        {
            results[key] = text;
        }
    }

    /// <summary>Sends the request at <paramref name="line"/> of <paramref name="path"/>; the response, or null when it got none.</summary>
    async Task<HttpResult?> SendOne(string path, int line, bool openResponse = true)
    {
        OrderedDictionary<string, string> variables;
        List<HttpRequestEntry> requests;
        try
        {
            (variables, requests) = HttpFiles.Parse(Py.ReadText(path));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"http: {HttpFiles.OsError(e, path)}");
            return null;
        }
        var request = requests.FirstOrDefault(r => r.Line == line);
        if (request == null)
        {
            Log($"http: no request at line {line + 1} of {Path.GetFileName(path)} any more (save the file?)");
            return null;
        }
        var resolver = MakeResolver(path, variables);
        var name = HttpFiles.Label(request);
        var key = $"{path}\n{line}";
        string method, url;
        List<(string Name, string Value)> headers;
        byte[]? body;
        try
        {
            (method, url, headers, body) = HttpFiles.Build(request, resolver, path);
        }
        catch (Exception e) when (e is VariableError or IOException or UnauthorizedAccessException)
        {
            Log($"http: {name}: {e.Message}");
            ShowView("output");
            SetResult(key, "variable error");
            Refresh();
            return null;
        }
        SetResult(key, "sending…");
        Refresh();
        HttpResult response;
        try
        {
            response = await Sender.PerformAsync(method, url, headers, body, Settings, handlers, Stopping);
        }
        catch (SendError e)
        {
            Log($"http: {method} {url}: {e.Message}");
            ShowView("output");
            SetResult(key, "failed");
            Refresh();
            return null;
        }
        var text = HttpFiles.BodyText(response, !(Settings["http.prettyPrint"] is JsonValue p && p.GetValueKind() == JsonValueKind.False));
        if (!string.IsNullOrEmpty(request.Name))
        {
            var raw = text != null ? HttpFiles.Decode(response.Data, HttpFiles.CharsetOf(response.Headers)) : "";
            lock (gate)
            {
                responses[request.Name] = new NamedResponse(response.Headers, raw, headers, body is { Length: > 0 } ? HttpFiles.Decode(body, "utf-8") : "");
            }
        }
        var document = HttpFiles.ResponseDocument(method, url, response, text);
        var saved = HttpFiles.SaveResponse(CacheDir, name, document, response, text == null);
        var summary = HttpFiles.Summary(response);
        SetResult(key, summary);
        Log($"http: {method} {url} → {summary}");
        if (openResponse)
        {
            OpenFile(saved);
        }
        Refresh();
        return response;
    }

    async Task SendAll(string path)
    {
        List<HttpRequestEntry> requests;
        try
        {
            (_, requests) = HttpFiles.Parse(Py.ReadText(path));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"http: {HttpFiles.OsError(e, path)}");
            return;
        }
        // In order, so later requests can use the responses of earlier named ones.
        foreach (var r in requests)
        {
            await SendOne(path, r.Line, openResponse: false);
        }
        Log($"http: sent {requests.Count} requests from {Path.GetFileName(path)}; responses in {CacheDir}");
        ShowView("output");
    }

    async Task PickRequest()
    {
        var root = Workspace;
        var files = active != null ? [active] : !string.IsNullOrEmpty(root) ? HttpFiles.FindHttpFiles(root) : [];
        var items = new List<PickItem>();
        foreach (var path in files)
        {
            if (ParseFile(path) is not ({ } _, { } requests))
            {
                continue;
            }
            foreach (var r in requests)
            {
                var where = files.Count > 1 ? Path.GetFileName(path) : $"line {r.Line + 1}";
                items.Add(new PickItem(HttpFiles.Label(r), $"{path}\n{r.Line}", $"{r.Method} {r.Url}", where));
            }
        }
        if (items.Count == 0)
        {
            Log("http: no requests here. Open an .http file (requests are read from the saved file).");
            return;
        }
        var value = await QuickPickAsync("Send a request", items);
        if (!string.IsNullOrEmpty(value) && SplitKey(value) is ({ } path_, var line))
        {
            await SendRequest(path_, line);
        }
    }

    void ShowCurl(string path, int line)
    {
        try
        {
            var (variables, requests) = HttpFiles.Parse(Py.ReadText(path));
            var request = requests.FirstOrDefault(r => r.Line == line) ?? throw new VariableError("");
            var (method, url, headers, body) = HttpFiles.Build(request, MakeResolver(path, variables), path);
            Log(HttpFiles.CurlCommand(method, url, headers, body));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"http: {HttpFiles.OsError(e, path)}");
        }
        catch (VariableError e)
        {
            Log($"http: {e.Message}");
        }
        ShowView("output");
    }

    /// <summary>"path\nline" (a request's id) split; throws FormatException for a line that isn't a number, as int() would.</summary>
    static (string Path, int Line)? SplitKey(string value)
    {
        var at = value.LastIndexOf('\n');
        return at < 0 ? null : (value[..at], (int)Py.Int(value[(at + 1)..]));
    }

    protected override async Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "http.send":
                await PickRequest();
                break;
            case "http.sendAll":
                if (active != null)
                {
                    await SendAll(active);
                }
                else
                {
                    Log("http: open an .http file first.");
                }
                break;
            case "http.rerun":
                (string, int)? again;
                lock (gate)
                {
                    again = last;
                }
                if (again is ({ } path, var line))
                {
                    await SendRequest(path, line);
                }
                else
                {
                    await PickRequest();
                }
                break;
            case "http.selectEnvironment":
                await PickEnvironment();
                break;
            case "http.showResponses":
                Log($"http: responses are saved in {CacheDir}");
                ShowView("output");
                break;
        }
    }

    protected override async Task OnTreeActionAsync(string view, string? item, string action)
    {
        if (item == null)
        {
            switch (action)
            {
                case "refresh":
                    Refresh();
                    break;
                case "environment":
                    await PickEnvironment();
                    break;
                case "responses":
                    Log($"http: responses are saved in {CacheDir}");
                    ShowView("output");
                    break;
            }
            return;
        }
        if (SplitKey(item) is ({ } path, var line))
        {
            switch (action)
            {
                case "click" or "send":
                    await SendRequest(path, line);
                    break;
                case "goto":
                    OpenFile(path, line, 0);
                    break;
                case "curl":
                    ShowCurl(path, line);
                    break;
            }
        }
        else if (action == "sendAll")
        {
            await SendAll(item);
        }
        else if (action is "open" or "click")
        {
            OpenFile(item);
        }
    }
}
