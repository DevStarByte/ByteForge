using System.Text.Json.Nodes;
using ByteForge.Modules;
using ByteForge.Modules.Testing;

namespace ByteForge.Lua.Tests;

/// <summary>PATH is process-wide: these tests take turns.</summary>
[CollectionDefinition("process environment", DisableParallelization = true)]
public sealed class ProcessEnvironment;

/// <summary>The module with lua / luac / stylua replaced: records each run and answers with <paramref name="run"/>.</summary>
sealed class FakeLua(Func<string, IReadOnlyList<string>, ProcessResult> run) : LuaModule
{
    public List<(string Program, string[] Arguments, string? Input, string? WorkingDirectory)> Runs { get; } = [];

    protected override Task<ProcessResult> RunAsync(string program, IReadOnlyList<string> arguments, string? input, string? workingDirectory)
    {
        lock (Runs)
        {
            Runs.Add((program, [.. arguments], input, workingDirectory));
        }
        return Task.FromResult(run(program, arguments));
    }
}

[Collection("process environment")]
public sealed class LuaTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-lua-").FullName;
    readonly string? path = Environment.GetEnvironmentVariable("PATH");

    public LuaTests()
    {
        // No real lua, luac or stylua from this machine.
        Environment.SetEnvironmentVariable("PATH", Directory.CreateDirectory(Path.Combine(dir, "empty")).FullName);
    }

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("PATH", path);
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    string ModuleDir => Path.Combine(dir, "modules", "lua");

    string LuaFile(string name = "main.lua")
    {
        var src = Directory.CreateDirectory(Path.Combine(dir, "project", "src")).FullName;
        var file = Path.Combine(src, name);
        File.WriteAllText(file, "print('hi')\n");
        return file;
    }

    static JsonObject Command(string id) => new() { ["type"] = "executeCommand", ["id"] = id };

    static JsonObject Editor(string? file) => new() { ["type"] = "event", ["name"] = "editorChanged", ["path"] = file };

    [Fact]
    public async Task Registers_its_commands_and_the_StyLua_formatter()
    {
        await using var module = ModuleHarness.Start(new LuaModule(), ModuleDir);
        var run = await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "lua.runFile");
        Assert.Equal("Run File", run["title"]!.GetValue<string>());
        Assert.Equal("Lua", run["category"]!.GetValue<string>());
        var check = await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "lua.checkSyntax");
        Assert.Equal("Check Syntax", check["title"]!.GetValue<string>());
        var formatter = await module.Next("registerFormatter");
        Assert.Equal("lua.stylua", formatter["id"]!.GetValue<string>());
        Assert.Equal("StyLua", formatter["name"]!.GetValue<string>());
        Assert.Equal(["lua"], formatter["extensions"]!.AsArray().Select(e => e!.GetValue<string>()));
    }

    [Fact]
    public async Task Run_File_runs_the_interpreter_in_a_terminal()
    {
        var file = LuaFile("my main.lua");
        await using var module = ModuleHarness.Start(new LuaModule(), ModuleDir);
        module.Activate(dir, new() { ["lua.interpreter"] = " luajit " });
        module.Send(Editor(file));
        module.Send(Command("lua.runFile"));
        var terminal = await module.Next("runInTerminal");
        Assert.Equal("Lua: my main.lua", terminal["title"]!.GetValue<string>());
        Assert.Equal($"luajit {LuaModule.Quote(file)}", terminal["command"]!.GetValue<string>());
        Assert.Equal(Path.GetDirectoryName(file), terminal["cwd"]!.GetValue<string>());
        Assert.True(terminal["saveAll"]!.GetValue<bool>());
    }

    [Fact]
    public async Task Commands_need_a_Lua_file_and_an_interpreter()
    {
        await using var module = ModuleHarness.Start(new LuaModule(), ModuleDir);
        module.Activate(dir);
        module.Send(Editor(Path.Combine(dir, "notes.txt")));
        module.Send(Command("lua.runFile"));
        await module.Next("log", m => m["text"]!.GetValue<string>() == "lua: open a .lua file first.");
        await module.Next("showView", m => m["id"]!.GetValue<string>() == "output");

        module.Send(Editor(LuaFile("a.ROCKSPEC")));
        module.Send(Command("lua.runFile"));
        await module.Next("log", m => m["text"]!.GetValue<string>() == "lua: no Lua interpreter found (install lua or luajit, or set \"lua.interpreter\").");

        module.Send(Command("lua.checkSyntax"));
        await module.Next("log", m => m["text"]!.GetValue<string>() == "lua: no luac or Lua interpreter found (install lua, or set \"lua.compiler\" / \"lua.interpreter\")");
    }

    [Fact]
    public async Task Check_Syntax_uses_luac_and_goes_to_the_error()
    {
        var file = LuaFile();
        var fake = new FakeLua((program, _) => new(1, "", $"{program}: {file}:3: 'end' expected (to close 'function' at line 1) near <eof>\n"));
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Activate(dir, new() { ["lua.compiler"] = "luac5.4", ["lua.interpreter"] = "lua5.4" });
        module.Send(Editor(file));
        module.Send(Command("lua.checkSyntax"));
        await module.Next("log", m => m["text"]!.GetValue<string>() == "lua: main.lua:3: 'end' expected (to close 'function' at line 1) near <eof>");
        await module.Next("showView", m => m["id"]!.GetValue<string>() == "output");
        var open = await module.Next("openFile");
        Assert.Equal(file, open["path"]!.GetValue<string>());
        Assert.Equal(2, open["line"]!.GetValue<int>());
        Assert.Equal(0, open["column"]!.GetValue<int>());
        var run = Assert.Single(fake.Runs);
        Assert.Equal("luac5.4", run.Program);
        Assert.Equal(["-p", file], run.Arguments);
        Assert.Equal("", run.Input);
    }

    [Fact]
    public async Task Without_luac_the_interpreter_compiles_the_file_without_running_it()
    {
        var file = LuaFile();
        var fake = new FakeLua((_, _) => new(0, "", ""));
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Activate(dir, new() { ["lua.interpreter"] = "lua5.1" });
        module.Send(Editor(file));
        module.Send(Command("lua.checkSyntax"));
        await module.Next("log", m => m["text"]!.GetValue<string>() == "lua: main.lua: no syntax errors.");
        var run = Assert.Single(fake.Runs);
        Assert.Equal("lua5.1", run.Program);
        Assert.Equal(["-", file], run.Arguments);
        Assert.Equal(LuaModule.CheckScript, run.Input);
    }

    [Theory]
    [InlineData("luac: a.lua:3: 'end' expected near <eof>", "a.lua:3: 'end' expected near <eof>", 3)]
    [InlineData("lua5.4: /home/me/x/a.lua:12: unexpected symbol near '='\nstack traceback:\n\t[C]: in ?", "a.lua:12: unexpected symbol near '='", 12)]
    [InlineData("/usr/bin/luac5.1: b.lua:1:   '=' expected near 'x'", "b.lua:1: '=' expected near 'x'", 1)]
    [InlineData("\n\nstack traceback:\n  something weird\n", "something weird", null)]
    [InlineData("cannot open nothere.lua", "cannot open nothere.lua", null)]
    [InlineData("luac: :7: oops", "main.lua:7: oops", 7)]
    [InlineData("", "syntax check failed", null)]
    [InlineData("luajit: [string \"x\"]:2: '<eof>' expected", "[string \"x\"]:2: '<eof>' expected", 2)]
    public void Parses_error_output(string output, string message, int? line)
    {
        var error = LuaModule.ParseError(output, Path.Combine("src", "main.lua"));
        Assert.Equal(message, error.Message);
        Assert.Equal(line, error.Line);
    }

    [Fact]
    public void Parses_Windows_paths_on_Windows()
    {
        if (!OperatingSystem.IsWindows())
        {
            return;
        }
        var error = LuaModule.ParseError(@"lua: C:\work\game\init.lua:5: syntax error near 'then'", @"C:\work\game\init.lua");
        Assert.Equal("init.lua:5: syntax error near 'then'", error.Message);
        Assert.Equal(5, error.Line);
    }

    [Theory]
    [InlineData("", "\"\"")]
    [InlineData("lua", "lua")]
    [InlineData(@"C:\Program Files\Lua\lua.exe", "\"C:\\Program Files\\Lua\\lua.exe\"")]
    [InlineData(@"C:\dir with space\", "\"C:\\dir with space\\\\\"")]
    [InlineData("a\"b", "a\\\"b")]
    [InlineData(@"a\\""b c", "\"a\\\\\\\\\\\"b c\"")]
    public void Quotes_like_list2cmdline(string argument, string quoted) => Assert.Equal(quoted, LuaModule.WindowsQuote(argument));

    [Theory]
    [InlineData("", "''")]
    [InlineData("/usr/bin/lua5.4", "/usr/bin/lua5.4")]
    [InlineData("/home/me/my game/main.lua", "'/home/me/my game/main.lua'")]
    [InlineData("it's.lua", "'it'\"'\"'s.lua'")]
    [InlineData("ä.lua", "'ä.lua'")]
    public void Quotes_like_shlex(string argument, string quoted) => Assert.Equal(quoted, LuaModule.PosixQuote(argument));

    [Fact]
    public async Task Formats_through_StyLua_from_the_files_folder()
    {
        var file = LuaFile();
        var stylua = Path.Combine(dir, OperatingSystem.IsWindows() ? "stylua.exe" : "stylua");
        File.WriteAllText(stylua, "");
        if (!OperatingSystem.IsWindows())
        {
            File.SetUnixFileMode(stylua, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }
        var fake = new FakeLua((_, _) => new(0, "local x = 1\n", ""));
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Activate(dir, new() { ["lua.styluaPath"] = stylua });
        module.Send(new() { ["type"] = "format", ["requestId"] = 3, ["path"] = file, ["text"] = "local   x=1" });
        var result = await module.Next("formatResult");
        Assert.Equal("local x = 1\n", result["text"]!.GetValue<string>());
        var run = Assert.Single(fake.Runs);
        Assert.Equal(stylua, run.Program);
        Assert.Equal(["--search-parent-directories", "--stdin-filepath", file, "-"], run.Arguments);
        Assert.Equal("local   x=1", run.Input);
        Assert.Equal(Path.GetDirectoryName(file), run.WorkingDirectory);

        // Untitled: no file path, no folder.
        module.Send(new() { ["type"] = "format", ["requestId"] = 4, ["text"] = "x=1" });
        await module.Next("formatResult", m => m["requestId"]!.GetValue<long>() == 4);
        Assert.Equal(["--search-parent-directories", "-"], fake.Runs[1].Arguments);
        Assert.Null(fake.Runs[1].WorkingDirectory);
    }

    [Fact]
    public async Task Formatting_handles_line_endings_like_pythons_text_mode()
    {
        var file = LuaFile();
        var stylua = Path.Combine(dir, "stylua-bin");
        File.WriteAllText(stylua, "");
        var fake = new FakeLua((_, _) => new(0, "local x = 1\r\nprint(x)\r", ""));
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Activate(dir, new() { ["lua.styluaPath"] = stylua });
        module.Send(new() { ["type"] = "format", ["requestId"] = 1, ["path"] = file, ["text"] = "local x=1\nprint(x)\n" });
        // The output comes back with "\n" only; the input goes out with the system's line ending.
        Assert.Equal("local x = 1\nprint(x)\n", (await module.Next("formatResult"))["text"]!.GetValue<string>());
        Assert.Equal(OperatingSystem.IsWindows() ? "local x=1\r\nprint(x)\r\n" : "local x=1\nprint(x)\n", Assert.Single(fake.Runs).Input);
    }

    [Fact]
    public async Task Reports_StyLua_errors_and_a_missing_StyLua()
    {
        var file = LuaFile();
        await using (var module = ModuleHarness.Start(new FakeLua((_, _) => new(1, "", "")), ModuleDir))
        {
            module.Activate(dir);
            module.Send(new() { ["type"] = "format", ["requestId"] = 1, ["path"] = file, ["text"] = "x" });
            Assert.Equal(LuaModule.StyluaMissing, (await module.Next("formatResult"))["error"]!.GetValue<string>());
        }
        var stylua = Path.Combine(dir, "stylua-bin");
        File.WriteAllText(stylua, "");
        await using (var module = ModuleHarness.Start(new FakeLua((_, _) => new(2, "out", "\nerror: failed to format: unexpected token\nmore\n")), ModuleDir))
        {
            module.Activate(dir, new() { ["lua.styluaPath"] = stylua });
            module.Send(new() { ["type"] = "format", ["requestId"] = 1, ["path"] = file, ["text"] = "x" });
            Assert.Equal("StyLua: error: failed to format: unexpected token", (await module.Next("formatResult"))["error"]!.GetValue<string>());
        }
        await using (var module = ModuleHarness.Start(new FakeLua((_, _) => new(2, "", "")), ModuleDir))
        {
            module.Activate(dir, new() { ["lua.styluaPath"] = stylua });
            module.Send(new() { ["type"] = "format", ["requestId"] = 1, ["path"] = file, ["text"] = "x" });
            Assert.Equal("StyLua: failed", (await module.Next("formatResult"))["error"]!.GetValue<string>());
        }
    }

    [Fact]
    public void A_configured_program_off_PATH_is_used_as_given()
    {
        Assert.Equal("my-lua", LuaModule.FindTool("  my-lua ", LuaModule.Interpreters));
        Assert.Null(LuaModule.FindTool("  ", ["definitely-not-a-program-xyz"]));
    }
}
