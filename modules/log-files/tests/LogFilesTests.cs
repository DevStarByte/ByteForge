using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.LogFiles;
using ByteForge.Modules.Testing;

namespace ByteForge.LogFiles.Tests;

static class Samples
{
    public const string AspNet = """
        info: Microsoft.Hosting.Lifetime[14]
              Now listening on: http://localhost:5031
        warn: Microsoft.AspNetCore.HttpsPolicy.HttpsRedirectionMiddleware[3]
              Failed to determine the https port for redirect.
        fail: Microsoft.AspNetCore.Diagnostics.ExceptionHandlerMiddleware[1]
              An unhandled exception has occurred while executing the request.
              System.InvalidOperationException: Something broke on purpose.
                 at Program.<>c.<<Main>$>b__0_9() in /src/ModuleTest/Program.cs:line 71
                 at lambda_method12(Closure, Object, HttpContext)
        dbug: Microsoft.AspNetCore.Routing.Matching.DfaMatcher[1001]
              1 candidate(s) found for the request path '/boom'
        info: Microsoft.Hosting.Lifetime[0]
              Application is shutting down...
        """;

    public const string Serilog = """
        [10:00:01 INF] Starting up
        [10:00:02 DBG] Config loaded from appsettings.json
        [10:00:03 WRN] Slow query took 1200 ms
        [10:00:04 ERR] Order 42 failed
        System.Data.SqlClient.SqlException (0x80131904): Timeout expired.
           at System.Data.SqlClient.SqlConnection.OnError(SqlException exception)
        [10:00:05 INF] Retrying
        """;

    public const string Plain = """
        2026-11-20 10:00:00.123 INFO  Server started on 0.0.0.0:8080
        2026-11-20 10:00:01.456 ERROR Request GET /api/orders failed with status 500
        2026-11-20 10:00:02.789 INFO  an error occurred? no: this is just a message about error handling
        2026-11-20 10:00:03.000 FATAL Out of memory
        """;

    /// <summary>The lines of a sample (raw string literals take the source file's line endings).</summary>
    public static string[] Lines(string text) => text.Replace("\r\n", "\n").Split('\n');
}

public sealed class HighlightTests
{
    const string Red = "regex - character class", Yellow = "string - escape character", Blue = "keyword", Dim = "excluded code";

