using System.Globalization;
using System.Text;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;

namespace ByteForge.LogFiles;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<LogFilesModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// Commands to find what matters in .log files (their highlighting is declared in module.json).
/// "Log: List Errors" prints every error (with its exception and the first stack frame) to the Output as
/// path:line: text, so a click goes there. "Log: Filter…" opens a copy with only errors, warnings and up, without
/// debug and trace, or just the exceptions with their stack traces. "Log: Follow" runs tail -F (Get-Content -Wait
/// on Windows) in a terminal. Logs are read as saved. The status bar counts the errors and warnings of the open log.
/// </summary>
public sealed class LogFilesModule : Module
{
    public const int MaxListed = 500;
    public const string StatusId = "log.counts";

    public static readonly (string Id, string Title)[] Commands =
    [
        ("log.listErrors", "List Errors"),
        ("log.filter", "Filter…"),
        ("log.follow", "Follow (tail -f)"),
    ];

    /// <summary>The filters of "Log: Filter…": key, title, detail.</summary>
    public static readonly (string Key, string Title, string Detail)[] Filters =
    [
        ("errors", "Errors only", "error and fatal entries, with their stack traces"),
        ("warnings", "Warnings and errors", "warning, error and fatal entries"),
        ("nodebug", "Without debug and trace", "everything but debug and trace entries"),
        ("exceptions", "Exceptions", "entries with an exception or a stack trace"),
    ];

    readonly Func<DateTime> now;

    /// <summary>The open log file, or null when the active tab isn't one.</summary>
    volatile string? active;

    public LogFilesModule() : this(() => DateTime.Now)
    {
    }

    /// <param name="now">The local time (stamped into the names of filtered copies).</param>
    public LogFilesModule(Func<DateTime> now) => this.now = now;

    /// <summary>Where "Log: Filter…" writes its copies: the module's own folder.</summary>
    public string FilteredDir => Path.Combine(ModuleDir, "filtered");

    protected override Task OnStartAsync()
    {
        foreach (var (id, title) in Commands)
        {
            RegisterCommand(id, title, "Log");
        }
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        var path = message["path"] is JsonValue value && value.TryGetValue<string>(out var text) ? text : null;
        if (name == "editorChanged")
        {
            active = !string.IsNullOrEmpty(path) && LogParser.IsLog(path) ? path : null;
            Guard(UpdateStatus);
        }
        else if (name == "fileSaved" && path == active)
        {
            Guard(UpdateStatus);
        }
        return Task.CompletedTask;
    }

    protected override async Task OnCommandAsync(string id)
    {
        try
        {
            switch (id)
            {
                case "log.listErrors":
                    ListErrors();
                    break;
                case "log.filter":
                    await PickFilter();
                    break;
                case "log.follow":
                    Follow();
                    break;
            }
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            Log($"log: {e.GetType().Name}: {e.Message}");
        }
    }

    void Guard(Action work)
    {
        try
        {
            work();
        }
        catch (Exception e)
        {
            Log($"log: {e.GetType().Name}: {e.Message}");
        }
    }

    string? Target()
    {
        var path = active;
        if (path == null)
        {
            Log("log: open a .log file first (it's read as saved).");
            ShowView("output");
        }
        return path;
    }

    void ListErrors()
    {
        if (Target() is not { } path)
        {
            return;
        }
        var lines = LogParser.ReadLines(path);
        var found = LogParser.ListErrors(path, lines);
        var c = LogParser.Counts(lines);
        Log($"log: {Path.GetFileName(path)}: {c["fatal"]} fatal, {c["error"]} errors, {c["warning"]} warnings");
        foreach (var line in found.Skip(Math.Max(0, found.Count - MaxListed)))
        {
            Log(line);
        }
        if (found.Count > MaxListed)
        {
            Log($"log: … and {found.Count - MaxListed} older errors (\"Log: Filter…\" shows them all)");
        }
        ShowView("output");
    }

    async Task PickFilter()
    {
        if (Target() == null)
        {
            return;
        }
        var picked = await QuickPickAsync("Show only…", Filters.Select(f => new PickItem(f.Title, f.Key, f.Detail)));
        if (picked != null && Filters.Any(f => f.Key == picked))
        {
            Filter(picked);
        }
    }

