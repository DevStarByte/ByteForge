using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;
using ByteForge.Modules.Testing;
using ByteForge.Sql;
using Microsoft.Data.Sqlite;

namespace ByteForge.Sql.Tests;

/// <summary>Temporary folders and the sample database the tests share.</summary>
public abstract class TempFolder : IDisposable
{
    protected readonly string Root = Directory.CreateTempSubdirectory("bf-sql-").FullName;

    public void Dispose()
    {
        try
        {
            Directory.Delete(Root, recursive: true);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
        }
        GC.SuppressFinalize(this);
    }

    protected static void Write(string path, string text = "")
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text);
    }

    protected static void Execute(string path, string sql)
    {
        using var db = new SqliteConnection(new SqliteConnectionStringBuilder { DataSource = path, Pooling = false }.ToString());
        db.Open();
        using var command = db.CreateCommand();
        command.CommandText = sql;
        command.ExecuteNonQuery();
    }

    protected static long Scalar(string path, string sql)
    {
        using var db = new SqliteConnection(new SqliteConnectionStringBuilder { DataSource = path, Pooling = false }.ToString());
        db.Open();
        using var command = db.CreateCommand();
        command.CommandText = sql;
        return (long)command.ExecuteScalar()!;
    }

    protected static void MakeDb(string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        Execute(path, """
            CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT);
            CREATE TABLE "order items" (id INTEGER PRIMARY KEY, qty INT);
            CREATE VIEW named AS SELECT name FROM customers;
            INSERT INTO customers (name, email) VALUES ('Ada', 'ada@example.com'), ('Linus', NULL);
            """);
    }
}

public sealed class ConnectionStringTests
{
    [Fact]
    public void Parses_quoted_values_and_keys()
    {
        var fields = Connections.ParseConnectionString("Server=localhost; Database = Shop;Password=\"a;b\"\"c\";;User Id=sa");
        Assert.Equal(new Dictionary<string, string> { ["server"] = "localhost", ["database"] = "Shop", ["password"] = "a;b\"c", ["user id"] = "sa" }, fields);
    }

    [Fact]
    public void Guesses_the_driver()
    {
        static string? Guess(string text, string? hint = null) => Connections.GuessDriver(Connections.ParseConnectionString(text), hint);
        Assert.Equal("sqlite", Guess("Data Source=app.db"));
        Assert.Equal("sqlite", Guess("Data Source=app.db", "sqlserver"));
        Assert.Equal("postgres", Guess("Host=localhost;Database=app;Username=postgres;Password=x"));
        Assert.Equal("sqlserver", Guess("Server=(localdb)\\MSSQLLocalDB;Database=Shop;Trusted_Connection=True"));
        Assert.Equal("mysql", Guess("Server=localhost;Database=shop;Uid=root;Pwd=secret"));
        Assert.Equal("sqlserver", Guess("Server=localhost;Database=shop"));
        Assert.Equal("postgres", Guess("Server=localhost;Database=shop", "postgres"));
        Assert.Null(Guess("Foo=bar"));
    }

    [Fact]
    public void Sql_server_fields()
    {
        var fields = Connections.ParseConnectionString("Server=tcp:db.example.com,1433;Initial Catalog=Shop;User ID=app;Password=pw;TrustServerCertificate=True");
        var conn = Connections.FromFields("sqlserver", fields);
        Assert.Equal("""{"driver":"sqlserver","host":"db.example.com,1433","database":"Shop","user":"app","password":"pw","integrated":false,"trust":true}""",
            conn.ToJson().ToJsonString());
    }

    [Fact]
    public void Sqlite_paths_are_relative_to_the_project()
    {
        var conn = Connections.FromFields("sqlite", new() { ["data source"] = "data/app.db" }, "/src/Web");
        Assert.Equal(Py.NormPath("/src/Web/data/app.db"), conn.File);
    }

