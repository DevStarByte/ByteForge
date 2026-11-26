using System.Globalization;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace ByteForge.GithubActions;

/// <summary>How runs, jobs, checks, pull requests, issues and releases look in the view and the status bar.</summary>
public static partial class Items
{
    public const string Green = "#3fb950", Yellow = "#d29922", Red = "#f85149", Grey = "#8b949e", Blue = "#58a6ff";

    /// <summary>The icon and its color for each <see cref="State"/>.</summary>
    public static readonly IReadOnlyDictionary<string, (string Icon, string Color)> StateIcons = new Dictionary<string, (string, string)>
    {
        ["success"] = ("check", Green),
        ["failure"] = ("error", Red),
        ["cancelled"] = ("stop", Grey),
        ["skipped"] = ("chevronRight", Grey),
        ["running"] = ("sync", Yellow),
        ["queued"] = ("pause", Yellow),
        ["neutral"] = ("info", Grey),
    };

    public static readonly IReadOnlyDictionary<string, string> StateSymbols = new Dictionary<string, string>
    {
        ["success"] = "✓",
        ["failure"] = "✗",
        ["cancelled"] = "⊘",
        ["skipped"] = "↷",
        ["running"] = "●",
        ["queued"] = "○",
        ["neutral"] = "–",
    };

    public static readonly IReadOnlyDictionary<string, string> StateWords = new Dictionary<string, string>
    {
        ["success"] = "succeeded",
        ["failure"] = "failed",
        ["cancelled"] = "cancelled",
        ["skipped"] = "skipped",
        ["running"] = "running",
        ["queued"] = "queued",
        ["neutral"] = "finished",
    };

    // ───────────── times ─────────────

    public static DateTimeOffset? ParseTime(string? value) =>
        string.IsNullOrEmpty(value) ? null : DateTimeOffset.Parse(value, CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal);

    /// <summary>"5m ago", "2h ago", "3d ago", "just now"; "" without a time.</summary>
    public static string Ago(string? value, DateTimeOffset? now = null)
    {
        if (ParseTime(value) is not { } moment)
        {
            return "";
        }
        var seconds = (long)((now ?? DateTimeOffset.UtcNow) - moment).TotalSeconds;
        foreach (var (unit, size) in new[] { ("d", 86400L), ("h", 3600L), ("m", 60L) })
        {
            if (seconds >= size)
            {
                return $"{seconds / size}{unit} ago";
            }
        }
        return "just now";
    }

    /// <summary>"12s", "2m 5s", "1h 30m" from start to end (or now); "" without a start.</summary>
    public static string Duration(string? start, string? end)
    {
        if (ParseTime(start) is not { } a)
        {
            return "";
        }
        var seconds = (long)((ParseTime(end) ?? DateTimeOffset.UtcNow) - a).TotalSeconds;
        if (seconds < 0)
        {
            return "";
        }
        if (seconds >= 3600)
        {
            return $"{seconds / 3600}h {seconds % 3600 / 60}m";
        }
        if (seconds >= 60)
        {
            return $"{seconds / 60}m {seconds % 60}s";
        }
        return $"{seconds}s";
    }

    // ───────────── states ─────────────

    /// <summary>One of success, failure, cancelled, skipped, running, queued, neutral.</summary>
    public static string State(JsonNode? item)
    {
        var status = item.Str("status");
        var conclusion = item.Str("conclusion");
        if (status is "queued" or "waiting" or "pending" or "requested")
        {
            return "queued";
        }
        if (status == "in_progress")
        {
            return "running";
        }
        return conclusion switch
        {
            "success" => "success",
            "failure" or "timed_out" or "startup_failure" or "action_required" => "failure",
            "cancelled" => "cancelled",
            "skipped" => "skipped",
            _ => "neutral",
        };
    }

    public static bool IsActive(string state) => state is "running" or "queued";

    /// <summary>The combined state of a commit's check runs (null without any).</summary>
    public static string? ChecksState(IEnumerable<JsonNode?> checks)
    {
        var states = checks.Select(State).ToList();
        if (states.Count == 0)
        {
            return null;
        }
        if (states.Contains("failure"))
        {
            return "failure";
        }
        if (states.Contains("running") || states.Contains("queued"))
        {
            return "running";
        }
        if (states.Contains("cancelled"))
        {
            return "cancelled";
        }
        return "success";
    }

    // ───────────── rows ─────────────

    static string JoinParts(IEnumerable<string?> parts) => string.Join(" · ", parts.Where(p => !string.IsNullOrEmpty(p)));

