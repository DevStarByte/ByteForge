using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace ByteForge.Sql;

/// <summary>A database: from settings, an appsettings ConnectionString or a SQLite file in the workspace.</summary>
public sealed class Connection
{
    public string Driver { get; set; } = "";
    public string? Name { get; set; }
    public string? Host { get; set; }
    public string? Port { get; set; }
    public string? Database { get; set; }
    public string? User { get; set; }
    public string? Password { get; set; }
    public string? File { get; set; }
    /// <summary>SQL Server: Windows authentication (null: not said).</summary>
    public bool? Integrated { get; set; }
    /// <summary>SQL Server: TrustServerCertificate (null: not said).</summary>
    public bool? Trust { get; set; }
    /// <summary>"settings", the appsettings file (relative to the workspace) or the SQLite file's folder.</summary>
    public string? Origin { get; set; }
    public string? Id { get; set; }

    public Connection Clone() => (Connection)MemberwiseClone();

    public JsonObject ToJson()
    {
        var json = new JsonObject { ["driver"] = Driver };
        void Add(string key, string? value)
        {
            if (value != null)
            {
                json[key] = value;
            }
        }
        Add("name", Name);
        Add("host", Host);
        Add("port", Port);
        Add("database", Database);
        Add("user", User);
        Add("password", Password);
        Add("file", File);
        if (Integrated != null)
        {
            json["integrated"] = Integrated;
        }
        if (Trust != null)
        {
            json["trust"] = Trust;
        }
        Add("origin", Origin);
        Add("id", Id);
        return json;
    }

    public static Connection FromJson(JsonObject json)
    {
        string? Get(string key) => json[key] is JsonValue value && value.TryGetValue<string>(out var s) ? s : null;
        bool? Flag(string key) => json[key] is JsonValue value && value.TryGetValue<bool>(out var b) ? b : null;
        return new Connection
        {
            Driver = Get("driver") ?? "",
            Name = Get("name"),
            Host = Get("host"),
            Port = Get("port"),
            Database = Get("database"),
            User = Get("user"),
            Password = Get("password"),
            File = Get("file"),
            Integrated = Flag("integrated"),
            Trust = Flag("trust"),
            Origin = Get("origin"),
            Id = Get("id"),
        };
    }
}

/// <summary>Connection strings, the workspace's connections and how they're named.</summary>
public static partial class Connections
{
    public const int MaxFiles = 100_000;
    public static readonly HashSet<string> SkipDirs = [".git", "bin", "obj", "node_modules", ".vs", ".idea", "target", "packages"];
    public static readonly string[] SqliteExtensions = [".db", ".sqlite", ".sqlite3", ".db3"];
    static readonly byte[] SqliteHeader = Encoding.ASCII.GetBytes("SQLite format 3\0");

    public static readonly Dictionary<string, string> Drivers = new()
    {
        ["sqlite"] = "SQLite", ["postgres"] = "PostgreSQL", ["mysql"] = "MySQL", ["sqlserver"] = "SQL Server",
    };

    static readonly Dictionary<string, string> DriverAliases = new()
    {
        ["sqlite"] = "sqlite", ["sqlite3"] = "sqlite",
        ["postgres"] = "postgres", ["postgresql"] = "postgres", ["pg"] = "postgres", ["npgsql"] = "postgres",
        ["mysql"] = "mysql", ["mariadb"] = "mysql",
        ["sqlserver"] = "sqlserver", ["mssql"] = "sqlserver", ["sql server"] = "sqlserver", ["sqlclient"] = "sqlserver",
    };

    static readonly (string Prefix, string Driver)[] PackageDrivers =
    [
        ("npgsql", "postgres"),
        ("pomelo.entityframeworkcore.mysql", "mysql"), ("mysqlconnector", "mysql"), ("mysql.data", "mysql"),
        ("mysql.entityframeworkcore", "mysql"),
        ("microsoft.entityframeworkcore.sqlserver", "sqlserver"), ("microsoft.data.sqlclient", "sqlserver"),
        ("system.data.sqlclient", "sqlserver"), ("aspire.microsoft.data.sqlclient", "sqlserver"),
        ("microsoft.entityframeworkcore.sqlite", "sqlite"), ("microsoft.data.sqlite", "sqlite"),
        ("system.data.sqlite", "sqlite"),
    ];

