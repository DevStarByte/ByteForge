using System.Globalization;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.GithubActions;
using ByteForge.Modules.Testing;

namespace ByteForge.GithubActions.Tests;

/// <summary>GitHub's answers for the repository o/r, without a network: what each request asks for is recorded.</summary>
sealed class FakeApi(string? token = null) : GitHubApi(token)
{
    public readonly List<(string Method, string Path, JsonNode? Body)> Calls = [];

    public readonly List<JsonObject> Runs =
    [
        Data.Run(3, status: "in_progress", conclusion: null, number: 3, minutesAgo: 1),
        Data.Run(2, conclusion: "failure", number: 2, minutesAgo: 30),
        Data.Run(1, name: "Release", branch: "v1.0", number: 1, minutesAgo: 600),
    ];

    public string Log = "";

    public override Task<JsonNode?> RequestAsync(string method, string path, JsonNode? body = null)
    {
        lock (Calls)
        {
            Calls.Add((method, path, body?.DeepClone()));
        }
        if (method == "POST")
        {
            if (!HasToken)
            {
                throw new ApiException(403, "needs a token");
            }
            return Task.FromResult<JsonNode?>(null);
        }
        return Task.FromResult<JsonNode?>(Answer(path));
    }

    JsonNode Answer(string path)
    {
        if (path.Contains("/pulls"))
        {
            return new JsonArray(Data.Pull(12, "feature/x", "Add export"), Data.Pull(7, "master", "Fix typo", fork: true, draft: true));
        }
        if (path.Contains("/issues"))
        {
            return JsonNode.Parse($$"""
                [{"number": 30, "title": "Crash on start!", "labels": [{"name": "bug"}], "assignees": [], "user": {"login": "a"}, "updated_at": "{{Data.Iso(5)}}", "comments": 2, "html_url": "https://i30"},
                 {"number": 31, "title": "Dark mode", "labels": [], "assignees": [{"login": "dev"}], "user": {"login": "b"}, "updated_at": "{{Data.Iso(50)}}", "comments": 0, "html_url": "https://i31"},
                 {"number": 12, "title": "Add export", "pull_request": {}, "labels": [], "assignees": []}]
                """)!;
        }
        if (path.Contains("/releases"))
        {
            return JsonNode.Parse($$"""
                [{"id": 9, "tag_name": "v1.1.2", "name": "1.1.2", "draft": false, "prerelease": false, "published_at": "{{Data.Iso(60)}}", "html_url": "https://r", "assets": [{"download_count": 5}, {"download_count": 2}]},
                 {"id": 8, "tag_name": "v1.2.0-beta.1", "name": "", "draft": false, "prerelease": true, "published_at": "{{Data.Iso(30)}}", "html_url": "https://r8", "assets": []}]
                """)!;
        }
        if (path == "/user")
        {
            return new JsonObject { ["login"] = "dev" };
        }
        if (path.Contains("/check-runs"))
        {
            return JsonNode.Parse($$"""
                {"check_runs": [
                  {"id": 501, "name": "tests", "status": "completed", "conclusion": "success", "app": {"slug": "github-actions"}, "started_at": "{{Data.Iso(5)}}", "completed_at": "{{Data.Iso(3)}}"},
                  {"id": 502, "name": "codecov", "status": "in_progress", "conclusion": null, "app": {"slug": "codecov"}, "details_url": "https://codecov", "started_at": "{{Data.Iso(2)}}", "completed_at": null}]}
                """)!;
        }
        if (path.Contains("/jobs"))
        {
            return JsonNode.Parse($$"""
                {"jobs": [{"id": 77, "name": "build", "status": "completed", "conclusion": "failure", "started_at": "{{Data.Iso(30)}}", "completed_at": "{{Data.Iso(28)}}", "html_url": "https://x",
                  "steps": [{"number": 1, "name": "Checkout", "status": "completed", "conclusion": "success", "started_at": "{{Data.Iso(30)}}", "completed_at": "{{Data.Iso(30)}}"},
                            {"number": 2, "name": "Test", "status": "completed", "conclusion": "failure", "started_at": "{{Data.Iso(30)}}", "completed_at": "{{Data.Iso(28)}}"}]}]}
                """)!;
        }
        if (path.Contains("/actions/workflows"))
        {
            return JsonNode.Parse("""
                {"workflows": [{"id": 41, "name": "CI", "path": ".github/workflows/ci.yml", "state": "active"},
                               {"id": 42, "name": "Nightly", "path": ".github/workflows/nightly.yml", "state": "active"},
                               {"id": 43, "name": "Old", "path": ".github/workflows/old.yml", "state": "disabled_manually"}]}
                """)!;
        }
        if (path.Contains("branch="))
        {
            var branch = path.Split("branch=")[1];
            return new JsonObject { ["workflow_runs"] = new JsonArray([.. Runs.Where(r => r["head_branch"]!.GetValue<string>() == branch).Select(r => r.DeepClone())]) };
        }
        return new JsonObject { ["workflow_runs"] = new JsonArray([.. Runs.Select(r => r.DeepClone())]) };
    }