    static string JoinLines(IEnumerable<string?> lines) => string.Join("\n", lines.Where(l => !string.IsNullOrEmpty(l)));

    static JsonObject Menu(string id, string? label = null) => label == null ? new() { ["id"] = id } : new() { ["id"] = id, ["label"] = label };

    static JsonObject Action(string id, string icon, string tooltip) => new() { ["id"] = id, ["icon"] = icon, ["tooltip"] = tooltip };

    /// <summary>A run's row; with <paramref name="jobs"/> (even empty) its jobs below it.</summary>
    public static JsonObject RunItem(JsonObject run, IReadOnlyList<JsonObject>? jobs = null, bool expanded = false, bool withWorkflow = true)
    {
        var s = State(run);
        var (icon, color) = StateIcons[s];
        var title = run.OrNull("display_title") ?? (run.Get("head_commit").Str("message") ?? "").Split('\n')[0];
        var label = withWorkflow ? $"{run.S("name")} #{run.S("run_number")}" : $"#{run.S("run_number")} {title}";
        var took = Duration(run.Str("run_started_at") ?? run.Str("created_at"), IsActive(s) ? null : run.Str("updated_at"));
        var parts = new List<string?>();
        if (!withWorkflow)
        {
            parts.Add(run.Or("head_branch"));
        }
        parts.AddRange([run.Or("event"), Ago(run.Str("created_at")), took]);
        var actor = run.Get("actor").Str("login");
        var tooltip = JoinLines([
            $"{run.S("name")} #{run.S("run_number")}: {StateWords[s]}",
            title,
            $"{run.S("head_branch")} @ {Py.Head(run.Or("head_sha"), 7)}" + (!string.IsNullOrEmpty(actor) ? $" by {actor}" : ""),
            "Click: jobs and steps.",
        ]);
        var menu = new JsonArray { Menu("open", "Open in Browser"), Menu("-") };
        if (IsActive(s))
        {
            menu.Add(Menu("cancel", "Cancel Run"));
        }
        else
        {
            menu.Add(Menu("rerun", "Re-run All Jobs"));
            if (s == "failure")
            {
                menu.Add(Menu("rerunFailed", "Re-run Failed Jobs"));
            }
        }
        var actions = IsActive(s) ? new JsonArray { Action("cancel", "stop", "Cancel Run") } : new JsonArray { Action("rerun", "refresh", "Re-run All Jobs") };
        var id = run.S("id");
        var item = new JsonObject
        {
            ["id"] = $"run:{id}",
            ["label"] = label,
            ["description"] = JoinParts(parts),
            ["tooltip"] = tooltip,
            ["icon"] = icon,
            ["iconColor"] = color,
            ["actions"] = actions,
            ["menu"] = menu,
        };
        if (jobs != null)
        {
            var children = new JsonArray([.. jobs.Select(j => (JsonNode)JobItem(j))]);
            if (children.Count == 0)
            {
                children.Add(new JsonObject { ["id"] = $"nojobs:{id}", ["label"] = "No jobs yet", ["icon"] = "info", ["iconColor"] = Grey });
            }
            item["children"] = children;
            item["expanded"] = expanded;
        }
        return item;
    }

    /// <summary>A job's row with its steps (open when it failed).</summary>
    public static JsonObject JobItem(JsonObject job)
    {
        var s = State(job);
        var (icon, color) = StateIcons[s];
        var children = new JsonArray();
        foreach (var step in job.List("steps"))
        {
            var ss = State(step);
            var (si, sc) = StateIcons[ss];
            children.Add(new JsonObject
            {
                ["id"] = $"step:{job.S("id")}:{step.S("number")}",
                ["label"] = step.Or("name"),
                ["description"] = Duration(step.Str("started_at"), step.Str("completed_at")),
                ["icon"] = si,
                ["iconColor"] = sc,
                ["tooltip"] = $"{step.S("name")}: {StateWords[ss]}. Click: the job's log.",
            });
        }
        return new JsonObject
        {
            ["id"] = $"job:{job.S("id")}",
            ["label"] = job.Or("name"),
            ["description"] = Duration(job.Str("started_at"), job.Str("completed_at")),
            ["tooltip"] = $"{job.S("name")}: {StateWords[s]}. Click: its log.",
            ["icon"] = icon,
            ["iconColor"] = color,
            ["expanded"] = s == "failure",
            ["children"] = children,
            ["actions"] = new JsonArray { Action("log", "file", "Open Log") },
            ["menu"] = new JsonArray { Menu("log", "Open Log"), Menu("openJob", "Open in Browser") },
        };
    }