    [Fact]
    public void Settings_entries()
    {
        var conn = Connections.FromSetting(JsonNode.Parse("""{"name": "Local", "driver": "PostgreSQL", "host": "localhost", "port": 5432, "database": "app"}"""), "/ws")!;
        Assert.Equal(("postgres", "5432", "Local", "settings"), (conn.Driver, conn.Port, conn.Name, conn.Origin));
        conn = Connections.FromSetting(JsonNode.Parse("""{"driver": "sqlite", "file": "cache.db"}"""), "/ws")!;
        Assert.Equal((Py.NormPath("/ws/cache.db"), "cache.db"), (conn.File, conn.Name));
        Assert.Null(Connections.FromSetting(JsonNode.Parse("""{"name": "no driver"}"""), "/ws"));
        conn = Connections.FromSetting(JsonNode.Parse("""{"connectionString": "Host=h;Database=d"}"""), "/ws")!;
        Assert.Equal("postgres", conn.Driver);
    }

    [Fact]
    public void Python_path_rules()
    {
        Assert.Equal(".", Py.NormPath(""));
        Assert.Equal(Path.Join("a", "c"), Py.NormPath("a/./b/../c/"));
        Assert.Equal(Programs.Home + "/x.db", Py.ExpandUser("~/x.db"));
        Assert.Equal("1.0", Py.FloatRepr(1.0));
        Assert.Equal("1e+16", Py.FloatRepr(1e16));
        Assert.Equal("1e-05", Py.FloatRepr(0.00001));
        Assert.Equal("0.0001", Py.FloatRepr(0.0001));
        Assert.Equal("0.1", Py.FloatRepr(0.1));
        Assert.Equal("-2.5", Py.FloatRepr(-2.5));
    }
}

public sealed class WorkspaceTests : TempFolder
{
    [Fact]
    public void Finds_appsettings_connections_and_sqlite_files()
    {
        Write(Path.Combine(Root, "Web", "Web.csproj"),
            "<Project><ItemGroup><PackageReference Include=\"Npgsql.EntityFrameworkCore.PostgreSQL\" Version=\"9\" /></ItemGroup></Project>");
        Write(Path.Combine(Root, "Web", "appsettings.json"), """
            {
                // comments and trailing commas are fine here
                "ConnectionStrings": {
                    "Default": "Server=localhost;Database=web;User Id=me;Password=pw",
                    "Cache": "Data Source=cache.db",
                },
            }
            """);
        MakeDb(Path.Combine(Root, "Web", "cache.db"));
        MakeDb(Path.Combine(Root, "tools", "other.sqlite"));
        Write(Path.Combine(Root, "tools", "fake.db"), "not a database");
        MakeDb(Path.Combine(Root, "Web", "bin", "copy.db"));
        var found = Connections.ScanWorkspace(Root);
        Assert.Equal([("Default", "postgres"), ("Cache", "sqlite"), ("other.sqlite", "sqlite")], found.Select(c => (c.Name, c.Driver)));
        Assert.Equal(Path.Join("Web", "appsettings.json"), found[0].Origin);
        Assert.Equal("me", found[0].User);
        Assert.Equal("tools", found[2].Origin);
    }

    [Fact]
    public void Merge_drops_duplicates()
    {
        var a = new Connection { Driver = "sqlite", File = "/x.db", Name = "x" };
        var merged = Connections.Merge([[a], [a.Clone(), new Connection { Driver = "sqlite", File = "/y.db", Name = "y" }]]);
        Assert.Equal(["x", "y"], merged.Select(c => c.Name));
        Assert.All(merged, c => Assert.Equal(12, c.Id!.Length));
    }

    [Fact]
    public void Connection_ids_are_the_python_ones()
    {
        // What the Python module made of it (sha1 of the fields, one per line): state.json keeps working.
        Assert.Equal("62070e4eb4bc", Connections.ConnectionId(new Connection { Driver = "sqlite", Name = "x", File = "/x.db" }));
    }
}

public sealed class SqliteTests : TempFolder
{
    [Fact]
    public void Tables_views_and_columns()
    {
        var path = Path.Combine(Root, "app.db");
        MakeDb(path);
        var tables = Sqlite.Tables(path);
        Assert.Equal([("customers", false), ("order items", false), ("named", true)], tables.Select(t => (t.Name, t.View)));
        Assert.Equal([("id", true, false), ("name", false, false), ("email", false, true)], tables[0].Columns.Select(c => (c.Name, c.Pk, c.Nullable)));
    }

