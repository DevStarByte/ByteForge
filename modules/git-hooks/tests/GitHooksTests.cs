using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;
using ByteForge.Modules.Testing;

namespace ByteForge.GitHooks.Tests;

/// <summary>A test that needs git on PATH (skipped without it).</summary>
public sealed class GitFactAttribute : FactAttribute
{
    public GitFactAttribute()
    {
        if (Programs.Which("git") == null)
        {
            Skip = "git isn't on PATH";
        }
    }
}

/// <summary>A test that runs scripts with sh (Git for Windows' on Windows; skipped without one).</summary>
public sealed class ShFactAttribute : FactAttribute
{
    public ShFactAttribute()
    {
        if (Programs.Which("sh") == null)
        {
            Skip = "sh isn't on PATH";
        }
    }
}

/// <summary>A test of the executable bit, which Windows doesn't have.</summary>
public sealed class UnixFactAttribute : FactAttribute
{
    public UnixFactAttribute()
    {
        if (OperatingSystem.IsWindows())
        {
            Skip = "no executable bit on Windows";
        }
    }
}

/// <summary>git as a test sets it up: a repository (or not), core.hooksPath, remotes and HEAD; records the calls.</summary>
public sealed class FakeGit
{
    public bool Repository = true;
    public string? HooksPath;
    public Dictionary<string, string> Remotes = [];
    public string Head = "0123456789abcdef0123456789abcdef01234567";
    public List<string> Calls = [];

    public Task<string?> Run(string root, params string[] args)
    {
        var line = string.Join(" ", args);
        lock (Calls)
        {
            Calls.Add(line);
        }
        if (!Repository)
        {
            return Task.FromResult<string?>(null);
        }
        return Task.FromResult(line switch
        {
            "rev-parse --git-path hooks" => HooksPath ?? ".git/hooks",
            "config core.hooksPath" => HooksPath,
            "remote" => string.Join("\n", Remotes.Keys),
            "rev-parse HEAD" => Head,
            _ when args is ["remote", "get-url", var name] => Remotes.GetValueOrDefault(name),
            _ => null,
        });
    }
}

public sealed class GitHooksTests : IDisposable
{
    /// <summary>git without the user's global and system config (hooksPath, signing), for this test process.</summary>
    static GitHooksTests()
    {
        var empty = Path.Combine(Path.GetTempPath(), "bf-git-hooks-empty-gitconfig");
        File.WriteAllText(empty, "");
        Environment.SetEnvironmentVariable("GIT_CONFIG_GLOBAL", empty);
        Environment.SetEnvironmentVariable("GIT_CONFIG_NOSYSTEM", "1");
    }

    /// <summary>A config folder laid out like ByteForge's: the module in &lt;tmp&gt;/modules/git-hooks; the repository beside it.</summary>
    readonly string tmp = Directory.CreateTempSubdirectory("bf-git-hooks-").FullName;
    readonly FakeGit fake = new();

    string ModuleDir => Path.Combine(tmp, "modules", "git-hooks");
    string Root => Path.Combine(tmp, "repo");
    string GitHooksDir => Path.Combine(Root, ".git", "hooks");

    public GitHooksTests()
    {
        Directory.CreateDirectory(GitHooksDir);
    }

    public void Dispose()
    {
        try
        {
            Directory.Delete(tmp, recursive: true);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
        }
    }

    static string S(JsonNode? node) => node!.GetValue<string>();

    /// <summary>The module on the fake git (or the real one), activated on the repository; its first tree is read.</summary>
    async Task<(ModuleHarness Module, JsonObject Tree)> Start(bool realGit = false, string? root = null)
    {
        var module = ModuleHarness.Start(realGit ? new GitHooksModule() : new GitHooksModule(fake.Run), ModuleDir);
        module.Activate(root ?? Root);
        return (module, await module.Next("setTree"));
    }

    static async Task<JsonObject> Refresh(ModuleHarness module)
    {
        module.Send(new() { ["type"] = "treeAction", ["view"] = Hooks.View, ["item"] = null, ["action"] = "refresh" });
        return await module.Next("setTree");
    }

    static void TreeAction(ModuleHarness module, string item, string action) =>
        module.Send(new() { ["type"] = "treeAction", ["view"] = Hooks.View, ["item"] = item, ["action"] = action });

