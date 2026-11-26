using System.Globalization;
using System.Numerics;
using System.Text;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;

namespace ByteForge.GitHooks;

/// <summary>Runs git in a folder: its output (trimmed), or null when it fails, can't start or takes over 10 seconds.</summary>
public delegate Task<string?> GitRunner(string root, params string[] args);

/// <summary>A hook file: its name without .off, where it is, whether it's on (not renamed to .off) and executable.</summary>
public sealed record Hook(string Name, string Path, bool Active, bool Executable);

/// <summary>A Husky.NET task of .husky/task-runner.json (<see cref="Index"/>: its place in "tasks").</summary>
public sealed record HuskyTask(int Index, string Name, string Group, string Command, List<string> Args);

/// <summary>The hooks git runs, the templates for new ones, and how they are listed, written and run.</summary>
public static partial class Hooks
{
    public const string View = "git-hooks";
    public const string Off = ".off";

    /// <summary>The hooks git runs on a client, with what each is for, in the order a commit and push meet them.</summary>
    public static readonly (string Name, string Description)[] All =
    [
        ("pre-commit", "before a commit is made; fails the commit when it exits non-zero"),
        ("prepare-commit-msg", "fills in the commit message before the editor opens"),
        ("commit-msg", "checks the commit message; fails the commit when it exits non-zero"),
        ("post-commit", "after a commit"),
        ("pre-rebase", "before a rebase"),
        ("post-checkout", "after a checkout or switch"),
        ("post-merge", "after a merge (and a pull)"),
        ("pre-push", "before a push; fails the push when it exits non-zero"),
        ("pre-merge-commit", "before a merge commit"),
        ("post-rewrite", "after commit --amend and rebase"),
        ("pre-auto-gc", "before git gc --auto"),
        ("applypatch-msg", "checks the message of git am"),
        ("pre-applypatch", "before git am commits"),
        ("post-applypatch", "after git am commits"),
        ("reference-transaction", "when references change"),
        ("push-to-checkout", "on a push to a checked-out branch (server side)"),
        ("sendemail-validate", "before git send-email"),
        ("fsmonitor-watchman", "file system monitor"),
        ("post-index-change", "after the index changes"),
    ];

    /// <summary>The templates of "New Hook…": the hook each is for (null: any), its title, the script after the header.</summary>
    public static readonly (string? Name, string Title, string Body)[] Templates =
    [
        ("pre-commit", "dotnet format: fail when files aren't formatted",
            "dotnet format --verify-no-changes --verbosity quiet || {\n  echo \"Files aren't formatted. Run: dotnet format\" >&2\n  exit 1\n}\n"),
        ("pre-commit", "dotnet build: fail when the solution doesn't build",
            "dotnet build --nologo --verbosity quiet || exit 1\n"),
        ("pre-commit", "No conflict markers or Console.WriteLine debugging in staged C# files",
            "files=$(git diff --cached --name-only --diff-filter=ACM -- '*.cs')\n[ -z \"$files\" ] && exit 0\nif echo \"$files\" | xargs grep -nE '^(<<<<<<<|=======|>>>>>>>)( |$)' ; then\n  echo 'Conflict markers in staged files.' >&2\n  exit 1\nfi\nif echo \"$files\" | xargs grep -n 'Console.WriteLine(\"DEBUG' ; then\n  echo 'Debug output left in staged files.' >&2\n  exit 1\nfi\n"),
        ("pre-push", "dotnet test: fail the push when tests fail",
            "dotnet test --nologo --verbosity quiet || {\n  echo 'Tests failed; not pushing.' >&2\n  exit 1\n}\n"),
        ("commit-msg", "Conventional Commits: feat:, fix:, docs: … at the start of the message",
            "msg=$(head -n1 \"$1\")\nif ! echo \"$msg\" | grep -qE '^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\\([^)]+\\))?!?: .+'; then\n  echo \"Commit message doesn't follow Conventional Commits: $msg\" >&2\n  echo 'Example: feat(api): add order export' >&2\n  exit 1\nfi\n"),
        ("prepare-commit-msg", "Put the issue number from the branch name (feature/123-…) in front of the message",
            "case \"$2\" in merge|squash|commit) exit 0 ;; esac\nissue=$(git symbolic-ref --short HEAD 2>/dev/null | grep -oE '[0-9]+' | head -n1)\n[ -n \"$issue\" ] && ! grep -q \"#$issue\" \"$1\" && sed -i.bak \"1s/^/#$issue /\" \"$1\" && rm -f \"$1.bak\"\nexit 0\n"),
        ("post-merge", "dotnet restore when a project file or Directory.Packages.props changed",
            "if git diff --name-only HEAD@{1} HEAD | grep -qE '\\.(csproj|fsproj|props|targets)$'; then\n  echo 'Project files changed; restoring…'\n  dotnet restore --verbosity quiet\nfi\n"),
        (null, "Empty hook",
            "# Exit non-zero to stop the git command (for the pre-* hooks and commit-msg).\nexit 0\n"),
    ];

