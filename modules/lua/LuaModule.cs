using System.Text;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;

namespace ByteForge.Lua;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<LuaModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// Lua: run the current file in a terminal, check its syntax, and format it with StyLua. Highlighting and the
/// file icon are declared in module.json. The interpreter is "lua.interpreter" from the settings, else the first
/// of lua, luajit, lua5.4, lua5.3, lua5.2, lua5.1 on PATH. Syntax checks use luac -p when it is there, else the
/// interpreter compiling the file without running it.
/// </summary>
public partial class LuaModule : Module
{
    public static readonly string[] Interpreters = ["lua", "luajit", "lua5.4", "lua5.3", "lua5.2", "lua5.1"];
    public static readonly string[] Compilers = ["luac", "luac5.4", "luac5.3", "luac5.2", "luac5.1"];

    /// <summary>Compiles the file named by its argument without running it (the interpreter reads this from stdin).</summary>
    public const string CheckScript = "local f, e = loadfile(...) if not f then io.stderr:write(e, '\\n') os.exit(1) end";

    public const string StyluaMissing = "StyLua isn't installed (cargo install stylua, or your package manager), or set \"lua.styluaPath\".";

    /// <summary>The file in the active editor (editorChanged).</summary>
    public string? EditorPath { get; private set; }

    protected override Task OnStartAsync()
    {
        RegisterCommand("lua.runFile", "Run File", "Lua");
        RegisterCommand("lua.checkSyntax", "Check Syntax", "Lua");
        RegisterFormatter("lua.stylua", "StyLua", "lua");
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        if (name == "editorChanged")
        {
            EditorPath = message["path"]?.GetValue<string>();
        }
        return Task.CompletedTask;
    }

    /// <summary>Runs a program to its end; tests override it.</summary>
    protected virtual Task<ProcessResult> RunAsync(string program, IReadOnlyList<string> arguments, string? input, string? workingDirectory) =>
        Programs.RunAsync(program, arguments, input: input, workingDirectory: workingDirectory, timeout: TimeSpan.FromSeconds(30), cancel: Stopping);