    /// <summary>A check run's row: an Actions check ("check:&lt;id&gt;:actions") opens its job's log.</summary>
    public static JsonObject CheckItem(JsonObject check)
    {
        var s = State(check);
        var (icon, color) = StateIcons[s];
        var app = check.Get("app").Str("slug");
        return new JsonObject
        {
            ["id"] = $"check:{check.S("id")}:{(app == "github-actions" ? "actions" : "other")}",
            ["label"] = check.Or("name"),
            ["icon"] = icon,
            ["iconColor"] = color,
            ["description"] = Duration(check.Str("started_at"), check.Str("completed_at")),
            ["tooltip"] = $"{check.S("name")}: {StateWords[s]}" + (!string.IsNullOrEmpty(app) ? $" ({app})" : "") +
                (app == "github-actions" ? ". Click: its log." : ". Click: its details in the browser."),
        };
    }

    /// <summary>A pull request's row; with <paramref name="checks"/> (even empty) its checks below it.</summary>
    public static JsonObject PullItem(JsonObject pr, IReadOnlyList<JsonObject>? checks = null, bool expanded = false, string? currentBranch = null)
    {
        var head = pr.Get("head");
        var s = checks != null ? ChecksState(checks) : null;
        var (icon, color) = s != null ? StateIcons[s] : ("sourceControl", Py.Truthy(pr.Get("draft")) ? Grey : Green);
        var parts = new List<string?>
        {
            pr.Get("user").Or("login"),
            $"{head.S("ref")} → {pr.Get("base").S("ref")}",
            Ago(pr.Str("updated_at")),
        };
        if (Py.Truthy(pr.Get("draft")))
        {
            parts.Insert(0, "draft");
        }
        if (!string.IsNullOrEmpty(currentBranch) && head.Str("ref") == currentBranch)
        {
            parts.Insert(0, "this branch");
        }
        var labels = string.Join(", ", pr.List("labels").Select(l => l.Get("name") is { } n ? Py.S(n) : ""));
        var number = pr.S("number");
        var tooltip = JoinLines([
            $"#{number} {pr.S("title")}",
            labels.Length > 0 ? $"Labels: {labels}" : null,
            s != null ? $"Checks {StateWords[s]}" : null,
            "Click: its checks.",
        ]);
        var item = new JsonObject
        {
            ["id"] = $"pr:{number}",
            ["label"] = $"#{number} {pr.S("title")}",
            ["description"] = JoinParts(parts),
            ["tooltip"] = tooltip,
            ["icon"] = icon,
            ["iconColor"] = color,
            ["actions"] = new JsonArray { Action("open", "openPreview", "Open in Browser") },
            ["menu"] = new JsonArray { Menu("open", "Open in Browser"), Menu("checkout", "Check Out") },
        };
        if (checks != null)
        {
            var children = new JsonArray([.. checks.Select(c => (JsonNode)CheckItem(c))]);
            if (children.Count == 0)
            {
                children.Add(new JsonObject { ["id"] = $"nochecks:{number}", ["label"] = "No checks", ["icon"] = "info", ["iconColor"] = Grey });
            }
            item["children"] = children;
            item["expanded"] = expanded;
        }
        return item;
    }

    /// <summary>The logins an issue is assigned to.</summary>
    public static List<string?> Assignees(JsonNode? issue) => [.. issue.List("assignees").Select(a => a.Str("login"))];

    /// <summary>An issue's row (blue when it's assigned to <paramref name="me"/>).</summary>
    public static JsonObject IssueItem(JsonObject issue, string? me = null)
    {
        var labels = issue.List("labels").Select(l => l.Get("name") is { } n ? Py.S(n) : "").ToList();
        var assignees = Assignees(issue);
        var mine = me != null && assignees.Contains(me);
        var parts = new List<string?>();
        if (mine)
        {
            parts.Add("assigned to you");
        }
        parts.AddRange(labels.Take(3));
        parts.Add(Ago(issue.Str("updated_at")));
        if (Py.Truthy(issue.Get("comments")))
        {
            parts.Add($"{issue.S("comments")} comments");
        }
        var number = issue.S("number");
        return new JsonObject
        {
            ["id"] = $"issue:{number}",
            ["label"] = $"#{number} {issue.S("title")}",
            ["description"] = JoinParts(parts),
            ["icon"] = "info",
            ["iconColor"] = mine ? Blue : Green,
            ["tooltip"] = JoinLines([
                $"#{number} {issue.S("title")}",
                $"by {issue.Get("user").S("login")}",
                assignees.Count > 0 ? "Assigned to " + string.Join(", ", assignees) : null,
                "Click: open in the browser.",
            ]),
            ["actions"] = new JsonArray { Action("branch", "add", "Create Branch") },
            ["menu"] = new JsonArray { Menu("open", "Open in Browser"), Menu("branch", "Create Branch") },
        };
    }