    public static bool IsHook(string name) => All.Any(h => h.Name == name);

    public static string Describe(string name) => All.First(h => h.Name == name).Description;

    static int Order(string name) => Array.FindIndex(All, h => h.Name == name);

    /// <summary>git through <see cref="Programs.RunAsync"/>, as the Python's subprocess.run with a 10 second timeout.</summary>
    public static async Task<string?> RunGit(string root, params string[] args)
    {
        if (Programs.Which("git") is not { } program)
        {
            return null;
        }
        try
        {
            var result = await Programs.RunAsync(program, args, workingDirectory: root, timeout: TimeSpan.FromSeconds(10));
            return result.Success ? result.Output.Trim() : null;
        }
        catch (Exception e) when (e is ModuleException or TimeoutException or IOException or InvalidOperationException)
        {
            return null;
        }
    }

    /// <summary>The folder git runs hooks from and its core.hooksPath (or null), or (null, null) outside a repository.</summary>
    public static async Task<(string? Folder, string? Configured)> HooksDir(string root, GitRunner git)
    {
        var path = await git(root, "rev-parse", "--git-path", "hooks");
        if (path == null)
        {
            return (null, null);
        }
        var configured = await git(root, "config", "core.hooksPath");
        return (NormPath(System.IO.Path.Combine(root, path)), configured);
    }

    /// <summary>
    /// Python's os.path.normpath: the platform's separators, "." and "x/.." taken out. Unlike Path.GetFullPath it
    /// leaves Windows' short names (C:\Users\ABCDEF~1) alone, so the folder starts with the workspace path as the
    /// IDE gave it (fileSaved paths are compared with it).
    /// </summary>
    public static string NormPath(string path)
    {
        if (path.Length == 0)
        {
            return ".";
        }
        var separator = System.IO.Path.DirectorySeparatorChar;
        if (OperatingSystem.IsWindows())
        {
            path = path.Replace('/', '\\');
        }
        var prefix = System.IO.Path.GetPathRoot(path) ?? "";
        if (!path.StartsWith(prefix, StringComparison.Ordinal))
        {
            return path;  // a root GetPathRoot rewrote (\\\x): left as it is, as Python does
        }
        if (!prefix.EndsWith(separator) && prefix.Length < path.Length && path[prefix.Length] == separator)
        {
            prefix += separator;  // \\server\share\ (Windows' GetPathRoot leaves the separator after the share out)
        }
        var rooted = prefix.EndsWith(separator);
        if (!OperatingSystem.IsWindows() && path.StartsWith("//", StringComparison.Ordinal) && !path.StartsWith("///", StringComparison.Ordinal))
        {
            prefix = "//";  // POSIX keeps exactly two leading slashes
        }
        var parts = new List<string>();
        foreach (var part in path[prefix.Length..].Split(separator))
        {
            if (part is "" or ".")
            {
                continue;
            }
            if (part == ".." && (rooted || (parts.Count > 0 && parts[^1] != "..")))
            {
                if (parts.Count > 0)
                {
                    parts.RemoveAt(parts.Count - 1);
                }
                continue;
            }
            parts.Add(part);
        }
        var normal = prefix + string.Join(separator, parts);
        return normal.Length > 0 ? normal : ".";
    }

