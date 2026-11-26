using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Aspire;
using ByteForge.Modules.Testing;

namespace ByteForge.Aspire.Tests;

static class Samples
{
    public const string ApphostCsproj = """
        <Project Sdk="Microsoft.NET.Sdk">
          <Sdk Name="Aspire.AppHost.Sdk" Version="13.0.0" />
          <PropertyGroup><OutputType>Exe</OutputType></PropertyGroup>
        </Project>
        """;

    public const string OldApphostCsproj = """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup><IsAspireHost>true</IsAspireHost></PropertyGroup>
        </Project>
        """;

    public const string WebCsproj = """<Project Sdk="Microsoft.NET.Sdk.Web"><PropertyGroup /></Project>""";

    public const string ShopPath = "/src/Shop/Shop.AppHost/Shop.AppHost.csproj";

    public static string PsOutput(string path = ShopPath) => $$"""
        [
          {"appHostPath": {{JsonSerializer.Serialize(path)}}, "appHostPid": 4242, "cliPid": 4200,
           "dashboardUrl": "https://localhost:17043/login?t=abc"}
        ]
        """;

    public const string DescribeOutput = """
        {"resources": [
          {"name": "webfrontend", "displayName": "webfrontend", "resourceType": "Project", "state": "Running",
           "stateStyle": "success", "healthStatus": "Healthy", "exitCode": null,
           "urls": [{"name": "https", "displayName": null, "url": "https://localhost:7001", "isInternal": false},
                    {"name": "http", "displayName": null, "url": "http://localhost:5001", "isInternal": true}],
           "commands": {"stop": {"state": "Enabled"}, "restart": {"state": "Enabled"},
                        "start": {"state": "Hidden", "visibility": "Hidden"}}},
          {"name": "cache-xyz", "displayName": "cache", "resourceType": "Container", "state": "Exited",
           "stateStyle": "error", "exitCode": 1, "urls": null,
           "commands": {"start": {"state": "Enabled"}, "stop": {"state": "Disabled"}}}
        ]}
        """;

    public static void Write(string path, string text = "")
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text);
    }

    public static JsonObject Obj(string json) => JsonNode.Parse(json)!.AsObject();

    public static string S(JsonNode? node) => node!.GetValue<string>();
}

public sealed class ParsingTests
{
    [Fact]
    public void Find_json_skips_banners()
    {
        Assert.Equal("[1,2]", AspireCli.FindJson("Aspire CLI 13.0\n[1, 2]\n").ToJsonString());
        Assert.Equal("""{"a":1}""", AspireCli.FindJson("Note: [preview]\n{\"a\": 1}").ToJsonString());
        Assert.Throws<AspireException>(() => AspireCli.FindJson("no json here"));
    }

    [Fact]
    public void Find_json_reads_the_first_value_only_and_keeps_the_last_of_a_repeated_key()
    {
        Assert.Equal("""{"a":2,"b":3}""", AspireCli.FindJson("""{"a": 1, "b": 3, "a": 2} trailing {"c": 4}""").ToJsonString());
    }

    [Fact]
    public void Parse_ps()
    {
        var hosts = AspireCli.ParsePs(Samples.PsOutput());
        var host = Assert.Single(hosts);
        Assert.Equal(Samples.ShopPath, host.Path);
        Assert.Equal(4242, host.Pid!.GetValue<int>());
        Assert.Equal("https://localhost:17043/login?t=abc", host.Dashboard);
    }

    [Fact]
    public void Parse_ps_single_object_and_empty()
    {
        Assert.Single(AspireCli.ParsePs("""{"appHostPath": "/a/apphost.ts", "appHostPid": 1}"""));
        Assert.Empty(AspireCli.ParsePs("[]"));
        Assert.Empty(AspireCli.ParsePs(""));
        Assert.Empty(AspireCli.ParsePs("""[{"nope": 1}]"""));
    }