    [Fact]
    public void Missing_file()
    {
        Assert.Throws<SqlException>(() => Sqlite.Tables(Path.Combine(Root, "nonexistent", "app.db")));
    }

    [Fact]
    public void Runs_a_script()
    {
        var path = Path.Combine(Root, "app.db");
        MakeDb(path);
        var output = new StringWriter();
        var code = Sqlite.RunScript(path, """

            -- a comment; with a semicolon
            INSERT INTO customers (name) VALUES ('Grace; Hopper');
            SELECT id, name FROM customers ORDER BY id;
            UPDATE customers SET email = 'x' WHERE id > 1

            """, output);
        var text = output.ToString();
        Assert.True(code == 0, text);
        Assert.Contains("OK, 1 row affected", text);
        Assert.Contains("Grace; Hopper", text);
        Assert.Contains("(3 rows)", text);
        Assert.Contains("OK, 2 rows affected", text);
        Assert.Equal(2, Scalar(path, "SELECT count(*) FROM customers WHERE email = 'x'"));
    }

    [Fact]
    public void Prints_like_the_python_launcher()
    {
        var path = Path.Combine(Root, "app.db");
        MakeDb(path);
        var output = new StringWriter();
        var code = Sqlite.RunScript(path, "CREATE TABLE t (a, b);\nINSERT INTO t VALUES (1.5, x'00ff'), (1e16, 'é⏎');\nSELECT * FROM t;\nSELECT * FROM t WHERE 0;", output);
        Assert.Equal(0, code);
        var text = output.ToString();
        Assert.StartsWith("OK, 2 rows affected\n\na     | b\n------+-------\n  1.5 | 0x00ff\n1e+16 | é⏎\n(2 rows)\n\na | b\n--+--\n(0 rows)\n\nDone in ", text);
    }

    [Fact]
    public void Stops_at_the_first_error()
    {
        var path = Path.Combine(Root, "app.db");
        MakeDb(path);
        var output = new StringWriter();
        var code = Sqlite.RunScript(path, "SELECT 1; SELECT * FROM nope; SELECT 2;", output);
        Assert.Equal(1, code);
        Assert.Contains("Error in statement 2", output.ToString());
        Assert.Contains("no such table", output.ToString());
    }

    [Fact]
    public void Splits_statements()
    {
        Assert.Equal(["SELECT ';';", "-- x;\nSELECT 2;"], Sqlite.SplitStatements("SELECT ';'; -- x;\nSELECT 2;\n-- only a comment"));
        Assert.Equal(["SELECT 1"], Sqlite.SplitStatements("SELECT 1"));
    }

    [Fact]
    public void Formats_a_table()
    {
        var text = Sqlite.FormatTable(["id", "name"], [[1L, "Ada"], [10L, null]]);
        Assert.Equal(["id | name", "---+-----", " 1 | Ada", "10 | NULL"], text.Split('\n'));
    }

    [Fact]
    public void The_shell_runs_statements_and_lists_tables()
    {
        var path = Path.Combine(Root, "app.db");
        MakeDb(path);
        var output = new StringWriter();
        var code = Sqlite.Shell(path, new StringReader(".tables\nSELECT name\nFROM customers ORDER BY id;\n.help\n"), output);
        Assert.Equal(0, code);
        var text = output.ToString();
        Assert.Contains("sqlite> customers\norder items\nnamed (view)\n", text);
        Assert.Contains("   ...> name\n-----\nAda\nLinus\n(2 rows)\n\nDone in ", text);
        Assert.Contains("Commands: .tables, .quit", text);
    }
}