    /// <summary>Writes the entries of the open log that pass filter <paramref name="which"/> to a new file and opens it.</summary>
    public void Filter(string which)
    {
        if (active is not { } path)
        {
            return;
        }
        var kept = LogParser.ApplyFilter(LogParser.ReadLines(path), which);
        if (kept.Count == 0)
        {
            Log($"log: nothing in {Path.GetFileName(path)} for \"{Filters.First(f => f.Key == which).Title}\".");
            ShowView("output");
            return;
        }
        Directory.CreateDirectory(FilteredDir);
        var stamp = now().ToString("HHmmss", CultureInfo.InvariantCulture);
        var output = Path.Combine(FilteredDir, $"{LogParser.Stem(Path.GetFileName(path))}.{which}-{stamp}.log");
        // As before: the platform's line endings (Python's text mode wrote \r\n on Windows).
        File.WriteAllText(output, string.Join(Environment.NewLine, kept) + Environment.NewLine, new UTF8Encoding(false));
        OpenFile(output);
    }

    void Follow()
    {
        if (Target() is not { } path)
        {
            return;
        }
        var command = OperatingSystem.IsWindows() ? $"Get-Content -Wait -Tail 200 -Path {LogParser.Quote(path, windows: true)}" : $"tail -n 200 -F {LogParser.Quote(path, windows: false)}";
        RunInTerminal($"follow {Path.GetFileName(path)}", command, LogParser.Dirname(path));
    }

    void UpdateStatus()
    {
        if (active is not { } path)
        {
            Send(new() { ["type"] = "status", ["id"] = StatusId, ["text"] = "" });
            return;
        }
        Dictionary<string, int> c;
        try
        {
            c = LogParser.Counts(LogParser.ReadLines(path));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return;
        }
        var errors = c["error"] + c["fatal"];
        Send(new()
        {
            ["type"] = "status",
            ["id"] = StatusId,
            ["text"] = $"✗ {errors}  ⚠ {c["warning"]}",
            ["command"] = "log.listErrors",
            ["tooltip"] = $"{Path.GetFileName(path)}: {c["fatal"]} fatal, {c["error"]} errors, {c["warning"]} warnings, {c["info"]} info. Click: list the errors.",
        });
    }
}

/// <summary>
/// Reading logs: levels in the common formats (ASP.NET Core's console logger fail: / warn: / info:, Serilog [ERR]
/// [WRN] [INF], NLog, log4net and Microsoft.Extensions.Logging Error / Warning / Information, plain ERROR / WARN /
/// INFO, syslog, GitHub Actions ##[error]) and entries (a line with a level or a timestamp, plus the indented
/// lines, stack frames and blank lines after it).
/// </summary>
public static class LogParser
{
    public const long MaxBytes = 200_000_000;

    public static readonly string[] Order = ["trace", "debug", "info", "warning", "error", "fatal"];

    static readonly (string Level, string[] Words)[] LevelWords =
    [
        ("fatal", ["fatal", "ftl", "crit", "critical", "emerg", "emergency", "alert", "panic"]),
        ("error", ["error", "err", "eror", "fail", "severe"]),
        ("warning", ["warn", "warning", "wrn"]),
        ("info", ["info", "inf", "information", "notice", "nfo"]),
        ("debug", ["debug", "dbg", "dbug", "fine"]),
        ("trace", ["trace", "trce", "trc", "verbose", "vrb", "finest", "finer"]),
    ];

    static readonly Dictionary<string, string> WordLevel = LevelWords.SelectMany(l => l.Words.Select(w => (w, l.Level))).ToDictionary(p => p.w, p => p.Level);

    // Python's \s and \S also take the separators \x1c-\x1f for whitespace; so do these.
    const string Ws = @"[\s\x1c-\x1f]";
    const string NotWs = @"[^\s\x1c-\x1f]";
    const RegexOptions Options = RegexOptions.CultureInvariant;

    // A level word as loggers write it: at the start of the line or after the timestamp, alone, in brackets, or
    // followed by a colon / pipe; plus GitHub Actions' ##[error]. Longer words first, as alternatives are tried in order.
    static readonly Regex Level = new(
        @"^##\[(error|warning|notice|debug)\]"
        + @"|(?:^|[\s\x1c-\x1f\[|(<])(" + string.Join("|", WordLevel.Keys.OrderByDescending(w => w.Length)) + @")(?=[\]\s\x1c-\x1f:|)>]|$)",
        Options | RegexOptions.IgnoreCase);