    public override Task<string> DownloadAsync(string path)
    {
        lock (Calls)
        {
            Calls.Add(("DOWNLOAD", path, null));
        }
        return HasToken ? Task.FromResult(Log) : throw new ApiException(404, "not found");
    }
}

/// <summary>The module with GitHub, git, gh, the environment and the browser replaced.</summary>
sealed class TestModule(string? token = null, Dictionary<string, string?>? git = null) : GithubActionsModule
{
    public readonly List<string> Opened = [];
    public readonly List<(string Root, string Args)> GitCalls = [];
    public FakeApi Fake => (FakeApi)Api;

    protected override GitHubApi CreateApi(string? t) => new FakeApi(t);

    protected override Task<string?> GitAsync(string root, params string[] args)
    {
        lock (GitCalls)
        {
            GitCalls.Add((root, string.Join(" ", args)));
        }
        return Task.FromResult(git?.GetValueOrDefault(string.Join(" ", args)));
    }

    protected override Task<string?> GhTokenAsync() => Task.FromResult(token);

    protected override string? Environment(string name) => null;

    protected override void OpenBrowser(string url)
    {
        lock (Opened)
        {
            Opened.Add(url);
        }
    }

    protected override Task PauseAsync(TimeSpan time) => Task.CompletedTask;
}

static class Data
{
    public static string Iso(double minutesAgo) => (DateTimeOffset.UtcNow - TimeSpan.FromMinutes(minutesAgo)).ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", CultureInfo.InvariantCulture);

    public static JsonObject Run(long id, string name = "CI", string branch = "master", string status = "completed", string? conclusion = "success", int number = 1, double minutesAgo = 10) => new()
    {
        ["id"] = id,
        ["name"] = name,
        ["path"] = ".github/workflows/ci.yml",
        ["run_number"] = number,
        ["head_branch"] = branch,
        ["head_sha"] = "abcdef1234",
        ["event"] = "push",
        ["status"] = status,
        ["conclusion"] = conclusion,
        ["display_title"] = $"Commit {id}",
        ["created_at"] = Iso(minutesAgo),
        ["run_started_at"] = Iso(minutesAgo),
        ["updated_at"] = Iso(minutesAgo - 3),
        ["html_url"] = $"https://github.com/o/r/actions/runs/{id}",
        ["actor"] = new JsonObject { ["login"] = "dev" },
    };

    public static JsonObject Pull(long number, string @ref, string title, bool fork = false, bool draft = false) => new()
    {
        ["number"] = number,
        ["title"] = title,
        ["draft"] = draft,
        ["user"] = new JsonObject { ["login"] = "dev" },
        ["labels"] = new JsonArray(),
        ["head"] = new JsonObject { ["ref"] = @ref, ["sha"] = $"sha{number}", ["repo"] = new JsonObject { ["full_name"] = fork ? "someone/r" : "o/r" } },
        ["base"] = new JsonObject { ["ref"] = "master" },
        ["updated_at"] = Iso(20),
        ["html_url"] = $"https://github.com/o/r/pull/{number}",
    };
}

public sealed class GithubActionsTests : IDisposable
{
    readonly string config = Directory.CreateTempSubdirectory("bf-github-").FullName;

    string ModuleDir => Path.Combine(config, "modules", "github-actions");

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

    static TestModule Module(string? token = null, string branch = "master")
    {
        var git = new Dictionary<string, string?>
        {
            ["symbolic-ref --quiet --short HEAD"] = branch,
            ["remote"] = "origin",
            ["remote get-url origin"] = "https://github.com/o/r.git",
        };
        var module = new TestModule(token, git) { Root = "/x", Repo = new Repository("o", "r", branch) };
        module.Api = new FakeApi(token);
        return module;
    }