public sealed class ServerTests
{
    [Fact]
    public void Client_commands_keep_passwords_in_the_environment()
    {
        var pg = new Connection { Driver = "postgres", Host = "h", Port = "5433", User = "u", Database = "d", Password = "pw" };
        var (args, env) = Clients.Command(pg, "psql", "run", file: "/q.sql");
        Assert.DoesNotContain("pw", args);
        Assert.Equal("pw", env["PGPASSWORD"]);
        Assert.Equal(["psql", "-X", "-h", "h", "-p", "5433", "-U", "u", "-d", "d"], args.Take(10));
        Assert.Equal(["-f", "/q.sql"], args.TakeLast(2));

        var my = new Connection { Driver = "mysql", Host = "h", User = "root", Database = "shop", Password = "pw" };
        (args, env) = Clients.Command(my, "mysql", "rows", query: "SELECT 1");
        Assert.Equal(("pw", (IEnumerable<string>)["-N", "-e", "SELECT 1"]), (env["MYSQL_PWD"], args.TakeLast(3)), new TupleComparer());
        Assert.DoesNotContain("pw", args);

        var ms = new Connection { Driver = "sqlserver", Host = "db", Port = "1444", User = "sa", Password = "pw", Trust = true };
        (args, env) = Clients.Command(ms, "sqlcmd", "shell");
        Assert.Equal(["sqlcmd", "-S", "db,1444", "-U", "sa", "-C", "-l", "10"], args);
        Assert.Equal(new Dictionary<string, string> { ["SQLCMDPASSWORD"] = "pw" }, env);
        (args, _) = Clients.Command(new Connection { Driver = "sqlserver", Host = "db", Integrated = true }, "sqlcmd", "shell");
        Assert.Contains("-E", args);
    }

    sealed class TupleComparer : IEqualityComparer<(string, IEnumerable<string>)>
    {
        public bool Equals((string, IEnumerable<string>) x, (string, IEnumerable<string>) y) => x.Item1 == y.Item1 && x.Item2.SequenceEqual(y.Item2);

        public int GetHashCode((string, IEnumerable<string>) obj) => obj.Item1.GetHashCode();
    }

    [Fact]
    public void Reads_schema_rows()
    {
        List<string[]> rows =
        [
            ["public", "users", "BASE TABLE", "id", "integer", "NO"],
            ["public", "users", "BASE TABLE", "name", "text", "YES"],
            ["public", "active", "VIEW", "id", "integer", "YES"],
            ["broken"],
        ];
        var tables = Clients.TablesFromRows(rows);
        Assert.Equal([("users", false, 2), ("active", true, 1)], tables.Select(t => (t.Name, t.View, t.Columns.Count)));
        Assert.False(tables[0].Columns[0].Nullable);
    }

    [Fact]
    public void Select_queries()
    {
        var pg = new Connection { Driver = "postgres" };
        Assert.Equal("SELECT * FROM users LIMIT 100;", Clients.SelectQuery(pg, new Table { Schema = "public", Name = "users" }));
        Assert.Equal("SELECT * FROM audit.\"Log\" LIMIT 100;", Clients.SelectQuery(pg, new Table { Schema = "audit", Name = "Log" }));
        Assert.Equal("SELECT TOP 100 * FROM [order];", Clients.SelectQuery(new Connection { Driver = "sqlserver" }, new Table { Schema = "dbo", Name = "order" }));
        Assert.Equal("SELECT * FROM `a b` LIMIT 100;", Clients.SelectQuery(new Connection { Driver = "mysql" }, new Table { Schema = "shop", Name = "a b" }));
    }
}

public sealed class FormatterTests
{
    [Fact]
    public void Uppercases_keywords_only_outside_strings_comments_and_names()
    {
        var text = "select id, \"select\" from t -- from here\nwhere name = 'from' and t.key = 1   \n\n\n";
        Assert.Equal("SELECT id, \"select\" FROM t -- from here\nWHERE name = 'from' AND t.key = 1\n", SqlFormatter.Format(text));
    }

    [Fact]
    public void Lowercase_and_preserve()
    {
        Assert.Equal("select X from [Order] /* SELECT */\n", SqlFormatter.Format("SELECT X FROM [Order] /* SELECT */", "lower"));
        Assert.Equal("Select 1\n", SqlFormatter.Format("Select 1", "preserve"));
    }

    [Fact]
    public void Dollar_quoted_bodies_stay()
    {
        Assert.Equal("CREATE FUNCTION f() RETURNS INT AS $$ select 1 $$ LANGUAGE sql;\n",
            SqlFormatter.Format("create function f() returns int as $$ select 1 $$ language sql;"));
    }

