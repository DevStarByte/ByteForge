using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Csv;
using ByteForge.Modules.Testing;

namespace ByteForge.Csv.Tests;

/// <summary>
/// The rainbow rules in module.json are generated here: BYTEFORGE_WRITE_RULES=1 dotnet test rewrites them.
/// </summary>
public sealed class HighlightTests
{
    // Theme tokens with clearly different colors in most themes, one per column, repeating.
    public static readonly string[] Tokens = ["keyword", "class name", "string", "method name", "property name", "number", "keyword - control", "regex - character class"];
    const int Columns = 20;

    // Tab first, then semicolon, then comma, then pipe: the earlier rule wins, and the semicolon rules only match
    // lines with semicolons, so German Excel files (1,5;Berlin) are split at ; and US ones at ,.
    static readonly (string RegexSep, string Raw)[] Separators = [("\\t", "\t"), (";", ";"), (",", ","), ("\\|", "|")];

    static readonly string ModuleJson = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "..", "module.json"));

    public static List<(string Match, int Group, string Token)> Rules()
    {
        var result = new List<(string, int, string)>();
        foreach (var (regexSep, raw) in Separators)
        {
            var cls = raw != "\t" ? raw : "\\t";
            var field = $"(?:\"(?:[^\"]|\"\")*\"|[^{cls}\"\\n]*)";
            for (var k = 0; k < Columns; k++)
            {
                var pattern = k == 0 ? $"^({field}){regexSep}" : $"^(?:{field}{regexSep}){{{k}}}({field})";
                result.Add((pattern, 1, Tokens[k % Tokens.Length]));
            }
        }
        return result;
    }

    static JsonObject Manifest() =>
        JsonNode.Parse(File.ReadAllText(ModuleJson), documentOptions: new() { CommentHandling = JsonCommentHandling.Skip })!.AsObject();

    static JsonArray ManifestRules() => Manifest()["languages"]![0]!["rules"]!.AsArray();

    /// <summary>The token the IDE gives the first character of <paramref name="text"/> in <paramref name="line"/> (the earlier rule wins).</summary>
    static string? TokenOf(string line, string text)
    {
        var start = line.IndexOf(text, StringComparison.Ordinal);
        foreach (var rule in ManifestRules())
        {
            var g = rule!["group"]?.GetValue<int>() ?? 0;
            foreach (Match m in Regex.Matches(line, rule["match"]!.GetValue<string>()))
            {
                var group = m.Groups[g];
                if (group.Success && group.Index <= start && start < group.Index + group.Length)
                {
                    return rule["token"]!.GetValue<string>();
                }
            }
        }
        return null;
    }

    static void WriteRules()
    {
        var text = File.ReadAllText(ModuleJson);
        var newline = text.Contains("\r\n") ? "\r\n" : "\n";
        var start = text.IndexOf("\"rules\": [", StringComparison.Ordinal);
        var end = text.IndexOf(newline + "      ]", start, StringComparison.Ordinal);
        var body = string.Join("," + newline, Rules().Select(r =>
            $"        {{\"match\": {CsvTools.JsonString(r.Match)}, \"group\": {r.Group}, \"token\": {CsvTools.JsonString(r.Token)}}}"));
        File.WriteAllText(ModuleJson, text[..start] + "\"rules\": [" + newline + body + text[end..]);
    }

    [Fact]
    public void Manifest_rules_are_the_generated_ones()
    {
        if (Environment.GetEnvironmentVariable("BYTEFORGE_WRITE_RULES") == "1")
        {
            WriteRules();
        }
        var rules = ManifestRules();
        var expected = Rules();
        Assert.Equal(expected.Count, rules.Count);
        for (var i = 0; i < expected.Count; i++)
        {
            Assert.Equal(expected[i].Match, rules[i]!["match"]!.GetValue<string>());
            Assert.Equal(expected[i].Group, rules[i]!["group"]!.GetValue<int>());
            Assert.Equal(expected[i].Token, rules[i]!["token"]!.GetValue<string>());
            Assert.Equal(3, rules[i]!.AsObject().Count);
        }
    }

    [Fact]
    public void Comma_columns()
    {
        var line = "id,name,\"Smith, John\",42";
        Assert.Equal(Tokens[0], TokenOf(line, "id"));
        Assert.Equal(Tokens[1], TokenOf(line, "name"));
        Assert.Equal(Tokens[2], TokenOf(line, "\"Smith, John\"")); // the comma in quotes doesn't split
        Assert.Equal(Tokens[3], TokenOf(line, "42"));
    }

    [Fact]
    public void Semicolon_lines_split_at_semicolons()
    {
        var line = "1,5;Berlin;2,75";
        Assert.Equal(Tokens[0], TokenOf(line, "1,5"));
        Assert.Equal(Tokens[1], TokenOf(line, "Berlin"));
        Assert.Equal(Tokens[2], TokenOf(line, "2,75"));
    }

    [Fact]
    public void Tabs_and_wrapping_colors()
    {
        var line = string.Join("\t", Enumerable.Range(0, 10).Select(i => $"c{i}"));
        Assert.Equal(Tokens[9 % Tokens.Length], TokenOf(line, "c9"));
        Assert.Equal(Tokens[1], TokenOf(line, "c1"));
    }
}