    static readonly Lazy<JsonObject> Manifest = new(() =>
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir != null; dir = dir.Parent)
        {
            var file = Path.Combine(dir.FullName, "module.json");
            if (File.Exists(file))
            {
                return JsonNode.Parse(File.ReadAllText(file), documentOptions: new() { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true })!.AsObject();
            }
        }
        throw new FileNotFoundException("module.json");
    });

    static JsonObject Language => Manifest.Value["languages"]![0]!.AsObject();

    /// <summary>The token the IDE gives <paramref name="text"/> (its nth occurrence) on a line: the earliest rule covering it wins.</summary>
    static string? TokenOf(string line, string text, int nth = 0)
    {
        var start = Regex.Matches(line, Regex.Escape(text))[nth].Index;
        foreach (var rule in Language["rules"]!.AsArray())
        {
            var group = rule!["group"]?.GetValue<int>() ?? 0;
            foreach (Match m in Regex.Matches(line, rule["match"]!.GetValue<string>()))
            {
                var g = m.Groups[group];
                if (g.Success && g.Index <= start && start < g.Index + g.Length)
                {
                    return rule["token"]!.GetValue<string>();
                }
            }
        }
        return null;
    }

    [Fact]
    public void AspNet_levels()
    {
        Assert.Equal(Red, TokenOf("fail: Microsoft.AspNetCore.Diagnostics[1]", "fail"));
        Assert.Equal(Yellow, TokenOf("warn: Microsoft.AspNetCore.HttpsPolicy[3]", "warn"));
        Assert.Equal(Blue, TokenOf("info: Microsoft.Hosting.Lifetime[14]", "info"));
        Assert.Equal(Dim, TokenOf("dbug: Microsoft.AspNetCore.Routing[1001]", "Routing"));
    }

    [Fact]
    public void Serilog_and_plain_levels()
    {
        Assert.Equal(Red, TokenOf("[10:00:04 ERR] Order 42 failed", "ERR"));
        Assert.Equal(Yellow, TokenOf("[10:00:03 WRN] Slow query", "WRN"));
        Assert.Equal(Dim, TokenOf("[10:00:02 DBG] Config loaded", "Config"));
        const string line = "2026-11-20 10:00:03.000 FATAL Out of memory";
        Assert.Equal(Red, TokenOf(line, "FATAL"));
        Assert.Equal("comment", TokenOf(line, "2026-11-20"));
        Assert.Equal(Yellow, TokenOf("2026-11-20 | Warning | Disk almost full", "Warning"));
    }

    [Fact]
    public void Words_in_messages_stay_plain()
    {
        const string line = "2026-11-20 10:00:02 INFO  an error occurred? no: about error handling";
        Assert.Null(TokenOf(line, "error"));
        Assert.Equal(Blue, TokenOf(line, "INFO"));
    }

    [Fact]
    public void Stack_frames_and_exceptions()
    {
        const string frame = "   at Program.Main(String[] args) in /src/App/Program.cs:line 71";
        Assert.Equal("method name", TokenOf(frame, "Program.Main"));
        Assert.Equal("string", TokenOf(frame, "/src/App/Program.cs"));
        Assert.Equal("number", TokenOf(frame, "71"));
        Assert.Equal("class name", TokenOf("System.InvalidOperationException: broke", "System.InvalidOperationException"));
        Assert.Equal("string", TokenOf("  File \"/app/main.py\", line 3, in <module>", "/app/main.py"));
        Assert.Equal(Red, TokenOf("##[error]Process completed with exit code 1.", "Process"));
    }

    [Fact]
    public void Values()
    {
        const string line = "GET /api/orders status=503 from 10.0.0.7:5000 id 3f2b9c1e-1111-2222-3333-444455556666 http://x.io/a";
        Assert.Equal("keyword - control", TokenOf(line, "GET"));
        Assert.Equal(Red, TokenOf(line, "503"));
        Assert.Equal("constant name", TokenOf(line, "10.0.0.7"));
        Assert.Equal("constant name", TokenOf(line, "3f2b9c1e"));
        Assert.Equal("string", TokenOf(line, "http://x.io/a"));
    }

    [Fact]
    public void Rules_have_no_lookaround()
    {
        var patterns = Language["rules"]!.AsArray().Select(r => r!["match"]!.GetValue<string>())
            .Concat(Language["detect"]!.AsArray().Select(d => d!.GetValue<string>()));
        foreach (var pattern in patterns)
        {
            _ = new Regex(pattern);
            Assert.False(Regex.IsMatch(pattern, @"\(\?<?[=!]"), $"Rust's regex crate has no lookaround: {pattern}");
        }
    }

    [Fact]
    public void Manifest_is_a_dotnet_module()
    {
        Assert.Equal("byteforge.log-files", Manifest.Value["id"]!.GetValue<string>());
        Assert.Equal("dotnet", Manifest.Value["runtime"]!.GetValue<string>());
        Assert.Equal("bin/LogFiles.dll", Manifest.Value["assembly"]!.GetValue<string>());
        Assert.Null(Manifest.Value["command"]);
    }
}

public sealed class LevelTests
{
    [Fact]
    public void Level_of()
    {
        Assert.Equal("error", LogParser.LevelOf("fail: Microsoft.AspNetCore[1]"));
        Assert.Equal("warning", LogParser.LevelOf("[10:00:03 WRN] Slow"));
        Assert.Equal("info", LogParser.LevelOf("2026-11-20 10:00:00 INFO  Started"));
        Assert.Equal("error", LogParser.LevelOf("2026-11-20 10:00:00 | Error | x"));
        Assert.Equal("error", LogParser.LevelOf("##[error]exit code 1"));
        Assert.Equal("info", LogParser.LevelOf("##[notice]heads up"));
        Assert.Null(LogParser.LevelOf("      Now listening on: http://localhost:5031"));
        Assert.Null(LogParser.LevelOf(new string('x', 80) + " ERROR past the first 80 characters"));
    }

    [Fact]
    public void Entries_keep_stack_traces_together()
    {
        var blocks = LogParser.Entries(Samples.Lines(Samples.AspNet)).ToList();
        Assert.Equal(["info", "warning", "error", "debug", "info"], blocks.Select(b => b.Level));
        var error = blocks[2];
        Assert.Equal(5, error.Lines.Count);
        Assert.Equal("System.InvalidOperationException", LogParser.ExceptionOf(error.Lines));
        Assert.StartsWith("at Program.", LogParser.FirstFrame(error.Lines));
    }

    [Fact]
    public void Serilog_exception_lines_belong_to_the_error()
    {
        var error = LogParser.Entries(Samples.Lines(Samples.Serilog)).First(b => b.Level == "error");
        Assert.Equal(3, error.Lines.Count);
    }

