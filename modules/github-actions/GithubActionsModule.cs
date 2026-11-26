using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;

namespace ByteForge.GithubActions;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<GithubActionsModule>(input, output, moduleDir, cancel);
}

/// <summary>The folder's GitHub repository: its owner, name and the branch checked out (null when detached).</summary>
public sealed record Repository(string Owner, string Name, string? Branch)
{
    public string Slug => $"{Owner}/{Name}";
}

/// <summary>
/// GitHub for ByteForge: pipelines, pull requests, issues and releases of the folder's GitHub repository.
/// The GitHub view shows the runs of the current branch and, per workflow, the latest runs of every branch; the
/// open pull requests with their checks (the current branch's pull request also in the status bar); the open
/// issues ("Create Branch" from one); and the latest releases. A click on a run lists its jobs and steps; a click
/// on a job opens its log in an editor (at the first error). "Run Workflow…" starts a workflow that has a
/// workflow_dispatch trigger on the current branch.
/// Talks to the GitHub REST API directly (no gh needed). Public repositories work without a token (60 requests
/// an hour); logs, re-running, cancelling, starting workflows and private repositories need one: "github.token"
/// in settings, the GH_TOKEN or GITHUB_TOKEN environment variable, or a logged-in gh CLI.
/// git, gh, the browser and the API are virtual so that tests run without any of them.
/// </summary>
public partial class GithubActionsModule : Module
{
    public const string View = "github-actions";

    static readonly (string Id, string Title)[] Commands =
    [
        ("github.show", "Show GitHub View"),
        ("github.refresh", "Refresh"),
        ("github.dispatch", "Run Workflow…"),
        ("github.openInBrowser", "Open Actions in the Browser"),
        ("github.openPullRequest", "Open This Branch's Pull Request"),
        ("github.openRepository", "Open Repository in the Browser"),
    ];

    readonly SemaphoreSlim refreshLock = new(1, 1);
    GitHubApi? api;
    volatile bool visible;
    int polls;

    // The data, each replaced as a whole when fetched (the view is built from other tasks meanwhile).
    List<JsonObject> branchRuns = [];
    List<JsonObject> allRuns = [];
    List<JsonObject> pulls = [];
    List<JsonObject> issues = [];
    List<JsonObject> releases = [];
    readonly ConcurrentDictionary<long, (string? UpdatedAt, List<JsonObject> Jobs)> jobs = new();
    readonly ConcurrentDictionary<string, List<JsonObject>> checks = new();
    readonly ConcurrentDictionary<long, bool> openRuns = new();
    readonly ConcurrentDictionary<long, bool> openPulls = new();

    /// <summary>The open folder (from activate and folderOpened).</summary>
    public string? Root { get; set; }

    /// <summary>Its GitHub repository, or null.</summary>
    public Repository? Repo { get; set; }

    public GitHubApi Api
    {
        get => api ??= CreateApi(null);
        set => api = value;
    }

    /// <summary>Where the token came from ("the github.token setting", GH_TOKEN, GITHUB_TOKEN, gh), or null.</summary>
    public string? TokenSource { get; private set; }

    /// <summary>The token's login (null: not asked yet, "": unknown).</summary>
    public string? Me { get; set; }

    /// <summary>The last error from GitHub, shown in the view.</summary>
    public string? Error { get; private set; }

    public IReadOnlyCollection<long> OpenRuns => [.. openRuns.Keys];
    public IReadOnlyCollection<long> OpenPulls => [.. openPulls.Keys];
    public IReadOnlyList<JsonObject> Pulls => pulls;
    public IReadOnlyList<JsonObject> Issues => issues;

    /// <summary>Where job logs are written: the module's own logs/ folder.</summary>
    public string LogsDir => Path.Combine(ModuleDir, "logs");

    // ───────────── what tests replace ─────────────

    /// <summary>The API client for a token.</summary>
    protected virtual GitHubApi CreateApi(string? token) => new(token, Stopping);

    /// <summary>Runs git in <paramref name="root"/>: its output, trimmed, or null when it failed.</summary>
    protected virtual async Task<string?> GitAsync(string root, params string[] args)
    {
        try
        {
            var result = await Programs.RunAsync(Programs.Which("git") ?? "git", args, workingDirectory: root, timeout: TimeSpan.FromSeconds(10), cancel: Stopping);
            return result.Success ? result.Output.Trim() : null;
        }
        catch (Exception e) when (e is ModuleException or TimeoutException)
        {
            return null;
        }
    }