    /// <summary>The module running in a harness (so that it can send), started.</summary>
    async Task<ModuleHarness> Start(GithubActionsModule module)
    {
        var harness = ModuleHarness.Start(module, ModuleDir);
        await harness.Next("registerCommand", m => S(m["id"]) == "github.openRepository");
        return harness;
    }

    static string S(JsonNode? node) => node!.GetValue<string>();

    static List<string> Labels(JsonNode? items) => [.. items!.AsArray().Select(i => S(i!["label"]))];

    static JsonObject Item(JsonObject tree, string id) => tree["items"]!.AsArray().Select(i => i!.AsObject()).First(i => S(i["id"]) == id);

    static List<JsonObject> SentOf(ModuleHarness harness, string type)
    {
        lock (harness.Sent)
        {
            return [.. harness.Sent.Where(m => S(m["type"]) == type)];
        }
    }

    // ───────────── repository ─────────────

    [Fact]
    public void Parse_remote()
    {
        foreach (var url in new[] { "git@github.com:DevStarByte/ByteForge.git", "https://github.com/DevStarByte/ByteForge",
                     "https://token@github.com/DevStarByte/ByteForge.git", "ssh://git@github.com/DevStarByte/ByteForge.git",
                     "https://github.com/DevStarByte/ByteForge/" })
        {
            Assert.Equal(("DevStarByte", "ByteForge"), GithubActionsModule.ParseRemote(url));
        }
        Assert.Equal(("me", "my.repo"), GithubActionsModule.ParseRemote("git@github.com:me/my.repo.git"));
        foreach (var url in new[] { "git@gitlab.com:a/b.git", "https://github.com/onlyowner", "", null })
        {
            Assert.Null(GithubActionsModule.ParseRemote(url));
        }
    }

    [Fact]
    public async Task Find_repository_prefers_the_upstream_remote()
    {
        var git = new Dictionary<string, string?>
        {
            ["symbolic-ref --quiet --short HEAD"] = "main",
            ["remote"] = "origin\nupstream",
            ["remote get-url origin"] = "https://github.com/fork/repo.git",
            ["remote get-url upstream"] = "git@github.com:org/repo.git",
        };
        var module = new TestModule(git: git);
        Assert.Equal(new Repository("fork", "repo", "main"), await module.FindRepositoryAsync("/repo"));
        Assert.All(module.GitCalls, c => Assert.Equal("/repo", c.Root));
        Assert.Equal(["symbolic-ref --quiet --short HEAD", "remote", "config branch.main.remote", "remote get-url origin"], module.GitCalls.Select(c => c.Args));
        git["config branch.main.remote"] = "upstream";
        Assert.Equal(new Repository("org", "repo", "main"), await module.FindRepositoryAsync("/repo"));
        // Detached: no branch, and no upstream asked for; no github.com remote: none.
        git.Remove("symbolic-ref --quiet --short HEAD");
        Assert.Equal(new Repository("fork", "repo", null), await module.FindRepositoryAsync("/repo"));
        Assert.Null(await new TestModule(git: new() { ["remote"] = "origin", ["remote get-url origin"] = "git@gitlab.com:a/b.git" }).FindRepositoryAsync("/repo"));
        Assert.Null(await module.FindRepositoryAsync(null));
    }

    [Fact]
    public void Dispatchable_workflows()
    {
        var folder = Directory.CreateDirectory(Path.Combine(config, ".github", "workflows")).FullName;
        var files = new Dictionary<string, string>
        {
            ["a.yml"] = "on:\n  push:\n  workflow_dispatch:\n",
            ["b.yml"] = "on: [push, workflow_dispatch]\n",
            ["c.yaml"] = "on: workflow_dispatch\n",
            ["d.yml"] = "on:\n  push:\n    branches: [main]\n",
            ["e.yml"] = "on:\r\n  workflow_dispatch:\r\n",
        };
        foreach (var (name, text) in files)
        {
            File.WriteAllText(Path.Combine(folder, name), text);
        }
        Assert.Equal(new HashSet<string> { "a.yml", "b.yml", "c.yaml", "e.yml" }, GithubActionsModule.DispatchableWorkflows(config));
        Assert.Empty(GithubActionsModule.DispatchableWorkflows(Path.Combine(config, "nothing")));
    }