    public static bool IsExecutable(string path) =>
        OperatingSystem.IsWindows() || (File.GetUnixFileMode(path) & (UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute)) != 0;

    /// <summary>The hooks in the folder (samples and other files left out), in the order of <see cref="All"/>.</summary>
    public static List<Hook> List(string? folder)
    {
        var hooks = new List<Hook>();
        if (string.IsNullOrEmpty(folder))
        {
            return hooks;
        }
        List<string> names;
        try
        {
            names = [.. Directory.EnumerateFileSystemEntries(folder).Select(p => System.IO.Path.GetFileName(p))];
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or ArgumentException)
        {
            return hooks;
        }
        names.Sort(StringComparer.Ordinal);
        foreach (var name in names)
        {
            var path = System.IO.Path.Combine(folder!, name);
            if (!File.Exists(path) || name.EndsWith(".sample", StringComparison.Ordinal))
            {
                continue;
            }
            var off = name.EndsWith(Off, StringComparison.Ordinal);
            var baseName = off ? name[..^Off.Length] : name;
            if (!IsHook(baseName))
            {
                continue;
            }
            hooks.Add(new Hook(baseName, path, !off, IsExecutable(path)));
        }
        return [.. hooks.OrderBy(h => Order(h.Name))];
    }

    [GeneratedRegex(@"(""(?:\\.|[^""\\])*"")|//[^\n]*|/\*.*?\*/", RegexOptions.Singleline)]
    private static partial Regex Comments();

    [GeneratedRegex(@"(""(?:\\.|[^""\\])*"")|,(\s*[}\]])")]
    private static partial Regex TrailingCommas();

    /// <summary>JSON with // and /* */ comments and trailing commas made plain JSON (strings left alone).</summary>
    public static string StripJsonComments(string text)
    {
        text = Comments().Replace(text, m => m.Groups[1].Success ? m.Groups[1].Value : "");
        return TrailingCommas().Replace(text, m => m.Groups[1].Success ? m.Groups[1].Value : m.Groups[2].Value);
    }

    /// <summary>Python's truth value of a JSON value.</summary>
    static bool Truthy(object? value) => value switch
    {
        null => false,
        bool b => b,
        string s => s.Length > 0,
        BigInteger n => !n.IsZero,
        double d => d != 0,
        List<object?> l => l.Count > 0,
        OrderedDictionary<string, object?> o => o.Count > 0,
        _ => true,
    };

    /// <summary>Python's str() of a JSON value.</summary>
    public static string PyStr(object? value) => value switch
    {
        string s => s,
        _ => PyRepr(value),
    };

    static string PyRepr(object? value) => value switch
    {
        null => "None",
        bool b => b ? "True" : "False",
        string s => StringRepr(s),
        BigInteger n => n.ToString(CultureInfo.InvariantCulture),
        double d => double.IsNaN(d) ? "nan" : double.IsPositiveInfinity(d) ? "inf" : double.IsNegativeInfinity(d) ? "-inf" : PyJson.FloatText(d),
        List<object?> l => "[" + string.Join(", ", l.Select(PyRepr)) + "]",
        OrderedDictionary<string, object?> o => "{" + string.Join(", ", o.Select(p => StringRepr(p.Key) + ": " + PyRepr(p.Value))) + "}",
        _ => value.ToString() ?? "",
    };

    /// <summary>repr() of a string: single quotes unless it holds one and no double quote; \\, \n, \r, \t and controls escaped.</summary>
    static string StringRepr(string s)
    {
        var quote = s.Contains('\'') && !s.Contains('"') ? '"' : '\'';
        var text = new StringBuilder().Append(quote);
        foreach (var c in s)
        {
            text.Append(c switch
            {
                '\\' => "\\\\",
                '\n' => "\\n",
                '\r' => "\\r",
                '\t' => "\\t",
                _ when c == quote => "\\" + c,
                _ when c < ' ' || c == '\x7f' => $"\\x{(int)c:x2}",
                _ => c.ToString(),
            });
        }
        return text.Append(quote).ToString();
    }

