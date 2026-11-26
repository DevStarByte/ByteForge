using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Channels;
using ByteForge.Modules;

namespace ByteForge.Sql;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<SqlModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// SQL: a Database view with the workspace's connections and their tables, views and columns, "Run File" for .sql
/// files, an interactive shell per connection, and a formatter for .sql files (keyword case, trailing whitespace).
///
/// Connections come from the `sql.connections` setting, the ConnectionStrings of the workspace's appsettings*.json
/// files and SQLite database files in the workspace. SQLite needs nothing else; PostgreSQL, MySQL / MariaDB and SQL
/// Server go through their command line clients (psql, mysql or mariadb, sqlcmd). Queries and shells run in an IDE
/// terminal through <see cref="Launcher"/>, so passwords never show up on a command line: the module writes the
/// connection to a private temporary file that the launcher reads, deletes and turns into the client's environment.
/// </summary>
public class SqlModule : Module
{
    public const string View = "sql";
    const string WarningColor = "#d29922";

    static readonly Dictionary<string, string> DriverColors = new()
    {
        ["sqlite"] = "#44a8e0", ["postgres"] = "#6b9bd1", ["mysql"] = "#f29111", ["sqlserver"] = "#e05252",
    };

    // Material Design Icons (Apache 2.0): database, table, table-eye (view) and table-column.
    const string DatabaseIcon = "M12,3C7.58,3 4,4.79 4,7C4,9.21 7.58,11 12,11C16.42,11 20,9.21 20,7C20,4.79 16.42,3 12,3M4,9V12C4,14.21 " +
        "7.58,16 12,16C16.42,16 20,14.21 20,12V9C20,11.21 16.42,13 12,13C7.58,13 4,11.21 4,9M4,14V17C4,19.21 " +
        "7.58,21 12,21C16.42,21 20,19.21 20,17V14C20,16.21 16.42,18 12,18C7.58,18 4,16.21 4,14Z";
    const string TableIcon = "M5,4H19A2,2 0 0,1 21,6V18A2,2 0 0,1 19,20H5A2,2 0 0,1 3,18V6A2,2 0 0,1 5,4M5,8V12H11V8H5M13,8V12H19V8H13M5," +
        "14V18H11V14H5M13,14V18H19V14H13Z";
    const string ViewIcon = "M17,18C17.56,18 18,18.44 18,19C18,19.56 17.56,20 17,20C16.44,20 16,19.56 16,19C16,18.44 16.44,18 17,18M17," +
        "15C14.27,15 11.94,16.66 11,19C11.94,21.34 14.27,23 17,23C19.73,23 22.06,21.34 23,19C22.06,16.66 19.73,15 17," +
        "15M17,21.5A2.5,2.5 0 0,1 14.5,19A2.5,2.5 0 0,1 17,16.5A2.5,2.5 0 0,1 19.5,19A2.5,2.5 0 0,1 17,21.5M9.27," +
        "20H6V16H10V17.63C10.5,16.97 11.12,16.4 11.84,15.94L12,15.87V10H18V13.5C18.7,13.6 19.37,13.78 20,14.04V6A2," +
        "2 0 0,0 18,4H4A2,2 0 0,0 2,6V18A2,2 0 0,0 4,20H9.27C9.12,19.74 9,19.47 8.9,19.2L8.84,19L8.9,18.8C9.03," +
        "18.53 9.13,18.26 9.27,18M12,10H4V6H18V8H12V10M10,14H6V10H10V14Z";
    const string ColumnIcon = "M8,2H16A2,2 0 0,1 18,4V20A2,2 0 0,1 16,22H8A2,2 0 0,1 6,20V4A2,2 0 0,1 8,2M8,10V14H16V10H8M8,16V20H16V" +
        "16H8M8,4V8H16V4H8Z";

    static readonly (string Id, string Title, string? Key)[] Commands =
    [
        ("sql.runFile", "Run File", "ctrl-shift-enter"),
        ("sql.selectConnection", "Select Connection…", null),
        ("sql.openShell", "Open Shell…", null),
        ("sql.addConnection", "Add Connection…", null),
        ("sql.refresh", "Refresh Connections", null),
    ];

    /// <summary>A connection's tables, or why they couldn't be loaded.</summary>
    sealed record Schema(List<Table>? Tables, string? Error);

