using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.Csharpier;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<CsharpierModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// CSharpier (https://csharpier.com) as the formatter for C# and MSBuild/XML files: the text goes through
/// <c>csharpier format --write-stdout</c>. A version pinned in the workspace's dotnet tool manifest
/// (.config/dotnet-tools.json) wins over a global install, as in other editors.
/// </summary>
public sealed class CsharpierModule : Module
{
    public static readonly string[] Extensions = ["cs", "csx", "csproj", "props", "targets", "xml", "config"];
    public const string InstallHint = "CSharpier isn't installed. Run \"CSharpier: Install\" from the command palette (dotnet tool install -g csharpier).";

    protected override Task OnStartAsync()
    {
        RegisterFormatter("csharpier", "CSharpier", Extensions);
        RegisterCommand("csharpier.install", "Install (global dotnet tool)", "CSharpier");
        RegisterCommand("csharpier.update", "Update", "CSharpier");
        RegisterCommand("csharpier.uninstall", "Uninstall the dotnet tool", "CSharpier");
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

    protected override async Task<string> FormatAsync(string? path, string text)
    {
        var tool = Find(path != null ? Path.GetDirectoryName(path) : Workspace) ?? throw new ModuleException(InstallHint);
        var result = await Programs.RunAsync(tool.Program, [.. tool.Arguments, "format", "--write-stdout", "--stdin-path", path ?? "Untitled.cs"],
            input: text, workingDirectory: tool.WorkingDirectory ?? Workspace, cancel: Stopping);
        if (!result.Success)
        {
            var error = (result.Error.Length > 0 ? result.Error : result.Output).Trim();
            throw new ModuleException(error.Length > 0 ? error : $"CSharpier exited with {result.ExitCode}");
        }
        // CSharpier prints nothing for file types it doesn't handle; never hand back an empty file.
        if (result.Output.Length == 0 && text.Trim().Length > 0)
        {
            throw new ModuleException("CSharpier can't format this file type.");
        }
        return result.Output;
    }

    protected override async Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "csharpier.install":
                await DotnetTool(["tool", "install", "-g", "csharpier"], "CSharpier is installed. Format a C# file with Shift+Alt+F.");
                break;
            case "csharpier.update":
                await DotnetTool(["tool", "update", "-g", "csharpier"], "CSharpier is up to date.");
                break;
            case "csharpier.uninstall":
                await DotnetTool(["tool", "uninstall", "-g", "csharpier"], "The CSharpier dotnet tool was uninstalled.");
                break;
        }
    }

    async Task DotnetTool(string[] arguments, string success)
    {
        Log("> dotnet " + string.Join(" ", arguments));
        try
        {
            var result = await Programs.RunAsync(Programs.Which("dotnet") ?? "dotnet", arguments, timeout: TimeSpan.FromMinutes(5), cancel: Stopping);
            var output = (result.Output + result.Error).Trim();
            if (output.Length > 0)
            {
                Log(output);
            }
            if (result.Success)
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

    void UpdateStatus()
    {
        if (Find(Workspace) != null)
        {
            SetStatus("csharpier.status", "", right: true);
        }
        else
        {
            SetStatus("csharpier.status", "CSharpier: not installed", "Click to install CSharpier (dotnet tool install -g csharpier)", "csharpier.install", right: true);
        }
    }

    /// <summary>How to run CSharpier for files in <paramref name="near"/> (or the workspace), or null.</summary>
    public Tool? Find(string? near)
    {
        if ((LocalToolDir(near) ?? LocalToolDir(Workspace)) is { } local)
        {
            return new Tool(Programs.Which("dotnet") ?? "dotnet", ["csharpier"], local);
        }
        var found = Programs.Which("csharpier");
        if (found == null)
        {
            var candidate = Path.Combine(Programs.Home, ".dotnet", "tools", OperatingSystem.IsWindows() ? "csharpier.exe" : "csharpier");
            found = File.Exists(candidate) ? candidate : null;
        }
        return found != null ? new Tool(found, [], null) : null;
    }

    /// <summary>The folder whose .config/dotnet-tools.json lists csharpier, searching upwards from <paramref name="start"/>.</summary>
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
                if (File.Exists(manifest) && JsonNode.Parse(File.ReadAllText(manifest))?["tools"] is JsonObject tools && tools.ContainsKey("csharpier"))
                {
                    return dir.FullName;
                }
            }
            catch (Exception e) when (e is IOException or JsonException or UnauthorizedAccessException)
            {
            }
        }
        return null;
    }

    /// <summary>A way to run CSharpier: the program, arguments before CSharpier's own, and where to run it.</summary>
    public sealed record Tool(string Program, string[] Arguments, string? WorkingDirectory);
}
