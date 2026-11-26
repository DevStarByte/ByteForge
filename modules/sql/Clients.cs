using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;

namespace ByteForge.Sql;

/// <summary>A failure for the user (a missing client, a server's error, a missing file).</summary>
public sealed class SqlException(string message) : Exception(message);

/// <summary>A table or view and its columns.</summary>
public sealed class Table
{
    public string Schema { get; set; } = "";
    public string Name { get; set; } = "";
    public bool View { get; set; }
    public List<Column> Columns { get; } = [];
}

public sealed record Column(string Name, string Type, bool Nullable, bool Pk = false);

/// <summary>PostgreSQL, MySQL / MariaDB and SQL Server through their command line clients (psql, mysql, sqlcmd).</summary>
public static partial class Clients
{
    public const int SelectLimit = 100;
    public static readonly TimeSpan QueryTimeout = TimeSpan.FromSeconds(30);

    public static readonly Dictionary<string, string> ClientNames = new()
    {
        ["postgres"] = "psql", ["mysql"] = "mysql (or mariadb)", ["sqlserver"] = "sqlcmd",
    };

    static readonly Dictionary<string, string[]> ProgramNames = new()
    {
        ["postgres"] = ["psql"], ["mysql"] = ["mysql", "mariadb"], ["sqlserver"] = ["sqlcmd"],
    };

    public static readonly Dictionary<string, string> DefaultSchemas = new() { ["postgres"] = "public", ["sqlserver"] = "dbo" };