    [Fact]
    public void List_errors_points_at_the_entry()
    {
        var output = LogParser.ListErrors("/x/app.log", Samples.Lines(Samples.AspNet));
        Assert.Single(output);
        Assert.StartsWith("/x/app.log:5:1: fail: Microsoft.AspNetCore.Diagnostics", output[0]);
        Assert.Contains("System.InvalidOperationException", output[0]);
        Assert.EndsWith("  [System.InvalidOperationException · at Program.<>c.<<Main>$>b__0_9() in /src/ModuleTest/Program.cs:line 71]", output[0]);
        output = LogParser.ListErrors("/x/plain.log", Samples.Lines(Samples.Plain));
        Assert.Equal(["2", "4"], output.Select(o => o.Split(':')[1]));
    }

    [Fact]
    public void Filters()
    {
        var lines = Samples.Lines(Samples.AspNet);
        Assert.Equal("fail: Microsoft.AspNetCore.Diagnostics.ExceptionHandlerMiddleware[1]", LogParser.ApplyFilter(lines, "errors")[0]);
        Assert.Equal(7, LogParser.ApplyFilter(lines, "warnings").Count);
        Assert.DoesNotContain("dbug", string.Join("\n", LogParser.ApplyFilter(lines, "nodebug")));
        Assert.Contains("SqlException", string.Join("\n", LogParser.ApplyFilter(Samples.Lines(Samples.Serilog), "exceptions")));
    }

    [Fact]
    public void Counts()
    {
        Assert.Equal(new Dictionary<string, int> { ["trace"] = 0, ["debug"] = 0, ["info"] = 2, ["warning"] = 0, ["error"] = 1, ["fatal"] = 1 },
            LogParser.Counts(Samples.Lines(Samples.Plain)));
    }

    [Fact]
    public void Is_log()
    {
        foreach (var name in new[] { "app.log", "app.log.1", "app.log.2026-11-20", "build.out", "APP.LOG" })
        {
            Assert.True(LogParser.IsLog("/x/" + name), name);
        }
        Assert.False(LogParser.IsLog("/x/catalog.json"));
        Assert.False(LogParser.IsLog("/x/app.log.old"));
    }

    [Fact]
    public void Path_helpers()
    {
        Assert.Equal("app.log", LogParser.Stem("app.log.1"));
        Assert.Equal("app", LogParser.Stem("app.log"));
        Assert.Equal(".log", LogParser.Stem(".log"));
        Assert.Equal("/var/log", LogParser.Dirname("/var/log/my app.log"));
        Assert.Equal("/", LogParser.Dirname("/app.log"));
        Assert.Equal("'/a/it''s.log'", LogParser.Quote("/a/it's.log", windows: true));
        Assert.Equal("'/a/it'\\''s.log'", LogParser.Quote("/a/it's.log", windows: false));
        Assert.Equal("x", LogParser.Strip("\x1c x \t"));
    }

    [Fact]
    public void Reads_only_the_end_of_a_huge_log_from_a_full_line()
    {
        var file = Path.GetTempFileName();
        try
        {
            File.WriteAllText(file, "first\r\nsecond\r\nthird");
            Assert.Equal(["first", "second", "third"], LogParser.ReadLines(file));
        }
        finally
        {
            File.Delete(file);
        }
    }
}

public sealed class ModuleTests : IDisposable
{
    readonly string config = Directory.CreateTempSubdirectory("bf-log-files-").FullName;

    string ModuleDir => Path.Combine(config, "modules", "log-files");