    /// <summary>gh's token for github.com (gh auth token), or null.</summary>
    protected virtual async Task<string?> GhTokenAsync()
    {
        if (Programs.Which("gh") is not { } gh)
        {
            return null;
        }
        try
        {
            var result = await Programs.RunAsync(gh, ["auth", "token", "--hostname", "github.com"], timeout: TimeSpan.FromSeconds(10), cancel: Stopping);
            return result.Success && result.Output.Trim().Length > 0 ? result.Output.Trim() : null;
        }
        catch (Exception e) when (e is ModuleException or TimeoutException)
        {
            return null;
        }
    }

    /// <summary>An environment variable (GH_TOKEN, GITHUB_TOKEN).</summary>
    protected virtual string? Environment(string name) => System.Environment.GetEnvironmentVariable(name);

    /// <summary>Opens a page in the default browser.</summary>
    protected virtual void OpenBrowser(string url)
    {
        try
        {
            ProcessStartInfo start;
            if (OperatingSystem.IsWindows())
            {
                start = new ProcessStartInfo(url) { UseShellExecute = true };
            }
            else
            {
                start = new ProcessStartInfo(OperatingSystem.IsMacOS() ? "open" : "xdg-open") { UseShellExecute = false };
                start.ArgumentList.Add(url);
            }
            using var process = Process.Start(start);
        }
        catch (Exception e) when (e is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            Log($"github: can't open the browser: {e.Message}");
        }
    }

    /// <summary>Waits before refreshing after an action (GitHub shows its effect a moment later).</summary>
    protected virtual Task PauseAsync(TimeSpan time) => Task.Delay(time, Stopping);

    // ───────────── repository ─────────────

    [GeneratedRegex(@"^(?:https?://(?:[^@/]+@)?github\.com/|ssh://git@github\.com(?::\d+)?/|git@github\.com:)([\w.-]+)/([\w.-]+?)(?:\.git)?/?$")]
    private static partial Regex GitHubRemote();

    [GeneratedRegex(@"^\s*workflow_dispatch\s*:|[\[,\s]workflow_dispatch[\],\s]|^on:\s*workflow_dispatch", RegexOptions.Multiline)]
    private static partial Regex DispatchTrigger();

    /// <summary>(owner, repo) of a github.com remote URL (https, ssh or scp-like), else null.</summary>
    public static (string Owner, string Name)? ParseRemote(string? url)
    {
        var m = GitHubRemote().Match((url ?? "").Trim());
        return m.Success ? (m.Groups[1].Value, m.Groups[2].Value) : null;
    }

    /// <summary>
    /// (owner, repo, branch) of the folder: the remote of the branch's upstream, else origin, else any github.com remote.
    /// </summary>
    public async Task<Repository?> FindRepositoryAsync(string? root)
    {
        if (string.IsNullOrEmpty(root))
        {
            return null;
        }
        var branch = await GitAsync(root, "symbolic-ref", "--quiet", "--short", "HEAD");  // null when detached
        var remotes = ((await GitAsync(root, "remote")) ?? "").Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
        var upstream = !string.IsNullOrEmpty(branch) ? await GitAsync(root, "config", $"branch.{branch}.remote") : null;
        var ordered = new[] { upstream, "origin" }.Where(r => r != null && remotes.Contains(r)).Select(r => r!)
            .Concat(remotes.Where(r => r != upstream && r != "origin"));
        foreach (var remote in ordered)
        {
            if (ParseRemote(await GitAsync(root, "remote", "get-url", remote)) is { } found)
            {
                return new Repository(found.Owner, found.Name, branch);
            }
        }
        return null;
    }