    static readonly HashSet<string> SqlServerKeys = ["initial catalog", "trusted_connection", "integrated security", "trustservercertificate",
        "multipleactiveresultsets", "encrypt", "user id", "application name", "attachdbfilename"];

    static readonly HashSet<string> MySqlKeys = ["uid", "pwd", "sslmode", "allowuservariables", "allowpublickeyretrieval", "treattinyasboolean",
        "convert zero datetime", "convertzerodatetime", "charset"];

    /// <summary>`Key=Value;…` (ADO.NET style) with lowercase keys; values may be quoted.</summary>
    public static Dictionary<string, string> ParseConnectionString(string text)
    {
        var fields = new Dictionary<string, string>();
        int i = 0, n = text.Length;
        while (i < n)
        {
            while (i < n && "; \t\r\n".Contains(text[i]))
            {
                i++;
            }
            var start = i;
            while (i < n && text[i] != '=' && text[i] != ';')
            {
                i++;
            }
            var key = Py.Strip(text[start..i]).ToLowerInvariant();
            if (i >= n || text[i] == ';')
            {
                continue;
            }
            i++; // '='
            while (i < n && (text[i] == ' ' || text[i] == '\t'))
            {
                i++;
            }
            string value;
            if (i < n && (text[i] == '"' || text[i] == '\''))
            {
                var quote = text[i];
                i++;
                var builder = new StringBuilder();
                while (i < n)
                {
                    if (text[i] == quote)
                    {
                        if (i + 1 < n && text[i + 1] == quote)
                        {
                            builder.Append(quote);
                            i += 2;
                            continue;
                        }
                        i++;
                        break;
                    }
                    builder.Append(text[i]);
                    i++;
                }
                value = builder.ToString();
                while (i < n && text[i] != ';')
                {
                    i++;
                }
            }
            else
            {
                start = i;
                while (i < n && text[i] != ';')
                {
                    i++;
                }
                value = Py.Strip(text[start..i]);
            }
            if (key.Length > 0)
            {
                fields[key] = value;
            }
        }
        return fields;
    }

    public static bool IsSqlitePath(string value)
    {
        value = Py.Strip(value).ToLowerInvariant();
        return value == ":memory:" || SqliteExtensions.Any(e => value.EndsWith(e, StringComparison.Ordinal));
    }

    static string? Field(Dictionary<string, string> fields, string key) => fields.TryGetValue(key, out var value) ? value : null;

    static string? FirstOf(Dictionary<string, string> fields, params string[] names)
    {
        foreach (var name in names)
        {
            if (fields.TryGetValue(name, out var value) && value.Length > 0)
            {
                return value;
            }
        }
        return null;
    }

    /// <summary>
    /// The driver of a parsed connection string: a SQLite file wins, then the project's provider
    /// (<paramref name="hint"/>), then the keys that only one provider uses.
    /// </summary>
    public static string? GuessDriver(Dictionary<string, string> fields, string? hint = null)
    {
        var source = FirstOf(fields, "data source", "datasource", "filename") ?? "";
        if (IsSqlitePath(source) || (fields.ContainsKey("filename") && !fields.ContainsKey("server")))
        {
            return "sqlite";
        }
        if (!string.IsNullOrEmpty(hint))
        {
            return hint;
        }
        var server = (FirstOf(fields, "server", "data source") ?? "").ToLowerInvariant();
        if (fields.Keys.Any(SqlServerKeys.Contains) || server.Contains('\\') ||
            new[] { "tcp:", "np:", "(localdb)", "lpc:" }.Any(p => server.StartsWith(p, StringComparison.Ordinal)))
        {
            return "sqlserver";
        }
        if (fields.Keys.Any(MySqlKeys.Contains))
        {
            return "mysql";
        }
        if (fields.ContainsKey("host"))
        {
            return "postgres";
        }
        if (fields.ContainsKey("server") || fields.ContainsKey("data source"))
        {
            return "sqlserver";
        }
        return null;
    }