    readonly Lock gate = new();
    List<Connection> connections = [];
    readonly Dictionary<string, Schema> schemas = [];
    readonly HashSet<string> loading = [];
    string? active;          // id of the connection "Run File" uses
    string? editorPath;
    string? lastTree;
    string? lastStatus;
    bool scanned;
    bool visible;
    bool stale;              // files changed while the view was hidden
    readonly Channel<(Func<Task> Work, TaskCompletionSource Done)> work = Channel.CreateUnbounded<(Func<Task>, TaskCompletionSource)>();

    string? Ws => string.IsNullOrEmpty(Workspace) ? null : Workspace;

    string StateFile => Path.Combine(ModuleDir, "state.json");

    /// <summary>The rows of a query against a server (tests replace the database clients).</summary>
    protected virtual Task<List<string[]>> RunRowsAsync(Connection conn, string query) =>
        Clients.RunRowsAsync(conn, query, Settings, Stopping);

    /// <summary>The database client for a server driver, or null (tests replace it).</summary>
    protected virtual string? FindClient(string driver) => Clients.FindClient(driver, Settings);

    // ───────────── start, the work queue ─────────────

    protected override Task OnStartAsync()
    {
        foreach (var (id, title, key) in Commands)
        {
            RegisterCommand(id, title, "SQL", key);
        }
        RegisterFormatter("sql.format", "SQL (keyword case)", "sql");
        _ = Task.Run(Worker);
        return Task.CompletedTask;
    }

    /// <summary>Rescans one after the other, off the message loop (scanning walks the workspace).</summary>
    Task Later(Action action)
    {
        var done = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        work.Writer.TryWrite((() =>
        {
            action();
            return Task.CompletedTask;
        }, done));
        return done.Task;
    }