    /// <summary>Husky.NET tasks from .husky/task-runner.json, or null when there's none (or it can't be read).</summary>
    public static List<HuskyTask>? HuskyTasks(string root)
    {
        object? data;
        try
        {
            var bytes = File.ReadAllBytes(System.IO.Path.Combine(root, ".husky", "task-runner.json"));
            var skip = bytes.AsSpan().StartsWith((ReadOnlySpan<byte>)[0xEF, 0xBB, 0xBF]) ? 3 : 0;
            var text = new UTF8Encoding(false, throwOnInvalidBytes: true).GetString(bytes, skip, bytes.Length - skip).Replace("\r\n", "\n").Replace('\r', '\n');
            data = PyJson.Loads(StripJsonComments(text));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonDecodeError or DecoderFallbackException)
        {
            return null;
        }
        if (data is not OrderedDictionary<string, object?> map)
        {
            return null;  // the Python failed here (data.get on a non-object)
        }
        var tasks = new List<HuskyTask>();
        var list = map.TryGetValue("tasks", out var t) && t is List<object?> l ? l : [];
        for (var i = 0; i < list.Count; i++)
        {
            if (list[i] is not OrderedDictionary<string, object?> task || !Truthy(task.GetValueOrDefault("command")))
            {
                continue;
            }
            var command = PyStr(task["command"]);
            var name = task.GetValueOrDefault("name") is var n && Truthy(n) ? PyStr(n) : command;
            var group = task.GetValueOrDefault("group") is var g && Truthy(g) ? PyStr(g) : "";
            var args = task.GetValueOrDefault("args") is var a && Truthy(a)
                ? a switch
                {
                    List<object?> items => items.Select(PyStr).ToList(),
                    string s => s.Select(c => c.ToString()).ToList(),
                    OrderedDictionary<string, object?> o => [.. o.Keys],
                    _ => [],
                }
                : [];
            tasks.Add(new HuskyTask(i, name, group, command, args));
        }
        return tasks;
    }

    [GeneratedRegex(@"[\s""]")]
    private static partial Regex NeedsWindowsQuotes();

    [GeneratedRegex(@"[^a-zA-Z0-9_@%+=:,./-]")]
    private static partial Regex UnsafeForShell();

    /// <summary>An argument for a command line: shlex.quote, or double quotes on Windows when it has blanks or quotes.</summary>
    public static string Quote(string arg)
    {
        if (OperatingSystem.IsWindows())
        {
            return NeedsWindowsQuotes().IsMatch(arg) ? "\"" + arg.Replace("\"", "\\\"") + "\"" : arg;
        }
        if (arg.Length == 0)
        {
            return "''";
        }
        return UnsafeForShell().IsMatch(arg) ? "'" + arg.Replace("'", "'\"'\"'") + "'" : arg;
    }

    /// <summary>
    /// Python's tempfile.gettempdir(): TMPDIR, TEMP or TMP when it's a folder (as given: Path.GetTempPath would expand
    /// Windows' short names), else the system's.
    /// </summary>
    static string TempDir()
    {
        foreach (var variable in new[] { "TMPDIR", "TEMP", "TMP" })
        {
            if (Environment.GetEnvironmentVariable(variable) is { Length: > 0 } dir && Directory.Exists(dir))
            {
                return NormPath(System.IO.Path.IsPathRooted(dir) ? dir : System.IO.Path.Combine(Environment.CurrentDirectory, dir));
            }
        }
        return System.IO.Path.GetTempPath();
    }

    /// <summary>A commit message file to try commit-msg hooks with (in the temp folder, written in text mode as Python did).</summary>
    public static string SampleMessage()
    {
        const string letters = "abcdefghijklmnopqrstuvwxyz0123456789_";
        var folder = TempDir();
        while (true)
        {
            var name = "byteforge-commit-msg-" + new string([.. Enumerable.Range(0, 8).Select(_ => letters[Random.Shared.Next(letters.Length)])]) + ".txt";
            var path = System.IO.Path.Combine(folder, name);
            try
            {
                var options = new FileStreamOptions { Mode = FileMode.CreateNew, Access = FileAccess.Write };
                if (!OperatingSystem.IsWindows())
                {
                    options.UnixCreateMode = UnixFileMode.UserRead | UnixFileMode.UserWrite;  // as mkstemp: 0600
                }
                using var file = new FileStream(path, options);
                file.Write(Encoding.UTF8.GetBytes("feat: try the commit-msg hook from ByteForge" + Environment.NewLine));
                return path;
            }
            catch (IOException) when (File.Exists(path))
            {
            }
        }
    }