public sealed class SniffTests
{
    [Fact]
    public void Separators()
    {
        Assert.Equal(',', CsvTools.Sniff("a,b,c\n1,2,3\n"));
        Assert.Equal(';', CsvTools.Sniff("Name;Preis;Ort\nApfel;1,50;Berlin\nBirne;2,75;Köln\n"));
        Assert.Equal('\t', CsvTools.Sniff("a\tb\n1\t2\n"));
        Assert.Equal('|', CsvTools.Sniff("a|b|c\n1|2|3\n"));
        Assert.Equal(',', CsvTools.Sniff("a,b\n\"x;y;z\",2\n"));
        Assert.Equal('\t', CsvTools.Sniff("whatever", "data.tsv"));
    }

    [Fact]
    public void Header()
    {
        Assert.True(CsvTools.HasHeader([["id", "name"], ["1", "Ada"]]));
        Assert.False(CsvTools.HasHeader([["1", "Ada"], ["2", "Linus"]]));
        Assert.False(CsvTools.HasHeader([["2024-01-01", "x"], ["2024-01-02", "y"]]));
    }
}

public sealed class TypeTests
{
    [Fact]
    public void Kinds()
    {
        var cases = new Dictionary<string, string>
        {
            ["42"] = "integer", ["-7"] = "integer", ["1,234,567"] = "integer", ["3.14"] = "decimal", ["1e5"] = "decimal",
            ["2024-05-01"] = "date", ["01.05.2024"] = "date", ["2024-05-01T10:00:00Z"] = "date", ["true"] = "boolean",
            ["Ja"] = "boolean", ["007"] = "text", ["hello"] = "text", [""] = "empty",
        };
        foreach (var (value, expected) in cases)
        {
            Assert.True(expected == CsvTools.Kind(value), value);
        }
    }

    [Fact]
    public void Decimal_comma_in_semicolon_files()
    {
        Assert.Equal("decimal", CsvTools.Kind("1,5", decimalComma: true));
        Assert.Equal("decimal", CsvTools.Kind("1.234,50", decimalComma: true));
        Assert.Equal("integer", CsvTools.Kind("1.234", decimalComma: true));
        Assert.Equal(1234.5, CsvTools.ToNumber("1.234,50", decimalComma: true));
        Assert.Equal(1234.0, CsvTools.ToNumber("1,234")); // a comma file: thousands
        Assert.Equal(1.5, CsvTools.ToNumber("1,5"));
    }

    [Fact]
    public void Column_stats()
    {
        var stats = CsvTools.Stats(["3", "1", "", "2"]);
        Assert.Equal(("integer", 1.0, 3.0, 1), (stats.Type, stats.MinNumber, stats.MaxNumber, stats.Empty));
        Assert.Equal("integer · 1 … 3 · 1 empty · unique", CsvTools.Describe(stats));
        Assert.Equal("text", CsvTools.Stats(["a", "1", "b"]).Type);
        Assert.Equal("2023-12-31 00:00:00", CsvTools.Stats(["2024-01-02", "2023-12-31"]).MinDate);
        Assert.Equal("mixed", CsvTools.Stats(["1", "true"]).Type);
    }
}