    static void Command(ModuleHarness module, string id) => module.Send(new() { ["type"] = "executeCommand", ["id"] = id });

    /// <summary>Answers the next quick pick with <paramref name="value"/>; the pick.</summary>
    static async Task<JsonObject> Pick(ModuleHarness module, string value)
    {
        var pick = await module.Next("quickPick");
        module.Send(new() { ["type"] = "quickPickResult", ["requestId"] = pick["requestId"]!.DeepClone(), ["value"] = value });
        return pick;
    }

    static int TemplateIndex(string name) => Array.FindIndex(Hooks.Templates, t => t.Name == name);

    static async Task<ProcessResult> Git(string root, params string[] args)
    {
        var result = await Programs.RunAsync(Programs.Which("git")!, args, workingDirectory: root);
        return result;
    }

    async Task InitRepository()
    {
        Directory.Delete(Path.Combine(Root, ".git"), recursive: true);
        Assert.True((await Git(Root, "init", "-q", "-b", "main")).Success);
        await Git(Root, "config", "user.email", "t@example.invalid");
        await Git(Root, "config", "user.name", "T");
        await Git(Root, "config", "commit.gpgsign", "false");
    }

    /// <summary>A command line as a terminal would run it (Python's shell=True): cmd on Windows, sh elsewhere.</summary>
    static async Task<(int Code, string Output)> Shell(string command)
    {
        var start = OperatingSystem.IsWindows()
            ? new ProcessStartInfo("cmd.exe", "/d /s /c \"" + command + "\"")
            : new ProcessStartInfo("/bin/sh") { ArgumentList = { "-c", command } };
        start.RedirectStandardOutput = true;
        start.RedirectStandardInput = true;
        start.UseShellExecute = false;
        using var process = Process.Start(start)!;
        process.StandardInput.Close();
        var output = await process.StandardOutput.ReadToEndAsync();
        await process.WaitForExitAsync();
        return (process.ExitCode, output);
    }

    // ───────────── the Python's tests ─────────────

    [Fact]
    public async Task Samples_are_left_out()
    {
        File.WriteAllText(Path.Combine(GitHooksDir, "pre-commit.sample"), "#!/bin/sh\n");
        File.WriteAllText(Path.Combine(GitHooksDir, "not-a-hook"), "#!/bin/sh\n");
        var (module, tree) = await Start();
        await using var _ = module;
        var hooks = tree["items"]![0]!;
        Assert.Equal(0, hooks["count"]!.GetValue<int>());
        Assert.Equal("Hooks in .git/hooks", S(hooks["label"]));
        Assert.Equal("none", S(hooks["children"]![0]!["id"]));
    }

    [GitFact]
    public async Task New_hook_from_template_works_with_git()
    {
        await InitRepository();
        var (module, _) = await Start(realGit: true);
        await using var __ = module;
        Command(module, "gitHooks.new");
        await Pick(module, TemplateIndex("commit-msg").ToString(System.Globalization.CultureInfo.InvariantCulture));
        var path = Path.Combine(GitHooksDir, "commit-msg");
        var open = await module.Next("openFile");
        Assert.Equal(path, S(open["path"]));
        Assert.True(Hooks.IsExecutable(path));
        File.WriteAllText(Path.Combine(Root, "a.txt"), "a");
        await Git(Root, "add", "a.txt");
        var bad = await Git(Root, "commit", "-q", "-m", "did stuff");
        Assert.NotEqual(0, bad.ExitCode);
        Assert.Contains("Conventional Commits", bad.Error);
        Assert.Equal(0, (await Git(Root, "commit", "-q", "-m", "feat: stuff")).ExitCode);
    }