    [Fact]
    public void Variables_stay()
    {
        Assert.Equal("DECLARE @table INT; SELECT :limit\n", SqlFormatter.Format("declare @table int; select :limit"));
    }

    [Fact]
    public void Line_ends_and_empty_text()
    {
        Assert.Equal("SELECT 1\n\nFROM t\n", SqlFormatter.Format("select 1 \r\n\r\nfrom t\r\n\r\n"));
        Assert.Equal("", SqlFormatter.Format("  \n\n"));
        Assert.Equal("SELECT $tag$ select $tag$ FROM t\n", SqlFormatter.Format("select $tag$ select $tag$ from t"));
    }
}

/// <summary>The module with the database clients replaced: no psql, mysql or sqlcmd needed.</summary>
sealed class TestSqlModule : SqlModule
{
    public List<string[]> Rows { get; set; } = [];

    protected override Task<List<string[]>> RunRowsAsync(Connection conn, string query) => Task.FromResult(Rows);

    protected override string? FindClient(string driver) => "/usr/bin/" + driver;
}

public sealed class ProtocolTests : TempFolder
{
    string ModuleDir => Directory.CreateDirectory(Path.Combine(Root, "config", "modules", "sql")).FullName;

    string Workspace => Path.Combine(Root, "ws");

    static string Id(JsonNode? node) => node!["id"]!.GetValue<string>();

    static List<JsonObject> Sent(ModuleHarness module)
    {
        lock (module.Sent)
        {
            return [.. module.Sent];
        }
    }

    /// <summary>The spec file a runInTerminal command line names (the last word), read and deleted.</summary>
    static JsonObject ReadSpec(JsonObject run)
    {
        var path = run["command"]!.GetValue<string>().Split(' ')[^1].Trim('\'', '"');
        var spec = JsonNode.Parse(File.ReadAllText(path))!.AsObject();
        File.Delete(path);
        return spec;
    }

    [Fact]
    public async Task Registers_commands_and_the_formatter()
    {
        await using var module = ModuleHarness.Start(new SqlModule(), ModuleDir);
        var expected = new (string Id, string Title, string? Key)[]
        {
            ("sql.runFile", "Run File", "ctrl-shift-enter"), ("sql.selectConnection", "Select Connection…", null),
            ("sql.openShell", "Open Shell…", null), ("sql.addConnection", "Add Connection…", null), ("sql.refresh", "Refresh Connections", null),
        };
        foreach (var (id, title, key) in expected)
        {
            var command = await module.Next("registerCommand");
            Assert.Equal((id, title, "SQL", key), (Id(command), command["title"]!.GetValue<string>(), command["category"]!.GetValue<string>(),
                command["keybinding"]?.GetValue<string>()));
        }
        var formatter = await module.Next("registerFormatter");
        Assert.Equal("""{"type":"registerFormatter","id":"sql.format","name":"SQL (keyword case)","extensions":["sql"]}""", formatter.ToJsonString());
    }

