using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.Csv;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<CsvModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// CSV for ByteForge: rainbow columns (declared in module.json), a CSV view with the active file's columns and
/// what's in them, "Show as Table" and "Convert to JSON".
///
/// The separator is found per file (comma, semicolon as Excel writes it with a German or French locale, tab or
/// pipe), and so are decimal commas, dates and quoted fields. Files are read as saved.
/// </summary>
public sealed class CsvModule : Module
{
    public const string View = "csv";

    static readonly Dictionary<char, string> SeparatorNames = new() { [','] = "comma", [';'] = "semicolon", ['\t'] = "tab", ['|'] = "pipe" };

    static readonly Dictionary<string, string> TypeIcons = new()
    {
        ["integer"] = "fileBinary", ["decimal"] = "fileBinary", ["date"] = "fileConfig", ["boolean"] = "check",
        ["text"] = "fileText", ["mixed"] = "warning", ["empty"] = "remove",
    };

    public sealed record Loaded(string Path, DateTime Modified, List<List<string>> Rows, char Separator, bool Header, bool Truncated);

    // Messages are handled one at a time, as the Python did (commands and view buttons would otherwise run alongside).
    readonly Lock gate = new();
    Loaded? loaded;

    /// <summary>The CSV file the view shows (the last one opened in an editor).</summary>
    public string? Active { get; set; }

    /// <summary>Where Show as Table and Convert to JSON write their files: the module's own folder.</summary>
    public string OutputDir => Path.Combine(ModuleDir, "output");

    protected override Task OnStartAsync()
    {
        RegisterCommand("csv.table", "Show as Table", "CSV");
        RegisterCommand("csv.json", "Convert to JSON", "CSV");
        RegisterCommand("csv.columns", "Show Columns", "CSV");
        return Task.CompletedTask;
    }

    protected override Task OnEventAsync(string name, JsonObject message)
    {
        Guarded(() =>
        {
            if (name == "editorChanged")
            {
                var path = message["path"]?.GetValue<string>();
                if (!string.IsNullOrEmpty(path) && CsvTools.Extensions.Any(path.ToLowerInvariant().EndsWith))
                {
                    Active = path;
                    Refresh();
                }
            }
            else if (name == "fileSaved" && message["path"]?.GetValue<string>() == Active)
            {
                Refresh();
            }
            else if (name == "viewShown")
            {
                Refresh();
            }
        });
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id)
    {
        Guarded(() =>
        {
            switch (id)
            {
                case "csv.table":
                    ShowTable();
                    break;
                case "csv.json":
                    Convert();
                    break;
                case "csv.columns":
                    ShowView(View);
                    break;
            }
        });
        return Task.CompletedTask;
    }

    protected override Task OnTreeActionAsync(string view, string? item, string action)
    {
        if (item != null)
        {
            return Task.CompletedTask;
        }
        Guarded(() =>
        {
            switch (action)
            {
                case "table":
                    ShowTable();
                    break;
                case "json":
                    Convert();
                    break;
                case "refresh":
                    loaded = null;
                    Refresh();
                    break;
            }
        });
        return Task.CompletedTask;
    }

    /// <summary>Keeps the module alive and says what went wrong, as "csv: Type: message".</summary>
    void Guarded(Action work)
    {
        lock (gate)
        {
            try
            {
                work();
            }
            catch (Exception e)
            {
                Log($"csv: {(e is PyException py ? py.Type : e.GetType().Name)}: {e.Message}");
            }
        }
    }

    static bool IsOsError(Exception e) => e is IOException or UnauthorizedAccessException;

    public Loaded Load(string path)
    {
        var info = new FileInfo(path);
        if (!info.Exists && !Directory.Exists(path))
        {
            throw new FileNotFoundException($"Could not find file '{path}'.", path);
        }
        var modified = info.Exists ? info.LastWriteTimeUtc : Directory.GetLastWriteTimeUtc(path);
        if (loaded != null && loaded.Path == path && loaded.Modified == modified)
        {
            return loaded;
        }
        var (text, truncated) = CsvTools.ReadText(path);
        var sep = CsvTools.Sniff(text, path);
        var rows = CsvTools.Parse(text, sep);
        loaded = new(path, modified, rows, sep, CsvTools.HasHeader(rows), truncated);
        return loaded;
    }

