using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace ByteForge.Csv;

/// <summary>Reading CSV as Python's csv.reader does (default dialect, any delimiter).</summary>
public static class CsvReader
{
    /// <summary>csv.field_size_limit()'s default.</summary>
    public const int FieldLimit = 128 * 1024;

    const int Eol = -2;

    enum State { StartRecord, StartField, InField, InQuotedField, QuoteInQuotedField, EatCrnl }

    /// <summary>
    /// list(csv.reader(io.StringIO(text), delimiter=delimiter)): lines end at \n (StringIO), \r and \n end a
    /// record outside quotes, a \r inside an unquoted line is an error, "" in quotes is ", text after a closing
    /// quote is kept, an unterminated quote runs to the end. Throws <see cref="CsvError"/> as Python does.
    /// </summary>
    public static List<List<string>> Read(string text, char delimiter)
    {
        var rows = new List<List<string>>();
        var fields = new List<string>();
        var field = new StringBuilder();
        var fieldLength = 0; // in code points, like Python's field_len
        var state = State.StartRecord;

        void Save()
        {
            fields.Add(field.ToString());
            field.Clear();
            fieldLength = 0;
        }

        void Add(char c)
        {
            if (char.IsLowSurrogate(c) && field.Length > 0 && char.IsHighSurrogate(field[^1]))
            {
                field.Append(c);
                return;
            }
            if (fieldLength >= FieldLimit)
            {
                throw new CsvError($"field larger than field limit ({FieldLimit})");
            }
            field.Append(c);
            fieldLength++;
        }

        void Process(int c)
        {
            switch (state)
            {
                case State.StartRecord:
                    if (c == Eol)
                    {
                        return;
                    }
                    if (c is '\n' or '\r')
                    {
                        state = State.EatCrnl;
                        return;
                    }
                    state = State.StartField;
                    goto case State.StartField;
                case State.StartField:
                    if (c is '\n' or '\r' or Eol)
                    {
                        Save();
                        state = c == Eol ? State.StartRecord : State.EatCrnl;
                    }
                    else if (c == '"')
                    {
                        state = State.InQuotedField;
                    }
                    else if (c == delimiter)
                    {
                        Save();
                    }
                    else
                    {
                        Add((char)c);
                        state = State.InField;
                    }
                    return;
                case State.InField:
                    if (c is '\n' or '\r' or Eol)
                    {
                        Save();
                        state = c == Eol ? State.StartRecord : State.EatCrnl;
                    }
                    else if (c == delimiter)
                    {
                        Save();
                        state = State.StartField;
                    }
                    else
                    {
                        Add((char)c);
                    }
                    return;
                case State.InQuotedField:
                    if (c == Eol)
                    {
                    }
                    else if (c == '"')
                    {
                        state = State.QuoteInQuotedField;
                    }
                    else
                    {
                        Add((char)c);
                    }
                    return;
                case State.QuoteInQuotedField:
                    if (c == '"')
                    {
                        Add('"');
                        state = State.InQuotedField;
                    }
                    else if (c == delimiter)
                    {
                        Save();
                        state = State.StartField;
                    }
                    else if (c is '\n' or '\r' or Eol)
                    {
                        Save();
                        state = c == Eol ? State.StartRecord : State.EatCrnl;
                    }
                    else
                    {
                        Add((char)c);
                        state = State.InField;
                    }
                    return;
                case State.EatCrnl:
                    if (c is '\n' or '\r')
                    {
                    }
                    else if (c == Eol)
                    {
                        state = State.StartRecord;
                    }
                    else
                    {
                        throw new CsvError("new-line character seen in unquoted field - do you need to open the file with newline=''?");
                    }
                    return;
            }
        }

        var pos = 0;
        while (true)
        {
            fields = [];
            field.Clear();
            fieldLength = 0;
            state = State.StartRecord;
            do
            {
                if (pos >= text.Length)
                {
                    if (fieldLength != 0 || state == State.InQuotedField)
                    {
                        Save();
                        rows.Add(fields);
                    }
                    return rows;
                }
                var newline = text.IndexOf('\n', pos);
                var end = newline < 0 ? text.Length : newline + 1;
                for (var i = pos; i < end; i++)
                {
                    Process(text[i]);
                }
                Process(Eol);
                pos = end;
            }
            while (state != State.StartRecord);
            rows.Add(fields);
        }
    }
}