    [Fact]
    public async Task View_status_and_run()
    {
        MakeDb(Path.Combine(Workspace, "app.db"));
        var query = Path.Combine(Workspace, "q.sql");
        Write(query, "SELECT 1;");
        await using var module = ModuleHarness.Start(new SqlModule(), ModuleDir);
        module.Activate(Workspace);
        module.Send(new() { ["type"] = "event", ["name"] = "editorChanged", ["path"] = query });
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "sql.runFile" });
        var run = await module.Next("runInTerminal");
        var sent = Sent(module);
        var connections = sent.Last(m => m["type"]!.GetValue<string>() == "setTree")["items"]![0]!["children"]!.AsArray();
        Assert.Equal(["app.db  ✓"], connections.Select(c => c!["label"]!.GetValue<string>()));
        Assert.Equal("SQL: app.db", sent.Last(m => m["type"]!.GetValue<string>() == "status")["text"]!.GetValue<string>());
        Assert.True(run["saveAll"]!.GetValue<bool>());
        Assert.Equal("SQL: q.sql → app.db", run["title"]!.GetValue<string>());
        Assert.Equal(Workspace, run["cwd"]!.GetValue<string>());
        Assert.Contains(Path.Combine(ModuleDir, "bin", "Sql.dll"), run["command"]!.GetValue<string>());
        var spec = ReadSpec(run);
        Assert.Equal(("run", query, "sqlite"), (spec["mode"]!.GetValue<string>(), spec["file"]!.GetValue<string>(), spec["connection"]!["driver"]!.GetValue<string>()));
    }

    [Fact]
    public async Task Loading_tables_and_inserting_names()
    {
        MakeDb(Path.Combine(Workspace, "app.db"));
        await using var module = ModuleHarness.Start(new SqlModule(), ModuleDir);
        module.Activate(Workspace);
        var tree = await module.Next("setTree", m => m["items"]![0]!["children"]![0]!["id"]!.GetValue<string>().StartsWith("conn:", StringComparison.Ordinal));
        var cid = Id(tree["items"]![0]!["children"]![0])["conn:".Length..];
        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = $"conn:{cid}", ["action"] = "click" });
        tree = await module.Next("setTree", m => m["items"]![0]!["children"]![0]!["count"] != null);
        var tables = tree["items"]![0]!["children"]![0]!["children"]!.AsArray();
        Assert.Equal(["customers", "order items", "named"], tables.Select(t => t!["label"]!.GetValue<string>()));
        Assert.Equal("view", tables[2]!["description"]!.GetValue<string>());
        var columns = tables[0]!["children"]!.AsArray();
        Assert.Equal(["integer not null", "text not null", "text"], columns.Select(c => c!["description"]!.GetValue<string>()));
        Assert.Equal("#44a8e0", columns[0]!["iconColor"]!.GetValue<string>());
        Assert.Equal("id INTEGER · primary key\nClick to insert the name", columns[0]!["tooltip"]!.GetValue<string>());

        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = $"table:{cid}:1", ["action"] = "insert" });
        Assert.Equal("\"order items\"", (await module.Next("insertText"))["text"]!.GetValue<string>());
        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = $"col:{cid}:0:1", ["action"] = "click" });
        Assert.Equal("name", (await module.Next("insertText"))["text"]!.GetValue<string>());
        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = $"table:{cid}:0", ["action"] = "select" });
        var run = await module.Next("runInTerminal");
        Assert.Equal("SQL: customers", run["title"]!.GetValue<string>());
        Assert.Equal("SELECT * FROM customers LIMIT 100;", ReadSpec(run)["query"]!.GetValue<string>());
    }

    [Fact]
    public async Task A_broken_database_shows_the_error()
    {
        Write(Path.Combine(Workspace, "appsettings.json"), """{ "ConnectionStrings": { "Db": "Data Source=missing.db" } }""");
        await using var module = ModuleHarness.Start(new SqlModule(), ModuleDir);
        module.Activate(Workspace);
        var tree = await module.Next("setTree", m => m["items"]![0]!["children"]![0]!["id"]!.GetValue<string>().StartsWith("conn:", StringComparison.Ordinal));
        var conn = tree["items"]![0]!["children"]![0]!;
        Assert.Equal("SQLite · appsettings.json", conn["description"]!.GetValue<string>());
        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = Id(conn), ["action"] = "click" });
        tree = await module.Next("setTree", m => Id(m["items"]![0]!["children"]![0]!["children"]?[0]).StartsWith("error:", StringComparison.Ordinal));
        var error = tree["items"]![0]!["children"]![0]!["children"]![0]!;
        Assert.Equal("Couldn't load the tables", error["label"]!.GetValue<string>());
        Assert.Equal(Path.Combine(Workspace, "missing.db") + " doesn't exist", error["description"]!.GetValue<string>());
    }

    [Fact]
    public async Task Server_tables_and_shells_keep_the_password_off_the_command_line()
    {
        var settings = JsonNode.Parse("""
            { "sql.discover": false, "sql.connections": [
              { "name": "Pg", "driver": "postgres", "host": "db", "database": "app", "user": "me", "password": "s3cret" },
              { "name": "Shop", "driver": "sqlserver", "connectionString": "Server=localhost;Database=Shop;Trusted_Connection=True" } ] }
            """)!.AsObject();
        var sql = new TestSqlModule { Rows = [["audit", "Log", "BASE TABLE", "id", "integer", "NO"], ["public", "users", "VIEW", "Name", "text", "YES"]] };
        await using var module = ModuleHarness.Start(sql, ModuleDir);
        module.Activate(Workspace, settings);
        var tree = await module.Next("setTree", m => m["items"]![0]!["children"]!.AsArray().Count == 2);
        var pg = tree["items"]![0]!["children"]![0]!;
        Assert.Equal("Pg", pg["label"]!.GetValue<string>());
        Assert.Equal("Pg (PostgreSQL)\nme@db/app\nFrom settings\nClick to load the tables", pg["tooltip"]!.GetValue<string>());
        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = Id(pg), ["action"] = "click" });
        tree = await module.Next("setTree", m => m["items"]![0]!["children"]![0]!["count"] != null);
        var tables = tree["items"]![0]!["children"]![0]!["children"]!.AsArray();
        Assert.Equal(["audit.Log", "users"], tables.Select(t => t!["label"]!.GetValue<string>()));
        var cid = Id(pg)["conn:".Length..];
        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = $"col:{cid}:1:0", ["action"] = "click" });
        Assert.Equal("\"Name\"", (await module.Next("insertText"))["text"]!.GetValue<string>());

        module.Send(new() { ["type"] = "treeAction", ["view"] = "sql", ["item"] = Id(pg), ["action"] = "shell" });
        var run = await module.Next("runInTerminal");
        Assert.DoesNotContain("s3cret", run.ToJsonString());
        var spec = ReadSpec(run);
        Assert.Equal(("shell", "s3cret"), (spec["mode"]!.GetValue<string>(), spec["connection"]!["password"]!.GetValue<string>()));
    }

    [Fact]
    public async Task Run_file_asks_which_connection_and_remembers_it()
    {
        MakeDb(Path.Combine(Workspace, "a.db"));
        MakeDb(Path.Combine(Workspace, "b.db"));
        var query = Path.Combine(Workspace, "q.sql");
        Write(query, "SELECT 1;");
        await using var module = ModuleHarness.Start(new SqlModule(), ModuleDir);
        module.Activate(Workspace);
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "sql.runFile" });
        Assert.Contains("open a .sql file first", (await module.Next("log"))["text"]!.GetValue<string>());
        module.Send(new() { ["type"] = "event", ["name"] = "editorChanged", ["path"] = query });
        var status = await module.Next("status", m => m["text"]!.GetValue<string>().Length > 0);
        Assert.Equal("SQL: no connection", status["text"]!.GetValue<string>());
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "sql.runFile" });
        var pick = await module.Next("quickPick");
        Assert.Equal("Run against which connection?", pick["placeholder"]!.GetValue<string>());
        var items = pick["items"]!.AsArray();
        Assert.Equal(["a.db", "b.db"], items.Select(i => i!["label"]!.GetValue<string>()));
        Assert.Equal("SQLite", items[0]!["hint"]!.GetValue<string>());
        module.Send(new() { ["type"] = "quickPickResult", ["requestId"] = pick["requestId"]!.DeepClone(), ["value"] = items[1]!["value"]!.DeepClone() });
        var run = await module.Next("runInTerminal");
        Assert.Equal("SQL: q.sql → b.db", run["title"]!.GetValue<string>());
        ReadSpec(run);
        Assert.Equal("SQL: b.db", Sent(module).Last(m => m["type"]!.GetValue<string>() == "status")["text"]!.GetValue<string>());
        // The choice is remembered in the module's own folder, per workspace.
        var state = JsonNode.Parse(File.ReadAllText(Path.Combine(ModuleDir, "state.json")))!;
        Assert.Equal(items[1]!["value"]!.GetValue<string>(), state["active"]![Workspace]!.GetValue<string>());
    }

    [Fact]
    public async Task No_connections()
    {
        Directory.CreateDirectory(Workspace);
        await using var module = ModuleHarness.Start(new SqlModule(), ModuleDir);
        module.Activate(Workspace);
        var tree = await module.Next("setTree", m => m["message"] == null);
        Assert.Equal("note:none", Id(tree["items"]![0]!["children"]![0]));
        Assert.Equal(["add", "refresh"], tree["actions"]!.AsArray().Select(Id));
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "sql.openShell" });
        Assert.StartsWith("sql: no connections.", (await module.Next("log"))["text"]!.GetValue<string>());
        await module.Next("showView", m => Id(m) == "output");
    }

    [Fact]
    public async Task Formatter_requests()
    {
        await using var module = ModuleHarness.Start(new SqlModule(), ModuleDir);
        module.Send(new() { ["type"] = "format", ["requestId"] = 7, ["path"] = "/a.sql", ["text"] = "select 1" });
        Assert.Equal("""{"type":"formatResult","requestId":7,"text":"SELECT 1\n"}""", (await module.Next("formatResult")).ToJsonString());
        module.Activate(null, new JsonObject { ["sql.format.keywordCase"] = "lower" });
        module.Send(new() { ["type"] = "format", ["requestId"] = 8, ["path"] = "/a.sql", ["text"] = "SELECT 1" });
        Assert.Equal("select 1\n", (await module.Next("formatResult"))["text"]!.GetValue<string>());
    }
}