    /// <summary>The command line client for a server driver: the `sql.&lt;driver&gt;Path` setting or PATH.</summary>
    public static string? FindClient(string driver, JsonObject? settings)
    {
        if (settings?[$"sql.{driver}Path"] is JsonValue value && value.TryGetValue<string>(out var configured) && Py.Strip(configured).Length > 0)
        {
            var path = Py.ExpandUser(Py.Strip(configured));
            return File.Exists(path) ? path : null;
        }
        foreach (var name in ProgramNames[driver])
        {
            if (Programs.Which(name) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>
    /// The arguments (the client first) and the environment additions for a server client; passwords only ever go
    /// into the environment. <paramref name="mode"/>: "rows" (machine-readable output of <paramref name="query"/>),
    /// "run" (a query or a <paramref name="file"/> for people) or "shell".
    /// </summary>
    public static (List<string> Args, Dictionary<string, string> Env) Command(Connection conn, string client, string mode, string? query = null, string? file = null)
    {
        var env = new Dictionary<string, string>();
        var args = new List<string> { client };
        switch (conn.Driver)
        {
            case "postgres":
                args.Add("-X");
                if (conn.Host != null)
                {
                    args.AddRange(["-h", conn.Host]);
                }
                if (conn.Port != null)
                {
                    args.AddRange(["-p", conn.Port]);
                }
                if (conn.User != null)
                {
                    args.AddRange(["-U", conn.User]);
                }
                if (conn.Database != null)
                {
                    args.AddRange(["-d", conn.Database]);
                }
                if (conn.Password != null)
                {
                    env["PGPASSWORD"] = conn.Password;
                }
                env["PGCONNECT_TIMEOUT"] = "10";
                if (mode == "rows")
                {
                    args.AddRange(["-w", "-q", "-A", "-t", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-c", query ?? ""]);
                }
                else if (mode == "run")
                {
                    args.AddRange(["-v", "ON_ERROR_STOP=1", "-P", "pager=off"]);
                    args.AddRange(file != null ? ["-f", file] : ["-c", query ?? ""]);
                }
                break;
            case "mysql":
                if (conn.Host != null)
                {
                    args.AddRange(["-h", conn.Host]);
                }
                if (conn.Port != null)
                {
                    args.AddRange(["-P", conn.Port]);
                }
                if (conn.User != null)
                {
                    args.AddRange(["-u", conn.User]);
                }
                if (conn.Database != null)
                {
                    args.AddRange(["-D", conn.Database]);
                }
                if (conn.Password != null)
                {
                    env["MYSQL_PWD"] = conn.Password;
                }
                args.Add("--connect-timeout=10");
                if (mode == "rows")
                {
                    args.AddRange(["-B", "-N", "-e", query ?? ""]);
                }
                else if (mode == "run")
                {
                    args.AddRange(["-t", "-v", "-e", file != null ? $"source {file}" : query ?? ""]);
                }
                break;
            case "sqlserver":
                var server = conn.Host ?? "localhost";
                if (conn.Port != null && !server.Contains(','))
                {
                    server += $",{conn.Port}";
                }
                args.AddRange(["-S", server]);
                if (conn.Database != null)
                {
                    args.AddRange(["-d", conn.Database]);
                }
                if (conn.Integrated == true || conn.User == null)
                {
                    args.Add("-E");
                }
                else
                {
                    args.AddRange(["-U", conn.User]);
                    if (conn.Password != null)
                    {
                        env["SQLCMDPASSWORD"] = conn.Password;
                    }
                }
                if (conn.Trust == true)
                {
                    args.Add("-C");
                }
                args.AddRange(["-l", "10"]);
                if (mode == "rows")
                {
                    args.AddRange(["-b", "-h", "-1", "-W", "-s", "|", "-Q", "SET NOCOUNT ON; " + query]);
                }
                else if (mode == "run")
                {
                    args.Add("-b");
                    args.AddRange(file != null ? ["-i", file] : ["-Q", query ?? ""]);
                }
                break;
        }
        return (args, env);
    }

    /// <summary>The rows of a query against a server, as lists of strings.</summary>
    public static async Task<List<string[]>> RunRowsAsync(Connection conn, string query, JsonObject? settings, CancellationToken cancel)
    {
        var client = FindClient(conn.Driver, settings) ?? throw new SqlException($"{ClientNames[conn.Driver]} isn't installed");
        var (args, extra) = Command(conn, client, "rows", query: query);
        ProcessResult done;
        try
        {
            done = await Programs.RunAsync(args[0], args.Skip(1), timeout: QueryTimeout,
                environment: extra.ToDictionary(p => p.Key, p => (string?)p.Value), cancel: cancel);
        }
        catch (ModuleException error)
        {
            throw new SqlException($"couldn't run {Path.GetFileName(client)}: {error.Message}");
        }
        catch (TimeoutException)
        {
            throw new SqlException("the server didn't answer in time");
        }
        if (!done.Success)
        {
            var text = Py.Strip(done.Error) is { Length: > 0 } error ? error : Py.Strip(done.Output) is { Length: > 0 } output ? output : $"exit code {done.ExitCode}";
            throw new SqlException(Py.SplitLines(text)[0]);
        }
        var separator = conn.Driver == "sqlserver" ? '|' : '\t';
        return Py.SplitLines(done.Output.Replace("\r\n", "\n")).Where(line => Py.Strip(line).Length > 0).Select(line => line.Split(separator)).ToList();
    }

    public static readonly Dictionary<string, string> SchemaQueries = new()
    {
        ["postgres"] = "SELECT c.table_schema, c.table_name, t.table_type, c.column_name, c.data_type, c.is_nullable " +
                       "FROM information_schema.columns c JOIN information_schema.tables t " +
                       "ON t.table_schema = c.table_schema AND t.table_name = c.table_name " +
                       "WHERE c.table_schema NOT IN ('pg_catalog', 'information_schema') " +
                       "ORDER BY c.table_schema, c.table_name, c.ordinal_position",
        ["mysql"] = "SELECT c.table_schema, c.table_name, t.table_type, c.column_name, c.column_type, c.is_nullable " +
                    "FROM information_schema.columns c JOIN information_schema.tables t " +
                    "ON t.table_schema = c.table_schema AND t.table_name = c.table_name " +
                    "WHERE c.table_schema = DATABASE() " +
                    "ORDER BY c.table_schema, c.table_name, c.ordinal_position",
        ["sqlserver"] = "SELECT c.TABLE_SCHEMA, c.TABLE_NAME, t.TABLE_TYPE, c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE " +
                        "FROM INFORMATION_SCHEMA.COLUMNS c JOIN INFORMATION_SCHEMA.TABLES t " +
                        "ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME " +
                        "ORDER BY c.TABLE_SCHEMA, c.TABLE_NAME, c.ORDINAL_POSITION",
    };

    /// <summary>Tables from (schema, table, type, column, data type, nullable) rows, in order.</summary>
    public static List<Table> TablesFromRows(IEnumerable<string[]> rows)
    {
        var tables = new List<Table>();
        var index = new Dictionary<(string, string), Table>();
        foreach (var row in rows)
        {
            if (row.Length < 6)
            {
                continue;
            }
            var values = row.Take(6).Select(Py.Strip).ToArray();
            var key = (values[0], values[1]);
            if (!index.TryGetValue(key, out var table))
            {
                table = new Table { Schema = values[0], Name = values[1], View = values[2].ToUpperInvariant().Contains("VIEW") };
                index[key] = table;
                tables.Add(table);
            }
            table.Columns.Add(new Column(values[3], values[4], values[5].ToUpperInvariant() == "YES"));
        }
        return tables;
    }

    public static string QuoteIdentifier(string driver, string name) => driver switch
    {
        "mysql" => "`" + name.Replace("`", "``") + "`",
        "sqlserver" => "[" + name.Replace("]", "]]") + "]",
        _ => "\"" + name.Replace("\"", "\"\"") + "\"",
    };

    [GeneratedRegex("^[A-Za-z_][A-Za-z0-9_]*$")]
    private static partial Regex PlainName();

    /// <summary>Whether a name can go into a query unquoted (PostgreSQL folds unquoted names to lowercase).</summary>
    public static bool PlainIdentifier(string name, string? driver = null)
    {
        if (driver == "postgres" && name != name.ToLowerInvariant())
        {
            return false;
        }
        return PlainName().IsMatch(name) && !name.EndsWith('\n') && !SqlFormatter.Keywords.Contains(name.ToUpperInvariant());
    }

    /// <summary>How to write a table in a query: schema-qualified unless it's in the default schema.</summary>
    public static string TableReference(Connection conn, Table table, bool alwaysQuote = false)
    {
        var driver = conn.Driver;
        string Part(string name) => PlainIdentifier(name, driver) && !alwaysQuote ? name : QuoteIdentifier(driver, name);
        if (table.Schema.Length > 0 && driver != "mysql" && table.Schema != DefaultSchemas.GetValueOrDefault(driver))
        {
            return $"{Part(table.Schema)}.{Part(table.Name)}";
        }
        return Part(table.Name);
    }

    public static string SelectQuery(Connection conn, Table table, int limit = SelectLimit)
    {
        var name = TableReference(conn, table);
        return conn.Driver == "sqlserver" ? $"SELECT TOP {limit} * FROM {name};" : $"SELECT * FROM {name} LIMIT {limit};";
    }
}