public sealed class OutputTests
{
    static readonly List<List<string>> Rows = [["name", "price", "city"], ["Apfel", "1,50", "Berlin"], ["Birne", "12,75", "Köln"], ["Kiwi", "", "Bonn"]];

    [Fact]
    public void Table_aligns_and_right_aligns_numbers()
    {
        var lines = CsvTools.Table(Rows, header: true).Split('\n');
        Assert.Equal("name  │ price │ city", lines[0]);
        Assert.StartsWith("──────┼", lines[1]);
        Assert.Equal("Apfel │  1,50 │ Berlin", lines[2]);
    }

    [Fact]
    public void Json_with_decimal_commas()
    {
        var data = JsonNode.Parse(CsvTools.ToJson(Rows, header: true, decimalComma: true))!.AsArray();
        Assert.True(JsonNode.DeepEquals(JsonNode.Parse("""{"name": "Apfel", "price": 1.5, "city": "Berlin"}"""), data[0]));
        Assert.Null(data[2]!["price"]);
        data = JsonNode.Parse(CsvTools.ToJson([["1", "x"], ["2", "y"]], header: false))!.AsArray();
        Assert.True(JsonNode.DeepEquals(JsonNode.Parse("""{"column1": 2, "column2": "y"}"""), data[1]));
    }

    [Fact]
    public void View_for_a_german_excel_file()
    {
        var folder = Directory.CreateTempSubdirectory("bf-csv-").FullName;
        try
        {
            var path = Path.Combine(folder, "preise.csv");
            File.WriteAllBytes(path, Encoding.Latin1.GetBytes("Name;Preis;Ort\r\nApfel;1,50;Berlin\r\nBirne;2,75;Köln\r\n")); // Excel's ANSI
            var module = new CsvModule { Active = path };
            var item = module.Tree()["items"]![0]!;
            Assert.Contains("semicolon separated", item["description"]!.GetValue<string>());
            Assert.Equal(["Name", "Preis", "Ort"], item["children"]!.AsArray().Select(c => c!["label"]!.GetValue<string>()));
            Assert.StartsWith("decimal · 1.5 … 2.75", item["children"]![1]!["description"]!.GetValue<string>());
            Assert.Equal("text · unique", item["children"]![2]!["description"]!.GetValue<string>());
        }
        finally
        {
            Directory.Delete(folder, recursive: true);
        }
    }

    [Fact]
    public void Long_tables_stop_after_2000_rows()
    {
        var rows = new List<List<string>> { new() { "n", "v" } };
        rows.AddRange(Enumerable.Range(0, 2100).Select(i => new List<string> { $"{i}", (i * 1.5).ToString(System.Globalization.CultureInfo.InvariantCulture) }));
        var lines = CsvTools.Table(rows, header: true).Split('\n');
        Assert.Equal(2004, lines.Length); // header, rule, 2000 rows, "more", and the empty one after the last \n
        Assert.Equal("… 100 more rows", lines[^2]);
        Assert.Equal("1999 │ 2998.5", lines[2001]);
    }
}

public sealed class ReaderTests
{
    static List<List<string>> Read(string text, char sep = ',') => CsvReader.Read(text, sep);

    [Fact]
    public void Quotes_and_newlines_as_pythons_csv_module()
    {
        Assert.Equal([["a", "b\r\nc", "d"], ["e", "f"]], Read("a,\"b\r\nc\",d\r\ne,f\r\n"));
        Assert.Equal([["a", "bx", "c"]], Read("a,\"b\"x,c\n"));          // text after the closing quote is kept
        Assert.Equal([["ab\n"]], Read("\"ab\n"));                        // an open quote runs to the end
        Assert.Equal([["a", "b"], [], [], ["c"]], Read("a,b\r\n\r\n\nc\n")); // blank lines are empty rows
        Assert.Equal([["a\0b", "c"]], Read("a\0b,c"));
        Assert.Equal([[" \"a\"", "b"]], Read(" \"a\",b"));               // a quote after a space is text
        Assert.Equal([["a", "b"]], Read("a,b\r"));
        Assert.Equal([["a", ""]], Read("a,\"\""));
        Assert.Equal([["a\"b"]], Read("\"a\"\"b\""));
        Assert.Empty(Read(""));
        Assert.Equal([["a"], ["b"]], Read("\"a\"\r\n\"b"));
        Assert.Equal([["1,5", "Berlin"]], Read("1,5;Berlin", ';'));
    }