public sealed class ManifestTests : TempFolder
{
    /// <summary>The module's folder: the tests build into its tests/bin/.</summary>
    static string ModuleFolder() => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", ".."));

    [Fact]
    public void Manifest()
    {
        var manifest = JsonNode.Parse(File.ReadAllText(Path.Combine(ModuleFolder(), "module.json")), documentOptions: new JsonDocumentOptions { CommentHandling = JsonCommentHandling.Skip })!;
        Assert.Equal("byteforge.sql", manifest["id"]!.GetValue<string>());
        Assert.Equal(("dotnet", "bin/Sql.dll"), (manifest["runtime"]!.GetValue<string>(), manifest["assembly"]!.GetValue<string>()));
        Assert.Null(manifest["command"]);
        Assert.Equal(SqlModule.View, manifest["views"]![0]!["id"]!.GetValue<string>());
        foreach (var rule in manifest["languages"]![0]!["rules"]!.AsArray())
        {
            _ = new Regex(rule!["match"]!.GetValue<string>());
        }
    }

    [Fact]
    public void Launcher_runs_sqlite_from_a_spec()
    {
        var path = Path.Combine(Root, "app.db");
        MakeDb(path);
        var spec = Launcher.WriteSpec(new JsonObject
        {
            ["connection"] = new JsonObject { ["driver"] = "sqlite", ["file"] = path, ["name"] = "app" },
            ["mode"] = "run",
            ["query"] = "SELECT count(*) AS n FROM customers;",
        });
        var output = new StringWriter();
        var code = Launcher.Launch(spec, output, new StringReader(""));
        Assert.Equal(0, code);
        Assert.False(File.Exists(spec));
        Assert.StartsWith($"── app · SQLite · {path}\n\nn\n-\n2\n(1 row)\n", output.ToString());
    }