/// <summary>Column statistics (column_stats in the Python): min/max are numbers or ISO dates.</summary>
public sealed class ColumnStats
{
    public required string Type { get; init; }
    public int Empty { get; init; }
    public int Distinct { get; init; }
    public int Count { get; init; }
    public double? MinNumber { get; init; }
    public double? MaxNumber { get; init; }
    public double? Sum { get; init; }
    public string? MinDate { get; init; }
    public string? MaxDate { get; init; }
    public int? Longest { get; init; }
    public bool HasMin => MinNumber != null || MinDate != null;
}

/// <summary>What the CSV module finds in a file: separator, header, column types, a text table, JSON.</summary>
public static class CsvTools
{
    public static readonly string[] Extensions = [".csv", ".tsv", ".tab", ".psv"];
    public const int MaxBytes = 50_000_000;
    public const int TableRows = 2000;
    public const int CellWidth = 40;

    // ───────────── reading ─────────────

    /// <summary>The file's text (UTF-8 with or without BOM, else Windows-1252, else Latin-1) and whether it was cut at MaxBytes.</summary>
    public static (string Text, bool Truncated) ReadText(string path)
    {
        byte[] data;
        using (var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete))
        using (var bytes = new MemoryStream())
        {
            var chunk = new byte[1 << 16];
            int n;
            while (bytes.Length < MaxBytes + 1 && (n = file.Read(chunk, 0, (int)Math.Min(chunk.Length, MaxBytes + 1 - bytes.Length))) > 0)
            {
                bytes.Write(chunk, 0, n);
            }
            data = bytes.ToArray();
        }
        var truncated = data.Length > MaxBytes;
        if (truncated)
        {
            data = data[..MaxBytes];
        }
        return (Decode(data), truncated);
    }

    static readonly UTF8Encoding StrictUtf8 = new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    /// <summary>Python's "utf-8-sig", then "cp1252" (strict: 0x81 0x8D 0x8F 0x90 0x9D fail), then "latin-1".</summary>
    public static string Decode(byte[] data)
    {
        try
        {
            var span = data.AsSpan();
            if (span.StartsWith((ReadOnlySpan<byte>)[0xEF, 0xBB, 0xBF]))
            {
                span = span[3..];
            }
            return StrictUtf8.GetString(span);
        }
        catch (DecoderFallbackException)
        {
        }
        var cp1252 = new StringBuilder(data.Length);
        foreach (var b in data)
        {
            if (b is >= 0x80 and <= 0x9F)
            {
                var c = Cp1252High[b - 0x80];
                if (c == '\0')
                {
                    return Latin1(data);
                }
                cp1252.Append(c);
            }
            else
            {
                cp1252.Append((char)b);
            }
        }
        return cp1252.ToString();
    }

    static string Latin1(byte[] data) => string.Create(data.Length, data, (chars, bytes) =>
    {
        for (var i = 0; i < bytes.Length; i++)
        {
            chars[i] = (char)bytes[i];
        }
    });

    /// <summary>0x80–0x9F in Windows-1252 ('\0': undefined, an error for Python's codec).</summary>
    const string Cp1252High = "€\0‚ƒ„…†‡ˆ‰Š‹Œ\0Ž\0\0‘’“”•–—˜™š›œ\0žŸ";

    /// <summary>
    /// The separator: tab for .tsv/.tab, pipe for .psv, else whichever of , ; \t | splits the first lines into
    /// the same number of fields most consistently (quotes respected).
    /// </summary>
    public static char Sniff(string text, string path = "")
    {
        var ext = Py.SplitExt(path).Ext.ToLowerInvariant();
        if (ext is ".tsv" or ".tab")
        {
            return '\t';
        }
        if (ext == ".psv")
        {
            return '|';
        }
        var sample = string.Join("\n", Py.SplitLines(text).Take(50));
        char best = ',';
        var bestScore = -1;
        foreach (var sep in new[] { ',', ';', '\t', '|' })
        {
            List<List<string>> rows;
            try
            {
                rows = [.. CsvReader.Read(sample, sep).Where(r => r.Count > 0)];
            }
            catch (CsvError)
            {
                continue;
            }
            if (rows.Count == 0)
            {
                continue;
            }
            var counts = rows.Select(r => r.Count).ToList();
            var width = MostCommon(counts);
            if (width < 2)
            {
                continue;
            }
            var score = counts.Count(c => c == width) * 10 + width; // consistency first, then more columns
            if (score > bestScore)
            {
                best = sep;
                bestScore = score;
            }
        }
        return best;
    }

    /// <summary>max(set(counts), key=counts.count): ties go to the first in the set's iteration order.</summary>
    static int MostCommon(List<int> counts)
    {
        int best = 0, bestCount = -1;
        foreach (var v in Py.SetOrder(counts))
        {
            var n = counts.Count(c => c == v);
            if (n > bestCount)
            {
                best = v;
                bestCount = n;
            }
        }
        return best;
    }

    /// <summary>The rows with anything in them.</summary>
    public static List<List<string>> Parse(string text, char sep) =>
        [.. CsvReader.Read(text, sep).Where(row => row.Any(c => !Py.IsBlank(c)))];

    /// <summary>A first row of distinct, non-empty, non-numeric names (or the only row).</summary>
    public static bool HasHeader(List<List<string>> rows)
    {
        if (rows.Count < 2)
        {
            return rows.Count > 0;
        }
        var first = rows[0];
        if (first.Distinct(StringComparer.Ordinal).Count() != first.Count || first.Any(Py.IsBlank))
        {
            return false;
        }
        return !first.Any(c => Kind(c) is "integer" or "decimal" or "date");
    }

    // ───────────── column types ─────────────

    static readonly Regex Integer = new(@"^[+-]?\d{1,3}(?:([.,' ])\d{3})*$|^[+-]?\d+$");
    static readonly Regex DecimalDot = new(@"^[+-]?(?:\d{1,3}(?:,\d{3})+|\d+)?\.\d+(?:[eE][+-]?\d+)?$|^[+-]?\d+(?:\.\d+)?[eE][+-]?\d+$");
    static readonly Regex DecimalComma = new(@"^[+-]?(?:\d{1,3}(?:\.\d{3})+|\d+)?,\d+$");
    static readonly Regex DotThousands = new(@"^[+-]?\d{1,3}(?:\.\d{3})+$");
    static readonly Regex DotThousandsGroup = new(@"^[+-]?\d{1,3}(\.\d{3})+$");
    static readonly Regex CommaThousand = new(@"^[+-]?\d{1,3},\d{3}$");
    static readonly Regex CommaThousands = new(@"^[+-]?\d{1,3}(,\d{3})+$");
    static readonly Regex Offset = new(@"[+-]\d\d:?\d\d$");
    static readonly HashSet<string> Bools = ["true", "false", "yes", "no", "ja", "nein", "y", "n"];

    /// <summary>The date formats tried in order, as Python's strptime matches them (its regexes, case-insensitive).</summary>
    static readonly Regex[] Dates = [.. new[]
    {
        "%Y-%m-%d", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S.%f", "%d.%m.%Y", "%d.%m.%Y %H:%M",
        "%d.%m.%Y %H:%M:%S", "%m/%d/%Y", "%d/%m/%Y", "%Y/%m/%d",
    }.Select(StrptimeRegex)];

    static Regex StrptimeRegex(string format)
    {
        var directives = new Dictionary<char, string>
        {
            ['Y'] = @"(?<Y>\d\d\d\d)",
            ['m'] = @"(?<m>1[0-2]|0[1-9]|[1-9])",
            ['d'] = @"(?<d>3[0-1]|[1-2]\d|0[1-9]|[1-9]| [1-9])",
            ['H'] = @"(?<H>2[0-3]|[0-1]\d|\d| \d)",
            ['M'] = @"(?<M>[0-5]\d|\d)",
            ['S'] = @"(?<S>6[0-1]|[0-5]\d|\d)",
            ['f'] = @"(?<f>[0-9]{1,6})",
        };
        var pattern = new StringBuilder("^");
        for (var i = 0; i < format.Length; i++)
        {
            if (format[i] == '%')
            {
                pattern.Append(directives[format[++i]]);
            }
            else if (format[i] == ' ')
            {
                pattern.Append(Py.SpaceClass + "+");
            }
            else
            {
                pattern.Append(Regex.Escape(format[i].ToString()));
            }
        }
        return new Regex(pattern.ToString(), RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    }

    static int Number(Group group) => int.Parse(Py.AsciiDigits(group.Value.Trim(' ')), CultureInfo.InvariantCulture);

    static DateTime? Strptime(string value, Regex format)
    {
        var m = format.Match(value);
        if (!m.Success || m.Length != value.Length)
        {
            return null;
        }
        int year = Number(m.Groups["Y"]), month = Number(m.Groups["m"]), day = Number(m.Groups["d"]);
        int hour = m.Groups["H"].Success ? Number(m.Groups["H"]) : 0;
        int minute = m.Groups["M"].Success ? Number(m.Groups["M"]) : 0;
        int second = m.Groups["S"].Success ? Number(m.Groups["S"]) : 0;
        int micro = m.Groups["f"].Success ? int.Parse(m.Groups["f"].Value.PadRight(6, '0'), CultureInfo.InvariantCulture) : 0;
        if (year < 1 || day > DateTime.DaysInMonth(year, month) || second > 59)
        {
            return null; // datetime() raises ValueError
        }
        return new DateTime(year, month, day, hour, minute, second).AddTicks(micro * 10L);
    }

    public static DateTime? ToDate(string value)
    {
        var v = Py.BmpDigits(Py.Strip(value)).TrimEnd('Z');
        if (v.Contains('T'))
        {
            v = Offset.Replace(v, "");
        }
        foreach (var format in Dates)
        {
            if (Strptime(v, format) is { } date)
            {
                return date;
            }
        }
        return null;
    }

    /// <summary>datetime.isoformat(sep=" ").</summary>
    public static string IsoFormat(DateTime d)
    {
        var text = d.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture);
        var micro = (int)(d.Ticks % TimeSpan.TicksPerSecond / 10);
        return micro != 0 ? text + "." + micro.ToString("000000", CultureInfo.InvariantCulture) : text;
    }

    /// <summary>
    /// empty, integer, decimal, boolean, date or text. With decimalComma (semicolon files, as Excel writes them in
    /// Germany or France), 1,5 and 1.234,5 are decimals and 1.234 is an integer.
    /// </summary>
    public static string Kind(string value, bool decimalComma = false)
    {
        var v = Py.BmpDigits(Py.Strip(value));
        if (v.Length == 0)
        {
            return "empty";
        }
        if (decimalComma && DecimalComma.IsMatch(v))
        {
            return "decimal";
        }
        if (decimalComma && DotThousands.IsMatch(v))
        {
            return "integer";
        }
        if (Integer.IsMatch(v) && !(Py.Len(v) > 1 && v[0] == '0' && v.All(char.IsDigit))) // 007 is a code, not a number
        {
            return "integer";
        }
        if (DecimalDot.IsMatch(v) || DecimalComma.IsMatch(v))
        {
            return "decimal";
        }
        if (Bools.Contains(v.ToLowerInvariant()))
        {
            return "boolean";
        }
        if (ToDate(v) != null)
        {
            return "date";
        }
        return "text";
    }

    public static double? ToNumber(string value, bool decimalComma = false)
    {
        var v = Py.BmpDigits(Py.Strip(value)).Replace("'", "").Replace(" ", "");
        if (decimalComma && (DecimalComma.IsMatch(v) || DotThousands.IsMatch(v)))
        {
            v = v.Replace(".", "").Replace(",", ".");
        }
        else if (DecimalComma.IsMatch(v) && !CommaThousand.IsMatch(v))
        {
            v = v.Replace(".", "").Replace(",", ".");
        }
        else if ((v.Contains(',') && v.Contains('.')) || CommaThousands.IsMatch(v))
        {
            v = v.Replace(",", "");
        }
        else if (DotThousandsGroup.IsMatch(v)) // 1.234.567 (German thousands)
        {
            v = v.Replace(".", "");
        }
        return Py.Float(v);
    }

    public static ColumnStats Stats(IReadOnlyList<string> values, bool decimalComma = false)
    {
        var kinds = values.Select(v => Kind(v, decimalComma)).ToList();
        var filled = kinds.Where(k => k != "empty").ToList();
        var present = values.Where((_, i) => kinds[i] != "empty").ToList();
        var kindsFound = filled.ToHashSet();
        string main;
        if (filled.Count == 0)
        {
            main = "empty";
        }
        else if (kindsFound.IsSubsetOf(["integer", "decimal"]))
        {
            main = kindsFound.Contains("decimal") ? "decimal" : "integer";
        }
        else if (kindsFound.Count == 1)
        {
            main = filled[0];
        }
        else
        {
            main = kindsFound.Contains("text") ? "text" : "mixed";
        }
        var empty = kinds.Count - filled.Count;
        var distinct = values.Select(Py.Strip).Where(v => v.Length > 0).Distinct(StringComparer.Ordinal).Count();
        if (main is "integer" or "decimal")
        {
            var numbers = present.Select(v => ToNumber(v, decimalComma)).OfType<double>().ToList();
            if (numbers.Count > 0)
            {
                return new() { Type = main, Empty = empty, Distinct = distinct, Count = values.Count, MinNumber = PyMin(numbers), MaxNumber = PyMax(numbers), Sum = Py.Sum(numbers) };
            }
        }
        else if (main == "date")
        {
            var dates = present.Select(ToDate).OfType<DateTime>().ToList();
            if (dates.Count > 0)
            {
                return new() { Type = main, Empty = empty, Distinct = distinct, Count = values.Count, MinDate = IsoFormat(dates.Min()), MaxDate = IsoFormat(dates.Max()) };
            }
        }
        else if (present.Count > 0)
        {
            return new() { Type = main, Empty = empty, Distinct = distinct, Count = values.Count, Longest = present.Max(Py.Len) };
        }
        return new() { Type = main, Empty = empty, Distinct = distinct, Count = values.Count };
    }

    // min()/max(): the first of equal values wins (matters only for 0.0 and -0.0).
    static double PyMin(List<double> values) => values.Aggregate((a, b) => b < a ? b : a);

    static double PyMax(List<double> values) => values.Aggregate((a, b) => b > a ? b : a);

    public static string NumberText(double n) =>
        double.IsFinite(n) && n == Math.Floor(n) && Math.Abs(n) < 1e15 ? ((long)n).ToString(CultureInfo.InvariantCulture) : Py.G6(n);

    public static string Describe(ColumnStats stats)
    {
        var parts = new List<string> { stats.Type };
        if (stats.MinNumber is { } lo && stats.MaxNumber is { } hi)
        {
            parts.Add($"{NumberText(lo)} … {NumberText(hi)}");
        }
        else if (stats.MinDate is { } first && stats.MaxDate is { } last)
        {
            parts.Add($"{first[..10]} … {last[..10]}");
        }
        if (stats.Empty != 0)
        {
            parts.Add($"{stats.Empty} empty");
        }
        parts.Add(stats.Distinct == stats.Count - stats.Empty && stats.Count > 1 ? "unique" : $"{stats.Distinct} distinct");
        return string.Join(" · ", parts);
    }

    // ───────────── output ─────────────

    static string Cell(List<string> row, int i) => i < row.Count ? row[i] : "";

    /// <summary>An aligned text table of the first TableRows rows, cells cut at CellWidth (lines end with \n).</summary>
    public static string Table(List<List<string>> rows, bool header)
    {
        var shown = rows.Take(TableRows + (header ? 1 : 0)).ToList();
        var width = shown.Max(r => r.Count);
        static string Cut(string c) => (Py.Len(c) <= CellWidth ? c : Py.Prefix(c, CellWidth - 1) + "…").Replace("\n", " ");
        var cells = shown.Select(r => Enumerable.Range(0, width).Select(i => i < r.Count ? Cut(r[i]) : "").ToList()).ToList();
        var body = cells.Skip(header ? 1 : 0).ToList();
        var numeric = Enumerable.Range(0, width).Select(i => body.All(row => Kind(row[i]) is "integer" or "decimal" or "empty")).ToList();
        var widths = Enumerable.Range(0, width).Select(i => cells.Max(row => Py.Len(row[i]))).ToList();
        var lines = new List<string>();
        for (var n = 0; n < cells.Count; n++)
        {
            var row = cells[n];
            lines.Add(Py.RStrip(string.Join(" │ ", row.Select((c, i) => numeric[i] && !(header && n == 0) ? Py.RJust(c, widths[i]) : Py.LJust(c, widths[i])))));
            if (header && n == 0)
            {
                lines.Add(string.Join("─┼─", widths.Select(w => new string('─', w))));
            }
        }
        if (rows.Count > shown.Count)
        {
            lines.Add($"… {rows.Count - shown.Count} more rows");
        }
        return string.Join("\n", lines) + "\n";
    }

    /// <summary>
    /// Objects keyed by the header (or column1…), numbers and booleans as JSON values: json.dumps(indent=2,
    /// ensure_ascii=False) + "\n".
    /// </summary>
    public static string ToJson(List<List<string>> rows, bool header, bool decimalComma = false)
    {
        var names = header ? rows[0] : [.. Enumerable.Range(1, rows.Max(r => r.Count)).Select(i => $"column{i}")];
        var data = header ? rows.Skip(1).ToList() : rows;
        var types = Enumerable.Range(0, names.Count).Select(i => Stats([.. data.Select(r => Cell(r, i))], decimalComma).Type).ToList();
        var json = new StringBuilder();
        if (data.Count == 0)
        {
            return "[]\n";
        }
        json.Append("[\n");
        for (var r = 0; r < data.Count; r++)
        {
            var row = data[r];
            // A dict: a repeated name keeps its first place and its last value.
            var item = new List<(string Name, string Value)>();
            for (var i = 0; i < names.Count; i++)
            {
                var v = Cell(row, i);
                string value;
                if (Py.IsBlank(v))
                {
                    value = "null";
                }
                else if (types[i] == "integer" && ToNumber(v, decimalComma) is { } whole)
                {
                    value = Py.Int(whole).ToString(CultureInfo.InvariantCulture);
                }
                else if (types[i] == "decimal" && ToNumber(v, decimalComma) is { } number)
                {
                    value = Py.Repr(number);
                }
                else if (types[i] == "boolean")
                {
                    value = Py.Strip(v).ToLowerInvariant() is "true" or "yes" or "ja" or "y" ? "true" : "false";
                }
                else
                {
                    value = JsonString(v);
                }
                var at = item.FindIndex(p => p.Name == names[i]);
                if (at >= 0)
                {
                    item[at] = (names[i], value);
                }
                else
                {
                    item.Add((names[i], value));
                }
            }
            if (item.Count == 0)
            {
                json.Append("  {}");
            }
            else
            {
                json.Append("  {\n");
                json.AppendJoin(",\n", item.Select(p => $"    {JsonString(p.Name)}: {p.Value}"));
                json.Append("\n  }");
            }
            json.Append(r < data.Count - 1 ? ",\n" : "\n");
        }
        json.Append("]\n");
        return json.ToString();
    }

    /// <summary>A JSON string as Python's json.dumps(ensure_ascii=False) writes it.</summary>
    public static string JsonString(string s)
    {
        var b = new StringBuilder(s.Length + 2).Append('"');
        foreach (var c in s)
        {
            b.Append(c switch
            {
                '"' => "\\\"",
                '\\' => "\\\\",
                '\n' => "\\n",
                '\r' => "\\r",
                '\t' => "\\t",
                '\b' => "\\b",
                '\f' => "\\f",
                < ' ' => $"\\u{(int)c:x4}",
                _ => c.ToString(),
            });
        }
        return b.Append('"').ToString();
    }

}