    [Fact]
    public void Errors_as_pythons_csv_module()
    {
        Assert.Equal("new-line character seen in unquoted field - do you need to open the file with newline=''?",
            Assert.Throws<CsvError>(() => Read("a,b\rc,d\n")).Message);
        Assert.Equal("new-line character seen in unquoted field - do you need to open the file with newline=''?",
            Assert.Throws<CsvError>(() => Read("\"a\"\r\"b\"")).Message);
        Assert.Equal("field larger than field limit (131072)", Assert.Throws<CsvError>(() => Read(new string('x', 131073))).Message);
        Assert.Equal(131072, Read("\"" + new string('x', 131072) + "\"")[0][0].Length);
        // The limit counts code points: 131072 emoji fit.
        Assert.Equal(2 * 131072, Read(string.Concat(Enumerable.Repeat("\U0001F600", 131072)))[0][0].Length);
    }
}

/// <summary>
/// The C# against the Python module it replaces: python-expected.json holds what the Python (csv_tools.py of
/// ByteForge-packages commit 6f46c2c, Python 3.14 on Windows) made of tricky files — the decoded text, the view's
/// tree, separator, rows, header, table, JSON, column statistics and each value's type, number and date.
/// </summary>
public sealed class PythonComparisonTests
{
    internal static JsonObject Report(string text, string path)
    {
        var report = new JsonObject();
        char sep;
        List<List<string>> rows;
        try
        {
            sep = CsvTools.Sniff(text, path);
            report["sep"] = sep.ToString();
            rows = CsvTools.Parse(text, sep);
        }
        catch (PyException e)
        {
            report["error"] = $"{e.Type}: {e.Message}";
            return report;
        }
        report["rows"] = new JsonArray([.. rows.Select(r => (JsonNode)new JsonArray([.. r.Select(c => (JsonNode)c)]))]);
        var header = CsvTools.HasHeader(rows);
        report["header"] = header;
        if (rows.Count == 0)
        {
            return report;
        }
        report["table"] = CsvTools.Table(rows, header);
        try
        {
            report["json"] = CsvTools.ToJson(rows, header, sep == ';');
        }
        catch (PyException e)
        {
            report["json_error"] = $"{e.Type}: {e.Message}";
        }
        var columns = new JsonArray();
        for (var i = 0; i < rows.Max(r => r.Count); i++)
        {
            var values = rows.Select(r => i < r.Count ? r[i] : "").ToList();
            columns.Add(new JsonArray(Column(values, false), Column(values, true)));
        }
        report["columns"] = columns;
        var kinds = new JsonObject();
        foreach (var v in rows.SelectMany(r => r).Distinct(StringComparer.Ordinal))
        {
            static string Number(double? n) => n is { } x ? Py.Repr(x) : "null";
            var date = CsvTools.ToDate(v);
            kinds[v] = new JsonArray(CsvTools.Kind(v), CsvTools.Kind(v, true), Number(CsvTools.ToNumber(v)), Number(CsvTools.ToNumber(v, true)),
                date is { } d ? CsvTools.IsoFormat(d) : null);
        }
        report["values"] = kinds;
        return report;
    }

    static JsonObject Column(List<string> values, bool decimalComma)
    {
        var s = CsvTools.Stats(values, decimalComma);
        var r = new JsonObject { ["describe"] = CsvTools.Describe(s), ["type"] = s.Type, ["empty"] = s.Empty, ["distinct"] = s.Distinct, ["count"] = s.Count };
        if (s.Sum is { } sum)
        {
            r["sum"] = CsvTools.NumberText(sum);
            r["min"] = CsvTools.NumberText(s.MinNumber!.Value);
            r["max"] = CsvTools.NumberText(s.MaxNumber!.Value);
        }
        else if (s.MinDate != null)
        {
            r["min"] = s.MinDate;
            r["max"] = s.MaxDate;
        }
        if (s.Longest is { } longest)
        {
            r["longest"] = longest;
        }
        return r;
    }