    [Fact]
    public void Launcher_runs_a_file_with_windows_line_ends()
    {
        var path = Path.Combine(Root, "app.db");
        MakeDb(path);
        var file = Path.Combine(Root, "q.sql");
        File.WriteAllText(file, "\uFEFFSELECT name\r\nFROM customers\r\nORDER BY id;\r\n");
        var spec = Launcher.WriteSpec(new JsonObject
        {
            ["connection"] = new JsonObject { ["driver"] = "sqlite", ["file"] = path, ["name"] = "app" },
            ["mode"] = "run",
            ["file"] = file,
        });
        var output = new StringWriter();
        Assert.Equal(0, Launcher.Launch(spec, output, new StringReader("")));
        Assert.Contains("\n\nname\n-----\nAda\nLinus\n(2 rows)\n\nDone in ", output.ToString());
    }

    [Fact]
    public void Spec_files_are_private()
    {
        var spec = Launcher.WriteSpec(new JsonObject { ["mode"] = "shell" });
        try
        {
            if (!OperatingSystem.IsWindows())
            {
                Assert.Equal(UnixFileMode.UserRead | UnixFileMode.UserWrite, File.GetUnixFileMode(spec));
            }
            Assert.StartsWith("byteforge-sql-", Path.GetFileName(spec));
        }
        finally
        {
            File.Delete(spec);
        }
    }
}
