using System.Text;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;

namespace ByteForge.Todos;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<TodosModule>(input, output, moduleDir, cancel);
}

/// <summary>A TODO / FIXME / HACK / BUG / XXX comment: where it is (line and column from 1), its tag and text.</summary>
public sealed record Todo(string Path, int Line, int Column, string Tag, string Text);

/// <summary>
/// Scans the workspace for TODO / FIXME / HACK / BUG / XXX comments. "TODOs: List All" prints them to the Output
/// panel as <c>path:line:column: text</c> (click to jump); the status bar shows the count and refreshes when a
/// file is saved or a folder is opened.
/// </summary>
public sealed class TodosModule : Module
{
    public static readonly string[] Tags = ["TODO", "FIXME", "HACK", "BUG", "XXX"];

    static readonly Regex Pattern = new(@"(?://|#|/\*|<!--|@\*|\*)\s*(" + string.Join("|", Tags) + @")\b[:\s]?(.*)", RegexOptions.CultureInvariant);

    static readonly HashSet<string> Extensions = [".cs", ".csx", ".razor", ".cshtml", ".fs", ".xml", ".csproj", ".props", ".targets", ".json", ".yaml", ".yml",
        ".js", ".ts", ".css", ".scss", ".html", ".md", ".py", ".sh", ".ps1", ".sql", ".dockerfile"];

    static readonly HashSet<string> SkipDirs = ["bin", "obj", ".git", "node_modules", ".vs", ".idea", "target", "TestResults", "wwwroot/lib"];

    const long MaxFile = 2_000_000;

    protected override Task OnStartAsync()
    {
        RegisterCommand("todos.list", "List All", "TODOs");
        RegisterCommand("todos.refresh", "Refresh Count", "TODOs");
        return Task.CompletedTask;
    }

    // Scans run in the background, so a big workspace doesn't hold up the IDE's next messages.
    protected override Task OnActivateAsync()
    {
        _ = Task.Run(UpdateStatus);
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        if (name is "folderOpened" or "fileSaved")
        {
            _ = Task.Run(UpdateStatus);
        }
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "todos.list":
                ListAll();
                break;
            case "todos.refresh":
                UpdateStatus();
                break;
        }
        return Task.CompletedTask;
    }

    /// <summary>Counts the comments into the status bar; the comments, or null without a workspace.</summary>
    public List<Todo>? UpdateStatus()
    {
        if (string.IsNullOrEmpty(Workspace))
        {
            SetStatus("todos.count", "");
            return null;
        }
        var items = Scan(Workspace);
        var fixme = items.Count(i => i.Tag is "FIXME" or "BUG");
        var text = $"☐ {items.Count}" + (fixme > 0 ? $" ({fixme} FIXME)" : "");
        SetStatus("todos.count", text, $"{items.Count} TODO comments in the workspace. Click to list them.", "todos.list");
        return items;
    }

    public void ListAll()
    {
        var items = UpdateStatus() ?? [];
        if (items.Count == 0)
        {
            Log("No TODO comments in the workspace.");
        }
        else
        {
            Log($"{items.Count} TODO comments:");
            foreach (var item in Sorted(items))
            {
                var rel = Path.GetRelativePath(Workspace!, item.Path);
                Log($"{rel}:{item.Line}:{item.Column}: {item.Tag} {item.Text}");
            }
        }
        ShowView("output");
    }

    /// <summary>In tag order (BUG with FIXME), then by path and line.</summary>
    public static IEnumerable<Todo> Sorted(IEnumerable<Todo> items) =>
        items.OrderBy(i => i.Tag == "BUG" ? 1 : Array.IndexOf(Tags, i.Tag)).ThenBy(i => i.Path, StringComparer.Ordinal).ThenBy(i => i.Line);

    /// <summary>The comments in the files under <paramref name="root"/>.</summary>
    public static List<Todo> Scan(string root)
    {
        var found = new List<Todo>();
        Walk(root, found);
        return found;
    }

    static void Walk(string directory, List<Todo> found)
    {
        string[] files, dirs;
        try
        {
            files = Directory.GetFiles(directory);
            dirs = Directory.GetDirectories(directory);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return;
        }
        foreach (var path in files)
        {
            var name = Path.GetFileName(path);
            if (!Extensions.Contains(SplitExtension(name).ToLowerInvariant()) && name != "Dockerfile")
            {
                continue;
            }
            try
            {
                if (new FileInfo(path).Length > MaxFile)
                {
                    continue;
                }
                ScanFile(path, File.ReadAllBytes(path), found);
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
            }
        }
        foreach (var dir in dirs)
        {
            var name = Path.GetFileName(dir);
            if (SkipDirs.Contains(name) || name.StartsWith('.') || IsLink(dir))
            {
                continue;
            }
            Walk(dir, found);
        }
    }

    static bool IsLink(string dir)
    {
        try
        {
            return new DirectoryInfo(dir).LinkTarget != null;
        }
        catch (IOException)
        {
            return true;
        }
    }

    /// <summary>The comments in one file's content (UTF-8, invalid bytes replaced).</summary>
    public static void ScanFile(string path, byte[] content, List<Todo> found)
    {
        var text = new UTF8Encoding(false).GetString(content).Replace("\r\n", "\n").Replace('\r', '\n');
        var lines = text.Split('\n');
        var count = text.EndsWith('\n') ? lines.Length - 1 : lines.Length;
        for (var i = 0; i < count; i++)
        {
            var m = Pattern.Match(lines[i]);
            if (m.Success)
            {
                var comment = m.Groups[2].Value.Trim().TrimEnd('*', '/', '-', '>').Trim();
                // Columns count characters (code points), not UTF-16 units.
                var column = CodePoints(lines[i].AsSpan(0, m.Groups[1].Index)) + 1;
                found.Add(new Todo(path, i + 1, column, m.Groups[1].Value, comment));
            }
        }
    }

    static int CodePoints(ReadOnlySpan<char> text)
    {
        var n = 0;
        foreach (var _ in text.EnumerateRunes())
        {
            n++;
        }
        return n;
    }

    /// <summary>A file name's extension as Python's os.path.splitext gives it (".bashrc" has none).</summary>
    public static string SplitExtension(string name)
    {
        var dot = name.LastIndexOf('.');
        return dot > 0 && name[..dot].TrimStart('.').Length > 0 ? name[dot..] : "";
    }
}