    /// <summary>Both sides through text, so numbers and strings compare by value.</summary>
    internal static void AssertSame(JsonNode? expected, JsonNode? actual, string what)
    {
        var e = JsonNode.Parse(expected?.ToJsonString() ?? "null");
        var a = JsonNode.Parse(actual?.ToJsonString() ?? "null");
        if (!JsonNode.DeepEquals(e, a))
        {
            Assert.Fail($"{what}\nPython: {e?.ToJsonString()}\nC#:     {a?.ToJsonString()}");
        }
    }

    public static IEnumerable<object[]> Samples() =>
        JsonNode.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "python-expected.json")))!.AsArray()
            .Select(s => new object[] { s!["name"]!.GetValue<string>() });

    [Theory]
    [MemberData(nameof(Samples))]
    public void Same_as_the_python(string name)
    {
        var sample = JsonNode.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "python-expected.json")))!.AsArray()
            .First(s => s!["name"]!.GetValue<string>() == name)!.AsObject();
        var folder = Directory.CreateTempSubdirectory("bf-csv-").FullName;
        try
        {
            var path = Path.Combine(folder, name);
            File.WriteAllBytes(path, Convert.FromBase64String(sample["bytes"]!.GetValue<string>()));
            var (text, truncated) = CsvTools.ReadText(path);
            Assert.Equal(sample["text"]!.GetValue<string>(), text);
            Assert.Equal(sample["truncated"]!.GetValue<bool>(), truncated);

            var module = new CsvModule { Active = path };
            if (sample["tree_error"] is { } treeError)
            {
                var e = Assert.ThrowsAny<PyException>(() => module.Tree());
                Assert.Equal(treeError.GetValue<string>(), $"{e.Type}: {e.Message}");
            }
            else
            {
                AssertSame(sample["tree"], module.Tree(), "tree");
            }

            var report = Report(text, name);
            foreach (var (key, expected) in sample)
            {
                if (key is "name" or "bytes" or "text" or "truncated" or "tree" or "tree_error")
                {
                    continue;
                }
                AssertSame(expected, report[key], $"{name}: {key}");
            }
            foreach (var (key, _) in report)
            {
                Assert.True(sample.ContainsKey(key), $"{name}: {key} only in C#");
            }
        }
        finally
        {
            Directory.Delete(folder, recursive: true);
        }
    }
}

[CollectionDefinition("module", DisableParallelization = true)]
public sealed class ModuleCollection;