    static readonly Regex StackFrame = new($@"^{Ws}+(?:at |---|File ""|\.\.\. \d+ more|Caused by:)|^{Ws}*--- End of", Options);
    static readonly Regex Exception = new($@"\b((?:[A-Za-z_]\w*\.)*[A-Z]\w*(?:Exception|Error))\b(?::|{Ws}*\(|$)", Options);
    static readonly Regex Continuation = new($@"^{Ws}+{NotWs}|^{Ws}*$", Options);
    static readonly Regex TimestampStart = new($@"^{Ws}*(?:\[?\d{{4}}-\d\d-\d\d|\[?\d\d:\d\d:\d\d|\w{{3}} [ \d]\d \d\d:|\d{{1,2}}/\d{{1,2}}/\d{{2,4}})", Options);
    static readonly Regex FrameStart = new($@"^{Ws}+at ", Options);
    static readonly Regex RotatedLog = new(@"\.log\.\d+$|\.log\.\d{4}-\d\d-\d\d$", Options);

    /// <summary>One log entry: where it starts (from 0), its level (or null) and its lines.</summary>
    public sealed record Entry(int Start, string? Level, List<string> Lines);

    /// <summary>The level a line is logged at, from the first level word near its start, or null.</summary>
    public static string? LevelOf(string line)
    {
        var m = Level.Match(Head(line, 80)); // loggers put the level first, or after a timestamp and a category
        if (!m.Success)
        {
            return null;
        }
        if (m.Groups[1].Success)
        {
            var word = m.Groups[1].Value.ToLowerInvariant();
            return word == "notice" ? "info" : word;
        }
        return WordLevel.GetValueOrDefault(m.Groups[2].Value.ToLowerInvariant());
    }

    /// <summary>
    /// Groups lines into log entries: a line with a level (or a timestamp) starts one; indented lines, stack frames
    /// and blank lines after it belong to it (and exception lines after a warning or an error).
    /// </summary>
    public static IEnumerable<Entry> Entries(IReadOnlyList<string> lines)
    {
        Entry? current = null;
        for (var i = 0; i < lines.Count; i++)
        {
            var line = lines[i];
            var level = LevelOf(line);
            var starts = level != null || TimestampStart.IsMatch(line);
            if (current != null && !starts && (Continuation.IsMatch(line) || StackFrame.IsMatch(line)
                || Exception.IsMatch(line) && current.Level is "error" or "fatal" or "warning"))
            {
                current.Lines.Add(line);
                continue;
            }
            if (current != null)
            {
                yield return current;
            }
            current = new Entry(i, level, [line]);
        }
        if (current != null)
        {
            yield return current;
        }
    }

    public static bool AtLeast(string? level, string minimum) => level != null && Array.IndexOf(Order, level) >= Array.IndexOf(Order, minimum);

    /// <summary>The first exception type named in these lines.</summary>
    public static string? ExceptionOf(IEnumerable<string> lines)
    {
        foreach (var line in lines)
        {
            if (Exception.Match(line) is { Success: true } m)
            {
                return m.Groups[1].Value;
            }
        }
        return null;
    }

    /// <summary>The first .NET / Java / JavaScript stack frame ("at …") in these lines.</summary>
    public static string? FirstFrame(IEnumerable<string> lines) => lines.FirstOrDefault(FrameStart.IsMatch) is { } line ? Strip(line) : null;

    /// <summary>Output lines (path:line:1: text) for the errors and fatal errors, newest last.</summary>
    public static List<string> ListErrors(string path, IReadOnlyList<string> lines)
    {
        var output = new List<string>();
        foreach (var entry in Entries(lines))
        {
            if (!AtLeast(entry.Level, "error"))
            {
                continue;
            }
            var text = Strip(entry.Lines[0]);
            var exception = !Exception.IsMatch(text) ? ExceptionOf(entry.Lines.Skip(1)) : null;
            var frame = FirstFrame(entry.Lines);
            var detail = string.Join(" · ", new[] { exception, frame }.Where(x => !string.IsNullOrEmpty(x)));
            output.Add($"{path}:{entry.Start + 1}:1: {Head(text, 300)}" + (detail.Length > 0 ? $"  [{Head(detail, 200)}]" : ""));
        }
        return output;
    }