    static bool Truthy(string? value) => value != null && Py.Strip(value).ToLowerInvariant() is "true" or "yes" or "sspi" or "1";

    static string? NotEmpty(string? value) => string.IsNullOrEmpty(value) ? null : value;

    /// <summary>A connection from a parsed connection string (relative SQLite files from <paramref name="baseDir"/>).</summary>
    public static Connection FromFields(string driver, Dictionary<string, string> fields, string? baseDir = null)
    {
        var conn = new Connection { Driver = driver };
        switch (driver)
        {
            case "sqlite":
                var path = FirstOf(fields, "data source", "datasource", "filename") ?? "";
                if (path.Contains("|DataDirectory|"))
                {
                    path = path.Replace("|DataDirectory|", "").TrimStart('\\', '/');
                }
                if (path.Length > 0 && path != ":memory:" && !Py.IsAbs(path) && !string.IsNullOrEmpty(baseDir))
                {
                    path = Py.NormPath(Py.Join(baseDir, path));
                }
                conn.File = NotEmpty(path);
                break;
            case "postgres":
                var host = FirstOf(fields, "host", "server");
                conn.Host = NotEmpty(host?.Split(',')[0]);
                conn.Port = FirstOf(fields, "port");
                conn.Database = FirstOf(fields, "database", "db", "initial catalog");
                conn.User = FirstOf(fields, "username", "user id", "userid", "user name", "user", "uid");
                conn.Password = FirstOf(fields, "password", "pwd");
                break;
            case "mysql":
                conn.Host = FirstOf(fields, "server", "host", "data source", "address", "addr");
                conn.Port = FirstOf(fields, "port");
                conn.Database = FirstOf(fields, "database", "initial catalog");
                conn.User = FirstOf(fields, "uid", "user id", "userid", "user", "username", "user name");
                conn.Password = FirstOf(fields, "pwd", "password");
                break;
            case "sqlserver":
                var server = FirstOf(fields, "server", "data source", "address", "addr", "network address") ?? "";
                if (server.StartsWith("tcp:", StringComparison.OrdinalIgnoreCase))
                {
                    server = server[4..];
                }
                conn.Host = NotEmpty(server);
                conn.Database = FirstOf(fields, "database", "initial catalog");
                conn.User = FirstOf(fields, "user id", "uid", "user", "username");
                conn.Password = FirstOf(fields, "password", "pwd");
                conn.Integrated = Truthy(FirstOf(fields, "integrated security", "trusted_connection"));
                conn.Trust = Truthy(FirstOf(fields, "trustservercertificate", "trust server certificate"));
                break;
        }
        return conn;
    }

    public static string? NormalizeDriver(JsonNode? value) =>
        value is JsonValue v && v.TryGetValue<string>(out var s) && DriverAliases.TryGetValue(Py.Strip(s).ToLowerInvariant(), out var driver) ? driver : null;

    /// <summary>Python's truth value of a JSON value.</summary>
    static bool IsTruthy(JsonNode? node) => node switch
    {
        null => false,
        JsonArray array => array.Count > 0,
        JsonObject obj => obj.Count > 0,
        JsonValue value when value.TryGetValue<bool>(out var b) => b,
        JsonValue value when value.TryGetValue<string>(out var s) => s.Length > 0,
        JsonValue value when value.GetValueKind() == JsonValueKind.Number => value.GetValue<double>() != 0,
        _ => false,
    };