    /// <summary>The command line that runs a hook as git would: with git's arguments for it, from the work tree.</summary>
    public static async Task<string> Command(Hook hook, string root, GitRunner git)
    {
        var args = new List<string>();
        switch (hook.Name)
        {
            case "commit-msg" or "prepare-commit-msg" or "applypatch-msg":
                args.Add(SampleMessage());
                if (hook.Name == "prepare-commit-msg")
                {
                    args.Add("message");
                }
                break;
            case "pre-push":
                var remotes = (await git(root, "remote") ?? "").Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
                var remote = remotes.Contains("origin") ? "origin" : "";
                var url = remote.Length > 0 ? await git(root, "remote", "get-url", remote) : "";
                args.AddRange([remote.Length > 0 ? remote : "origin", url ?? ""]);
                break;
            case "post-checkout":
                var head = await git(root, "rev-parse", "HEAD") ?? "";
                args.AddRange([head, head, "1"]);
                break;
            case "post-merge":
                args.Add("0");
                break;
        }
        var line = string.Join(" ", new[] { hook.Path }.Concat(args).Select(Quote));
        if (OperatingSystem.IsWindows())
        {
            return "sh " + line;  // Git for Windows' sh runs hooks
        }
        if (hook.Name == "pre-push")
        {
            line += " < /dev/null";  // git feeds the refs being pushed on stdin; nothing here
        }
        return line;
    }

    public static void MakeExecutable(string path)
    {
        if (!OperatingSystem.IsWindows())
        {
            File.SetUnixFileMode(path, File.GetUnixFileMode(path) | UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute);
        }
    }

    /// <summary>The bytes of a hook file: a #!/bin/sh header naming the hook, then <paramref name="body"/>; LF line ends, UTF-8.</summary>
    public static byte[] HookFile(string name, string body) =>
        Encoding.UTF8.GetBytes("#!/bin/sh\n" + $"# git {name} hook: {Describe(name)}.\n# Written by ByteForge's Git Hooks module.\n\n" + body);

    /// <summary>Writes a hook into <paramref name="folder"/> and makes it executable; its path.</summary>
    public static string Write(string folder, string name, string body)
    {
        Directory.CreateDirectory(folder);
        var path = System.IO.Path.Combine(folder, name);
        File.WriteAllBytes(path, HookFile(name, body));
        MakeExecutable(path);
        return path;
    }

    // ───────────── the view ─────────────