    [Fact]
    public async Task Turn_off_and_on()
    {
        var path = Hooks.Write(GitHooksDir, "pre-commit", "exit 1\n");
        var (module, tree) = await Start();
        await using var _ = module;
        Assert.Equal("play", S(tree["items"]![0]!["children"]![0]!["icon"]));
        TreeAction(module, "hook:pre-commit", "toggle");
        Assert.Equal("git-hooks: pre-commit is off", S((await module.Next("log"))["text"]));
        var row = (await module.Next("setTree"))["items"]![0]!["children"]![0]!;
        Assert.True(File.Exists(path + Hooks.Off));
        Assert.False(File.Exists(path));
        Assert.Equal("off", S(row["description"]));
        Assert.Equal("pause", S(row["icon"]));
        Assert.Equal("foreground.muted", S(row["iconColor"]));
        Assert.Contains("Turn On", row["menu"]!.AsArray().Select(m => m!["label"]?.GetValue<string>()));
        TreeAction(module, "hook:pre-commit", "toggle");
        Assert.Equal("git-hooks: pre-commit is on", S((await module.Next("log"))["text"]));
        await module.Next("setTree");
        Assert.True(File.Exists(path));
        Assert.False(File.Exists(path + Hooks.Off));
    }

    [GitFact]
    public async Task Git_ignores_a_hook_turned_off()
    {
        await InitRepository();
        Hooks.Write(GitHooksDir, "pre-commit", "exit 1\n");
        var (module, _) = await Start(realGit: true);
        await using var __ = module;
        File.WriteAllText(Path.Combine(Root, "b.txt"), "b");
        await Git(Root, "add", "b.txt");
        Assert.NotEqual(0, (await Git(Root, "commit", "-q", "-m", "x")).ExitCode);
        TreeAction(module, "hook:pre-commit", "toggle");
        await module.Next("setTree");
        Assert.Equal(0, (await Git(Root, "commit", "-q", "-m", "x")).ExitCode);
    }

    [UnixFact]
    public async Task Not_executable_is_marked()
    {
        var path = Path.Combine(GitHooksDir, "pre-push");
        File.WriteAllText(path, "#!/bin/sh\nexit 0\n");
        if (!OperatingSystem.IsWindows())
        {
            File.SetUnixFileMode(path, UnixFileMode.UserRead | UnixFileMode.UserWrite);
        }
        var (module, tree) = await Start();
        await using var _ = module;
        var row = tree["items"]![0]!["children"]![0]!;
        Assert.Contains("not executable", S(row["description"]));
        Assert.Contains("Make Executable", row["menu"]!.AsArray().Select(m => m!["label"]?.GetValue<string>()));
        TreeAction(module, "hook:pre-push", "chmod");
        row = (await module.Next("setTree"))["items"]![0]!["children"]![0]!;
        Assert.True(Hooks.IsExecutable(path));
        Assert.Equal(Hooks.Describe("pre-push"), S(row["description"]));
        Assert.DoesNotContain("Make Executable", row["menu"]!.AsArray().Select(m => m!["label"]?.GetValue<string>()));
    }

    [Fact]
    public async Task Core_hooks_path()
    {
        fake.HooksPath = ".husky";
        var (module, tree) = await Start();
        await using var _ = module;
        Assert.Equal("core.hooksPath is .husky, which doesn't exist yet.", S(tree["message"]));
        Hooks.Write(Path.Combine(Root, ".husky"), "pre-commit", "exit 0\n");
        tree = await Refresh(module);
        Assert.Equal("Hooks in .husky (core.hooksPath)", S(tree["items"]![0]!["label"]));
        Assert.Equal("pre-commit", S(tree["items"]![0]!["children"]![0]!["label"]));
        Assert.Null(tree["message"]);
        Assert.True(tree.ContainsKey("message"));
    }

    [GitFact]
    public async Task Core_hooks_path_with_git()
    {
        await InitRepository();
        await Git(Root, "config", "core.hooksPath", ".husky");
        var (folder, configured) = await Hooks.HooksDir(Root, Hooks.RunGit);
        Assert.Equal(Path.Combine(Root, ".husky"), folder);
        Assert.Equal(".husky", configured);
    }