    /// <summary>A connection from one `sql.connections` entry, or null when it doesn't say enough.</summary>
    public static Connection? FromSetting(JsonNode? node, string? workspace)
    {
        if (node is not JsonObject entry)
        {
            return null;
        }
        var driver = NormalizeDriver(entry["driver"]);
        Connection conn;
        if (entry["connectionString"] is JsonValue cs && cs.TryGetValue<string>(out var text) && Py.Strip(text).Length > 0)
        {
            var fields = ParseConnectionString(text);
            driver ??= GuessDriver(fields);
            if (driver == null)
            {
                return null;
            }
            conn = FromFields(driver, fields, workspace);
        }
        else
        {
            if (driver == null)
            {
                return null;
            }
            conn = new Connection { Driver = driver };
            string? Value(string key)
            {
                // A string or a number (Python's str and int; a bool is an int there too).
                if (entry[key] is not JsonValue value)
                {
                    return null;
                }
                string? text = value.GetValueKind() switch
                {
                    JsonValueKind.String => value.GetValue<string>(),
                    JsonValueKind.True => "True",
                    JsonValueKind.False => "False",
                    JsonValueKind.Number when value.ToJsonString() is var number && !number.Contains('.') && !number.Contains('e') && !number.Contains('E') => number,
                    _ => null,
                };
                return string.IsNullOrEmpty(text) ? null : text;
            }
            conn.Host = Value("host");
            conn.Port = Value("port");
            conn.Database = Value("database");
            conn.User = Value("user");
            conn.Password = Value("password");
            conn.File = Value("file");
            if (driver == "sqlite" && conn.File != null)
            {
                var path = Py.ExpandUser(conn.File);
                if (!Py.IsAbs(path) && !string.IsNullOrEmpty(workspace))
                {
                    path = Py.NormPath(Py.Join(workspace, path));
                }
                conn.File = path;
            }
            if (IsTruthy(entry["integratedSecurity"]))
            {
                conn.Integrated = true;
            }
            if (IsTruthy(entry["trustServerCertificate"]))
            {
                conn.Trust = true;
            }
        }
        conn.Name = entry["name"] is JsonValue name && name.TryGetValue<string>(out var n) && n.Length > 0 ? n : DefaultName(conn);
        conn.Origin = "settings";
        return conn;
    }

    public static string DefaultName(Connection conn)
    {
        if (conn.Driver == "sqlite")
        {
            var name = Path.GetFileName(conn.File ?? "");
            return name.Length > 0 ? name : "SQLite";
        }
        var target = conn.Database ?? conn.Host ?? "";
        return $"{Drivers[conn.Driver]} {target}".Trim();
    }

    /// <summary>The first 12 hex digits of a SHA-1 over what makes a connection (as the Python module had it).</summary>
    public static string ConnectionId(Connection conn)
    {
        var key = string.Join("\n", conn.Driver, conn.Name ?? "", conn.Host ?? "", conn.Port ?? "", conn.Database ?? "", conn.User ?? "", conn.File ?? "");
        return Convert.ToHexStringLower(SHA1.HashData(Encoding.UTF8.GetBytes(key)))[..12];
    }

    /// <summary>Where the connection points, for the view: `user@host:port/database` or the file.</summary>
    public static string Target(Connection conn)
    {
        if (conn.Driver == "sqlite")
        {
            return conn.File ?? "";
        }
        var host = conn.Host ?? "localhost";
        if (conn.Port != null)
        {
            host += $":{conn.Port}";
        }
        if (conn.User != null)
        {
            host = $"{conn.User}@{host}";
        }
        return conn.Database != null ? $"{host}/{conn.Database}" : host;
    }

    // ───────────── the workspace ─────────────

    [GeneratedRegex(@",(\s*[}\]])")]
    private static partial Regex TrailingComma();

    /// <summary>JSON with comments and trailing commas (appsettings files allow both).</summary>
    public static JsonDocument LoadsLenient(string text)
    {
        var builder = new StringBuilder();
        int i = 0, n = text.Length;
        while (i < n)
        {
            var c = text[i];
            if (c == '"')
            {
                var j = i + 1;
                while (j < n && text[j] != '"')
                {
                    j += text[j] == '\\' ? 2 : 1;
                }
                builder.Append(text, i, Math.Min(j + 1, n) - i);
                i = j + 1;
            }
            else if (string.CompareOrdinal(text, i, "//", 0, 2) == 0)
            {
                while (i < n && text[i] != '\n')
                {
                    i++;
                }
            }
            else if (string.CompareOrdinal(text, i, "/*", 0, 2) == 0)
            {
                var end = text.IndexOf("*/", i + 2, StringComparison.Ordinal);
                i = end < 0 ? n : end + 2;
            }
            else
            {
                builder.Append(c);
                i++;
            }
        }
        var cleaned = TrailingComma().Replace(builder.ToString(), "$1").TrimStart('﻿');
        return JsonDocument.Parse(cleaned);
    }

