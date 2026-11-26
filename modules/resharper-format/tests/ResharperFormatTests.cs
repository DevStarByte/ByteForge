using System.Text.Json.Nodes;
using ByteForge.Modules;
using ByteForge.Modules.Testing;

namespace ByteForge.ResharperFormat.Tests;

/// <summary>PATH is process-wide: these tests take turns.</summary>
[CollectionDefinition("process environment", DisableParallelization = true)]
public sealed class ProcessEnvironment;

/// <summary>The module with jb / dotnet replaced: records each run and lets the test act as the program.</summary>
sealed class FakeResharper(Func<string, IReadOnlyList<string>, ProcessResult> run) : ResharperFormatModule
{
    public List<(string Program, string[] Arguments, string? WorkingDirectory, TimeSpan Timeout)> Runs { get; } = [];

    protected override Task<ProcessResult> RunAsync(string program, IReadOnlyList<string> arguments, string? workingDirectory, TimeSpan timeout)
    {
        lock (Runs)
        {
            Runs.Add((program, [.. arguments], workingDirectory, timeout));
        }
        return Task.FromResult(run(program, arguments));
    }
}

[Collection("process environment")]
public sealed class ResharperFormatTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-resharper-test-").FullName;
    readonly string? path = Environment.GetEnvironmentVariable("PATH");

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

    string ModuleDir => Path.Combine(dir, "modules", "resharper-format");

    /// <summary>A repository pinning the ReSharper tools in its dotnet tool manifest, with a solution in it.</summary>
    string Repo(bool dotSettings = true)
    {
        var repo = Directory.CreateDirectory(Path.Combine(dir, "repo")).FullName;
        Directory.CreateDirectory(Path.Combine(repo, ".config"));
        File.WriteAllText(Path.Combine(repo, ".config", "dotnet-tools.json"),
            """{ "version": 1, "isRoot": true, "tools": { "JetBrains.ReSharper.GlobalTools": { "version": "2025.3.0", "commands": ["jb"] } } }""");
        File.WriteAllText(Path.Combine(repo, "App.sln"), "");
        if (dotSettings)
        {
            File.WriteAllText(Path.Combine(repo, "App.sln.DotSettings"), "<wpf:ResourceDictionary />");
        }
        Directory.CreateDirectory(Path.Combine(repo, "src"));
        return repo;
    }

    static JsonObject Format(string? file, string text) => new() { ["type"] = "format", ["requestId"] = 5, ["path"] = file, ["text"] = text };

    [Fact]
    public async Task Registers_the_formatter_its_commands_and_status()
    {
        await using var module = ModuleHarness.Start(new FakeResharper((_, _) => new(0, "", "")), ModuleDir);
        var formatter = await module.Next("registerFormatter");
        Assert.Equal("resharper", formatter["id"]!.GetValue<string>());
        Assert.Equal("ReSharper", formatter["name"]!.GetValue<string>());
        Assert.Equal(["cs"], formatter["extensions"]!.AsArray().Select(e => e!.GetValue<string>()));
        foreach (var (id, title) in new[] { ("resharper.install", "Install (global dotnet tool)"), ("resharper.update", "Update"), ("resharper.uninstall", "Uninstall the dotnet tool") })
        {
            var command = await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == id);
            Assert.Equal(title, command["title"]!.GetValue<string>());
            Assert.Equal("ReSharper", command["category"]!.GetValue<string>());
        }
        var status = await module.Next("status", m => m["id"]!.GetValue<string>() == "resharper.status");
        Assert.Equal("right", status["alignment"]!.GetValue<string>());
    }

    [Fact]
    public async Task Cleans_up_a_hidden_copy_next_to_the_file_with_the_exact_arguments()
    {
        var repo = Repo();
        var file = Path.Combine(repo, "src", "A.cs");
        string? seen = null;
        var fake = new FakeResharper((_, arguments) =>
        {
            var scratch = arguments[^1];
            seen = File.ReadAllText(scratch);
            File.WriteAllText(scratch, "class A\r\n{\r\n}\r\n");
            return new(0, "", "");
        });
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Activate(repo);
        module.Send(Format(file, "class A{\r\n}"));
        var result = await module.Next("formatResult");
        Assert.Equal(5, result["requestId"]!.GetValue<long>());
        Assert.Null(result["error"]);
        Assert.Equal("class A\r\n{\r\n}\r\n", result["text"]!.GetValue<string>());
        Assert.Equal("class A{\r\n}", seen);

        var run = Assert.Single(fake.Runs);
        Assert.Equal("dotnet", Path.GetFileNameWithoutExtension(run.Program));
        Assert.Equal(repo, run.WorkingDirectory);
        Assert.Equal(TimeSpan.FromSeconds(300), run.Timeout);
        var scratch = run.Arguments[^1];
        Assert.Equal(["jb", "cleanupcode", "--profile=Built-in: Reformat Code", "--verbosity=WARN", $"--settings={Path.Combine(repo, "App.sln.DotSettings")}", scratch], run.Arguments);
        Assert.Equal(Path.Combine(repo, "src"), Path.GetDirectoryName(scratch));
        Assert.Matches(@"^\.bf-resharper-\d+-\d+\.cs$", Path.GetFileName(scratch));
        Assert.False(File.Exists(scratch));
        lock (module.Sent)
        {
            Assert.Contains(module.Sent, m => m["type"]!.GetValue<string>() == "status" && m["text"]!.GetValue<string>() == "ReSharper: formatting…");
        }
    }

    [Fact]
    public async Task Uses_the_profile_setting_and_no_settings_without_a_DotSettings_file()
    {
        var repo = Repo(dotSettings: false);
        var fake = new FakeResharper((_, a) =>
        {
            File.WriteAllText(a[^1], "x");
            return new(0, "", "");
        });
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Activate(repo, new() { ["resharper.profile"] = "Full Cleanup" });
        module.Send(Format(Path.Combine(repo, "src", "A.cs"), "y"));
        await module.Next("formatResult");
        var run = Assert.Single(fake.Runs);
        Assert.Equal(["jb", "cleanupcode", "--profile=Full Cleanup", "--verbosity=WARN", run.Arguments[^1]], run.Arguments);
    }

    [Fact]
    public async Task An_untitled_file_is_cleaned_up_in_a_temp_folder_that_goes_away()
    {
        var repo = Repo();
        var fake = new FakeResharper((_, a) =>
        {
            File.WriteAllText(a[^1], "formatted");
            return new(0, "", "");
        });
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Activate(repo);
        module.Send(Format(null, "text"));
        var result = await module.Next("formatResult");
        Assert.Equal("formatted", result["text"]!.GetValue<string>());
        var scratch = Assert.Single(fake.Runs).Arguments[^1];
        var temp = Path.GetDirectoryName(scratch)!;
        Assert.StartsWith("bf-resharper-", Path.GetFileName(temp));
        Assert.False(Directory.Exists(temp));
    }

    [Fact]
    public async Task Reports_failures_and_empty_results()
    {
        var repo = Repo();
        var file = Path.Combine(repo, "src", "A.cs");
        await using (var module = ModuleHarness.Start(new FakeResharper((_, _) => new(3, "out", "err ")), ModuleDir))
        {
            module.Activate(repo);
            module.Send(Format(file, "class A{}"));
            Assert.Equal("err out", (await module.Next("formatResult"))["error"]!.GetValue<string>());
        }
        await using (var module = ModuleHarness.Start(new FakeResharper((_, _) => new(3, "", "")), ModuleDir))
        {
            module.Activate(repo);
            module.Send(Format(file, "class A{}"));
            Assert.Equal("jb cleanupcode exited with 3", (await module.Next("formatResult"))["error"]!.GetValue<string>());
        }
        await using (var module = ModuleHarness.Start(new FakeResharper((_, a) =>
        {
            File.WriteAllText(a[^1], "");
            return new(0, "", "");
        }), ModuleDir))
        {
            module.Activate(repo);
            module.Send(Format(file, "class A{}"));
            Assert.Equal("ReSharper returned an empty file.", (await module.Next("formatResult"))["error"]!.GetValue<string>());
        }
        Assert.Empty(Directory.GetFiles(Path.Combine(repo, "src")));
    }

    [Fact]
    public async Task Without_the_tools_it_says_how_to_install_them()
    {
        Environment.SetEnvironmentVariable("PATH", Path.Combine(dir, "empty"));
        if (File.Exists(Path.Combine(Programs.Home, ".dotnet", "tools", OperatingSystem.IsWindows() ? "jb.exe" : "jb")))
        {
            return; // installed globally here
        }
        var folder = Directory.CreateDirectory(Path.Combine(dir, "plain")).FullName;
        await using var module = ModuleHarness.Start(new FakeResharper((_, _) => new(0, "", "")), ModuleDir);
        module.Activate(folder);
        var status = await module.Next("status", m => m["text"]!.GetValue<string>() == "ReSharper: not installed");
        Assert.Equal("resharper.install", status["command"]!.GetValue<string>());
        module.Send(Format(Path.Combine(folder, "A.cs"), "class A{}"));
        Assert.Equal(ResharperFormatModule.InstallHint, (await module.Next("formatResult"))["error"]!.GetValue<string>());
    }

    [Fact]
    public async Task Install_runs_the_dotnet_tool_command()
    {
        var fake = new FakeResharper((_, _) => new(0, "Tool installed.", ""));
        await using var module = ModuleHarness.Start(fake, ModuleDir);
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "resharper.install" });
        await module.Next("log", m => m["text"]!.GetValue<string>() == "> dotnet tool install -g JetBrains.ReSharper.GlobalTools");
        await module.Next("log", m => m["text"]!.GetValue<string>() == "Tool installed.");
        await module.Next("log", m => m["text"]!.GetValue<string>().StartsWith("The ReSharper command line tools are installed."));
        await module.Next("showView", m => m["id"]!.GetValue<string>() == "output");
        var run = Assert.Single(fake.Runs);
        Assert.Equal(["tool", "install", "-g", "JetBrains.ReSharper.GlobalTools"], run.Arguments);
        Assert.Equal(TimeSpan.FromSeconds(600), run.Timeout);
    }

    [Fact]
    public void The_nearest_folder_with_a_solution_decides_the_DotSettings()
    {
        var root = Directory.CreateDirectory(Path.Combine(dir, "root")).FullName;
        var inner = Directory.CreateDirectory(Path.Combine(root, "inner", "deep")).FullName;
        Assert.Null(ResharperFormatModule.SolutionSettings(inner));
        File.WriteAllText(Path.Combine(root, "B.sln"), "");
        File.WriteAllText(Path.Combine(root, "B.sln.DotSettings"), "");
        Assert.Equal(Path.Combine(root, "B.sln.DotSettings"), ResharperFormatModule.SolutionSettings(inner));
        // A solution without .DotSettings nearer stops the search.
        File.WriteAllText(Path.Combine(root, "inner", "A.slnx"), "");
        Assert.Null(ResharperFormatModule.SolutionSettings(inner));
        // The first by name decides; other *.sln* files (like .DotSettings) are skipped.
        File.WriteAllText(Path.Combine(root, "inner", "A.slnx.DotSettings"), "");
        Assert.Equal(Path.Combine(root, "inner", "A.slnx.DotSettings"), ResharperFormatModule.SolutionSettings(inner));
    }

    [Fact]
    public void A_tool_manifest_up_the_tree_wins()
    {
        var repo = Repo();
        var src = Path.Combine(repo, "src");
        Assert.Equal(repo, ResharperFormatModule.LocalToolDir(src));
        Assert.Null(ResharperFormatModule.LocalToolDir(Directory.CreateDirectory(Path.Combine(dir, "other")).FullName));
        var tool = new ResharperFormatModule().Find(src);
        Assert.NotNull(tool);
        Assert.Equal(["jb"], tool.Arguments);
        Assert.Equal(repo, tool.WorkingDirectory);
    }
}
