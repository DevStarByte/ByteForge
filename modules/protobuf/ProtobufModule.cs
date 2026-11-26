using ByteForge.Modules;

namespace ByteForge.Protobuf;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<ProtobufModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// Format Document for .proto files with buf or clang-format: "protobuf.formatter" picks one ("auto": buf if it's
/// installed, else clang-format). Highlighting and the file icon are declared in module.json.
/// </summary>
public sealed class ProtobufModule : Module
{
    public const string InstallHint = "install buf (buf.build) or clang-format to format .proto files";
    static readonly TimeSpan Timeout = TimeSpan.FromSeconds(30);

    protected override Task OnStartAsync()
    {
        RegisterFormatter("protobuf", "Protocol Buffers (buf / clang-format)", "proto");
        return Task.CompletedTask;
    }

    /// <summary>The formatter to use: its name and where it is, or null when none is installed.</summary>
    public (string Name, string Program)? Formatter()
    {
        var choice = Setting<string?>("protobuf.formatter", null) is { Length: > 0 } c ? c : "auto";
        string[] candidates = choice == "auto" ? ["buf", "clang-format"] : [choice];
        foreach (var name in candidates)
        {
            if (Programs.Which(name) is { } program)
            {
                return (name, program);
            }
        }
        return null;
    }

    protected override async Task<string> FormatAsync(string? path, string text)
    {
        try
        {
            return await FormatProto(path, text);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or TimeoutException)
        {
            throw new ModuleException(e.Message);
        }
    }

    async Task<string> FormatProto(string? path, string text)
    {
        var (tool, program) = Formatter() ?? throw new ModuleException(InstallHint);
        var folder = !string.IsNullOrEmpty(path) ? Path.GetDirectoryName(path) : null;
        folder = string.IsNullOrEmpty(folder) ? null : folder;
        // What Python's text mode did: "\n" written as the system's line ending, the output read with universal newlines.
        var input = OperatingSystem.IsWindows() ? text.Replace("\n", "\r\n") : text;
        ProcessResult result;
        if (tool == "buf")
        {
            // buf format reads a file or directory, not stdin: give it a temporary file next to the original, so
            // buf.yaml settings around it apply.
            var temp = Path.Combine(folder != null && Directory.Exists(folder) ? folder : Path.GetTempPath(), $"tmp{Guid.NewGuid().ToString("N")[..8]}.proto");
            await File.WriteAllTextAsync(temp, input, Stopping);
            try
            {
                result = await Programs.RunAsync(program, ["format", temp], timeout: Timeout, cancel: Stopping);
            }
            finally
            {
                File.Delete(temp);
            }
        }
        else
        {
            result = await Programs.RunAsync(program, ["--assume-filename=" + (string.IsNullOrEmpty(path) ? "file.proto" : path)], input: input,
                workingDirectory: folder, timeout: Timeout, cancel: Stopping);
        }
        if (!result.Success)
        {
            throw new ModuleException($"{tool}: {Newlines(result.Error.Length > 0 ? result.Error : result.Output).Trim()}");
        }
        return Newlines(result.Output);
    }

    /// <summary>Universal newlines, as Python's text mode read the program's output.</summary>
    static string Newlines(string text) => text.Replace("\r\n", "\n").Replace('\r', '\n');
}
