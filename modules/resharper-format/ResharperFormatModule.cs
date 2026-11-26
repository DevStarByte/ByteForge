using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.ResharperFormat;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<ResharperFormatModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// JetBrains' ReSharper command line tools (<c>jb cleanupcode</c>, the formatter of Rider and ReSharper) as a
/// formatter for C#. cleanupcode only works on files, so the text the IDE sends is written to a hidden file next
/// to the original (so the folder's .editorconfig applies), cleaned up there and read back. The solution's
/// &lt;name&gt;.DotSettings is passed along when there is one. A version pinned in the workspace's dotnet tool
/// manifest (.config/dotnet-tools.json) wins over a global install.
/// </summary>
public class ResharperFormatModule : Module
{
    public const string Package = "jetbrains.resharper.globaltools";
    public static readonly string[] Extensions = ["cs"];
    public const string DefaultProfile = "Built-in: Reformat Code";
    public const string InstallHint = "The ReSharper command line tools aren't installed. Run \"ReSharper: Install\" from the command palette " +
        "(dotnet tool install -g JetBrains.ReSharper.GlobalTools).";

    static readonly UTF8Encoding Utf8 = new(false);
    static int counter = -1;

    protected override Task OnStartAsync()
    {
        RegisterFormatter("resharper", "ReSharper", Extensions);
        RegisterCommand("resharper.install", "Install (global dotnet tool)", "ReSharper");
        RegisterCommand("resharper.update", "Update", "ReSharper");
        RegisterCommand("resharper.uninstall", "Uninstall the dotnet tool", "ReSharper");
        UpdateStatus();
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync()
    {
        UpdateStatus();
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        if (name == "folderOpened")
        {
            UpdateStatus();
        }
        return Task.CompletedTask;
    }

    /// <summary>Runs a program (jb or dotnet); tests override it.</summary>
    protected virtual Task<ProcessResult> RunAsync(string program, IReadOnlyList<string> arguments, string? workingDirectory, TimeSpan timeout) =>
        Programs.RunAsync(program, arguments, workingDirectory: workingDirectory, timeout: timeout, cancel: Stopping);

    protected override async Task<string> FormatAsync(string? path, string text)
    {
        var near = !string.IsNullOrEmpty(path) ? Path.GetDirectoryName(path) : Workspace;
        var tool = Find(near) ?? throw new ModuleException(InstallHint);
        var profile = Setting<string?>("resharper.profile", null) is { Length: > 0 } configured ? configured : DefaultProfile;
        var (scratch, tempDir) = ScratchFile(path);
        List<string> arguments = [.. tool.Arguments, "cleanupcode", $"--profile={profile}", "--verbosity=WARN"];
        if (SolutionSettings(near) is { } dotSettings)
        {
            arguments.Add($"--settings={dotSettings}");
        }
        arguments.Add(scratch);
        UpdateStatus("ReSharper: formatting…");
        ProcessResult done;
        string output;
        try
        {
            // The document's line endings stay as they are: the text is written and read back unchanged.
            File.WriteAllText(scratch, text, Utf8);
            done = await RunAsync(tool.Program, arguments, tool.WorkingDirectory ?? (string.IsNullOrEmpty(near) ? null : near), TimeSpan.FromSeconds(300));
            output = Utf8.GetString(File.ReadAllBytes(scratch));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or ModuleException or TimeoutException)
        {
            throw new ModuleException($"ReSharper failed: {e.Message}");
        }
        finally
        {
            try
            {
                File.Delete(scratch);
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
            }
            if (tempDir != null)
            {
                try
                {
                    Directory.Delete(tempDir, recursive: true);
                }
                catch (Exception e) when (e is IOException or UnauthorizedAccessException)
                {
                }
            }
            UpdateStatus();
        }
        if (!done.Success)
        {
            var error = (done.Error + done.Output).Trim();
            throw new ModuleException(error.Length > 0 ? error : $"jb cleanupcode exited with {done.ExitCode}");
        }
        if (output.Length == 0 && text.Trim().Length > 0)
        {
            throw new ModuleException("ReSharper returned an empty file.");
        }
        return output;
    }

    protected override async Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "resharper.install":
                await RunTool(["tool", "install", "-g", "JetBrains.ReSharper.GlobalTools"],
                    "The ReSharper command line tools are installed. Pick ReSharper with \"Format Document With…\".");
                break;
            case "resharper.update":
                await RunTool(["tool", "update", "-g", "JetBrains.ReSharper.GlobalTools"], "The ReSharper command line tools are up to date.");
                break;
            case "resharper.uninstall":
                await RunTool(["tool", "uninstall", "-g", "JetBrains.ReSharper.GlobalTools"], "The ReSharper command line tools were uninstalled.");
                break;
        }
    }

    async Task RunTool(string[] arguments, string success)
    {
        Log("> dotnet " + string.Join(" ", arguments));
        try
        {
            var done = await RunAsync(Programs.Which("dotnet") ?? "dotnet", arguments, null, TimeSpan.FromSeconds(600));
            var output = (done.Output + done.Error).Trim();
            if (output.Length > 0)
            {
                Log(output);
            }
            if (done.Success)
            {
                Log(success);
            }
        }
        catch (Exception e) when (e is ModuleException or TimeoutException)
        {
            Log($"dotnet failed: {e.Message}");
        }
        ShowView("output");
        UpdateStatus();
    }

    void UpdateStatus(string? text = null)
    {
        if (!string.IsNullOrEmpty(text))
        {
            SetStatus("resharper.status", text, right: true);
        }
        else if (Find(Workspace) != null)
        {
            SetStatus("resharper.status", "", right: true);
        }
        else
        {
            SetStatus("resharper.status", "ReSharper: not installed",
                "Click to install the ReSharper command line tools (dotnet tool install -g JetBrains.ReSharper.GlobalTools)", "resharper.install", right: true);
        }
    }

    /// <summary>How to run <c>jb</c> for files in <paramref name="near"/> (or the workspace), or null.</summary>
    public Tool? Find(string? near)
    {
        if ((LocalToolDir(near) ?? LocalToolDir(Workspace)) is { } local)
        {
            return new Tool(Programs.Which("dotnet") ?? "dotnet", ["jb"], local);
        }
        var found = Programs.Which("jb");
        if (found == null)
        {
            var candidate = Path.Combine(Programs.Home, ".dotnet", "tools", OperatingSystem.IsWindows() ? "jb.exe" : "jb");
            found = File.Exists(candidate) ? candidate : null;
        }
        return found != null ? new Tool(found, [], null) : null;
    }

    /// <summary>The folder whose .config/dotnet-tools.json lists the ReSharper tools, searching upwards from <paramref name="start"/>.</summary>
    public static string? LocalToolDir(string? start)
    {
        if (string.IsNullOrEmpty(start))
        {
            return null;
        }
        for (var dir = new DirectoryInfo(Path.GetFullPath(start)); dir != null; dir = dir.Parent)
        {
            var manifest = Path.Combine(dir.FullName, ".config", "dotnet-tools.json");
            try
            {
                if (File.Exists(manifest) && JsonNode.Parse(File.ReadAllText(manifest))?["tools"] is JsonObject tools
                    && tools.Any(t => t.Key.Equals(Package, StringComparison.OrdinalIgnoreCase)))
                {
                    return dir.FullName;
                }
            }
            catch (Exception e) when (e is IOException or JsonException or UnauthorizedAccessException or InvalidOperationException)
            {
            }
        }
        return null;
    }

    /// <summary>
    /// The &lt;solution&gt;.DotSettings of the nearest solution above <paramref name="start"/>, or null. The first
    /// solution (by name) of the nearest folder with one decides, even when it has no .DotSettings.
    /// </summary>
    public static string? SolutionSettings(string? start)
    {
        if (string.IsNullOrEmpty(start))
        {
            return null;
        }
        for (var dir = new DirectoryInfo(Path.GetFullPath(start)); dir != null; dir = dir.Parent)
        {
            string[] candidates;
            try
            {
                // Like a glob of *.sln*: hidden names (starting with a dot) don't count.
                candidates = [.. Directory.GetFiles(dir.FullName, "*.sln*").Where(f => !Path.GetFileName(f).StartsWith('.')).Order(StringComparer.Ordinal)];
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                continue;
            }
            foreach (var solution in candidates)
            {
                if (solution.EndsWith(".sln", StringComparison.Ordinal) || solution.EndsWith(".slnx", StringComparison.Ordinal))
                {
                    var dotSettings = solution + ".DotSettings";
                    return File.Exists(dotSettings) ? dotSettings : null;
                }
            }
        }
        return null;
    }

    /// <summary>A new hidden file next to <paramref name="path"/> (so .editorconfig applies), or in a temp folder for untitled files; and that temp folder.</summary>
    public static (string File, string? TempDir) ScratchFile(string? path)
    {
        var name = $".bf-resharper-{Environment.ProcessId}-{Interlocked.Increment(ref counter)}.cs";
        var folder = !string.IsNullOrEmpty(path) ? Path.GetDirectoryName(path) : null;
        if (!string.IsNullOrEmpty(folder) && Directory.Exists(folder) && Writable(folder))
        {
            return (Path.Combine(folder, name), null);
        }
        var temp = Directory.CreateTempSubdirectory("bf-resharper-").FullName;
        return (Path.Combine(temp, name), temp);
    }

    static bool Writable(string folder)
    {
        if (OperatingSystem.IsWindows())
        {
            // As before (os.access(W_OK) on Windows): an existing folder always counts as writable.
            return true;
        }
        try
        {
            var probe = Path.Combine(folder, $".bf-resharper-probe-{Environment.ProcessId}-{Guid.NewGuid():N}");
            using (File.Create(probe, 1, FileOptions.DeleteOnClose))
            {
            }
            return true;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    /// <summary>A way to run jb: the program, arguments before jb's own, and where to run it.</summary>
    public sealed record Tool(string Program, string[] Arguments, string? WorkingDirectory);
}