[Collection("module")]
public sealed class ModuleTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-csv-").FullName;

    string ModuleDir => Path.Combine(dir, "modules", "csv");

    public void Dispose()
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    string Write(string name, string text)
    {
        var path = Path.Combine(dir, name);
        File.WriteAllText(path, text);
        return path;
    }

    static JsonObject Event(string name, string? path = null) => new() { ["type"] = "event", ["name"] = name, ["path"] = path };

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = ModuleHarness.Start(new CsvModule(), ModuleDir);
        foreach (var (id, title) in new[] { ("csv.table", "Show as Table"), ("csv.json", "Convert to JSON"), ("csv.columns", "Show Columns") })
        {
            var command = await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == id);
            Assert.Equal(title, command["title"]!.GetValue<string>());
            Assert.Equal("CSV", command["category"]!.GetValue<string>());
        }
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "csv.columns" });
        Assert.Equal("csv", (await module.Next("showView"))["id"]!.GetValue<string>());
    }

    [Fact]
    public async Task The_view_follows_the_csv_editor()
    {
        await using var module = ModuleHarness.Start(new CsvModule(), ModuleDir);
        module.Send(Event("viewShown"));
        var tree = await module.Next("setTree");
        Assert.Equal("Open a .csv or .tsv file to see its columns.", tree["message"]!.GetValue<string>());

        var path = Write("people.CSV", "id,name\n1,Ada\n2,Linus\n");
        module.Send(Event("editorChanged", Path.Combine(dir, "notes.txt"))); // not a CSV file: ignored
        module.Send(Event("editorChanged", path));
        tree = await module.Next("setTree");
        Assert.Equal("2 rows · 2 columns · comma separated", tree["message"]!.GetValue<string>());
        var file = tree["items"]![0]!;
        Assert.Equal("people.CSV", file["label"]!.GetValue<string>());
        Assert.Equal("integer · 1 … 2 · unique", file["children"]![0]!["description"]!.GetValue<string>());
        Assert.Equal("id: integer\n2 values, 2 distinct\nsum 3", file["children"]![0]!["tooltip"]!.GetValue<string>());
        Assert.Equal(["table", "json", "refresh"], tree["actions"]!.AsArray().Select(a => a!["id"]!.GetValue<string>()));

        File.WriteAllText(path, "id;name\n1;Ada\n");
        File.SetLastWriteTimeUtc(path, DateTime.UtcNow.AddMinutes(1));
        module.Send(Event("fileSaved", path));
        tree = await module.Next("setTree");
        Assert.Equal("1 rows · 2 columns · semicolon separated", tree["message"]!.GetValue<string>());

        File.Delete(path);
        module.Send(new() { ["type"] = "treeAction", ["view"] = "csv", ["action"] = "refresh" });
        tree = await module.Next("setTree");
        Assert.Contains("people.CSV", tree["error"]!.GetValue<string>());
    }

    [Fact]
    public async Task Convert_to_json_and_show_as_table_write_into_the_module_folder()
    {
        await using var module = ModuleHarness.Start(new CsvModule(), ModuleDir);
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "csv.json" });
        Assert.Equal("csv: open a .csv or .tsv file first (it's read as saved).", (await module.Next("log"))["text"]!.GetValue<string>());

        var path = Write("preise.csv", "Name;Preis\nApfel;1,50\nBirne;2,75\n");
        module.Send(Event("editorChanged", path));
        await module.Next("setTree");
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "csv.json" });
        var log = (await module.Next("log"))["text"]!.GetValue<string>();
        var opened = (await module.Next("openFile"))["path"]!.GetValue<string>();
        Assert.Equal($"csv: 2 rows → {opened}", log);
        Assert.Equal(Path.Combine(ModuleDir, "output"), Path.GetDirectoryName(opened));
        Assert.Matches(@"^preise-\d{6}\.json$", Path.GetFileName(opened));
        var nl = Environment.NewLine;
        Assert.Equal($"[{nl}  {{{nl}    \"Name\": \"Apfel\",{nl}    \"Preis\": 1.5{nl}  }},{nl}  {{{nl}    \"Name\": \"Birne\",{nl}    \"Preis\": 2.75{nl}  }}{nl}]{nl}",
            File.ReadAllText(opened));

        module.Send(new() { ["type"] = "treeAction", ["view"] = "csv", ["action"] = "table" });
        opened = (await module.Next("openFile"))["path"]!.GetValue<string>();
        Assert.Matches(@"^preise-\d{6}\.table\.txt$", Path.GetFileName(opened));
        Assert.Equal($"Name  │ Preis{nl}──────┼──────{nl}Apfel │  1,50{nl}Birne │  2,75{nl}", File.ReadAllText(opened));
    }

    [Fact]
    public async Task Errors_are_logged_and_the_module_keeps_running()
    {
        await using var module = ModuleHarness.Start(new CsvModule(), ModuleDir);
        var path = Write("mac.csv", "a,b\rc,d\r");
        module.Send(Event("editorChanged", path));
        Assert.Equal("csv: Error: new-line character seen in unquoted field - do you need to open the file with newline=''?",
            (await module.Next("log"))["text"]!.GetValue<string>());

        var big = Write("big.csv", "n\n" + new string('9', 400) + "\n");
        module.Send(Event("editorChanged", big));
        await module.Next("setTree");
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "csv.json" });
        Assert.Equal("csv: OverflowError: cannot convert float infinity to integer", (await module.Next("log"))["text"]!.GetValue<string>());
    }
}