    [Fact]
    public async Task Run_command_passes_git_arguments()
    {
        Hooks.Write(GitHooksDir, "commit-msg", "echo \"$1\"\n");
        Hooks.Write(GitHooksDir, "pre-push", "echo \"$1 $2\"\n");
        Hooks.Write(GitHooksDir, "prepare-commit-msg", "echo \"$2\"\n");
        Hooks.Write(GitHooksDir, "post-checkout", "echo \"$1 $2 $3\"\n");
        Hooks.Write(GitHooksDir, "post-merge", "echo \"$1\"\n");
        fake.Remotes["origin"] = "https://github.com/o/r.git";
        var (module, _) = await Start();
        await using var __ = module;
        var commands = new Dictionary<string, string>();
        foreach (var name in new[] { "commit-msg", "pre-push", "prepare-commit-msg", "post-checkout", "post-merge" })
        {
            TreeAction(module, $"hook:{name}", "run");
            var run = await module.Next("runInTerminal");
            Assert.Equal($"hook: {name}", S(run["title"]));
            Assert.Equal(Root, S(run["cwd"]));
            Assert.True(run["saveAll"]!.GetValue<bool>());
            commands[name] = S(run["command"]);
        }
        Assert.Matches("commit-msg.* .*byteforge-commit-msg-", commands["commit-msg"]);
        var path = Hooks.Quote(Path.Combine(GitHooksDir, "pre-push"));
        Assert.Equal(OperatingSystem.IsWindows() ? $"sh {path} origin https://github.com/o/r.git" : $"{path} origin https://github.com/o/r.git < /dev/null", commands["pre-push"]);
        Assert.EndsWith(".txt message", commands["prepare-commit-msg"]);
        Assert.EndsWith($" {fake.Head} {fake.Head} 1", commands["post-checkout"]);
        Assert.EndsWith(" 0", commands["post-merge"]);
        if (Programs.Which("sh") == null)
        {
            return;  // running them needs sh
        }
        var (code, output) = await Shell(commands["commit-msg"]);
        Assert.Equal(0, code);
        Assert.Contains("feat:", File.ReadAllText(output.Trim()));
        Assert.Equal("origin https://github.com/o/r.git", (await Shell(commands["pre-push"])).Output.Trim());
        Assert.Equal($"{fake.Head} {fake.Head} 1", (await Shell(commands["post-checkout"])).Output.Trim());
    }