    /// <summary>The configured program (a name on PATH or a path), else the first of <paramref name="names"/> on PATH.</summary>
    public static string? FindTool(string? configured, IEnumerable<string> names)
    {
        if (!string.IsNullOrWhiteSpace(configured))
        {
            return Programs.Which(configured.Trim()) ?? configured.Trim();
        }
        foreach (var name in names)
        {
            if (Programs.Which(name) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    public string? Interpreter() => FindTool(SettingString("lua.interpreter"), Interpreters);

    public string? Compiler() => FindTool(SettingString("lua.compiler"), Compilers);

    string? CurrentLuaFile()
    {
        var path = EditorPath;
        if (string.IsNullOrEmpty(path) || !(path.EndsWith(".lua", StringComparison.OrdinalIgnoreCase) || path.EndsWith(".rockspec", StringComparison.OrdinalIgnoreCase)))
        {
            Log("lua: open a .lua file first.");
            ShowView("output");
            return null;
        }
        return path;
    }

    // ── commands ──

    protected override async Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "lua.runFile":
                RunFile();
                break;
            case "lua.checkSyntax":
                await CheckSyntax();
                break;
        }
    }

    void RunFile()
    {
        if (CurrentLuaFile() is not { } path)
        {
            return;
        }
        if (Interpreter() is not { } lua)
        {
            Log("lua: no Lua interpreter found (install lua or luajit, or set \"lua.interpreter\").");
            ShowView("output");
            return;
        }
        RunInTerminal($"Lua: {Path.GetFileName(path)}", $"{Quote(lua)} {Quote(path)}", Path.GetDirectoryName(path), saveAll: true);
    }

    async Task CheckSyntax()
    {
        if (CurrentLuaFile() is not { } path)
        {
            return;
        }
        if (await Check(path, Compiler(), Interpreter()) is not { } error)
        {
            Log($"lua: {Path.GetFileName(path)}: no syntax errors.");
            return;
        }
        Log($"lua: {error.Message}");
        ShowView("output");
        if (error.Line is { } line and not 0)
        {
            // Lua reports 1-based lines; openFile takes 0-based ones.
            OpenFile(path, line - 1, 0);
        }
    }

    /// <summary>Null when the file compiles, else the error. Uses luac -p, else the interpreter's loadfile.</summary>
    public async Task<SyntaxError?> Check(string path, string? luac, string? lua)
    {
        string program;
        string[] arguments;
        string input = "";
        if (luac != null)
        {
            (program, arguments) = (luac, ["-p", path]);
        }
        else if (lua != null)
        {
            // Compile without running: the checking script comes on stdin ("-"), the file is its argument.
            (program, arguments, input) = (lua, ["-", path], CheckScript);
        }
        else
        {
            return new("no luac or Lua interpreter found (install lua, or set \"lua.compiler\" / \"lua.interpreter\")", null);
        }
        ProcessResult result;
        try
        {
            result = await RunAsync(program, arguments, input, null);
        }
        catch (Exception e) when (e is ModuleException or TimeoutException)
        {
            return new(e.Message, null);
        }
        return result.Success ? null : ParseError(result.Error.Length > 0 ? result.Error : result.Output, path);
    }

    /// <summary>The message and line from luac's or lua's error output, e.g. "luac: a.lua:3: 'end' expected near &lt;eof&gt;".</summary>
    public static SyntaxError ParseError(string output, string path)
    {
        foreach (var raw in output.Trim().Split('\n'))
        {
            var line = raw.Trim();
            if (line.Length == 0 || line.StartsWith("stack traceback", StringComparison.Ordinal))
            {
                continue;
            }
            // Drop the program name in front ("luac: ", "lua: ").
            line = ProgramName().Replace(line, "", 1);
            var match = Location().Match(line);
            if (match.Success)
            {
                var name = Path.GetFileName(match.Groups[1].Value);
                if (name.Length == 0)
                {
                    name = Path.GetFileName(path);
                }
                // Python's int() of the line number: digits beyond what fits never occur in practice.
                var number = int.TryParse(match.Groups[2].Value, out var n) ? n : 0;
                return new($"{name}:{match.Groups[2].Value}: {match.Groups[3].Value}", number);
            }
            return new(line, null);
        }
        return new("syntax check failed", null);
    }

    [GeneratedRegex(@"^[\w.\-/\\]*luac?[\w.\-]*:\s+")]
    private static partial Regex ProgramName();

    [GeneratedRegex(@"^(.*?):(\d+):\s*(.*)$")]
    private static partial Regex Location();

    // ── formatting ──

    protected override async Task<string> FormatAsync(string? path, string text)
    {
        var stylua = FindTool(SettingString("lua.styluaPath"), ["stylua"]);
        if (stylua == null || Programs.Which(stylua) == null)
        {
            throw new ModuleException(StyluaMissing);
        }
        // From the file's folder, so its stylua.toml / .stylua.toml applies.
        List<string> arguments = ["--search-parent-directories"];
        if (!string.IsNullOrEmpty(path))
        {
            arguments.AddRange(["--stdin-filepath", path]);
        }
        arguments.Add("-");
        // What Python's text mode did: "\n" written as the system's line ending, the output read with universal newlines.
        var input = OperatingSystem.IsWindows() ? text.Replace("\n", "\r\n") : text;
        ProcessResult result;
        try
        {
            result = await RunAsync(stylua, arguments, input, !string.IsNullOrEmpty(path) ? NullIfEmpty(Path.GetDirectoryName(path)) : null);
        }
        catch (Exception e) when (e is ModuleException or TimeoutException)
        {
            throw new ModuleException($"StyLua: {e.Message}");
        }
        if (!result.Success)
        {
            var first = (result.Error.Length > 0 ? result.Error : result.Output).Trim().Split('\n')[0].TrimEnd('\r');
            throw new ModuleException($"StyLua: {(first.Length > 0 ? first : "failed")}");
        }
        return result.Output.Replace("\r\n", "\n").Replace('\r', '\n');
    }

    static string? NullIfEmpty(string? s) => string.IsNullOrEmpty(s) ? null : s;

    // ── quoting for the terminal ──

    /// <summary>An argument for a command line: Windows' rules on Windows, a POSIX shell's elsewhere.</summary>
    public static string Quote(string argument) => OperatingSystem.IsWindows() ? WindowsQuote(argument) : PosixQuote(argument);

    /// <summary>As Python's subprocess.list2cmdline for one argument (the MS C runtime's rules).</summary>
    public static string WindowsQuote(string argument)
    {
        var result = new StringBuilder();
        var backslashes = 0;
        var needQuote = argument.Length == 0 || argument.Contains(' ') || argument.Contains('\t');
        if (needQuote)
        {
            result.Append('"');
        }
        foreach (var c in argument)
        {
            if (c == '\\')
            {
                backslashes++;
            }
            else if (c == '"')
            {
                result.Append('\\', backslashes * 2).Append("\\\"");
                backslashes = 0;
            }
            else
            {
                result.Append('\\', backslashes).Append(c);
                backslashes = 0;
            }
        }
        result.Append('\\', backslashes);
        if (needQuote)
        {
            result.Append('\\', backslashes).Append('"');
        }
        return result.ToString();
    }

    /// <summary>As Python's shlex.quote.</summary>
    public static string PosixQuote(string argument)
    {
        if (argument.Length == 0)
        {
            return "''";
        }
        if (Safe().IsMatch(argument))
        {
            return argument;
        }
        return "'" + argument.Replace("'", "'\"'\"'") + "'";
    }

    [GeneratedRegex(@"^[A-Za-z0-9_@%+=:,./-]+$")]
    private static partial Regex Safe();
}

/// <summary>A syntax error: the message to log and its line (1-based), when known.</summary>
public sealed record SyntaxError(string Message, int? Line);