    /// <summary>The view's setTree message for the active file.</summary>
    public JsonObject Tree()
    {
        JsonObject Empty(string key, string text) => new() { ["type"] = "setTree", ["view"] = View, ["items"] = new JsonArray(), [key] = text };
        if (Active == null)
        {
            return Empty("message", "Open a .csv or .tsv file to see its columns.");
        }
        Loaded file;
        try
        {
            file = Load(Active);
        }
        catch (Exception e) when (IsOsError(e))
        {
            return Empty("error", e.Message);
        }
        var rows = file.Rows;
        if (rows.Count == 0)
        {
            return Empty("message", "The file is empty.");
        }
        var names = file.Header ? rows[0] : [.. Enumerable.Range(1, rows.Max(r => r.Count)).Select(i => $"Column {i}")];
        var data = file.Header ? rows.Skip(1).ToList() : rows;
        var widths = data.Select(r => r.Count).ToHashSet();
        var children = new JsonArray();
        for (var i = 0; i < names.Count; i++)
        {
            var name = names[i];
            var stats = CsvTools.Stats([.. data.Select(r => i < r.Count ? r[i] : "")], file.Separator == ';');
            var tooltip = new List<string> { $"{name}: {stats.Type}", $"{stats.Count - stats.Empty} values, {stats.Distinct} distinct" };
            if (stats.Sum is { } sum)
            {
                tooltip.Add($"sum {CsvTools.NumberText(sum)}");
            }
            if (stats.Longest is { } longest)
            {
                tooltip.Add($"longest {longest} characters");
            }
            children.Add(new JsonObject
            {
                ["id"] = $"col:{i}",
                ["label"] = name.Length > 0 ? name : $"Column {i + 1}",
                ["description"] = CsvTools.Describe(stats),
                ["tooltip"] = string.Join("\n", tooltip),
                ["icon"] = TypeIcons.GetValueOrDefault(stats.Type, "fileText"),
            });
        }
        var summary = new List<string> { $"{data.Count} rows", $"{names.Count} columns", $"{SeparatorNames[file.Separator]} separated" };
        if (!file.Header)
        {
            summary.Add("no header row");
        }
        if (widths.Count > 1)
        {
            summary.Add($"rows have {widths.Min()}–{widths.Max()} fields");
        }
        if (file.Truncated)
        {
            summary.Add($"first {CsvTools.MaxBytes / 1_000_000} MB");
        }
        var description = string.Join(" · ", summary);
        var items = new JsonArray(new JsonObject
        {
            ["id"] = "file",
            ["label"] = Py.Basename(Active),
            ["header"] = true,
            ["expanded"] = true,
            ["count"] = names.Count,
            ["children"] = children,
            ["description"] = description,
        });
        return new()
        {
            ["type"] = "setTree",
            ["view"] = View,
            ["items"] = items,
            ["message"] = description,
            ["actions"] = new JsonArray(
                new JsonObject { ["id"] = "table", ["icon"] = "openPreview", ["tooltip"] = "Show as Table" },
                new JsonObject { ["id"] = "json", ["icon"] = "fileJson", ["tooltip"] = "Convert to JSON" },
                new JsonObject { ["id"] = "refresh", ["icon"] = "refresh", ["tooltip"] = "Refresh" }),
        };
    }

    void Refresh() => Send(Tree());

    Loaded? Current()
    {
        if (Active == null)
        {
            Log("csv: open a .csv or .tsv file first (it's read as saved).");
            return null;
        }
        try
        {
            return Load(Active);
        }
        catch (Exception e) when (IsOsError(e))
        {
            Log($"csv: {e.Message}");
            return null;
        }
    }

    /// <summary>Writes <paramref name="text"/> as &lt;name&gt;-HHMMSS&lt;suffix&gt; (UTF-8, the platform's line ends, as Python's text files).</summary>
    string WriteOutput(string path, string suffix, string text)
    {
        Directory.CreateDirectory(OutputDir);
        var stamp = DateTime.Now.ToString("HHmmss", System.Globalization.CultureInfo.InvariantCulture);
        var output = Path.Combine(OutputDir, $"{Py.SplitExt(Py.Basename(path)).Root}-{stamp}{suffix}");
        File.WriteAllText(output, Environment.NewLine == "\n" ? text : text.Replace("\n", Environment.NewLine));
        return output;
    }

    void ShowTable()
    {
        if (Current() is { Rows.Count: > 0 } file)
        {
            OpenFile(WriteOutput(file.Path, ".table.txt", CsvTools.Table(file.Rows, file.Header)));
        }
    }

    void Convert()
    {
        if (Current() is { Rows.Count: > 0 } file)
        {
            var output = WriteOutput(file.Path, ".json", CsvTools.ToJson(file.Rows, file.Header, file.Separator == ';'));
            Log($"csv: {file.Rows.Count - (file.Header ? 1 : 0)} rows → {output}");
            OpenFile(output);
        }
    }
}