    [Fact]
    public async Task Pre_push_without_origin()
    {
        Hooks.Write(GitHooksDir, "pre-push", "exit 0\n");
        fake.Remotes["upstream"] = "https://example.invalid/u.git";
        var (module, _) = await Start();
        await using var __ = module;
        TreeAction(module, "hook:pre-push", "run");
        var command = S((await module.Next("runInTerminal"))["command"]);
        Assert.EndsWith(OperatingSystem.IsWindows() ? " origin " : " origin '' < /dev/null", command);
        Assert.DoesNotContain(fake.Calls, c => c.StartsWith("remote get-url", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Replace_asks_first()
    {
        Hooks.Write(GitHooksDir, "pre-push", "exit 0\n");
        var (module, _) = await Start();
        await using var __ = module;
        Command(module, "gitHooks.new");
        var pick = await Pick(module, TemplateIndex("pre-push").ToString(System.Globalization.CultureInfo.InvariantCulture));
        var item = pick["items"]!.AsArray().First(i => S(i!["value"]) == TemplateIndex("pre-push").ToString(System.Globalization.CultureInfo.InvariantCulture))!;
        Assert.Equal("pre-push  (replaces the current one)", S(item["detail"]));
        Assert.DoesNotContain(pick["items"]!.AsArray(), i => S(i!["value"]) == "empty:pre-push");
        Assert.Contains(pick["items"]!.AsArray(), i => S(i!["value"]) == "empty:pre-commit");
        var prompt = await module.Next("prompt");
        Assert.Equal("Replace the pre-push hook?", S(prompt["message"]));
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 0 });
        await module.Next("openFile");
        Assert.Contains("dotnet test", File.ReadAllText(Path.Combine(GitHooksDir, "pre-push")));
    }

    [Fact]
    public async Task Writing_over_a_hook_turned_off_removes_the_off_file()
    {
        Hooks.Write(GitHooksDir, "pre-commit", "exit 1\n");
        File.Move(Path.Combine(GitHooksDir, "pre-commit"), Path.Combine(GitHooksDir, "pre-commit.off"));
        var (module, _) = await Start();
        await using var __ = module;
        Command(module, "gitHooks.new");
        await Pick(module, "empty:pre-commit");
        var prompt = await module.Next("prompt");
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 0 });
        await module.Next("openFile");
        Assert.False(File.Exists(Path.Combine(GitHooksDir, "pre-commit.off")));
        Assert.EndsWith(Hooks.Templates[^1].Body, File.ReadAllText(Path.Combine(GitHooksDir, "pre-commit")));
    }

    [Fact]
    public async Task Delete_asks_first()
    {
        var path = Hooks.Write(GitHooksDir, "post-merge", "exit 0\n");
        var (module, _) = await Start();
        await using var __ = module;
        TreeAction(module, "hook:post-merge", "delete");
        var prompt = await module.Next("prompt");
        Assert.Equal("Delete the post-merge hook?", S(prompt["message"]));
        Assert.Equal(path, S(prompt["detail"]));
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 0 });
        Assert.Equal($"git-hooks: deleted {path}", S((await module.Next("log"))["text"]));
        Assert.False(File.Exists(path));
    }

    [Fact]
    public async Task Husky_tasks()
    {
        Directory.CreateDirectory(Path.Combine(Root, ".husky"));
        File.WriteAllText(Path.Combine(Root, ".husky", "task-runner.json"),
            "{\n  // comment\n  \"tasks\": [\n"
            + "    {\"name\": \"format\", \"group\": \"pre-commit\", \"command\": \"dotnet\", \"args\": [\"format\", \"--verify-no-changes\"]},\n"
            + "    {\"name\": \"lint\", \"command\": \"npm\", \"args\": [\"run\", \"lint\"]},\n  ]\n}\n");
        var (module, tree) = await Start();
        await using var _ = module;
        var husky = tree["items"]![1]!;
        Assert.Equal(2, husky["count"]!.GetValue<int>());
        Assert.Equal(["pre-commit", "(no group: run by name)"], husky["children"]!.AsArray().Select(g => S(g!["label"])));
        Assert.Equal("dotnet format --verify-no-changes", S(husky["children"]![0]!["children"]![0]!["description"]));
        TreeAction(module, "husky:0", "click");
        var run = await module.Next("runInTerminal");
        Assert.Equal("dotnet husky run --name format", S(run["command"]));
        Assert.Equal("husky: format", S(run["title"]));
        TreeAction(module, "husky", "huskyOpen");
        Assert.Equal(Path.Combine(Root, ".husky", "task-runner.json"), S((await module.Next("openFile"))["path"]));
    }

    [Fact]
    public async Task Not_a_repository()
    {
        fake.Repository = false;
        var (module, tree) = await Start();
        await using var _ = module;
        Assert.Contains("isn't a git repository", S(tree["message"]));
        Command(module, "gitHooks.new");
        Assert.Equal("git-hooks: open a git repository first.", S((await module.Next("log"))["text"]));
        Command(module, "gitHooks.run");
        Assert.Equal("git-hooks: no active hooks in this repository.", S((await module.Next("log"))["text"]));
    }

    [Fact]
    public async Task No_folder()
    {
        await using var module = ModuleHarness.Start(new GitHooksModule(fake.Run), ModuleDir);
        module.Activate(null);
        Assert.Equal("Open a folder to see its git hooks.", S((await module.Next("setTree"))["message"]));
        Assert.Empty(fake.Calls);
    }

    // ───────────── commands, files, parsing ─────────────

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = ModuleHarness.Start(new GitHooksModule(fake.Run), ModuleDir);
        foreach (var (id, title) in new[] { ("gitHooks.new", "New Hook…"), ("gitHooks.run", "Run Hook…"), ("gitHooks.show", "Show Hooks") })
        {
            var command = await module.Next("registerCommand", m => S(m["id"]) == id);
            Assert.Equal(title, S(command["title"]));
            Assert.Equal("Git Hooks", S(command["category"]));
        }
        Command(module, "gitHooks.show");
        Assert.Equal(Hooks.View, S((await module.Next("showView"))["id"]));
    }

    [Fact]
    public async Task Run_hook_command_picks_an_active_hook()
    {
        Hooks.Write(GitHooksDir, "post-merge", "exit 0\n");
        Hooks.Write(GitHooksDir, "pre-commit", "exit 0\n");
        File.Move(Path.Combine(GitHooksDir, "post-merge"), Path.Combine(GitHooksDir, "post-merge.off"));
        var (module, _) = await Start();
        await using var __ = module;
        Command(module, "gitHooks.run");
        var pick = await Pick(module, "pre-commit");
        Assert.Equal("Run a git hook", S(pick["placeholder"]));
        Assert.Equal(["pre-commit"], pick["items"]!.AsArray().Select(i => S(i!["value"])));
        Assert.Equal("hook: pre-commit", S((await module.Next("runInTerminal"))["title"]));
    }

    [Fact]
    public async Task Hooks_are_listed_in_the_order_git_meets_them()
    {
        foreach (var name in new[] { "pre-push", "commit-msg", "pre-commit" })
        {
            Hooks.Write(GitHooksDir, name, "exit 0\n");
        }
        var (module, tree) = await Start();
        await using var _ = module;
        Assert.Equal(["pre-commit", "commit-msg", "pre-push"], tree["items"]![0]!["children"]!.AsArray().Select(r => S(r!["label"])));
        Assert.Equal(3, tree["items"]![0]!["count"]!.GetValue<int>());
        Assert.Single(tree["items"]!.AsArray());  // no .husky/task-runner.json
    }

    [Fact]
    public void Hook_files_are_written_as_the_python_wrote_them()
    {
        var path = Hooks.Write(Path.Combine(tmp, "new", "hooks"), "pre-push", "exit 0\n");
        var expected = "#!/bin/sh\n# git pre-push hook: before a push; fails the push when it exits non-zero.\n# Written by ByteForge's Git Hooks module.\n\nexit 0\n";
        Assert.Equal(Encoding.UTF8.GetBytes(expected), File.ReadAllBytes(path));
        var post = File.ReadAllBytes(Hooks.Write(Path.Combine(tmp, "new", "hooks"), "post-merge", Hooks.Templates[TemplateIndex("post-merge")].Body));
        Assert.DoesNotContain((byte)'\r', post);
        Assert.Contains("restoring…", Encoding.UTF8.GetString(post));
        Assert.NotEqual(0xEF, post[0]);
    }

    [UnixFact]
    public void Writing_a_hook_adds_the_executable_bits_to_its_mode()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }
        var path = Path.Combine(GitHooksDir, "pre-commit");
        File.WriteAllText(path, "old");
        File.SetUnixFileMode(path, UnixFileMode.UserRead | UnixFileMode.UserWrite);
        Hooks.Write(GitHooksDir, "pre-commit", "exit 0\n");
        Assert.Equal((UnixFileMode)Convert.ToInt32("711", 8), File.GetUnixFileMode(path));
        var fresh = Hooks.Write(GitHooksDir, "pre-push", "exit 0\n");
        var mode = File.GetUnixFileMode(fresh);
        Assert.Equal(UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute, mode & (UnixFileMode)Convert.ToInt32("111", 8));
        Assert.True(mode.HasFlag(UnixFileMode.UserRead | UnixFileMode.UserWrite));
    }

    [Fact]
    public void Json_comments_and_trailing_commas()
    {
        Assert.Equal("{\"a\": \"// not a comment\",  \"b\": [1, 2]\n}",
            Hooks.StripJsonComments("{\"a\": \"// not a comment\", /* x\ny */ \"b\": [1, 2,]// c\n,}"));
    }

    [Fact]
    public void Husky_tasks_as_python_read_them()
    {
        Directory.CreateDirectory(Path.Combine(Root, ".husky"));
        var file = Path.Combine(Root, ".husky", "task-runner.json");
        Assert.Null(Hooks.HuskyTasks(Root));
        File.WriteAllText(file, "﻿{\"tasks\": [{\"command\": \"echo\", \"args\": [1, true, null, 1.5, \"x y\"]}, {\"name\": \"no command\"}, 3, {\"command\": \"b\", \"name\": \"\", \"group\": \"g\"}]}");
        var tasks = Hooks.HuskyTasks(Root)!;
        Assert.Equal([0, 3], tasks.Select(t => t.Index));
        Assert.Equal("echo", tasks[0].Name);
        Assert.Equal(["1", "True", "None", "1.5", "x y"], tasks[0].Args);
        Assert.Equal(("b", "g"), (tasks[1].Name, tasks[1].Group));
        File.WriteAllText(file, "{ broken");
        Assert.Null(Hooks.HuskyTasks(Root));
    }

    [Fact]
    public void Paths_normalized_as_python_without_expanding_short_names()
    {
        var s = Path.DirectorySeparatorChar;
        Assert.Equal($"{s}a{s}c", Hooks.NormPath("/a/b/../c"));
        Assert.Equal($"{s}a{s}b{s}c", Hooks.NormPath("/a/./b//c/"));
        Assert.Equal($"..{s}b", Hooks.NormPath("a/../../b"));
        Assert.Equal($"{s}x", Hooks.NormPath("/../x"));
        Assert.Equal("rel", Hooks.NormPath("rel/./x/.."));
        Assert.Equal(".", Hooks.NormPath("x/.."));
        if (OperatingSystem.IsWindows())
        {
            Assert.Equal(@"C:\Users\ABCDEF~1\repo\.git\hooks", Hooks.NormPath("C:/Users/ABCDEF~1/repo/.git/hooks"));
            Assert.Equal(@"\\server\share\y", Hooks.NormPath(@"\\server\share\x\..\y"));
        }
        else
        {
            Assert.Equal("//srv/y", Hooks.NormPath("//srv/x/../y"));
        }
    }

    [Fact]
    public void Quote_as_python()
    {
        if (OperatingSystem.IsWindows())
        {
            Assert.Equal("plain", Hooks.Quote("plain"));
            Assert.Equal("\"a b\"", Hooks.Quote("a b"));
            Assert.Equal("\"a\\\"b\"", Hooks.Quote("a\"b"));
            Assert.Equal("", Hooks.Quote(""));
        }
        else
        {
            Assert.Equal("plain/path-1.txt", Hooks.Quote("plain/path-1.txt"));
            Assert.Equal("'a b'", Hooks.Quote("a b"));
            Assert.Equal("'it'\"'\"'s'", Hooks.Quote("it's"));
            Assert.Equal("''", Hooks.Quote(""));
        }
    }

    [Fact]
    public void Sample_message_file()
    {
        var path = Hooks.SampleMessage();
        try
        {
            Assert.Matches("^byteforge-commit-msg-[a-z0-9_]{8}\\.txt$", Path.GetFileName(path));
            Assert.Equal("feat: try the commit-msg hook from ByteForge" + Environment.NewLine, File.ReadAllText(path));
            if (!OperatingSystem.IsWindows())
            {
                Assert.Equal(UnixFileMode.UserRead | UnixFileMode.UserWrite, File.GetUnixFileMode(path));
            }
        }
        finally
        {
            File.Delete(path);
        }
    }

    // ───────────── templates (the Python's TemplateTests) ─────────────

    [ShFact]
    public async Task Templates_are_valid_sh()
    {
        foreach (var (name, title, body) in Hooks.Templates)
        {
            var result = await Programs.RunAsync(Programs.Which("sh")!, ["-n"], input: body);
            Assert.True(result.Success, $"{title}: {result.Error}");
            Assert.True(name == null || Hooks.IsHook(name));
        }
    }

    [ShFact]
    public async Task Conventional_commits_check()
    {
        var body = Hooks.Templates.First(t => t.Name == "commit-msg").Body;
        var path = Path.Combine(tmp, "message.txt");
        foreach (var (message, ok) in new[] { ("feat(api): add export", true), ("fix!: breaking", true), ("update stuff", false) })
        {
            File.WriteAllText(path, message + "\n");
            var result = await Programs.RunAsync(Programs.Which("sh")!, ["-c", body, "hook", path]);
            Assert.True(result.Success == ok, message);
        }
    }

    [Fact]
    public void Manifest()
    {
        var text = File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "..", "..", "module.json"));
        var manifest = JsonNode.Parse(Regex.Replace(text, @"(?m)^\s*//.*$", ""))!;
        Assert.Equal(Hooks.View, S(manifest["views"]![0]!["id"]));
        Assert.Equal("dotnet", S(manifest["runtime"]));
        Assert.Equal("bin/GitHooks.dll", S(manifest["assembly"]));
        Assert.Null(manifest["command"]);
        Assert.Equal(JsonValueKind.Array, manifest["requires"]!.GetValueKind());
    }
}