    [GeneratedRegex("<packagereference\\s+include=\"([^\"]+)\"")]
    private static partial Regex PackageReference();

    /// <summary>The database provider the project in <paramref name="directory"/> references, when it's exactly one.</summary>
    public static string? ProjectDriver(string directory)
    {
        var found = new HashSet<string>();
        string[] names;
        try
        {
            names = Directory.GetFiles(directory, "*.csproj");
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return null;
        }
        foreach (var file in names)
        {
            if (!file.EndsWith(".csproj", StringComparison.Ordinal))
            {
                continue;
            }
            string text;
            try
            {
                text = ReadStart(file, 500_000).ToLowerInvariant();
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                continue;
            }
            foreach (Match match in PackageReference().Matches(text))
            {
                var package = match.Groups[1].Value;
                foreach (var (prefix, driver) in PackageDrivers)
                {
                    if (package.StartsWith(prefix, StringComparison.Ordinal))
                    {
                        found.Add(driver);
                        break;
                    }
                }
            }
        }
        return found.Count == 1 ? found.First() : null;
    }

    /// <summary>The first <paramref name="chars"/> characters of a UTF-8 file (bad bytes replaced).</summary>
    static string ReadStart(string path, int chars)
    {
        using var reader = new StreamReader(path, new UTF8Encoding(false));
        var buffer = new char[chars];
        var read = reader.ReadBlock(buffer, 0, chars);
        return new string(buffer, 0, read);
    }

    /// <summary>The connections in one appsettings*.json file.</summary>
    public static List<Connection> AppSettingsConnections(string path, string? workspace)
    {
        var result = new List<Connection>();
        List<(string Name, JsonElement Value)> strings;
        try
        {
            using var document = LoadsLenient(ReadStart(path, 1_000_000));
            if (document.RootElement.ValueKind != JsonValueKind.Object ||
                LastProperty(document.RootElement, "ConnectionStrings") is not { ValueKind: JsonValueKind.Object } section)
            {
                return result;
            }
            strings = Properties(section).Select(p => (p.Name, p.Value.Clone())).ToList();
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
            return result;
        }
        var directory = Path.GetDirectoryName(path) ?? "";
        var hint = ProjectDriver(directory);
        foreach (var (name, value) in strings)
        {
            if (value.ValueKind != JsonValueKind.String || value.GetString() is not { } text || !text.Contains('='))
            {
                continue;
            }
            var fields = ParseConnectionString(text);
            var driver = GuessDriver(fields, hint);
            if (driver == null)
            {
                continue;
            }
            var conn = FromFields(driver, fields, directory);
            conn.Name = name;
            conn.Origin = Relative(path, workspace);
            result.Add(conn);
        }
        return result;
    }

    /// <summary>An object's properties as a Python dict would keep them: a repeated key keeps its first place and its last value.</summary>
    static List<(string Name, JsonElement Value)> Properties(JsonElement obj)
    {
        var list = new List<(string Name, JsonElement Value)>();
        var index = new Dictionary<string, int>();
        foreach (var property in obj.EnumerateObject())
        {
            if (index.TryGetValue(property.Name, out var at))
            {
                list[at] = (property.Name, property.Value);
            }
            else
            {
                index[property.Name] = list.Count;
                list.Add((property.Name, property.Value));
            }
        }
        return list;
    }

    static JsonElement? LastProperty(JsonElement obj, string name)
    {
        JsonElement? found = null;
        foreach (var property in obj.EnumerateObject())
        {
            if (property.Name == name)
            {
                found = property.Value;
            }
        }
        return found;
    }