    // ───────────── presentation ─────────────

    [Fact]
    public void States()
    {
        Assert.Equal("running", Items.State(new JsonObject { ["status"] = "in_progress" }));
        Assert.Equal("queued", Items.State(new JsonObject { ["status"] = "queued" }));
        Assert.Equal("failure", Items.State(new JsonObject { ["status"] = "completed", ["conclusion"] = "timed_out" }));
        Assert.Equal("cancelled", Items.State(new JsonObject { ["status"] = "completed", ["conclusion"] = "cancelled" }));
        Assert.Equal("success", Items.State(new JsonObject { ["status"] = "completed", ["conclusion"] = "success" }));
        Assert.Equal("neutral", Items.State(new JsonObject { ["status"] = "completed", ["conclusion"] = null }));
    }

    [Fact]
    public void Times()
    {
        Assert.Equal("1h ago", Items.Ago(Data.Iso(90)));
        Assert.Equal("just now", Items.Ago(Data.Iso(0)));
        Assert.Equal("", Items.Ago(null));
        Assert.Equal("2m 5s", Items.Duration("2026-01-01T10:00:00Z", "2026-01-01T10:02:05Z"));
        Assert.Equal("1h 30m", Items.Duration("2026-01-01T10:00:00Z", "2026-01-01T11:30:00Z"));
        Assert.Equal("", Items.Duration("2026-01-01T10:00:00Z", "2026-01-01T09:00:00Z"));
    }

    [Fact]
    public void Clean_log_strips_timestamps_and_finds_the_first_error()
    {
        var raw = "﻿2026-11-20T09:00:00.1234567Z ##[group]Run dotnet test\r\n" +
                  "2026-11-20T09:00:01.0000000Z Passed!\r\n" +
                  "2026-11-20T09:00:02.0000000Z ##[error]Process completed with exit code 1.\r\n";
        var (text, line) = Items.CleanLog(raw);
        Assert.Equal(["##[group]Run dotnet test", "Passed!", "##[error]Process completed with exit code 1."], text.Split('\n')[..3]);
        Assert.Equal(2, line);
        Assert.Null(Items.CleanLog("no errors\n").FirstError);
    }

    [Fact]
    public void Slugs_and_branch_names()
    {
        Assert.Equal("build-test", Items.Slug("build / test"));
        Assert.Equal("job", Items.Slug("///"));
        Assert.Equal("issue-5", Items.BranchName("{title}", new JsonObject { ["number"] = 5, ["title"] = "!!!" }));
        Assert.Equal("feature-5-a-b", Items.BranchName("feature//{number}-{title}..", new JsonObject { ["number"] = 5, ["title"] = "A  b" }));
    }

    // ───────────── the view ─────────────

    [Fact]
    public async Task Tree_groups_branch_runs_and_workflows()
    {
        var module = Module();
        await module.FetchAsync();
        var tree = module.Tree();
        Assert.Equal(["Branch master", "Pull Requests", "Workflows", "Issues", "Releases"], Labels(tree["items"]));
        var branch = tree["items"]![0]!;
        var workflows = tree["items"]![2]!;
        Assert.Equal(["CI #3", "CI #2"], Labels(branch["children"]));
        Assert.Equal(Items.Yellow, S(branch["children"]![0]!["iconColor"]));
        Assert.Equal(["CI", "Release"], Labels(workflows["children"]));
        Assert.Equal(1, tree["badge"]!.GetValue<int>());
        var failed = branch["children"]![1]!;
        Assert.Contains("rerunFailed", failed["menu"]!.AsArray().Select(m => S(m!["id"])));
        Assert.Contains("cancel", branch["children"]![0]!["menu"]!.AsArray().Select(m => S(m!["id"])));
        // The workflows' rows: the runs without the workflow's name.
        Assert.Equal("#3 Commit 3", S(workflows["children"]![0]!["children"]![0]!["label"]));
    }

