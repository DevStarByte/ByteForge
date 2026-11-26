using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.Sql;

/// <summary>
/// What runs in the IDE's terminal: `dotnet bin/Sql.dll launch &lt;spec&gt;` runs a query, a file or a shell for the
/// connection in the spec file, which it deletes first (it may hold a password). The password goes to the
/// database client through its environment, never a command line.
/// </summary>
public static class Launcher
{
    public static int Main(string[] args)
    {
        if (args.Length >= 2 && args[0] == "launch")
        {
            try
            {
                Console.OutputEncoding = new UTF8Encoding(false);
                Console.InputEncoding = new UTF8Encoding(false);
            }
            catch (Exception e) when (e is IOException or PlatformNotSupportedException or System.Security.SecurityException)
            {
            }
            return Launch(args[1], Console.Out, Console.In);
        }
        Console.Error.WriteLine("The SQL module for ByteForge: ByteForge loads it. (Sql.dll launch <spec> runs a query in a terminal.)");
        return 2;
    }

    /// <summary>A private temporary file with what the launcher needs (it deletes the file when it reads it).</summary>
    public static string WriteSpec(JsonObject spec)
    {
        var bytes = Encoding.UTF8.GetBytes(spec.ToJsonString());
        while (true)
        {
            var path = Path.Combine(Path.GetTempPath(), "byteforge-sql-" + Convert.ToHexStringLower(RandomNumberGenerator.GetBytes(5)) + ".json");
            var options = new FileStreamOptions { Mode = FileMode.CreateNew, Access = FileAccess.Write, Share = FileShare.None };
            if (!OperatingSystem.IsWindows())
            {
                options.UnixCreateMode = UnixFileMode.UserRead | UnixFileMode.UserWrite;
            }
            try
            {
                using var file = new FileStream(path, options);
                file.Write(bytes);
                return path;
            }
            catch (IOException) when (File.Exists(path))
            {
                // Taken: another name.
            }
        }
    }

    static void Print(TextWriter output, string text)
    {
        output.Write(text + "\n");
        output.Flush();
    }

    public static int Launch(string specPath, TextWriter output, TextReader input)
    {
        JsonObject spec;
        try
        {
            spec = JsonNode.Parse(File.ReadAllText(specPath, Encoding.UTF8)) as JsonObject ?? throw new InvalidDataException("the spec isn't an object");
        }
        finally
        {
            try
            {
                File.Delete(specPath);
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
            }
        }
        var conn = Connection.FromJson(spec["connection"] as JsonObject ?? []);
        var mode = spec["mode"]?.GetValue<string>() ?? "run";
        string? Text(string key) => spec[key] is JsonValue value && value.TryGetValue<string>(out var s) ? s : null;
        Print(output, $"── {conn.Name ?? "None"} · {Connections.Drivers[conn.Driver]} · {Connections.Target(conn)}\n");
        if (conn.Driver == "sqlite")
        {
            if (mode == "shell")
            {
                if (Programs.Which("sqlite3") is { } program)
                {
                    return Call(program, ["-header", "-column", conn.File ?? ""], new Dictionary<string, string>());
                }
                return Sqlite.Shell(conn.File ?? "", input, output);
            }
            var text = Text("query");
            if (Text("file") is { } file)
            {
                // As Python read it: without a BOM, bad bytes replaced, line ends as \n.
                text = File.ReadAllText(file, Encoding.UTF8).Replace("\r\n", "\n").Replace('\r', '\n');
            }
            return Sqlite.RunScript(conn.File, text ?? "", output);
        }
        var client = Clients.FindClient(conn.Driver, spec["settings"] as JsonObject);
        if (client == null)
        {
            Print(output, $"{Clients.ClientNames[conn.Driver]} isn't installed (or set \"sql.{conn.Driver}Path\").");
            return 127;
        }
        var (args, extra) = Clients.Command(conn, client, mode, query: Text("query"), file: Text("file"));
        return Call(args[0], args.Skip(1), extra);
    }

    /// <summary>Runs a client in this terminal and waits for it; Ctrl+C goes to the client, not to the launcher.</summary>
    static int Call(string program, IEnumerable<string> arguments, Dictionary<string, string> environment)
    {
        var start = new ProcessStartInfo(program) { UseShellExecute = false };
        foreach (var argument in arguments)
        {
            start.ArgumentList.Add(argument);
        }
        foreach (var (key, value) in environment)
        {
            start.Environment[key] = value;
        }
        static void Ignore(object? sender, ConsoleCancelEventArgs e) => e.Cancel = true;
        Console.CancelKeyPress += Ignore;
        try
        {
            using var process = Process.Start(start);
            if (process == null)
            {
                return 127;
            }
            process.WaitForExit();
            return process.ExitCode;
        }
        catch (System.ComponentModel.Win32Exception error)
        {
            Console.Error.WriteLine($"couldn't run {Path.GetFileName(program)}: {error.Message}");
            return 127;
        }
        finally
        {
            Console.CancelKeyPress -= Ignore;
        }
    }
}