    public void Dispose()
    {
        try
        {
            Directory.Delete(config, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    ModuleHarness Start() => ModuleHarness.Start(new LogFilesModule(() => new DateTime(2026, 11, 25, 9, 8, 7)), ModuleDir);

    static JsonObject Editor(string? path) => new() { ["type"] = "event", ["name"] = "editorChanged", ["path"] = path };

    static JsonObject Run(string id) => new() { ["type"] = "executeCommand", ["id"] = id };

    static JsonObject Picked(JsonObject pick, string value) => new() { ["type"] = "quickPickResult", ["requestId"] = pick["requestId"]!.DeepClone(), ["value"] = value };

    static string S(JsonNode? node) => node!.GetValue<string>();

    string Write(string name, string text)
    {
        var path = Path.Combine(config, name);
        File.WriteAllText(path, text.Replace("\r\n", "\n"));
        return path;
    }

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = Start();
        foreach (var (id, title) in LogFilesModule.Commands)
        {
            var command = await module.Next("registerCommand", m => S(m["id"]) == id);
            Assert.Equal(title, S(command["title"]));
            Assert.Equal("Log", S(command["category"]));
        }
    }

    [Fact]
    public async Task Status_and_filter_file()
    {
        var path = Write("app.log.1", Samples.AspNet);
        await using var module = Start();
        module.Send(Editor(path));
        var status = await module.Next("status");
        Assert.Equal("✗ 1  ⚠ 1", S(status["text"]));
        Assert.Equal("log.listErrors", S(status["command"]));
        Assert.Equal("app.log.1: 0 fatal, 1 errors, 1 warnings, 2 info. Click: list the errors.", S(status["tooltip"]));

        module.Send(Run("log.filter"));
        var pick = await module.Next("quickPick");
        Assert.Equal("Show only…", S(pick["placeholder"]));
        Assert.Equal(["errors", "warnings", "nodebug", "exceptions"], pick["items"]!.AsArray().Select(i => S(i!["value"])));
        module.Send(Picked(pick, "errors"));
        var opened = S((await module.Next("openFile"))["path"]);
        Assert.Equal(Path.Combine(ModuleDir, "filtered", "app.log.errors-090807.log"), opened);
        var text = File.ReadAllText(opened);
        Assert.StartsWith("fail:", text);
        Assert.EndsWith("at lambda_method12(Closure, Object, HttpContext)" + Environment.NewLine, text);
    }

    [Fact]
    public async Task A_filter_with_nothing_to_show_says_so()
    {
        var path = Write("quiet.log", "2026-11-20 10:00:00 INFO  all good\n");
        await using var module = Start();
        module.Send(Editor(path));
        await module.Next("status");
        module.Send(Run("log.filter"));
        module.Send(Picked(await module.Next("quickPick"), "exceptions"));
        await module.Next("log", m => S(m["text"]) == "log: nothing in quiet.log for \"Exceptions\".");
        await module.Next("showView", m => S(m["id"]) == "output");
    }

    [Fact]
    public async Task List_errors_goes_to_the_output()
    {
        var path = Write("plain.log", Samples.Plain);
        await using var module = Start();
        module.Send(Editor(path));
        await module.Next("status");
        module.Send(Run("log.listErrors"));
        await module.Next("log", m => S(m["text"]) == "log: plain.log: 1 fatal, 1 errors, 0 warnings");
        Assert.Equal($"{path}:2:1: 2026-11-20 10:00:01.456 ERROR Request GET /api/orders failed with status 500", S((await module.Next("log"))["text"]));
        Assert.Equal($"{path}:4:1: 2026-11-20 10:00:03.000 FATAL Out of memory", S((await module.Next("log"))["text"]));
        await module.Next("showView", m => S(m["id"]) == "output");
    }

    [Fact]
    public async Task Lists_only_the_newest_errors_of_many()
    {
        var path = Write("many.log", string.Concat(Enumerable.Range(1, 503).Select(i => $"ERROR number {i}\n")));
        await using var module = Start();
        module.Send(Editor(path));
        await module.Next("status");
        module.Send(Run("log.listErrors"));
        await module.Next("log", m => S(m["text"]) == "log: many.log: 0 fatal, 503 errors, 0 warnings");
        Assert.Equal($"{path}:4:1: ERROR number 4", S((await module.Next("log"))["text"]));
        await module.Next("log", m => S(m["text"]) == "log: … and 3 older errors (\"Log: Filter…\" shows them all)");
    }

    [Fact]
    public async Task Without_a_log_open_the_commands_say_so_and_the_status_is_hidden()
    {
        await using var module = Start();
        module.Send(Editor(Path.Combine(config, "Program.cs")));
        Assert.Equal("", S((await module.Next("status"))["text"]));
        module.Send(Run("log.follow"));
        await module.Next("log", m => S(m["text"]) == "log: open a .log file first (it's read as saved).");
        await module.Next("showView", m => S(m["id"]) == "output");
    }

    [Fact]
    public async Task Follow_command()
    {
        await using var module = Start();
        module.Send(Editor("/var/log/my app.log")); // doesn't exist: no status, but it's the active log
        module.Send(Run("log.follow"));
        var terminal = await module.Next("runInTerminal");
        Assert.Equal(OperatingSystem.IsWindows() ? "Get-Content -Wait -Tail 200 -Path '/var/log/my app.log'" : "tail -n 200 -F '/var/log/my app.log'", S(terminal["command"]));
        Assert.Equal("follow my app.log", S(terminal["title"]));
        Assert.Equal("/var/log", S(terminal["cwd"]));
    }

    [Fact]
    public async Task Saving_the_open_log_updates_the_status()
    {
        var path = Write("app.log", "ERROR one\n");
        await using var module = Start();
        module.Send(Editor(path));
        Assert.Equal("✗ 1  ⚠ 0", S((await module.Next("status"))["text"]));
        File.WriteAllText(path, "ERROR one\nWARN two\nFATAL three\n");
        module.Send(new() { ["type"] = "event", ["name"] = "fileSaved", ["path"] = path });
        Assert.Equal("✗ 2  ⚠ 1", S((await module.Next("status"))["text"]));
    }
}