    [Fact]
    public async Task Click_on_a_run_lists_its_jobs_and_steps()
    {
        var module = Module();
        await using var harness = await Start(module);
        await module.FetchAsync();
        await module.RunActionAsync(2, "click");
        var tree = SentOf(harness, "setTree")[^1];
        var row = tree["items"]![0]!["children"]![1]!;
        Assert.True(row["expanded"]!.GetValue<bool>());
        var job = row["children"]![0]!;
        Assert.Equal("build", S(job["label"]));
        Assert.True(job["expanded"]!.GetValue<bool>());  // failed jobs open on their steps
        Assert.Equal(["Checkout", "Test"], Labels(job["children"]));
        Assert.Equal(Items.Red, S(job["children"]![1]!["iconColor"]));
        await module.RunActionAsync(2, "click");
        Assert.DoesNotContain(2L, module.OpenRuns);
    }

    [Fact]
    public async Task Rerun_without_a_token_explains_and_with_one_posts()
    {
        var module = Module();
        await using (var harness = await Start(module))
        {
            await module.FetchAsync();
            await module.RunActionAsync(2, "rerunFailed");
            Assert.Contains(SentOf(harness, "log"), m => S(m["text"]).Contains("needs a GitHub token"));
            Assert.DoesNotContain(module.Fake.Calls, c => c.Method == "POST");
        }
        module = Module(token: "t");
        await using (var harness = await Start(module))
        {
            await module.FetchAsync();
            await module.RunActionAsync(2, "rerunFailed");
            var post = Assert.Single(module.Fake.Calls, c => c.Method == "POST");
            Assert.Equal("/repos/o/r/actions/runs/2/rerun-failed-jobs", post.Path);
            Assert.Equal("{}", post.Body!.ToJsonString());
            await harness.Next("log", m => S(m["text"]) == "github: Re-running the failed jobs of CI #2");
            await harness.Next("setTree");
        }
    }

    [Fact]
    public async Task Status_bar_shows_the_latest_branch_run()
    {
        var module = Module();
        await using var harness = await Start(module);
        await module.FetchAsync();
        module.UpdateStatus();
        var run = await harness.Next("status", m => S(m["id"]) == "github.run");
        Assert.Equal("● CI", S(run["text"]));
        Assert.Equal("github.show", S(run["command"]));
        // github.showStatus off: the item goes (a settingsChanged refresh shows that).
        harness.Send(new() { ["type"] = "event", ["name"] = "settingsChanged", ["settings"] = new JsonObject { ["github.showStatus"] = false } });
        await harness.Next("status", m => S(m["id"]) == "github.run" && S(m["text"]) == "");
    }

    [Fact]
    public void Not_a_github_folder()
    {
        var module = new TestModule { Root = "/x" };
        Assert.Contains("isn't a GitHub repository", S(module.Tree()["message"]));
        Assert.Contains("Open a folder", S(new TestModule().Tree()["message"]));
    }

    [Fact]
    public async Task Activate_finds_the_repository_with_git_and_shows_the_view()
    {
        var git = new Dictionary<string, string?>
        {
            ["symbolic-ref --quiet --short HEAD"] = "master",
            ["remote"] = "origin",
            ["remote get-url origin"] = "git@github.com:o/r.git",
        };
        var module = new TestModule(git: git);
        await using var harness = await Start(module);
        harness.Activate(config);
        var tree = await harness.Next("setTree");
        Assert.Equal(["Branch master", "Pull Requests", "Workflows", "Issues", "Releases"], Labels(tree["items"]));
        Assert.All(module.GitCalls, c => Assert.Equal(config, c.Root));
        Assert.Equal(new Repository("o", "r", "master"), module.Repo);
        // Without a token: no /user, and the run and pull request status items.
        Assert.DoesNotContain(module.Fake.Calls, c => c.Path == "/user");
        await harness.Next("status", m => S(m["id"]) == "github.pr");
    }

    [Fact]
    public async Task Commands_are_registered()
    {
        await using var harness = ModuleHarness.Start(new TestModule(), ModuleDir);
        foreach (var (id, title) in new[] { ("github.show", "Show GitHub View"), ("github.refresh", "Refresh"), ("github.dispatch", "Run Workflow…"),
                     ("github.openInBrowser", "Open Actions in the Browser"), ("github.openPullRequest", "Open This Branch's Pull Request"),
                     ("github.openRepository", "Open Repository in the Browser") })
        {
            var command = await harness.Next("registerCommand", m => S(m["id"]) == id);
            Assert.Equal(title, S(command["title"]));
            Assert.Equal("GitHub", S(command["category"]));
        }
    }