    [Fact]
    public void Parse_resources_sorted_by_display_name()
    {
        var resources = AspireCli.ParseResources(Samples.DescribeOutput);
        Assert.Equal(["cache", "webfrontend"], resources.Select(AspireCli.DisplayName));
    }

    [Fact]
    public void Commands()
    {
        var resources = AspireCli.ParseResources(Samples.DescribeOutput);
        var (web, cache) = (resources[1], resources[0]);
        Assert.True(AspireCli.CommandEnabled(web, "stop"));
        Assert.True(AspireCli.CommandEnabled(web, "restart"));
        Assert.False(AspireCli.CommandEnabled(web, "start"));
        Assert.True(AspireCli.CommandEnabled(cache, "start"));
        Assert.False(AspireCli.CommandEnabled(cache, "stop"));
        Assert.True(AspireCli.CommandEnabled(Samples.Obj("""{"commands": {"resource-stop": {}}}"""), "stop"));
        Assert.False(AspireCli.CommandEnabled(Samples.Obj("""{"commands": null}"""), "stop"));
    }

    [Fact]
    public void Status()
    {
        string Of(string json) => AspireCli.ResourceStatus(Samples.Obj(json));
        Assert.Equal("running", Of("""{"state": "Running"}"""));
        Assert.Equal("warning", Of("""{"state": "Running", "healthStatus": "Unhealthy"}"""));
        Assert.Equal("error", Of("""{"state": "Running", "stateStyle": "error"}"""));
        Assert.Equal("busy", Of("""{"state": "Starting"}"""));
        Assert.Equal("idle", Of("""{"state": "Exited", "exitCode": 0}"""));
        Assert.Equal("error", Of("""{"state": "Exited", "exitCode": 137}"""));
        Assert.Equal("warning", Of("""{"state": "FailedToStart"}"""));
        Assert.Equal("idle", Of("""{"state": null}"""));
    }

    [Fact]
    public void Urls_hide_internal_ones()
    {
        var web = AspireCli.ParseResources(Samples.DescribeOutput)[1];
        var url = Assert.Single(AspireCli.ResourceUrls(web));
        Assert.Equal("https", url.Label.GetValue<string>());
        Assert.Equal("https://localhost:7001", url.Url);
    }

    [Fact]
    public void Command_lines_quote_what_a_shell_would_split()
    {
        Assert.Equal("/usr/bin/aspire run --apphost /src/Shop/Shop.AppHost/Shop.AppHost.csproj",
            AspireWorkspace.CommandLine(["/usr/bin/aspire", "run", "--apphost", Samples.ShopPath], windows: false));
        Assert.Equal("aspire logs 'my app' '' 'it'\\''s'", AspireWorkspace.CommandLine(["aspire", "logs", "my app", "", "it's"], windows: false));
        Assert.Equal("\"C:\\Program Files\\aspire.exe\" run \"a \\\"b\\\"\"", AspireWorkspace.CommandLine(["C:\\Program Files\\aspire.exe", "run", "a \"b\""], windows: true));
    }

    [Fact]
    public void The_last_line_of_cli_output()
    {
        Assert.Equal("c", Python.LastLine("a\nb\r\nc"));
        Assert.Equal("b", Python.LastLine("a\nb\r\n"));
        Assert.Equal("one", Python.LastLine("one"));
    }
}

/// <summary>PATH is process-wide: these tests take turns.</summary>
[CollectionDefinition("process environment", DisableParallelization = true)]
public sealed class ProcessEnvironment;