    /// <summary>The setTree message for the view, as the Python built it.</summary>
    public static JsonObject Tree(string? root, string? folder, string? configured)
    {
        if (string.IsNullOrEmpty(root))
        {
            return new JsonObject { ["type"] = "setTree", ["view"] = View, ["items"] = new JsonArray(), ["message"] = "Open a folder to see its git hooks." };
        }
        if (string.IsNullOrEmpty(folder))
        {
            return new JsonObject { ["type"] = "setTree", ["view"] = View, ["items"] = new JsonArray(), ["message"] = "This folder isn't a git repository." };
        }
        var rows = new JsonArray();
        foreach (var h in List(folder))
        {
            var notes = new List<string>();
            if (!h.Active)
            {
                notes.Add("off");
            }
            else if (!h.Executable)
            {
                notes.Add("not executable: git skips it");
            }
            var menu = new JsonArray(
                new JsonObject { ["id"] = "open", ["label"] = "Open" },
                new JsonObject { ["id"] = "run", ["label"] = "Run" },
                new JsonObject { ["id"] = "-" },
                new JsonObject { ["id"] = "toggle", ["label"] = h.Active ? "Turn Off" : "Turn On" });
            if (h.Active && !h.Executable)
            {
                menu.Add(new JsonObject { ["id"] = "chmod", ["label"] = "Make Executable" });
            }
            menu.Add(new JsonObject { ["id"] = "-" });
            menu.Add(new JsonObject { ["id"] = "delete", ["label"] = "Delete…" });
            var ready = h.Active && h.Executable;
            var row = new JsonObject
            {
                ["id"] = $"hook:{h.Name}",
                ["label"] = h.Name,
                ["description"] = notes.Count > 0 ? string.Join(", ", notes) : Describe(h.Name),
                ["tooltip"] = $"{h.Path}\nRuns {Describe(h.Name)}.\nClick: open it.",
                ["icon"] = ready ? "play" : "pause",
            };
            if (!ready)
            {
                row["iconColor"] = "foreground.muted";
            }
            row["actions"] = new JsonArray(new JsonObject { ["id"] = "run", ["icon"] = "play", ["tooltip"] = "Run (as git would)" });
            row["menu"] = menu;
            rows.Add(row);
        }
        var rel = System.IO.Path.GetRelativePath(root, folder).Replace(System.IO.Path.DirectorySeparatorChar, '/');
        var where = !string.IsNullOrEmpty(configured) ? $"{rel} (core.hooksPath)" : rel;
        var count = rows.Count;
        var items = new JsonArray(new JsonObject
        {
            ["id"] = "hooks",
            ["label"] = $"Hooks in {where}",
            ["header"] = true,
            ["expanded"] = true,
            ["count"] = count,
            ["children"] = count > 0 ? rows : new JsonArray(new JsonObject { ["id"] = "none", ["label"] = "No hooks. + adds one.", ["icon"] = "info" }),
        });
        if (HuskyTasks(root) is { } tasks)
        {
            var groups = new OrderedDictionary<string, List<HuskyTask>>();
            foreach (var t in tasks)
            {
                var group = t.Group.Length > 0 ? t.Group : "(no group: run by name)";
                if (!groups.TryGetValue(group, out var members))
                {
                    groups[group] = members = [];
                }
                members.Add(t);
            }
            var children = new JsonArray();
            foreach (var (group, members) in groups)
            {
                children.Add(new JsonObject
                {
                    ["id"] = $"husky-group:{group}",
                    ["label"] = group,
                    ["icon"] = "layers",
                    ["expanded"] = true,
                    ["count"] = members.Count,
                    ["children"] = new JsonArray([.. members.Select(t =>
                    {
                        var description = string.Join(" ", new[] { t.Command }.Concat(t.Args));
                        return (JsonNode)new JsonObject
                        {
                            ["id"] = $"husky:{t.Index}",
                            ["label"] = t.Name,
                            ["description"] = description.Length > 80 ? description[..80] : description,
                            ["tooltip"] = "Click: run it with dotnet husky run --name …",
                            ["icon"] = "play",
                            ["actions"] = new JsonArray(new JsonObject { ["id"] = "run", ["icon"] = "play", ["tooltip"] = "Run" }),
                        };
                    })]),
                });
            }
            items.Add(new JsonObject
            {
                ["id"] = "husky",
                ["label"] = "Husky.NET tasks",
                ["header"] = true,
                ["expanded"] = true,
                ["count"] = tasks.Count,
                ["children"] = children,
                ["actions"] = new JsonArray(new JsonObject { ["id"] = "huskyOpen", ["icon"] = "file", ["tooltip"] = "Open task-runner.json" }),
            });
        }
        string? message = null;
        if (!string.IsNullOrEmpty(configured) && !Directory.Exists(folder))
        {
            message = $"core.hooksPath is {configured}, which doesn't exist yet.";
        }
        return new JsonObject
        {
            ["type"] = "setTree",
            ["view"] = View,
            ["items"] = items,
            ["message"] = message,
            ["actions"] = new JsonArray(
                new JsonObject { ["id"] = "new", ["icon"] = "add", ["tooltip"] = "New Hook…" },
                new JsonObject { ["id"] = "refresh", ["icon"] = "refresh", ["tooltip"] = "Refresh" }),
        };
    }
}