    // ───────────── GitHub ─────────────

    [Fact]
    public async Task Branch_pull_request_comes_first_with_its_checks()
    {
        var module = Module(branch: "feature/x");
        await module.FetchAsync();
        var pulls = module.Tree()["items"]![1]!;
        Assert.Equal("Pull Requests", S(pulls["label"]));
        var first = pulls["children"]![0]!;
        Assert.Equal("#12 Add export", S(first["label"]));
        Assert.StartsWith("this branch", S(first["description"]));
        Assert.Equal(Items.Yellow, S(first["iconColor"]));  // one check still running
        Assert.Equal(["tests", "codecov"], Labels(first["children"]));
        Assert.Contains("draft", S(pulls["children"]![1]!["description"]));
        Assert.Null(pulls["children"]![1]!["children"]);  // checks are asked for on a click
    }

    [Fact]
    public async Task Status_bar_shows_the_pull_request()
    {
        var module = Module(branch: "feature/x");
        await using var harness = await Start(module);
        await module.FetchAsync();
        module.UpdateStatus();
        var pr = await harness.Next("status", m => S(m["id"]) == "github.pr");
        Assert.Equal("PR #12 ●", S(pr["text"]));
    }

    [Fact]
    public async Task Click_on_a_pull_request_loads_its_checks()
    {
        var module = Module();
        await using var harness = await Start(module);
        await module.FetchAsync();
        await module.PullActionAsync(7, "click");
        var tree = SentOf(harness, "setTree")[^1];
        var row = tree["items"]![1]!["children"]!.AsArray().First(c => S(c!["id"]) == "pr:7")!;
        Assert.True(row["expanded"]!.GetValue<bool>());
        Assert.Equal(2, row["children"]!.AsArray().Count);
    }

    [Fact]
    public async Task Checkout_commands()
    {
        var module = Module(branch: "feature/x");
        await module.FetchAsync();
        Assert.Equal("git fetch origin feature/x && git switch feature/x", Items.CheckoutCommand(module.FindPull(12)!, "o/r"));
        Assert.Equal("git fetch origin pull/7/head:pr-7 && git switch pr-7", Items.CheckoutCommand(module.FindPull(7)!, "o/r"));
    }

    [Fact]
    public async Task Issues_leave_out_pull_requests_and_put_yours_first()
    {
        var module = Module(token: "t", branch: "feature/x");
        await module.FetchAsync();
        var issues = Item(module.Tree(), "issues");
        Assert.Equal(["#31 Dark mode", "#30 Crash on start!"], Labels(issues["children"]));
        Assert.StartsWith("assigned to you", S(issues["children"]![0]!["description"]));
        Assert.Equal(Items.Blue, S(issues["children"]![0]!["iconColor"]));
    }

    [Fact]
    public async Task Create_branch_from_an_issue()
    {
        var git = new Dictionary<string, string?>
        {
            ["symbolic-ref --quiet --short HEAD"] = "feature/x",
            ["remote"] = "origin",
            ["remote get-url origin"] = "https://github.com/o/r",
        };
        var module = new TestModule(git: git);
        await using var harness = await Start(module);
        harness.Activate(config);
        await harness.Next("setTree");
        module.IssueAction(30, "branch");
        var terminal = await harness.Next("runInTerminal");
        Assert.Equal("git switch -c feature/30-crash-on-start", S(terminal["command"]));
        Assert.Equal("branch feature/30-crash-on-start", S(terminal["title"]));
        Assert.Equal(config, S(terminal["cwd"]));
        harness.Send(new() { ["type"] = "event", ["name"] = "settingsChanged", ["settings"] = new JsonObject { ["github.branchName"] = "{number}_{title}" } });
        await harness.Next("setTree");
        module.IssueAction(31, "branch");
        Assert.Equal("git switch -c 31_dark-mode", S((await harness.Next("runInTerminal"))["command"]));
    }

    [Fact]
    public async Task Releases()
    {
        var module = Module();
        await module.FetchAsync();
        var releases = Item(module.Tree(), "releases");
        var rows = releases["children"]!.AsArray().OrderBy(r => S(r!["label"]), StringComparer.Ordinal).ToList();
        var (stable, beta) = (rows[0]!, rows[1]!);
        Assert.Equal("v1.1.2 1.1.2", S(stable["label"]));
        Assert.StartsWith("latest", S(stable["description"]));
        Assert.Contains("7 downloads", S(stable["description"]));
        Assert.StartsWith("pre-release", S(beta["description"]));
        Assert.Equal("v1.2.0-beta.1", S(beta["label"]));
    }