    /// <summary>A release's row: draft, pre-release or latest, its age and downloads.</summary>
    public static JsonObject ReleaseItem(JsonObject release, bool latest = false)
    {
        var kind = Py.Truthy(release.Get("draft")) ? "draft" : Py.Truthy(release.Get("prerelease")) ? "pre-release" : latest ? "latest" : "";
        var assets = release.List("assets").ToList();
        var downloads = assets.Sum(a => a.Get("download_count") is { } c && Py.Truthy(c) ? c.GetValue<long>() : 0);
        var parts = new List<string?>
        {
            kind,
            Ago(release.Str("published_at") is { Length: > 0 } published ? published : release.Str("created_at")),
            downloads != 0 ? $"{downloads} downloads" : "",
        };
        var tag = release.Str("tag_name") ?? "";
        var name = release.OrNull("name") ?? release.Str("tag_name");
        var label = tag + (!string.IsNullOrEmpty(name) && name != release.Str("tag_name") ? $" {name}" : "");
        return new JsonObject
        {
            ["id"] = $"release:{release.S("id")}",
            ["label"] = label,
            ["description"] = JoinParts(parts),
            ["icon"] = "layers",
            ["iconColor"] = kind == "latest" ? Green : kind == "pre-release" ? Yellow : Grey,
            ["tooltip"] = $"{label}\n{assets.Count} assets. Click: open in the browser.",
        };
    }

    // ───────────── commands ─────────────

    [GeneratedRegex("[^a-z0-9]+")]
    private static partial Regex NotLowerAlnum();

    [GeneratedRegex(@"[^\w./-]+")]
    private static partial Regex NotRefChar();

    [GeneratedRegex(@"\.{2,}|/{2,}")]
    private static partial Regex RepeatedDotOrSlash();

    [GeneratedRegex(@"^[\w./-]+\z")]
    private static partial Regex SafeRef();

    /// <summary>The branch name for an issue from the pattern ({number}, {title}: lower case with dashes).</summary>
    public static string BranchName(string? pattern, JsonObject issue)
    {
        var title = Py.StripChars(Py.Head(Py.StripChars(NotLowerAlnum().Replace(issue.Or("title").ToLowerInvariant(), "-"), "-"), 40), "-");
        var number = issue.S("number");
        var name = (string.IsNullOrEmpty(pattern) ? "feature/{number}-{title}" : pattern).Replace("{number}", number).Replace("{title}", title);
        name = Py.StripChars(NotRefChar().Replace(name, "-"), "-/.");
        name = RepeatedDotOrSlash().Replace(name, "-");
        return name.Length > 0 ? name : $"issue-{number}";
    }

    /// <summary>The command line that checks out a pull request: its branch when it's in the repository, else pull/N/head.</summary>
    public static string CheckoutCommand(JsonObject pr, string repoSlug)
    {
        var head = pr.Get("head");
        var @ref = head.Or("ref");
        if (head.Get("repo").Str("full_name") == repoSlug && SafeRef().IsMatch(@ref))
        {
            return $"git fetch origin {@ref} && git switch {@ref}";
        }
        var number = pr.S("number");
        return $"git fetch origin pull/{number}/head:pr-{number} && git switch pr-{number}";
    }

    // ───────────── logs ─────────────

    [GeneratedRegex(@"^\ufeff?\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z ?", RegexOptions.Multiline)]
    private static partial Regex Timestamp();

    [GeneratedRegex(@"[^\w.-]+")]
    private static partial Regex NotSlugChar();

    /// <summary>The log without the timestamp in front of every line; and the 0-based line of its first error.</summary>
    public static (string Text, int? FirstError) CleanLog(string text)
    {
        text = Timestamp().Replace(text.Replace("\r\n", "\n"), "");
        var lines = text.Split('\n');
        for (var number = 0; number < lines.Length; number++)
        {
            if (lines[number].StartsWith("##[error]", StringComparison.Ordinal))
            {
                return (text, number);
            }
        }
        return (text, null);
    }

    /// <summary>A file name part from a job's name.</summary>
    public static string Slug(string text)
    {
        var slug = Py.Head(Py.StripChars(NotSlugChar().Replace(text, "-"), "-"), 60);
        return slug.Length > 0 ? slug : "job";
    }
}