    public static bool IsSqliteFile(string path)
    {
        try
        {
            using var stream = System.IO.File.OpenRead(path);
            var buffer = new byte[16];
            var read = stream.ReadAtLeast(buffer, 16, throwOnEndOfStream: false);
            return read == 16 && buffer.AsSpan().SequenceEqual(SqliteHeader);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    /// <summary>os.walk, top-down, without following links into folders; skipped folders aren't entered.</summary>
    static IEnumerable<(string Directory, List<string> Files)> Walk(string root)
    {
        var stack = new Stack<string>();
        stack.Push(root);
        while (stack.Count > 0)
        {
            var directory = stack.Pop();
            var dirs = new List<string>();
            var files = new List<string>();
            try
            {
                foreach (var entry in new DirectoryInfo(directory).EnumerateFileSystemInfos())
                {
                    var isDir = entry is DirectoryInfo || (entry.LinkTarget != null && Directory.Exists(entry.FullName));
                    if (isDir)
                    {
                        if (entry.LinkTarget == null)
                        {
                            dirs.Add(entry.Name);
                        }
                    }
                    else
                    {
                        files.Add(entry.Name);
                    }
                }
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException or System.Security.SecurityException)
            {
                continue;
            }
            dirs = dirs.Where(d => !SkipDirs.Contains(d) && !d.StartsWith('.')).Order(StringComparer.Ordinal).ToList();
            yield return (directory, files);
            for (var i = dirs.Count - 1; i >= 0; i--)
            {
                stack.Push(Path.Join(directory, dirs[i]));
            }
        }
    }

    /// <summary>Connections found in the workspace: appsettings connection strings, then SQLite files.</summary>
    public static List<Connection> ScanWorkspace(string? root)
    {
        var found = new List<Connection>();
        var files = new List<string>();
        if (string.IsNullOrEmpty(root) || !Directory.Exists(root))
        {
            return found;
        }
        var seen = 0;
        foreach (var (directory, names) in Walk(root))
        {
            seen += names.Count;
            if (seen > MaxFiles)
            {
                break;
            }
            foreach (var name in names.Order(StringComparer.Ordinal))
            {
                var lower = name.ToLowerInvariant();
                var path = Path.Join(directory, name);
                if (lower.StartsWith("appsettings", StringComparison.Ordinal) && lower.EndsWith(".json", StringComparison.Ordinal))
                {
                    found.AddRange(AppSettingsConnections(path, root));
                }
                else if (SqliteExtensions.Any(e => lower.EndsWith(e, StringComparison.Ordinal)) && IsSqliteFile(path))
                {
                    files.Add(path);
                }
            }
        }
        var known = found.Where(c => c.File != null).Select(c => PathKey(c.File!)).ToHashSet();
        foreach (var path in files)
        {
            if (!known.Contains(PathKey(path)))
            {
                var origin = Relative(Path.GetDirectoryName(path) ?? "", root);
                found.Add(new Connection { Driver = "sqlite", File = path, Name = Path.GetFileName(path), Origin = origin.Length > 0 ? origin : "." });
            }
        }
        return found;
    }

    /// <summary>All connections without duplicates (the same target under the same name), in order, with ids.</summary>
    public static List<Connection> Merge(IEnumerable<IEnumerable<Connection>> lists)
    {
        var result = new List<Connection>();
        var seen = new HashSet<string>();
        foreach (var original in lists.SelectMany(l => l))
        {
            var conn = original.Clone();
            conn.Id = ConnectionId(conn);
            if (seen.Add(conn.Id))
            {
                result.Add(conn);
            }
        }
        return result;
    }

    public static string PathKey(string path)
    {
        var full = Path.GetFullPath(path);
        return OperatingSystem.IsWindows() ? full.ToLowerInvariant() : full;
    }

    /// <summary><paramref name="path"/> relative to <paramref name="baseDir"/> ("" for the folder itself), or as is when outside it.</summary>
    public static string Relative(string path, string? baseDir)
    {
        if (!string.IsNullOrEmpty(baseDir))
        {
            var rel = Path.GetRelativePath(Path.GetFullPath(baseDir), Path.GetFullPath(path));
            if (!rel.StartsWith("..", StringComparison.Ordinal) && !Path.IsPathRooted(rel))
            {
                return rel == "." ? "" : rel;
            }
        }
        return path;
    }
}