    [Fact]
    public async Task Pipelines_only_refresh_skips_the_rest()
    {
        var module = Module();
        await module.FetchAsync(full: false);
        Assert.Empty(module.Pulls);
        Assert.DoesNotContain(module.Fake.Calls, c => c.Path.Contains("/pulls"));
        Assert.Contains(module.Fake.Calls, c => c.Path == "/repos/o/r/actions/runs?per_page=10&branch=master");
    }

    [Fact]
    public async Task Branch_names_are_quoted_in_the_runs_request()
    {
        var module = Module(branch: "feature/x y");
        await module.FetchAsync(full: false);
        Assert.Contains(module.Fake.Calls, c => c.Path == "/repos/o/r/actions/runs?per_page=10&branch=feature%2Fx%20y");
    }

    [Fact]
    public async Task Job_log_opens_at_the_first_error()
    {
        var module = Module(token: "t");
        await using var harness = await Start(module);
        await module.FetchAsync();
        await module.RunActionAsync(2, "click");
        module.Fake.Log = "2026-11-20T09:00:00.0000000Z one\n2026-11-20T09:00:01.0000000Z ##[error]boom\n";
        await module.OpenLogAsync(77);
        var open = await harness.Next("openFile");
        var path = S(open["path"]);
        Assert.Equal(Path.Combine(ModuleDir, "logs", "77-build.log"), path);
        Assert.Equal(1, open["line"]!.GetValue<int>());
        Assert.Equal(0, open["column"]!.GetValue<int>());
        Assert.Equal($"one{Environment.NewLine}##[error]boom{Environment.NewLine}", File.ReadAllText(path));
        Assert.Contains(module.Fake.Calls, c => c.Method == "DOWNLOAD" && c.Path == "/repos/o/r/actions/jobs/77/logs");
    }

    [Fact]
    public async Task Job_log_without_a_token_explains()
    {
        var module = Module();
        await using var harness = await Start(module);
        await module.FetchAsync();
        await module.OpenLogAsync(77);
        await harness.Next("log", m => S(m["text"]).StartsWith("github: Reading logs needs a GitHub token", StringComparison.Ordinal));
        await harness.Next("showView", m => S(m["id"]) == "output");
    }

    [Fact]
    public async Task Run_workflow_offers_the_dispatchable_ones_and_starts_the_picked()
    {
        var folder = Directory.CreateDirectory(Path.Combine(config, ".github", "workflows")).FullName;
        File.WriteAllText(Path.Combine(folder, "ci.yml"), "on:\n  push:\n  workflow_dispatch:\n");
        File.WriteAllText(Path.Combine(folder, "nightly.yml"), "on:\n  schedule:\n");
        File.WriteAllText(Path.Combine(folder, "old.yml"), "on: workflow_dispatch\n");
        var module = new TestModule(token: "t") { Root = config, Repo = new Repository("o", "r", "master") };
        module.Api = new FakeApi("t");
        await using var harness = await Start(module);
        var dispatch = module.DispatchAsync();
        var pick = await harness.Next("quickPick");
        Assert.Equal("Run a workflow on master", S(pick["placeholder"]));
        var item = Assert.Single(pick["items"]!.AsArray())!;
        Assert.Equal("CI", S(item["label"]));
        Assert.Equal("41", S(item["value"]));
        Assert.Equal(".github/workflows/ci.yml", S(item["detail"]));
        harness.Send(new() { ["type"] = "quickPickResult", ["requestId"] = pick["requestId"]!.GetValue<long>(), ["value"] = "41" });
        await dispatch;
        var post = Assert.Single(module.Fake.Calls, c => c.Method == "POST");
        Assert.Equal("/repos/o/r/actions/workflows/41/dispatches", post.Path);
        Assert.Equal("""{"ref":"master"}""", post.Body!.ToJsonString());
    }

