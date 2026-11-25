using System.Text.Json.Nodes;
using ByteForge.Csharpier;
using ByteForge.Modules;
using ByteForge.Modules.Testing;

namespace ByteForge.Csharpier.Tests;

/// <summary>PATH and the current folder are process-wide: these tests take turns.</summary>
[CollectionDefinition("process environment", DisableParallelization = true)]
public sealed class ProcessEnvironment;

[Collection("process environment")]
public sealed class CsharpierTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-csharpier-").FullName;
    readonly string? path = Environment.GetEnvironmentVariable("PATH");
    readonly string cwd = Environment.CurrentDirectory;

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("PATH", path);
        Environment.CurrentDirectory = cwd;
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    /// <summary>A fake `csharpier` in its own folder: prints <paramref name="output"/>, or fails with it.</summary>
    string FakeCsharpier(string output, bool fail = false)
    {
        var bin = Directory.CreateDirectory(Path.Combine(dir, "fake-" + Guid.NewGuid().ToString("N")[..6])).FullName;
        if (OperatingSystem.IsWindows())
        {
            File.WriteAllText(Path.Combine(bin, "csharpier.cmd"), fail ? $"@echo {output} 1>&2\r\n@exit /b 3\r\n" : $"@echo {output}\r\n");
        }
        else
        {
            var script = Path.Combine(bin, "csharpier");
            File.WriteAllText(script, fail ? $"#!/bin/sh\necho {output} >&2\nexit 3\n" : $"#!/bin/sh\ncat >/dev/null\necho {output}\n");
            File.SetUnixFileMode(script, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }
        return bin;
    }

    static JsonObject Format(string path, string text) => new() { ["type"] = "format", ["requestId"] = 7, ["path"] = path, ["text"] = text };

    [Fact]
    public async Task Registers_the_formatter_and_its_commands()
    {
        await using var module = ModuleHarness.Start(new CsharpierModule(), Path.Combine(dir, "modules", "csharpier"));
        var formatter = await module.Next("registerFormatter");
        Assert.Equal("csharpier", formatter["id"]!.GetValue<string>());
        Assert.Equal(CsharpierModule.Extensions, formatter["extensions"]!.AsArray().Select(e => e!.GetValue<string>()));
        foreach (var command in new[] { "csharpier.install", "csharpier.update", "csharpier.uninstall" })
        {
            await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == command);
        }
        await module.Next("status", m => m["id"]!.GetValue<string>() == "csharpier.status");
    }

    [Fact]
    public async Task Formats_through_csharpier_and_reports_its_errors()
    {
        Environment.SetEnvironmentVariable("PATH", FakeCsharpier("formatted") + Path.PathSeparator + path);
        await using (var module = ModuleHarness.Start(new CsharpierModule(), dir))
        {
            module.Activate(dir);
            module.Send(Format(Path.Combine(dir, "A.cs"), "class A{}"));
            var result = await module.Next("formatResult");
            Assert.Equal(7, result["requestId"]!.GetValue<long>());
            Assert.Equal("formatted", result["text"]!.GetValue<string>().Trim());
        }

        Environment.SetEnvironmentVariable("PATH", FakeCsharpier("bad syntax", fail: true) + Path.PathSeparator + path);
        await using (var module = ModuleHarness.Start(new CsharpierModule(), dir))
        {
            module.Activate(dir);
            module.Send(Format(Path.Combine(dir, "A.cs"), "class A{"));
            var result = await module.Next("formatResult");
            Assert.Null(result["text"]);
            Assert.Equal("bad syntax", result["error"]!.GetValue<string>().Trim());
        }
    }

    [Fact]
    public void Finds_programs_on_path_only_not_in_the_current_folder()
    {
        // A module's folder holding a file named like the program (the old csharpier.py) isn't a match.
        var here = Directory.CreateDirectory(Path.Combine(dir, "module")).FullName;
        File.WriteAllText(Path.Combine(here, OperatingSystem.IsWindows() ? "csharpier.cmd" : "csharpier"), "");
        Environment.CurrentDirectory = here;
        Environment.SetEnvironmentVariable("PATH", Path.Combine(dir, "empty"));
        Assert.Null(Programs.Which("csharpier"));
    }

    [Fact]
    public void A_tool_manifest_up_the_tree_wins()
    {
        var repo = Directory.CreateDirectory(Path.Combine(dir, "repo")).FullName;
        var src = Directory.CreateDirectory(Path.Combine(repo, "src", "App")).FullName;
        Assert.Null(CsharpierModule.LocalToolDir(src));
        Directory.CreateDirectory(Path.Combine(repo, ".config"));
        File.WriteAllText(Path.Combine(repo, ".config", "dotnet-tools.json"), """{ "version": 1, "tools": { "csharpier": { "version": "1.0.0", "commands": ["csharpier"] } } }""");
        Assert.Equal(repo, CsharpierModule.LocalToolDir(src));
        var tool = new CsharpierModule().Find(src);
        Assert.NotNull(tool);
        Assert.Equal(["csharpier"], tool.Arguments);
        Assert.Equal(repo, tool.WorkingDirectory);
    }

    [Fact]
    public async Task Formats_with_the_real_csharpier_when_installed()
    {
        var installed = Programs.Which("csharpier") ?? Path.Combine(Programs.Home, ".dotnet", "tools", OperatingSystem.IsWindows() ? "csharpier.exe" : "csharpier");
        if (!File.Exists(installed))
        {
            return; // not installed here
        }
        await using var module = ModuleHarness.Start(new CsharpierModule(), dir);
        module.Activate(dir);
        module.Send(Format(Path.Combine(dir, "A.cs"), "class A{void M(){}}"));
        var result = await module.Next("formatResult", timeout: TimeSpan.FromMinutes(1));
        Assert.Null(result["error"]);
        Assert.Equal("class A\n{\n    void M() { }\n}\n", result["text"]!.GetValue<string>());
    }
}