    async Task Worker()
    {
        try
        {
            await foreach (var (job, done) in work.Reader.ReadAllAsync(Stopping))
            {
                try
                {
                    await job();
                }
                catch (Exception e) when (e is not OperationCanceledException)
                {
                    // A bug here must never take the module down.
                    Log($"sql: {e.Message}");
                }
                finally
                {
                    done.TrySetResult();
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    // ───────────── state ─────────────

    JsonObject LoadState()
    {
        try
        {
            return JsonNode.Parse(File.ReadAllText(StateFile)) as JsonObject ?? [];
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
            return [];
        }
    }

    void RememberActive()
    {
        var state = LoadState();
        if (state["active"] is not JsonObject remembered)
        {
            state["active"] = remembered = [];
        }
        string? id;
        lock (gate)
        {
            id = active;
        }
        remembered[Ws ?? ""] = id;
        try
        {
            File.WriteAllText(StateFile, state.ToJsonString());
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
        }
    }

    void Rescan()
    {
        var fromSettings = (Settings["sql.connections"] as JsonArray ?? []).Select(e => Connections.FromSetting(e, Ws)).OfType<Connection>().ToList();
        var discover = !(Settings["sql.discover"] is JsonValue value && value.GetValueKind() == JsonValueKind.False);
        var found = discover ? Connections.ScanWorkspace(Ws) : [];
        var merged = Connections.Merge([fromSettings, found]);
        lock (gate)
        {
            connections = merged;
            var ids = merged.Select(c => c.Id!).ToHashSet();
            foreach (var id in schemas.Keys.Where(k => !ids.Contains(k)).ToList())
            {
                schemas.Remove(id);
            }
            if (active == null || !ids.Contains(active))
            {
                var remembered = LoadState()["active"] is JsonObject map && map[Ws ?? ""] is JsonValue v && v.TryGetValue<string>(out var s) ? s : null;
                // With several connections and none chosen yet, "Run File" asks (and remembers the answer).
                var only = merged.Count == 1 ? merged[0].Id : null;
                active = remembered != null && ids.Contains(remembered) ? remembered : only;
            }
            scanned = true;
        }
        Publish();
    }

    Connection? Find(string? id)
    {
        lock (gate)
        {
            return connections.FirstOrDefault(c => c.Id == id);
        }
    }

    List<Table> TablesOf(string id)
    {
        lock (gate)
        {
            return schemas.GetValueOrDefault(id)?.Tables ?? [];
        }
    }

    // ───────────── state → view ─────────────

    static JsonObject TableItem(Connection conn, Table table, int index)
    {
        var tid = $"table:{conn.Id}:{index}";
        var columns = new JsonArray();
        for (var i = 0; i < table.Columns.Count; i++)
        {
            var c = table.Columns[i];
            var column = new JsonObject
            {
                ["id"] = $"col:{conn.Id}:{index}:{i}",
                ["label"] = c.Name,
                ["description"] = c.Type.ToLowerInvariant() + (c.Nullable ? "" : " not null"),
                ["icon"] = ColumnIcon,
                ["tooltip"] = $"{c.Name} {c.Type}{(c.Pk ? " · primary key" : "")}\nClick to insert the name",
            };
            if (c.Pk)
            {
                column["iconColor"] = DriverColors[conn.Driver];
            }
            columns.Add(column);
        }
        var showSchema = table.Schema.Length > 0 && conn.Driver != "mysql" && table.Schema != Clients.DefaultSchemas.GetValueOrDefault(conn.Driver);
        var item = new JsonObject
        {
            ["id"] = tid,
            ["label"] = showSchema ? $"{table.Schema}.{table.Name}" : table.Name,
            ["icon"] = table.View ? ViewIcon : TableIcon,
            ["count"] = columns.Count,
            ["expanded"] = false,
            ["children"] = columns,
            ["actions"] = new JsonArray(
                new JsonObject { ["id"] = "select", ["icon"] = "play", ["tooltip"] = $"Select Top {Clients.SelectLimit}" },
                new JsonObject { ["id"] = "insert", ["icon"] = "add", ["tooltip"] = "Insert the Name" }),
        };
        if (table.View)
        {
            item["description"] = "view";
        }
        return item;
    }

    JsonObject ConnectionItem(Connection conn)
    {
        var cid = conn.Id!;
        Schema? schema;
        bool isLoading, isActive;
        lock (gate)
        {
            schema = schemas.GetValueOrDefault(cid);
            isLoading = loading.Contains(cid);
            isActive = cid == active;
        }
        var driverName = Connections.Drivers[conn.Driver];
        var description = driverName + (conn.Origin is { Length: > 0 } origin && origin != "settings" ? $" · {origin}" : "");
        var tooltip = new List<string> { $"{conn.Name} ({driverName})", Connections.Target(conn) };
        if (!string.IsNullOrEmpty(conn.Origin))
        {
            tooltip.Add(conn.Origin == "settings" ? "From settings" : $"From {conn.Origin}");
        }
        if (isActive)
        {
            tooltip.Add("Used by \"SQL: Run File\"");
        }
        tooltip.Add(schema == null ? "Click to load the tables" : "");
        var item = new JsonObject
        {
            ["id"] = $"conn:{cid}",
            ["label"] = conn.Name + (isActive ? "  ✓" : ""),
            ["description"] = description,
            ["tooltip"] = string.Join("\n", tooltip.Where(t => t.Length > 0)),
            ["icon"] = DatabaseIcon,
            ["iconColor"] = DriverColors[conn.Driver],
            ["actions"] = new JsonArray(
                new JsonObject { ["id"] = "run", ["icon"] = "play", ["tooltip"] = "Run the Active .sql File Here", ["label"] = "Run Active File Here" },
                new JsonObject { ["id"] = "shell", ["icon"] = "terminal", ["tooltip"] = "Open Shell" },
                new JsonObject { ["id"] = "refresh", ["icon"] = "refresh", ["tooltip"] = "Reload Tables" }),
        };
        if (isLoading)
        {
            item["children"] = new JsonArray(new JsonObject { ["id"] = $"loading:{cid}", ["label"] = "Loading…", ["icon"] = "sync" });
        }
        else if (schema?.Error is { } error)
        {
            item["children"] = new JsonArray(new JsonObject
            {
                ["id"] = $"error:{cid}",
                ["label"] = "Couldn't load the tables",
                ["icon"] = "warning",
                ["iconColor"] = WarningColor,
                ["description"] = error,
                ["tooltip"] = error + "\nClick to try again",
            });
        }
        else if (schema?.Tables is { } tables)
        {
            item["count"] = tables.Count;
            var children = new JsonArray();
            for (var i = 0; i < tables.Count; i++)
            {
                children.Add(TableItem(conn, tables[i], i));
            }
            if (children.Count == 0)
            {
                children.Add(new JsonObject { ["id"] = $"empty:{cid}", ["label"] = "No tables" });
            }
            item["children"] = children;
        }
        return item;
    }

    /// <summary>The view's content: items, the top row's buttons and a message while scanning.</summary>
    public (JsonArray Items, JsonArray Actions, string? Message) Tree()
    {
        List<Connection> list;
        bool done;
        lock (gate)
        {
            list = [.. connections];
            done = scanned;
        }
        var children = new JsonArray();
        foreach (var conn in list)
        {
            children.Add(ConnectionItem(conn));
        }
        if (children.Count == 0)
        {
            children.Add(new JsonObject
            {
                ["id"] = "note:none",
                ["label"] = "No connections yet",
                ["tooltip"] = "Add one to \"sql.connections\" in settings, put ConnectionStrings in appsettings.json, " +
                              "or add a SQLite database file to the workspace",
            });
        }
        var items = new JsonArray(new JsonObject
        {
            ["id"] = "section:connections", ["label"] = "CONNECTIONS", ["header"] = true, ["count"] = list.Count, ["children"] = children,
        });
        var actions = new JsonArray(
            new JsonObject { ["id"] = "add", ["icon"] = "add", ["tooltip"] = "Add a Connection (opens settings)", ["label"] = "Add Connection…" },
            new JsonObject { ["id"] = "refresh", ["icon"] = "refresh", ["tooltip"] = "Refresh" });
        return (items, actions, done ? null : "Looking for connections…");
    }

    void Publish(bool force = false)
    {
        var (items, actions, message) = Tree();
        var text = new JsonObject { ["items"] = items.DeepClone(), ["actions"] = actions.DeepClone(), ["message"] = message }.ToJsonString();
        lock (gate)
        {
            if (!force && text == lastTree)
            {
                return;
            }
            lastTree = text;
            SetTree(View, items, message, actions: actions);
        }
        PublishStatus();
    }

    /// <summary>"SQL: &lt;connection&gt;" in the status bar while a .sql file is active.</summary>
    void PublishStatus()
    {
        lock (gate)
        {
            var conn = active != null ? connections.FirstOrDefault(c => c.Id == active) : null;
            var show = editorPath != null && editorPath.ToLowerInvariant().EndsWith(".sql", StringComparison.Ordinal);
            var text = !show ? "" : conn != null ? $"SQL: {conn.Name}" : "SQL: no connection";
            if (text == lastStatus)
            {
                return;
            }
            lastStatus = text;
            SetStatus("sql.connection", text, "The connection \"SQL: Run File\" uses. Click to change it.", "sql.selectConnection", right: true);
        }
    }

    // ───────────── loading tables ─────────────

    /// <summary>Loads a connection's tables in the background (the view shows "Loading…" meanwhile).</summary>
    public Task Load(string cid)
    {
        if (Find(cid) is not { } conn)
        {
            return Task.CompletedTask;
        }
        lock (gate)
        {
            if (!loading.Add(cid))
            {
                return Task.CompletedTask;
            }
        }
        Publish();
        return Task.Run(async () =>
        {
            Schema result;
            try
            {
                result = new Schema(conn.Driver == "sqlite" ? Sqlite.Tables(conn.File) : Clients.TablesFromRows(await RunRowsAsync(conn, Clients.SchemaQueries[conn.Driver])), null);
            }
            catch (SqlException error)
            {
                result = new Schema(null, error.Message);
            }
            catch (OperationCanceledException) when (Stopping.IsCancellationRequested)
            {
                return;
            }
            catch (Exception error)
            {
                // A bug must not leave the row loading forever.
                result = new Schema(null, $"{error.GetType().Name}: {error.Message}");
            }
            lock (gate)
            {
                loading.Remove(cid);
                schemas[cid] = result;
            }
            Publish();
        });
    }

    // ───────────── picking ─────────────

    async Task<string?> PickConnection(string placeholder, bool preferActive = false)
    {
        List<Connection> list;
        string? current;
        lock (gate)
        {
            list = [.. connections];
            current = active;
        }
        if (list.Count == 0)
        {
            Log("sql: no connections. Add one to \"sql.connections\" in settings, put ConnectionStrings in " +
                "appsettings.json, or add a SQLite database file to the workspace.");
            ShowView("output");
            return null;
        }
        if (preferActive && current != null && Find(current) != null)
        {
            return current;
        }
        if (list.Count == 1)
        {
            return list[0].Id;
        }
        var items = list.Select(c => new PickItem(c.Name ?? "", c.Id, Connections.Target(c), (c.Id == current ? "✓ " : "") + Connections.Drivers[c.Driver]));
        var value = await QuickPickAsync(placeholder, items);
        return string.IsNullOrEmpty(value) ? null : value;
    }

    void SetActive(string cid)
    {
        lock (gate)
        {
            active = cid;
        }
        RememberActive();
        Publish();
    }

    // ───────────── running ─────────────

    static string ShellQuote(string arg)
    {
        if (arg.Length > 0 && arg.All(c => char.IsLetterOrDigit(c) || "-_./:=@%+,".Contains(c)))
        {
            return arg;
        }
        if (OperatingSystem.IsWindows())
        {
            return "\"" + arg.Replace("\"", "\\\"") + "\"";
        }
        return "'" + arg.Replace("'", "'\\''") + "'";
    }

    /// <summary>
    /// The dotnet that runs the launcher: plain "dotnet" when it's on PATH (a quoted path as the first word, such as
    /// "C:\Program Files\dotnet\dotnet.exe", isn't run by PowerShell), else the one running the module host.
    /// </summary>
    static string Dotnet()
    {
        if (Programs.Which("dotnet") != null)
        {
            return "dotnet";
        }
        var process = Environment.ProcessPath;
        return process != null && Path.GetFileNameWithoutExtension(process).Equals("dotnet", StringComparison.OrdinalIgnoreCase) ? process : "dotnet";
    }

    void LaunchInTerminal(Connection conn, string title, string mode, string? query = null, string? file = null, bool saveAll = false)
    {
        if (conn.Driver != "sqlite" && FindClient(conn.Driver) == null)
        {
            Log($"sql: {Clients.ClientNames[conn.Driver]} isn't installed (or set \"sql.{conn.Driver}Path\" in settings).");
            ShowView("output");
            return;
        }
        var settings = new JsonObject();
        foreach (var (key, value) in Settings)
        {
            if (key.StartsWith("sql.", StringComparison.Ordinal) && key.EndsWith("Path", StringComparison.Ordinal))
            {
                settings[key] = value?.DeepClone();
            }
        }
        var spec = Launcher.WriteSpec(new JsonObject
        {
            ["connection"] = conn.ToJson(), ["mode"] = mode, ["query"] = query, ["file"] = file, ["settings"] = settings,
        });
        var launcher = Path.Combine(ModuleDir, "bin", "Sql.dll");
        var command = string.Join(" ", new[] { Dotnet(), launcher, "launch", spec }.Select(ShellQuote));
        var cwd = file != null ? Path.GetDirectoryName(file) : Ws ?? Programs.Home;
        RunInTerminal(title, command, cwd, saveAll);
    }

    async Task RunFile(string? cid = null)
    {
        string? path;
        lock (gate)
        {
            path = editorPath;
        }
        if (path == null || !path.ToLowerInvariant().EndsWith(".sql", StringComparison.Ordinal))
        {
            Log("sql: open a .sql file first (\"SQL: Run File\" runs the active one; save new files first).");
            ShowView("output");
            return;
        }
        if (cid == null)
        {
            await EnsureScanned();
            cid = await PickConnection("Run against which connection?", preferActive: true);
        }
        if (cid == null || Find(cid) is not { } conn)
        {
            return;
        }
        string? current;
        lock (gate)
        {
            current = active;
        }
        if (cid != current)
        {
            SetActive(cid);
        }
        LaunchInTerminal(conn, $"SQL: {Path.GetFileName(path)} → {conn.Name}", "run", file: path, saveAll: true);
    }

    void OpenShell(string? cid)
    {
        if (Find(cid) is { } conn)
        {
            LaunchInTerminal(conn, $"SQL: {conn.Name}", "shell");
        }
    }

    async Task EnsureScanned()
    {
        bool done;
        lock (gate)
        {
            done = scanned;
        }
        if (!done)
        {
            await Later(Rescan);
        }
    }

    void OpenSettings()
    {
        var path = Path.Combine(ConfigDir, "settings.json");
        Log("sql: add connections to \"sql.connections\" in settings, for example\n" +
            "  \"sql.connections\": [\n" +
            "    { \"name\": \"Local Postgres\", \"driver\": \"postgres\", \"host\": \"localhost\", \"database\": \"app\", " +
            "\"user\": \"postgres\", \"password\": \"…\" },\n" +
            "    { \"name\": \"Shop\", \"driver\": \"sqlserver\", \"connectionString\": \"Server=localhost;Database=Shop;" +
            "Trusted_Connection=True;TrustServerCertificate=True\" },\n" +
            "    { \"name\": \"Cache\", \"driver\": \"sqlite\", \"file\": \"data/cache.db\" }\n" +
            "  ]\n" +
            "Drivers: sqlite, postgres, mysql, sqlserver.");
        ShowView("output");
        if (File.Exists(path))
        {
            OpenFile(path);
        }
    }

    Task Refresh()
    {
        lock (gate)
        {
            schemas.Clear();
        }
        return Later(Rescan);
    }

    // ───────────── messages from the IDE ─────────────

    protected override Task OnActivateAsync()
    {
        Publish(force: true);
        _ = Later(Rescan);
        return Task.CompletedTask;
    }

    protected override Task OnSettingsChangedAsync()
    {
        _ = Later(Rescan);
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        var view = message["view"]?.GetValue<string>();
        switch (name)
        {
            case "editorChanged":
                lock (gate)
                {
                    editorPath = message["path"]?.GetValue<string>();
                }
                PublishStatus();
                break;
            case "folderOpened":
                lock (gate)
                {
                    schemas.Clear();
                    active = null;
                    scanned = false;
                }
                _ = Later(Rescan);
                break;
            case "filesChanged":
                // Scanning walks the workspace: only while the view shows, else once it's shown again.
                if (visible)
                {
                    _ = Later(Rescan);
                }
                else
                {
                    stale = true;
                }
                break;
            case "viewShown" when view == View:
                visible = true;
                if (stale)
                {
                    stale = false;
                    _ = Later(Rescan);
                }
                break;
            case "viewHidden" when view == View:
                visible = false;
                break;
        }
        return Task.CompletedTask;
    }

    protected override async Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "sql.runFile":
                await RunFile();
                break;
            case "sql.selectConnection":
                await EnsureScanned();
                if (await PickConnection("Connection for \"SQL: Run File\"") is { } chosen)
                {
                    SetActive(chosen);
                }
                break;
            case "sql.openShell":
                await EnsureScanned();
                OpenShell(await PickConnection("Open a shell for which connection?"));
                break;
            case "sql.addConnection":
                OpenSettings();
                break;
            case "sql.refresh":
                await Refresh();
                break;
        }
    }

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
                await Refresh();
            }
            else if (action == "add")
            {
                OpenSettings();
            }
            return;
        }
        var colon = item.IndexOf(':');
        var kind = colon < 0 ? item : item[..colon];
        var rest = colon < 0 ? "" : item[(colon + 1)..];
        switch (kind)
        {
            case "conn" when action == "run":
                await RunFile(rest);
                break;
            case "conn" when action == "shell":
                OpenShell(rest);
                break;
            case "conn" when action is "refresh" or "click":
                await Load(rest);
                break;
            case "error":
                await Load(rest);
                break;
            case "table" or "col":
                var parts = rest.Split(':');
                if (parts.Length < 2 || !int.TryParse(parts[1], out var index) || Find(parts[0]) is not { } conn)
                {
                    return;
                }
                var tables = TablesOf(parts[0]);
                if (index < 0 || index >= tables.Count)
                {
                    return;
                }
                var table = tables[index];
                if (kind == "table" && action == "select")
                {
                    LaunchInTerminal(conn, $"SQL: {table.Name}", "run", query: Clients.SelectQuery(conn, table));
                }
                else if (kind == "table" && action == "insert")
                {
                    InsertText(Clients.TableReference(conn, table));
                }
                else if (kind == "col" && action == "click" && parts.Length > 2 && int.TryParse(parts[2], out var c) && c >= 0 && c < table.Columns.Count)
                {
                    var column = table.Columns[c].Name;
                    InsertText(Clients.PlainIdentifier(column, conn.Driver) ? column : Clients.QuoteIdentifier(conn.Driver, column));
                }
                break;
        }
    }

    protected override Task<string> FormatAsync(string? path, string text)
    {
        var keywordCase = Settings["sql.format.keywordCase"] is JsonValue value && value.TryGetValue<string>(out var s) && s is "upper" or "lower" or "preserve" ? s! : "upper";
        return Task.FromResult(SqlFormatter.Format(text, keywordCase));
    }

    /// <summary>The connections as of the last scan (for tests).</summary>
    public IReadOnlyList<Connection> CurrentConnections
    {
        get
        {
            lock (gate)
            {
                return [.. connections];
            }
        }
    }
}