    [Fact]
    public async Task Tree_actions_open_the_browser()
    {
        var module = Module(branch: "feature/x");
        await using var harness = await Start(module);
        await module.FetchAsync();
        foreach (var (item, action) in new (string?, string)[] { ("pr:12", "open"), ("issue:30", "click"), ("release:9", "click"), ("check:502:other", "click"),
                     ("run:2", "open"), ("wf:CI", "openWorkflow"), ("issues", "newIssue"), (null, "browser") })
        {
            harness.Send(new() { ["type"] = "treeAction", ["view"] = GithubActionsModule.View, ["item"] = item, ["action"] = action });
        }
        string[] expected = ["https://github.com/o/r/pull/12", "https://i30", "https://r", "https://codecov", "https://github.com/o/r/actions/runs/2",
            "https://github.com/o/r/actions/workflows/ci.yml", "https://github.com/o/r/issues/new", "https://github.com/o/r/actions"];
        for (var i = 0; i < 100 && module.Opened.Count < expected.Length; i++)
        {
            await Task.Delay(20);
        }
        lock (module.Opened)
        {
            Assert.Equal(expected.Order(), module.Opened.Order());
        }
        harness.Send(new() { ["type"] = "treeAction", ["view"] = GithubActionsModule.View, ["item"] = "pr:7", ["action"] = "checkout" });
        var terminal = await harness.Next("runInTerminal");
        Assert.Equal("git fetch origin pull/7/head:pr-7 && git switch pr-7", S(terminal["command"]));
        Assert.Equal("PR #7", S(terminal["title"]));
    }

    // ───────────── API ─────────────

    [Fact]
    public async Task Token_from_the_setting_then_the_environment_then_gh()
    {
        static Task<string?> NoGh() => Task.FromResult<string?>(null);
        Assert.Equal(("abc", "the github.token setting"), await GithubActionsModule.FindTokenAsync(new() { ["github.token"] = " abc " }, _ => "env", NoGh));
        Assert.Equal(("env", "GH_TOKEN"), await GithubActionsModule.FindTokenAsync([], name => name == "GH_TOKEN" ? "env " : null, NoGh));
        Assert.Equal(("env2", "GITHUB_TOKEN"), await GithubActionsModule.FindTokenAsync([], name => name == "GITHUB_TOKEN" ? "env2" : null, NoGh));
        Assert.Equal(("gh-token", "gh"), await GithubActionsModule.FindTokenAsync([], _ => null, () => Task.FromResult<string?>("gh-token")));
        Assert.Equal((null, null), await GithubActionsModule.FindTokenAsync([], _ => null, NoGh));
    }

    [Fact]
    public void Error_messages()
    {
        var api = new GitHubApi();
        var headers = new Dictionary<string, string> { ["X-RateLimit-Remaining"] = "0", ["X-RateLimit-Reset"] = "0" };
        var text = api.Explain(403, """{"message": "API rate limit exceeded"}""", headers.GetValueOrDefault);
        Assert.Contains("rate limit", text);
        Assert.Contains("github.token", text);
        Assert.DoesNotContain("github.token", new GitHubApi("t").Explain(429, "", headers.GetValueOrDefault));
        Assert.Equal("GitHub didn't accept the token (expired or revoked?).", api.Explain(401, "", _ => null));
        Assert.StartsWith("repository not found", api.Explain(404, "", _ => null));
        Assert.Equal("Resource not accessible: this needs a token (\"github.token\", GH_TOKEN or gh).", api.Explain(403, """{"message": "Resource not accessible"}""", _ => null));
        Assert.Equal("Not Found", new GitHubApi("t").Explain(404, """{"message": "Not Found"}""", _ => null));
        Assert.Equal("HTTP 500", new GitHubApi("t").Explain(500, "<html>", _ => null));
    }

    // ───────────── manifest ─────────────

    [Fact]
    public void Manifest_settings_and_view()
    {
        var text = File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "..", "..", "module.json"));
        var manifest = JsonNode.Parse(Regex.Replace(text, @"(?m)^\s*//.*$", ""))!;
        Assert.Equal(GithubActionsModule.View, S(manifest["views"]![0]!["id"]));
        Assert.Equal(["github"], manifest["settings"]!.AsArray().Select(s => S(s!["key"]).Split('.')[0]).Distinct());
        Assert.Equal("dotnet", S(manifest["runtime"]));
        Assert.Equal("bin/GithubActions.dll", S(manifest["assembly"]));
    }
}