    /// <summary>The lines of the entries that pass a filter: errors, warnings, nodebug, or (anything else) exceptions.</summary>
    public static List<string> ApplyFilter(IReadOnlyList<string> lines, string which)
    {
        var kept = new List<string>();
        foreach (var entry in Entries(lines))
        {
            var keep = which switch
            {
                "errors" => AtLeast(entry.Level, "error"),
                "warnings" => AtLeast(entry.Level, "warning"),
                "nodebug" => entry.Level is not ("debug" or "trace"),
                _ => entry.Lines.Any(Exception.IsMatch) || entry.Lines.Any(StackFrame.IsMatch),
            };
            if (keep)
            {
                kept.AddRange(entry.Lines);
            }
        }
        return kept;
    }

    /// <summary>How many entries there are of each level (in <see cref="Order"/>).</summary>
    public static Dictionary<string, int> Counts(IReadOnlyList<string> lines)
    {
        var result = Order.ToDictionary(level => level, _ => 0);
        foreach (var entry in Entries(lines))
        {
            if (entry.Level != null)
            {
                result[entry.Level]++;
            }
        }
        return result;
    }

    /// <summary>A log's lines as saved; of a huge log only the last 200 MB (from the first full line).</summary>
    public static List<string> ReadLines(string path)
    {
        byte[] data;
        using (var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete))
        {
            var size = file.Length;
            if (size > MaxBytes)
            {
                // The end of a huge log is what matters.
                file.Seek(size - MaxBytes, SeekOrigin.Begin);
                int b;
                while ((b = file.ReadByte()) != -1 && b != '\n')
                {
                }
            }
            using var rest = new MemoryStream();
            file.CopyTo(rest);
            data = rest.ToArray();
        }
        return [.. Encoding.UTF8.GetString(data).Replace("\r\n", "\n").Split('\n')];
    }

    /// <summary>Whether a file is a log: *.log, *.out, and rotated ones (app.log.1, app.log.2024-05-01).</summary>
    public static bool IsLog(string path)
    {
        var name = Path.GetFileName(path).ToLowerInvariant();
        return name.EndsWith(".log", StringComparison.Ordinal) || name.EndsWith(".out", StringComparison.Ordinal) || RotatedLog.IsMatch(name);
    }

    /// <summary>A path quoted for PowerShell (<paramref name="windows"/>) or a POSIX shell.</summary>
    public static string Quote(string path, bool windows) =>
        windows ? "'" + path.Replace("'", "''") + "'" : "'" + path.Replace("'", "'\\''") + "'";

    /// <summary>A file name without its last extension (a leading dot isn't one: ".log" stays ".log").</summary>
    public static string Stem(string name)
    {
        var dot = name.LastIndexOf('.');
        return dot > 0 && name[..dot].Any(c => c != '.') ? name[..dot] : name;
    }

    /// <summary>The folder part of a path as given (separators untouched, unlike Path.GetDirectoryName on Windows).</summary>
    public static string Dirname(string path)
    {
        var drive = OperatingSystem.IsWindows() && path.Length >= 2 && path[1] == ':' ? path[..2] : "";
        var rest = path[drive.Length..];
        var i = rest.Length;
        while (i > 0 && !IsSeparator(rest[i - 1]))
        {
            i--;
        }
        var head = rest[..i];
        var trimmed = OperatingSystem.IsWindows() ? head.TrimEnd('/', '\\') : head.TrimEnd('/');
        return drive + (trimmed.Length > 0 ? trimmed : head);
    }

    static bool IsSeparator(char c) => c == '/' || OperatingSystem.IsWindows() && c == '\\';

    /// <summary>The first <paramref name="count"/> characters (code points, not halves of surrogate pairs).</summary>
    public static string Head(string text, int count)
    {
        var end = 0;
        for (var n = 0; n < count && end < text.Length; n++)
        {
            end += char.IsHighSurrogate(text[end]) && end + 1 < text.Length && char.IsLowSurrogate(text[end + 1]) ? 2 : 1;
        }
        return text[..end];
    }

    /// <summary>Trims whitespace as Python's str.strip() does (which also counts \x1c-\x1f).</summary>
    public static string Strip(string text) => text.Trim(PythonWhitespace);

    static readonly char[] PythonWhitespace =
    [
        .. new[] { 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x85, 0xa0, 0x1680 }.Select(c => (char)c),
        .. Enumerable.Range(0x2000, 11).Select(c => (char)c),
        .. new[] { 0x2028, 0x2029, 0x202f, 0x205f, 0x3000 }.Select(c => (char)c),
    ];
}