    /// <summary>File names in .github/workflows that have a workflow_dispatch trigger.</summary>
    public static HashSet<string> DispatchableWorkflows(string root)
    {
        var names = new HashSet<string>(StringComparer.Ordinal);
        try
        {
            foreach (var file in Directory.EnumerateFiles(Path.Combine(root, ".github", "workflows")))
            {
                var name = Path.GetFileName(file);
                if ((name.EndsWith(".yml", StringComparison.Ordinal) || name.EndsWith(".yaml", StringComparison.Ordinal)) &&
                    DispatchTrigger().IsMatch(File.ReadAllText(file, new UTF8Encoding(false, false)).Replace("\r\n", "\n")))
                {
                    names.Add(name);
                }
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
        }
        return names;
    }

    /// <summary>The token and where it came from: the github.token setting, GH_TOKEN, GITHUB_TOKEN, or gh's login.</summary>
    public static async Task<(string? Token, string? Source)> FindTokenAsync(JsonObject settings, Func<string, string?> environment, Func<Task<string?>> gh)
    {
        var token = (settings.Str("github.token") ?? "").Trim();
        if (token.Length > 0)
        {
            return (token, "the github.token setting");
        }
        foreach (var name in new[] { "GH_TOKEN", "GITHUB_TOKEN" })
        {
            if (environment(name) is { Length: > 0 } value)
            {
                return (value.Trim(), name);
            }
        }
        return await gh() is { } fromGh ? (fromGh, "gh") : (null, null);
    }

    // ───────────── data ─────────────

    async Task ConfigureAsync()
    {
        var (token, source) = await FindTokenAsync(Settings, Environment, GhTokenAsync);
        if (token != Api.Token)
        {
            Api = CreateApi(token);
        }
        TokenSource = source;
        Repo = await FindRepositoryAsync(Root);
    }

    string Slug => Repo!.Slug;

    static List<JsonObject> Objects(JsonNode? list) => list is JsonArray a ? [.. a.OfType<JsonObject>()] : [];

    static long Id(JsonNode? item) => item.Long("id") ?? 0;

    static long Number(JsonNode? item) => item.Long("number") ?? 0;

    /// <summary>Fetches the runs (and the jobs of the opened ones); with <paramref name="full"/> the rest too.</summary>
    public async Task FetchAsync(bool full = true)
    {
        if (Repo == null)
        {
            return;
        }
        try
        {
            var runsPath = $"/repos/{Slug}/actions/runs";
            allRuns = Objects((await Api.RequestAsync("GET", $"{runsPath}?per_page=40")).Get("workflow_runs"));
            if (!string.IsNullOrEmpty(Repo.Branch))
            {
                var quoted = Uri.EscapeDataString(Repo.Branch);
                branchRuns = Objects((await Api.RequestAsync("GET", $"{runsPath}?per_page=10&branch={quoted}")).Get("workflow_runs"));
            }
            else
            {
                branchRuns = [];
            }
            Error = null;
            foreach (var runId in openRuns.Keys.ToList())
            {
                await FetchJobsAsync(runId);
            }
            if (full)
            {
                await FetchRestAsync();
            }
        }
        catch (ApiException e)
        {
            Error = e.Message;
        }
    }

    /// <summary>Pull requests (with the checks of the current branch's and the opened ones), issues, releases.</summary>
    async Task FetchRestAsync()
    {
        var repo = $"/repos/{Slug}";
        pulls = Objects(await Api.RequestAsync("GET", $"{repo}/pulls?state=open&per_page=30&sort=updated&direction=desc"));
        var all = Objects(await Api.RequestAsync("GET", $"{repo}/issues?state=open&per_page=30&sort=updated&direction=desc"));
        issues = [.. all.Where(i => !i.ContainsKey("pull_request"))];  // the issues API lists pull requests too
        releases = Objects(await Api.RequestAsync("GET", $"{repo}/releases?per_page=5"));
        if (Api.HasToken && Me == null)
        {
            Me = (await Api.RequestAsync("GET", "/user")).OrNull("login") ?? "";
        }
        var wanted = pulls.Select(Number).Where(openPulls.ContainsKey).ToHashSet();
        if (BranchPull() is { } branchPull)
        {
            wanted.Add(Number(branchPull));
        }
        foreach (var pr in pulls.Where(p => wanted.Contains(Number(p))))
        {
            await FetchChecksAsync(pr);
        }
    }

    static string ShaKey(JsonObject pr) => pr.Get("head").Str("sha") ?? "";

    async Task<List<JsonObject>> FetchChecksAsync(JsonObject pr)
    {
        var sha = pr.Get("head").S("sha");
        var data = await Api.RequestAsync("GET", $"/repos/{Slug}/commits/{sha}/check-runs?per_page=100");
        var list = Objects(data.Get("check_runs"));
        checks[ShaKey(pr)] = list;
        return list;
    }

    /// <summary>The open pull request of the current branch (from this repository, not a fork).</summary>
    public JsonObject? BranchPull()
    {
        var branch = Repo?.Branch;
        var slug = Repo?.Slug;
        return pulls.FirstOrDefault(pr => pr.Get("head").Str("ref") == branch && pr.Get("head").Get("repo").Str("full_name") == slug);
    }

    public JsonObject? FindPull(long number) => pulls.FirstOrDefault(pr => Number(pr) == number);

    async Task<List<JsonObject>> FetchJobsAsync(long runId)
    {
        var run = FindRun(runId);
        if (jobs.TryGetValue(runId, out var cached) && run != null && cached.UpdatedAt == run.Str("updated_at") && !Items.IsActive(Items.State(run)))
        {
            return cached.Jobs;
        }
        var data = await Api.RequestAsync("GET", $"/repos/{Slug}/actions/runs/{runId}/jobs?per_page=100");
        var list = Objects(data.Get("jobs"));
        jobs[runId] = (run?.Str("updated_at"), list);
        return list;
    }

    JsonObject? FindRun(long runId) => branchRuns.Concat(allRuns).FirstOrDefault(r => Id(r) == runId);

    JsonObject? FindJob(long jobId) => jobs.Values.SelectMany(j => j.Jobs).FirstOrDefault(j => Id(j) == jobId);

    /// <summary>The runs going on (running or queued), each once.</summary>
    public List<JsonObject> ActiveRuns()
    {
        var byId = new Dictionary<long, JsonObject>();
        var order = new List<long>();
        foreach (var run in branchRuns.Concat(allRuns))
        {
            if (!byId.ContainsKey(Id(run)))
            {
                order.Add(Id(run));
            }
            byId[Id(run)] = run;
        }
        return [.. order.Select(id => byId[id]).Where(r => Items.IsActive(Items.State(r)))];
    }

    // ───────────── view ─────────────

    static JsonObject TreeMessage(string message) => new() { ["type"] = "setTree", ["view"] = View, ["items"] = new JsonArray(), ["message"] = message };

    /// <summary>The setTree message of the view.</summary>
    public JsonObject Tree()
    {
        if (string.IsNullOrEmpty(Root))
        {
            return TreeMessage("Open a folder to see its GitHub repository.");
        }
        if (Repo is not { } repo)
        {
            return TreeMessage("This folder isn't a GitHub repository (no github.com remote).");
        }
        var items = new JsonArray();
        var hasBranch = !string.IsNullOrEmpty(repo.Branch);
        if (hasBranch)
        {
            var children = new JsonArray([.. branchRuns.Select(r => (JsonNode)RunRow(r, withWorkflow: true))]);
            var count = children.Count;
            if (count == 0 && Error == null)
            {
                children.Add(new JsonObject { ["id"] = "none", ["label"] = "No runs on this branch", ["icon"] = "info", ["iconColor"] = Items.Grey });
            }
            items.Add(new JsonObject { ["id"] = "branch", ["label"] = $"Branch {repo.Branch}", ["header"] = true, ["expanded"] = true, ["count"] = count, ["children"] = children });
        }
        var workflows = new Dictionary<string, List<JsonObject>>();
        var names = new List<string>();
        foreach (var run in allRuns)
        {
            var name = run.OrNull("name") ?? run.OrNull("path") ?? "?";
            if (!workflows.TryGetValue(name, out var list))
            {
                workflows[name] = list = [];
                names.Add(name);
            }
            list.Add(run);
        }
        var rows = new JsonArray();
        foreach (var name in names.OrderBy(n => n.ToLowerInvariant(), StringComparer.Ordinal))
        {
            var runs = workflows[name];
            var s = Items.State(runs[0]);
            var (icon, color) = Items.StateIcons[s];
            rows.Add(new JsonObject
            {
                ["id"] = $"wf:{name}",
                ["label"] = name,
                ["icon"] = icon,
                ["iconColor"] = color,
                ["description"] = $"{Items.StateWords[s]} {Items.Ago(runs[0].Str("created_at"))}",
                ["count"] = runs.Count,
                ["children"] = new JsonArray([.. runs.Take(10).Select(r => (JsonNode)RunRow(r, withWorkflow: false))]),
                ["menu"] = new JsonArray { new JsonObject { ["id"] = "openWorkflow", ["label"] = "Open in Browser" } },
            });
        }
        if (rows.Count > 0)
        {
            items.Add(new JsonObject { ["id"] = "workflows", ["label"] = "Workflows", ["header"] = true, ["expanded"] = true, ["count"] = rows.Count, ["children"] = rows });
        }
        var currentPulls = pulls;
        if (currentPulls.Count > 0)
        {
            var branchPull = BranchPull();
            var ordered = (branchPull != null ? [branchPull] : new List<JsonObject>()).Concat(currentPulls.Where(pr => !ReferenceEquals(pr, branchPull)));
            var children = new JsonArray();
            foreach (var pr in ordered)
            {
                var number = Number(pr);
                var prChecks = openPulls.ContainsKey(number) || ReferenceEquals(pr, branchPull) ? checks.GetValueOrDefault(ShaKey(pr)) : null;
                children.Add(Items.PullItem(pr, prChecks, expanded: openPulls.ContainsKey(number), currentBranch: repo.Branch));
            }
            items.Insert(hasBranch ? 1 : 0, new JsonObject { ["id"] = "pulls", ["label"] = "Pull Requests", ["header"] = true, ["expanded"] = true, ["count"] = children.Count, ["children"] = children });
        }
        var currentIssues = issues;
        if (currentIssues.Count > 0)
        {
            var me = Me;
            var mine = currentIssues.Where(i => !string.IsNullOrEmpty(me) && Items.Assignees(i).Contains(me)).ToList();
            var ordered = mine.Concat(currentIssues.Where(i => !mine.Contains(i))).ToList();
            items.Add(new JsonObject
            {
                ["id"] = "issues",
                ["label"] = "Issues",
                ["header"] = true,
                ["expanded"] = true,
                ["count"] = ordered.Count,
                ["children"] = new JsonArray([.. ordered.Select(i => (JsonNode)Items.IssueItem(i, me))]),
                ["actions"] = new JsonArray { new JsonObject { ["id"] = "newIssue", ["icon"] = "add", ["tooltip"] = "New Issue (browser)" } },
            });
        }
        var currentReleases = releases;
        if (currentReleases.Count > 0)
        {
            var latest = currentReleases.FirstOrDefault(r => !Py.Truthy(r.Get("draft")) && !Py.Truthy(r.Get("prerelease")));
            items.Add(new JsonObject
            {
                ["id"] = "releases",
                ["label"] = "Releases",
                ["header"] = true,
                ["expanded"] = false,
                ["count"] = currentReleases.Count,
                ["children"] = new JsonArray([.. currentReleases.Select(r => (JsonNode)Items.ReleaseItem(r, latest: ReferenceEquals(r, latest)))]),
            });
        }
        var actions = new JsonArray
        {
            new JsonObject { ["id"] = "dispatch", ["icon"] = "play", ["tooltip"] = "Run Workflow…" },
            new JsonObject { ["id"] = "browser", ["icon"] = "openPreview", ["tooltip"] = "Open Actions in the Browser" },
            new JsonObject { ["id"] = "refresh", ["icon"] = "refresh", ["tooltip"] = "Refresh" },
        };
        string? message = null;
        if (items.Count == 0 && Error == null)
        {
            message = $"Nothing in {repo.Slug} yet: no workflow runs, pull requests, issues or releases.";
        }
        var error = Error;
        var currentApi = Api;
        if (error == null && currentApi.Remaining is { } remaining && remaining < 10 && !currentApi.HasToken)
        {
            error = $"{remaining} GitHub requests left this hour. Set \"github.token\" (or GH_TOKEN, or log in with gh) for more.";
        }
        return new JsonObject
        {
            ["type"] = "setTree",
            ["view"] = View,
            ["items"] = items,
            ["message"] = message,
            ["error"] = error,
            ["badge"] = ActiveRuns().Count,
            ["actions"] = actions,
        };
    }

    JsonObject RunRow(JsonObject run, bool withWorkflow)
    {
        var id = Id(run);
        var open = openRuns.ContainsKey(id);
        var runJobs = open ? (jobs.TryGetValue(id, out var cached) ? cached.Jobs : null) : null;
        return Items.RunItem(run, runJobs, expanded: open, withWorkflow: withWorkflow);
    }

    bool ShowStatusOff => Settings.Get("github.showStatus") is JsonValue v && v.GetValueKind() == System.Text.Json.JsonValueKind.False;

    /// <summary>The status bar items: the branch's latest run, and its pull request.</summary>
    public void UpdateStatus()
    {
        UpdatePullStatus();
        if (ShowStatusOff || Repo == null || branchRuns.Count == 0)
        {
            Send(new() { ["type"] = "status", ["id"] = "github.run", ["text"] = "" });
            return;
        }
        var latest = branchRuns[0];
        var s = Items.State(latest);
        Send(new()
        {
            ["type"] = "status",
            ["id"] = "github.run",
            ["text"] = $"{Items.StateSymbols[s]} {latest.S("name")}",
            ["command"] = "github.show",
            ["tooltip"] = $"GitHub Actions: {latest.S("name")} #{latest.S("run_number")} on {(Repo.Branch ?? "None")} " +
                $"{Items.StateWords[s]} ({Items.Ago(latest.Str("created_at"))}). Click: the Actions view.",
        });
    }

    void UpdatePullStatus()
    {
        var pr = Repo != null ? BranchPull() : null;
        if (ShowStatusOff || pr == null)
        {
            Send(new() { ["type"] = "status", ["id"] = "github.pr", ["text"] = "" });
            return;
        }
        var s = Items.ChecksState(checks.GetValueOrDefault(ShaKey(pr)) ?? []);
        var number = pr.S("number");
        Send(new()
        {
            ["type"] = "status",
            ["id"] = "github.pr",
            ["text"] = $"PR #{number}" + (s != null ? $" {Items.StateSymbols[s]}" : ""),
            ["command"] = "github.openPullRequest",
            ["tooltip"] = $"#{number} {pr.S("title")}" + (s != null ? $": checks {Items.StateWords[s]}" : "") + ". Click: open it in the browser.",
        });
    }

    /// <summary>Fetches and shows: the view and the status bar.</summary>
    public async Task RefreshAsync(bool reconfigure = false, bool full = true)
    {
        await refreshLock.WaitAsync(Stopping);
        try
        {
            if (reconfigure)
            {
                await ConfigureAsync();
            }
            await FetchAsync(full);
            Send(Tree());
            UpdateStatus();
        }
        finally
        {
            refreshLock.Release();
        }
    }

    void RefreshInBackground(bool reconfigure = false) => _ = Task.Run(async () =>
    {
        try
        {
            await RefreshAsync(reconfigure);
        }
        catch (OperationCanceledException) when (Stopping.IsCancellationRequested)
        {
        }
        catch (Exception e)
        {
            Log($"github: {e.GetType().Name}: {e.Message}");
        }
    });

    /// <summary>
    /// Refreshes on a timer: often while runs are going, rarely otherwise (or never, with the view hidden and the
    /// status bar item off).
    /// </summary>
    async Task PollAsync()
    {
        try
        {
            while (true)
            {
                var interval = Setting<double>("github.refreshSeconds", 60);
                if (interval == 0 || double.IsNaN(interval))
                {
                    interval = 60;
                }
                if (!Api.HasToken)
                {
                    interval = Math.Max(interval, 240);  // 60 requests an hour without a token
                }
                if (ActiveRuns().Count == 0)
                {
                    interval *= 5;
                }
                await Task.Delay(TimeSpan.FromSeconds(Math.Clamp(interval, 1, TimeSpan.FromDays(1).TotalSeconds)), Stopping);
                if (Repo != null && (visible || ActiveRuns().Count > 0 || !ShowStatusOff))
                {
                    polls++;
                    try
                    {
                        // Pipelines every time; pull requests, issues and releases every third time.
                        await RefreshAsync(full: polls % 3 == 0);
                    }
                    catch (Exception e) when (e is not OperationCanceledException)
                    {
                        Log($"github: {e.GetType().Name}: {e.Message}");
                    }
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    // ───────────── actions ─────────────

    bool NeedsToken(string what)
    {
        if (Api.HasToken)
        {
            return false;
        }
        Log($"github: {what} needs a GitHub token: set \"github.token\" in settings (a fine-grained token " +
            "with Actions: read and write; Contents, Pull requests and Issues: read), or GH_TOKEN, or log in with gh.");
        ShowView("output");
        return true;
    }

    static readonly Dictionary<string, string> RunPaths = new() { ["rerun"] = "/rerun", ["rerunFailed"] = "/rerun-failed-jobs", ["cancel"] = "/cancel" };
    static readonly Dictionary<string, string> RunWords = new() { ["rerun"] = "Re-running", ["rerunFailed"] = "Re-running the failed jobs of", ["cancel"] = "Cancelling" };

    /// <summary>A run's row was clicked, expanded, or one of its actions chosen.</summary>
    public async Task RunActionAsync(long runId, string action)
    {
        var run = FindRun(runId);
        if (action == "open")
        {
            OpenBrowser(run != null ? run.S("html_url") : $"https://github.com/{Slug}/actions/runs/{runId}");
            return;
        }
        if (action is "click" or "expand")
        {
            if (openRuns.ContainsKey(runId) && action == "click")
            {
                openRuns.TryRemove(runId, out _);
            }
            else
            {
                openRuns[runId] = true;
                try
                {
                    await FetchJobsAsync(runId);
                }
                catch (ApiException e)
                {
                    openRuns.TryRemove(runId, out _);
                    Log($"github: {e.Message}");
                }
            }
            Send(Tree());
            return;
        }
        if (!RunPaths.TryGetValue(action, out var path) || NeedsToken(RunWords[action].Split(' ')[0].ToLowerInvariant()))
        {
            return;
        }
        try
        {
            await Api.RequestAsync("POST", $"/repos/{Slug}/actions/runs/{runId}" + path, new JsonObject());
            Log($"github: {RunWords[action]} {(run != null ? run.S("name") : "")} #{(run != null ? run.S("run_number") : runId.ToString())}");
        }
        catch (ApiException e)
        {
            Log($"github: {e.Message}");
            ShowView("output");
        }
        await PauseAsync(TimeSpan.FromSeconds(2));
        await RefreshAsync();
    }

    /// <summary>A pull request's row was clicked or one of its actions chosen.</summary>
    public async Task PullActionAsync(long number, string action)
    {
        if (FindPull(number) is not { } pr)
        {
            return;
        }
        if (action == "open")
        {
            OpenBrowser(pr.S("html_url"));
        }
        else if (action == "checkout")
        {
            RunInTerminal($"PR #{number}", Items.CheckoutCommand(pr, Slug), Root);
        }
        else if (action == "click")
        {
            if (openPulls.ContainsKey(number))
            {
                openPulls.TryRemove(number, out _);
            }
            else
            {
                openPulls[number] = true;
                try
                {
                    await FetchChecksAsync(pr);
                }
                catch (ApiException e)
                {
                    openPulls.TryRemove(number, out _);
                    Log($"github: {e.Message}");
                }
            }
            Send(Tree());
        }
    }

    /// <summary>An issue's row was clicked or one of its actions chosen.</summary>
    public void IssueAction(long number, string action)
    {
        if (issues.FirstOrDefault(i => Number(i) == number) is not { } issue)
        {
            return;
        }
        if (action is "click" or "open")
        {
            OpenBrowser(issue.S("html_url"));
        }
        else if (action == "branch")
        {
            var name = Items.BranchName(Settings.OrNull("github.branchName"), issue);
            RunInTerminal($"branch {name}", $"git switch -c {name}", Root);
        }
    }

    /// <summary>Downloads a job's log into the module's logs/ folder and opens it at the first error.</summary>
    public async Task OpenLogAsync(long jobId)
    {
        var job = FindJob(jobId);
        string text;
        try
        {
            text = await Api.DownloadAsync($"/repos/{Slug}/actions/jobs/{jobId}/logs");
        }
        catch (ApiException e)
        {
            if (e.Status is 403 or 404 && !Api.HasToken)
            {
                NeedsToken("Reading logs");
                return;
            }
            Log(e.Status == 404 ? "github-actions: the log isn't there (yet, or any more: logs are kept for 90 days)." : $"github: {e.Message}");
            ShowView("output");
            return;
        }
        var (clean, firstError) = Items.CleanLog(text);
        var folder = LogsDir;
        Directory.CreateDirectory(folder);
        var path = Path.Combine(folder, $"{jobId}-{Items.Slug(job != null ? job.Str("name") ?? "" : "job")}.log");
        // A new file when the job is still running (an open tab isn't reloaded from disk).
        if (job != null && Items.IsActive(Items.State(job)))
        {
            path = Path.Combine(folder, $"{jobId}-{DateTimeOffset.UtcNow.ToUnixTimeSeconds()}-{Items.Slug(job.Str("name") ?? "")}.log");
        }
        // Written as Python's text mode wrote it: the system's line endings.
        await File.WriteAllTextAsync(path, clean.Replace("\n", System.Environment.NewLine), new UTF8Encoding(false));
        OpenFile(path, firstError, firstError != null ? 0 : null);
    }

    /// <summary>"Run Workflow…": a pick of the active workflows with a workflow_dispatch trigger; starts the one picked.</summary>
    public async Task DispatchAsync()
    {
        if (Repo == null || NeedsToken("Running a workflow"))
        {
            return;
        }
        JsonNode? data;
        try
        {
            data = await Api.RequestAsync("GET", $"/repos/{Slug}/actions/workflows?per_page=100");
        }
        catch (ApiException e)
        {
            Log($"github: {e.Message}");
            return;
        }
        var local = DispatchableWorkflows(Root ?? "");
        var workflows = Objects(data.Get("workflows")).Where(w => w.Str("state") == "active" && local.Contains(Path.GetFileName(w.Str("path") ?? ""))).ToList();
        if (workflows.Count == 0)
        {
            Log("github-actions: no active workflow here has a workflow_dispatch trigger.");
            ShowView("output");
            return;
        }
        var branch = (Repo.Branch ?? "None");
        var picked = await QuickPickAsync($"Run a workflow on {branch}", workflows.Select(w => new PickItem(w.S("name"), w.S("id"), w.S("path"))));
        if (!string.IsNullOrEmpty(picked))
        {
            await StartWorkflowAsync(picked);
        }
    }

    async Task StartWorkflowAsync(string workflowId)
    {
        var branch = Repo!.Branch;
        try
        {
            await Api.RequestAsync("POST", $"/repos/{Slug}/actions/workflows/{workflowId}/dispatches", new JsonObject { ["ref"] = branch });
            Log($"github: started the workflow on {(branch ?? "None")} (inputs take their defaults)");
        }
        catch (ApiException e)
        {
            Log($"github: {e.Message}");
            ShowView("output");
            return;
        }
        await PauseAsync(TimeSpan.FromSeconds(3));  // the run shows up in the API a moment later
        await RefreshAsync();
    }

    // ───────────── messages ─────────────

    protected override Task OnStartAsync()
    {
        foreach (var (id, title) in Commands)
        {
            RegisterCommand(id, title, "GitHub");
        }
        _ = Task.Run(PollAsync);
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync()
    {
        Root = Workspace;
        RefreshInBackground(reconfigure: true);
        return Task.CompletedTask;
    }

    protected override Task OnSettingsChangedAsync()
    {
        RefreshInBackground(reconfigure: true);
        return Task.CompletedTask;
    }

    protected override async Task OnEventAsync(string name, JsonObject message)
    {
        try
        {
            switch (name)
            {
                case "folderOpened":
                    Root = message.Str("path");
                    branchRuns = [];
                    allRuns = [];
                    jobs.Clear();
                    openRuns.Clear();
                    pulls = [];
                    issues = [];
                    releases = [];
                    checks.Clear();
                    openPulls.Clear();
                    RefreshInBackground(reconfigure: true);
                    break;
                case "viewShown" when message.Str("view") == View:
                    visible = true;
                    RefreshInBackground(reconfigure: true);
                    break;
                case "viewHidden" when message.Str("view") == View:
                    visible = false;
                    break;
                case "filesChanged" when Repo != null:
                    // A checkout changes the branch; check it cheaply (git only) before asking GitHub.
                    if (await FindRepositoryAsync(Root) != Repo)
                    {
                        RefreshInBackground(reconfigure: true);
                    }
                    break;
            }
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            Log($"github: {e.GetType().Name}: {e.Message}");
        }
    }

    protected override async Task OnCommandAsync(string id)
    {
        try
        {
            switch (id)
            {
                case "github.show":
                    ShowView(View);
                    break;
                case "github.refresh":
                    RefreshInBackground(reconfigure: true);
                    break;
                case "github.dispatch":
                    await DispatchAsync();
                    break;
                case "github.openInBrowser" when Repo != null:
                    OpenBrowser($"https://github.com/{Slug}/actions");
                    break;
                case "github.openPullRequest":
                    if ((Repo != null ? BranchPull() : null) is { } pr)
                    {
                        OpenBrowser(pr.S("html_url"));
                    }
                    else
                    {
                        ShowView(View);
                    }
                    break;
                case "github.openRepository" when Repo != null:
                    OpenBrowser($"https://github.com/{Slug}");
                    break;
            }
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            Log($"github: {e.GetType().Name}: {e.Message}");
        }
    }

    protected override async Task OnTreeActionAsync(string view, string? item, string action)
    {
        try
        {
            await TreeActionAsync(item, action);
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            Log($"github: {e.GetType().Name}: {e.Message}");
        }
    }

    async Task TreeActionAsync(string? item, string action)
    {
        if (item == null)
        {
            if (action == "refresh")
            {
                RefreshInBackground(reconfigure: true);
            }
            else if (action == "dispatch")
            {
                await DispatchAsync();
            }
            else if (action == "browser" && Repo != null)
            {
                OpenBrowser($"https://github.com/{Slug}/actions");
            }
            return;
        }
        if (item == "issues" && action == "newIssue")
        {
            OpenBrowser($"https://github.com/{Slug}/issues/new");
            return;
        }
        var colon = item.IndexOf(':');
        var prefix = colon < 0 ? item : item[..colon];
        var rest = colon < 0 ? "" : item[(colon + 1)..];
        switch (prefix)
        {
            case "pr":
                await PullActionAsync(long.Parse(rest, CultureInfo.InvariantCulture), action);
                break;
            case "issue":
                IssueAction(long.Parse(rest, CultureInfo.InvariantCulture), action);
                break;
            case "release" when action == "click":
                if (releases.FirstOrDefault(r => r.S("id") == rest) is { } release)
                {
                    OpenBrowser(release.S("html_url"));
                }
                break;
            case "check" when action == "click":
                var parts = rest.Split(':');
                if (parts.Length != 2)
                {
                    throw new FormatException($"not enough values to unpack: {rest}");
                }
                if (parts[1] == "actions")
                {
                    // A GitHub Actions check run is a job: same id.
                    await OpenLogAsync(long.Parse(parts[0], CultureInfo.InvariantCulture));
                }
                else if (checks.Values.SelectMany(c => c).FirstOrDefault(c => c.S("id") == parts[0]) is { } check)
                {
                    OpenBrowser(check.OrNull("details_url") ?? check.S("html_url"));
                }
                break;
            case "run":
                await RunActionAsync(long.Parse(rest, CultureInfo.InvariantCulture), action);
                break;
            case "job" when action is "click" or "log":
                await OpenLogAsync(long.Parse(rest, CultureInfo.InvariantCulture));
                break;
            case "job" when action == "openJob":
                if (FindJob(long.Parse(rest, CultureInfo.InvariantCulture)) is { } job && job.OrNull("html_url") is { } url)
                {
                    OpenBrowser(url);
                }
                break;
            case "step" when action == "click":
                await OpenLogAsync(long.Parse(rest.Split(':')[0], CultureInfo.InvariantCulture));
                break;
            case "wf" when action == "openWorkflow":
                if (allRuns.FirstOrDefault(r => (r.OrNull("name") ?? r.OrNull("path")) == rest) is { } first)
                {
                    OpenBrowser($"https://github.com/{Slug}/actions/workflows/{Path.GetFileName(first.Str("path") ?? "")}");
                }
                break;
        }
    }
}