[Collection("process environment")]
public sealed class WorkspaceTests : IDisposable
{
    readonly string root = Directory.CreateTempSubdirectory("bf-aspire-").FullName;
    readonly string? path = Environment.GetEnvironmentVariable("PATH");

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("PATH", path);
        try
        {
            Directory.Delete(root, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    [Fact]
    public void Finds_project_and_file_apphosts()
    {
        Samples.Write(Path.Combine(root, "Shop.AppHost", "Shop.AppHost.csproj"), Samples.ApphostCsproj);
        Samples.Write(Path.Combine(root, "Shop.AppHost", "AppHost.cs"), "var builder = DistributedApplication.CreateBuilder(args);");
        Samples.Write(Path.Combine(root, "Legacy", "Legacy.AppHost.csproj"), Samples.OldApphostCsproj);
        Samples.Write(Path.Combine(root, "Web", "Web.csproj"), Samples.WebCsproj);
        Samples.Write(Path.Combine(root, "single", "apphost.cs"), "#:sdk Aspire.AppHost.Sdk@13.0.0\n");
        Samples.Write(Path.Combine(root, "ts", "apphost.ts"), "import { createBuilder } from '@aspire/apphost';");
        Samples.Write(Path.Combine(root, "other", "apphost.ts"), "console.log('unrelated');");
        Samples.Write(Path.Combine(root, "Web", "bin", "Debug", "Copy.AppHost.csproj"), Samples.ApphostCsproj);
        Samples.Write(Path.Combine(root, ".hidden", "Hidden.AppHost.csproj"), Samples.ApphostCsproj);
        var found = AspireWorkspace.FindApphosts(root).Select(p => Path.GetRelativePath(root, p)).Order(StringComparer.Ordinal);
        Assert.Equal(new[]
        {
            Path.Combine("Legacy", "Legacy.AppHost.csproj"),
            Path.Combine("Shop.AppHost", "Shop.AppHost.csproj"),
            Path.Combine("single", "apphost.cs"),
            Path.Combine("ts", "apphost.ts"),
        }.Order(StringComparer.Ordinal), found);
    }

    [Fact]
    public void No_workspace()
    {
        Assert.Empty(AspireWorkspace.FindApphosts(null));
        Assert.Empty(AspireWorkspace.FindApphosts(Path.Combine(root, "missing")));
    }

    [Fact]
    public void Labels_and_source()
    {
        var project = Path.Combine(root, "Shop.AppHost", "Shop.AppHost.csproj");
        Samples.Write(project, Samples.ApphostCsproj);
        Assert.Equal("Shop.AppHost", AspireWorkspace.ApphostLabel(project));
        Assert.Equal("frontend", AspireWorkspace.ApphostLabel(Path.Combine(root, "frontend", "apphost.ts")));
        Assert.Equal("elsewhere", AspireWorkspace.ApphostLabel("/elsewhere/apphost.ts"));
        Assert.Equal(project, AspireWorkspace.SourceFile(project));
        Samples.Write(Path.Combine(root, "Shop.AppHost", "AppHost.cs"));
        Assert.Equal(Path.Combine(root, "Shop.AppHost", "AppHost.cs"), AspireWorkspace.SourceFile(project));
        Assert.Equal(Path.Combine("Shop.AppHost", "Shop.AppHost.csproj"), AspireWorkspace.Relative(project, root));
        Assert.Equal(project, AspireWorkspace.Relative(project, Path.Combine(root, "other")));
    }

    [Fact]
    public void Find_cli_from_setting()
    {
        var fake = Path.Combine(root, "aspire");
        Samples.Write(fake);
        Assert.Equal(fake, AspireCli.FindCli(new JsonObject { ["aspire.cliPath"] = fake }));
        var fromHome = AspireCli.FindCli(new JsonObject { ["aspire.cliPath"] = "  ~/aspire " }, home: root);
        Assert.NotNull(fromHome);
        Assert.Equal(Path.GetFullPath(fake), Path.GetFullPath(fromHome));
        Assert.Null(AspireCli.FindCli(new JsonObject { ["aspire.cliPath"] = Path.Combine(root, "nope") }));
    }

    [Fact]
    public void Find_cli_on_path_then_where_the_install_script_puts_it()
    {
        var exe = OperatingSystem.IsWindows() ? "aspire.exe" : "aspire";
        var home = Directory.CreateDirectory(Path.Combine(root, "home")).FullName;
        Environment.SetEnvironmentVariable("PATH", Directory.CreateDirectory(Path.Combine(root, "empty")).FullName);
        Assert.Null(AspireCli.FindCli([], home));

        var tool = Path.Combine(home, ".dotnet", "tools", exe);
        Samples.Write(tool);
        Assert.Equal(tool, AspireCli.FindCli([], home));
        var installed = Path.Combine(home, ".aspire", "bin", exe);
        Samples.Write(installed);
        Assert.Equal(installed, AspireCli.FindCli([], home));

        var bin = Directory.CreateDirectory(Path.Combine(root, "bin")).FullName;
        var onPath = Path.Combine(bin, exe);
        Samples.Write(onPath);
        if (!OperatingSystem.IsWindows())
        {
            File.SetUnixFileMode(onPath, UnixFileMode.UserRead | UnixFileMode.UserExecute);
        }
        Environment.SetEnvironmentVariable("PATH", bin);
        Assert.Equal(onPath, AspireCli.FindCli([], home));
    }

    [Fact]
    public async Task Runs_the_cli_without_colors_and_reports_its_last_error_line()
    {
        var bin = Directory.CreateDirectory(Path.Combine(root, "fake")).FullName;
        string cli;
        if (OperatingSystem.IsWindows())
        {
            cli = Path.Combine(bin, "aspire.cmd");
            File.WriteAllText(cli, "@if \"%1\"==\"fail\" (\r\n@echo first 1>&2\r\n@echo last problem 1>&2\r\n@exit /b 2\r\n)\r\n@echo color=%NO_COLOR% args=%*\r\n");
        }
        else
        {
            cli = Path.Combine(bin, "aspire");
            File.WriteAllText(cli, "#!/bin/sh\nif [ \"$1\" = fail ]; then echo first >&2; echo last problem >&2; exit 2; fi\necho color=$NO_COLOR args=$*\n");
            File.SetUnixFileMode(cli, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }
        var output = await AspireCli.RunAsync(cli, ["ps", "--format", "json"], TimeSpan.FromSeconds(30), CancellationToken.None);
        Assert.Equal("color=1 args=ps --format json", output.Trim());
        var failure = await Assert.ThrowsAsync<AspireException>(() => AspireCli.RunAsync(cli, ["fail"], TimeSpan.FromSeconds(30), CancellationToken.None));
        Assert.Equal("last problem", failure.Message.Trim());
        var missing = await Assert.ThrowsAsync<AspireException>(() => AspireCli.RunAsync(Path.Combine(bin, "nope"), ["ps"], TimeSpan.FromSeconds(30), CancellationToken.None));
        Assert.Equal("the aspire CLI isn't installed", missing.Message);
    }
}

/// <summary>The module running against a fake CLI: what it's asked and what it prints are set by the test.</summary>
public sealed class TreeTests : IDisposable
{
    const string Cli = "/usr/bin/aspire";

    readonly string config = Directory.CreateTempSubdirectory("bf-aspire-").FullName;
    readonly ConcurrentQueue<string[]> calls = new();
    readonly ConcurrentQueue<string> opened = new();
    string? cli = Cli;
    string ps = "[]";
    string describe = """{"resources": []}""";

    string ModuleDir => Path.Combine(config, "modules", "aspire");

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

    ModuleHarness Start() => ModuleHarness.Start(new AspireModule(_ => cli, RunCli, opened.Enqueue, TimeSpan.FromMinutes(10)), ModuleDir);

    Task<string> RunCli(string program, IReadOnlyList<string> args, TimeSpan timeout, CancellationToken cancel)
    {
        Assert.Equal(cli, program);
        calls.Enqueue([.. args]);
        return Task.FromResult(args[0] switch
        {
            "ps" => ps,
            "describe" => describe,
            _ => "line one\nresource restarted\n",
        });
    }

    static Task<JsonObject> Tree(ModuleHarness module, Func<JsonObject, bool> where) => module.Next("setTree", where);

    /// <summary>The APP HOSTS section's rows of a refreshed tree (not the first one, sent before the CLI was looked for).</summary>
    static JsonArray Hosts(JsonObject tree) => tree["items"]!.AsArray().Last()!["children"]!.AsArray();

    static bool Refreshed(JsonObject tree) => tree["message"]?.GetValue<string>() != "Looking for the Aspire CLI…";

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = Start();
        foreach (var (id, title) in AspireModule.Commands)
        {
            var command = await module.Next("registerCommand", m => Samples.S(m["id"]) == id);
            Assert.Equal(title, Samples.S(command["title"]));
            Assert.Equal("Aspire", Samples.S(command["category"]));
        }
    }

    [Fact]
    public async Task Stopped_apphost_offers_run()
    {
        var project = Path.Combine(config, "Shop", "Shop.AppHost", "Shop.AppHost.csproj");
        Samples.Write(project, Samples.ApphostCsproj);
        await using var module = Start();
        module.Activate(Path.Combine(config, "Shop"));
        var first = await Tree(module, _ => true);
        Assert.Equal("Looking for the Aspire CLI…", Samples.S(first["message"]));
        var tree = await Tree(module, Refreshed);
        var host = Hosts(tree)[0]!;
        Assert.Equal("Shop.AppHost", Samples.S(host["label"]));
        Assert.Equal(Path.Combine("Shop.AppHost", "Shop.AppHost.csproj"), Samples.S(host["description"]));
        Assert.Equal(["run"], host["actions"]!.AsArray().Select(a => Samples.S(a!["id"])));
        Assert.Equal(0, tree["badge"]!.GetValue<int>());
        Assert.Null(tree["message"]);
        Assert.Equal(["new", "refresh"], tree["actions"]!.AsArray().Select(a => Samples.S(a!["id"])));
        Assert.Equal(["ps", "--format", "json"], calls.Single());
    }

    [Fact]
    public async Task Running_apphost_lists_resources()
    {
        var project = Path.Combine(config, "Shop", "Shop.AppHost", "Shop.AppHost.csproj");
        Samples.Write(project, Samples.ApphostCsproj);
        ps = Samples.PsOutput(project);
        describe = Samples.DescribeOutput;
        await using var module = Start();
        module.Activate(Path.Combine(config, "Shop"));
        var tree = await Tree(module, Refreshed);
        var item = Assert.Single(Hosts(tree))!;
        Assert.Equal(["dashboard", "stop"], item["actions"]!.AsArray().Select(a => Samples.S(a!["id"])));
        Assert.Equal("running · 1/2 up", Samples.S(item["description"]));
        Assert.Equal($"{project}\nRunning (pid 4242)", Samples.S(item["tooltip"]));
        Assert.Equal(["Dashboard", "cache", "webfrontend"], item["children"]!.AsArray().Select(c => Samples.S(c!["label"])));
        var cache = item["children"]![1]!;
        Assert.Equal("Exited · Container", Samples.S(cache["description"]));
        Assert.Equal("cache (Container)\nState: Exited\nExit code: 1", Samples.S(cache["tooltip"]));
        Assert.Equal("error", Samples.S(cache["icon"]));
        var web = item["children"]![2]!;
        Assert.Equal(["restart", "stop", "logs"], web["actions"]!.AsArray().Select(a => Samples.S(a!["id"])));
        Assert.Equal("https://localhost:7001", Samples.S(web["children"]![0]!["description"]));
        Assert.Equal($"url:https://localhost:7001\nres:{project}\nwebfrontend", Samples.S(web["children"]![0]!["id"]));
        Assert.Equal(1, tree["badge"]!.GetValue<int>());
        Assert.Contains(calls, c => c.SequenceEqual(["describe", "--format", "json", "--apphost", project]));

        module.Send(new() { ["type"] = "treeAction", ["view"] = "aspire", ["item"] = $"dash:{project}", ["action"] = "click" });
        module.Send(new() { ["type"] = "treeAction", ["view"] = "aspire", ["item"] = $"url:https://localhost:7001\nres:{project}\nwebfrontend", ["action"] = "click" });
        await WaitFor(() => opened.Count == 2);
        Assert.Equal(["https://localhost:17043/login?t=abc", "https://localhost:7001"], opened.Order(StringComparer.Ordinal));
    }

    [Fact]
    public async Task Running_apphost_outside_the_workspace_is_listed()
    {
        ps = """[{"appHostPath": "/elsewhere/apphost.ts", "appHostPid": 7}]""";
        await using var module = Start();
        module.Activate(Directory.CreateDirectory(Path.Combine(config, "empty")).FullName);
        var tree = await Tree(module, Refreshed);
        var host = Hosts(tree)[0]!;
        Assert.Equal("elsewhere", Samples.S(host["label"]));
        Assert.Equal("Waiting for resources…", Samples.S(host["children"]![0]!["label"]));
    }

    [Fact]
    public async Task No_apphost_says_how_to_make_one()
    {
        await using var module = Start();
        module.Activate(Directory.CreateDirectory(Path.Combine(config, "empty")).FullName);
        var tree = await Tree(module, Refreshed);
        Assert.Equal("note:none", Samples.S(Hosts(tree)[0]!["id"]));
        Assert.Equal(0, tree["items"]![0]!["count"]!.GetValue<int>());
    }

    [Fact]
    public async Task Missing_cli_shows_setup()
    {
        cli = null;
        await using var module = Start();
        module.Activate(config);
        var tree = await Tree(module, Refreshed);
        Assert.Equal("SETUP", Samples.S(tree["items"]![0]!["label"]));
        Assert.Contains("isn't installed", Samples.S(tree["message"]));
        Assert.Empty(calls);

        module.Send(new() { ["type"] = "treeAction", ["view"] = "aspire", ["item"] = "setup:install", ["action"] = "install" });
        var terminal = await module.Next("runInTerminal");
        Assert.Equal(OperatingSystem.IsWindows() ? AspireModule.InstallScriptWindows : AspireModule.InstallScript, Samples.S(terminal["command"]));
        Assert.Null(terminal["saveAll"]);
    }

    [Fact]
    public async Task A_failing_ps_is_shown_as_the_views_error()
    {
        await using var module = ModuleHarness.Start(new AspireModule(_ => Cli, (_, _, _, _) => throw new AspireException("boom"), _ => { }, TimeSpan.FromMinutes(10)), ModuleDir);
        module.Activate(config);
        var tree = await Tree(module, Refreshed);
        Assert.Equal("Couldn't list the running AppHosts.", Samples.S(tree["message"]));
        Assert.Equal("boom", Samples.S(tree["error"]));
    }

    [Fact]
    public async Task Resource_action_runs_the_cli_command()
    {
        await using var module = Start();
        module.Send(new() { ["type"] = "treeAction", ["view"] = "aspire", ["item"] = "res:/a/AppHost.csproj\ncache-xyz", ["action"] = "restart" });
        await module.Next("log", m => Samples.S(m["text"]) == "aspire restart cache-xyz: resource restarted");
        Assert.Equal(["resource", "cache-xyz", "restart", "--non-interactive", "--apphost", "/a/AppHost.csproj"], calls.First());
        // While it runs the view says so.
        Assert.Contains(module.Sent, m => m["message"]?.GetValue<string>() == "aspire restart cache-xyz…");
    }

    [Fact]
    public async Task Run_opens_a_terminal()
    {
        await using var module = Start();
        module.Send(new() { ["type"] = "treeAction", ["view"] = "aspire", ["item"] = $"host:{Samples.ShopPath}", ["action"] = "run" });
        var message = await module.Next("runInTerminal");
        Assert.Equal("aspire run: Shop.AppHost", Samples.S(message["title"]));
        Assert.Equal("/usr/bin/aspire run --apphost /src/Shop/Shop.AppHost/Shop.AppHost.csproj", Samples.S(message["command"]));
        Assert.Equal("/src/Shop/Shop.AppHost", Samples.S(message["cwd"]));
        Assert.True(message["saveAll"]!.GetValue<bool>());
    }

    [Fact]
    public async Task Commands_for_one_apphost_pick_it_when_there_are_several()
    {
        var shop = Path.Combine(config, "ws", "Shop.AppHost", "Shop.AppHost.csproj");
        var other = Path.Combine(config, "ws", "other", "apphost.ts");
        Samples.Write(shop, Samples.ApphostCsproj);
        Samples.Write(other, "import aspire");
        ps = Samples.PsOutput(shop);
        await using var module = Start();
        module.Activate(Path.Combine(config, "ws"));
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "aspire.add" });
        var pick = await module.Next("quickPick");
        Assert.Equal("Choose an AppHost", Samples.S(pick["placeholder"]));
        var items = pick["items"]!.AsArray();
        Assert.Equal(["Shop.AppHost", "other"], items.Select(i => Samples.S(i!["label"])));
        Assert.Equal(["running", ""], items.Select(i => Samples.S(i!["hint"])));
        module.Send(new() { ["type"] = "quickPickResult", ["requestId"] = pick["requestId"]!.DeepClone(), ["value"] = other });
        var terminal = await module.Next("runInTerminal");
        Assert.Equal("aspire add: other", Samples.S(terminal["title"]));
        Assert.Equal(AspireWorkspace.CommandLine([Cli, "add", "--apphost", other]), Samples.S(terminal["command"]));

        // Stopping offers the running one only, so there's nothing to pick.
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "aspire.stop" });
        await module.Next("log", m => Samples.S(m["text"]).StartsWith("aspire stop Shop.AppHost:"));
        Assert.Contains(calls, c => c.SequenceEqual(["stop", "--apphost", shop]));
    }

    [Fact]
    public async Task Commands_without_an_apphost_say_so()
    {
        await using var module = Start();
        module.Activate(Directory.CreateDirectory(Path.Combine(config, "empty")).FullName);
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "aspire.run" });
        await module.Next("log", m => Samples.S(m["text"]).StartsWith("aspire: no AppHost in this folder."));
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "aspire.openDashboard" });
        await module.Next("log", m => Samples.S(m["text"]) == "aspire: no running AppHost.");
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "aspire.new" });
        var terminal = await module.Next("runInTerminal");
        Assert.Equal("/usr/bin/aspire new", Samples.S(terminal["command"]));
        Assert.Equal(Path.Combine(config, "empty"), Samples.S(terminal["cwd"]));
    }

    static async Task WaitFor(Func<bool> condition)
    {
        for (var i = 0; i < 300 && !condition(); i++)
        {
            await Task.Delay(20);
        }
        Assert.True(condition());
    }
}

public sealed class ManifestTests
{
    [Fact]
    public void Manifest()
    {
        JsonObject? manifest = null;
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir != null && manifest == null; dir = dir.Parent)
        {
            var file = Path.Combine(dir.FullName, "module.json");
            if (File.Exists(file))
            {
                manifest = JsonNode.Parse(File.ReadAllText(file), documentOptions: new() { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true })!.AsObject();
            }
        }
        Assert.NotNull(manifest);
        Assert.Equal("byteforge.aspire", Samples.S(manifest["id"]));
        Assert.Equal("dotnet", Samples.S(manifest["runtime"]));
        Assert.Equal("bin/Aspire.dll", Samples.S(manifest["assembly"]));
        Assert.Null(manifest["command"]);
        Assert.Equal(AspireModule.View, Samples.S(manifest["views"]![0]!["id"]));
        Assert.Equal("aspire.cliPath", Samples.S(manifest["settings"]![0]!["key"]));
    }
}
