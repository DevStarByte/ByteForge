using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace ByteForge.FakeData;

/// <summary>Records as JSON, C#, CSV, SQL INSERT or YAML.</summary>
public static partial class Render
{
    /// <summary>A record kind: its label and the fields, in the pick list.</summary>
    public sealed record Kind(string Key, string Label, string Detail);

    public static readonly Kind[] RecordKinds =
    [
        new("person", "People", "id, firstName, lastName, email, phone, birthDate"),
        new("customer", "Customers", "name, email, phone, company, street, zip, city, country, createdAt"),
        new("user", "User accounts", "id (GUID), userName, email, displayName, role, isActive, lastLogin"),
        new("product", "Products", "sku, name, category, price, stock, isAvailable"),
        new("order", "Orders", "orderNumber, customer, email, total, currency, status, createdAt"),
        new("company", "Companies", "name, vatId, email, phone, website, city, iban"),
    ];

    public static readonly Kind[] Formats =
    [
        new("json", "JSON", "an array of objects (also for .http bodies)"),
        new("csharp", "C#", "new List<T> { new T { … } } with typed values"),
        new("csv", "CSV", "a header row, comma separated"),
        new("sql", "SQL INSERT", "one INSERT with a row per record, snake_case columns"),
        new("yaml", "YAML", "a list of mappings"),
    ];

    static readonly Dictionary<string, string> ClassNames = new()
    {
        ["person"] = "Person", ["customer"] = "Customer", ["user"] = "User", ["product"] = "Product", ["order"] = "Order", ["company"] = "Company",
    };

    static string Plural(string kind) => kind switch
    {
        "person" => "people",
        "company" => "companies",
        _ => kind + "s",
    };

    public static string ToText(IReadOnlyList<Record> records, string kind, string format) => format switch
    {
        "json" => ToJson(records),
        "csharp" => ToCSharp(records, kind),
        "csv" => ToCsv(records),
        "sql" => ToSql(records, kind),
        _ => ToYaml(records),
    };

    public static string IsoDate(DateOnly date) => date.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture);

    /// <summary>ISO 8601 in UTC to the second, with Z.</summary>
    public static string IsoMoment(DateTimeOffset moment) => moment.UtcDateTime.ToString("yyyy-MM-dd'T'HH:mm:ss", CultureInfo.InvariantCulture) + "Z";

    static string Snake(string name) => UpperCase().Replace(name, "_").ToLowerInvariant();

    [GeneratedRegex("(?<!^)(?=[A-Z])")]
    private static partial Regex UpperCase();

    [GeneratedRegex("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")]
    private static partial Regex GuidText();

    static string Pascal(string name) => char.ToUpperInvariant(name[0]) + name[1..];

    /// <summary>Dates as ISO text, everything else as is.</summary>
    static object? Plain(object? value) => value switch
    {
        DateTimeOffset moment => IsoMoment(moment),
        DateOnly date => IsoDate(date),
        _ => value,
    };

    static string Number(object value) => value switch
    {
        double d => Py.Repr(d),
        IFormattable f => f.ToString(null, CultureInfo.InvariantCulture),
        _ => value.ToString() ?? "",
    };

    static string Json(object? value) => Plain(value) switch
    {
        null => "null",
        bool b => b ? "true" : "false",
        double d => Py.JsonNumber(d),
        string s => Py.JsonString(s),
        var other => Number(other),
    };

    public static string ToJson(IReadOnlyList<Record> records)
    {
        if (records.Count == 0)
        {
            return "[]";
        }
        var json = new StringBuilder("[\n");
        for (var i = 0; i < records.Count; i++)
        {
            var record = records[i];
            if (record.Count == 0)
            {
                json.Append("  {}");
            }
            else
            {
                json.Append("  {\n");
                var n = 0;
                foreach (var (key, value) in record)
                {
                    json.Append("    ").Append(Py.JsonString(key)).Append(": ").Append(Json(value));
                    json.Append(++n < record.Count ? ",\n" : "\n");
                }
                json.Append("  }");
            }
            json.Append(i + 1 < records.Count ? ",\n" : "\n");
        }
        return json.Append(']').ToString();
    }

    static string CSharpValue(object? value) => value switch
    {
        bool b => b ? "true" : "false",
        double d => d.ToString("F2", CultureInfo.InvariantCulture) + "m",
        DateTimeOffset m => $"new DateTimeOffset({m.Year}, {m.Month}, {m.Day}, {m.Hour}, {m.Minute}, {m.Second}, TimeSpan.Zero)",
        DateOnly d => $"new DateOnly({d.Year}, {d.Month}, {d.Day})",
        string s when GuidText().IsMatch(s) => $"Guid.Parse(\"{s}\")",
        string s => Py.JsonString(s), // a C# string literal for everything generated
        null => "null",
        _ => Number(value),
    };

    public static string ToCSharp(IReadOnlyList<Record> records, string kind)
    {
        var name = ClassNames[kind];
        var lines = new List<string> { $"var {Plural(kind)} = new List<{name}>", "{" };
        foreach (var record in records)
        {
            var properties = string.Join(", ", record.Select(p => $"{Pascal(p.Key)} = {CSharpValue(p.Value)}"));
            lines.Add($"    new {name} {{ {properties} }},");
        }
        lines.Add("};");
        return string.Join("\n", lines);
    }

    static string CsvCell(object? value)
    {
        var text = Plain(value) switch
        {
            true => "true",
            false => "false",
            null => "",
            string s => s,
            var other => Number(other),
        };
        return text.AsSpan().IndexOfAny("\",\n") >= 0 ? "\"" + text.Replace("\"", "\"\"", StringComparison.Ordinal) + "\"" : text;
    }

    public static string ToCsv(IReadOnlyList<Record> records)
    {
        var keys = records[0].Keys.ToList();
        return string.Join("\n", new[] { string.Join(",", keys) }.Concat(records.Select(r => string.Join(",", keys.Select(k => CsvCell(r[k]))))));
    }

    static string SqlValue(object? value) => Plain(value) switch
    {
        bool b => b ? "TRUE" : "FALSE",
        null => "NULL",
        string s => "'" + s.Replace("'", "''", StringComparison.Ordinal) + "'",
        var other => Number(other),
    };

    public static string ToSql(IReadOnlyList<Record> records, string kind)
    {
        var keys = records[0].Keys.ToList();
        var rows = string.Join(",\n", records.Select(r => "    (" + string.Join(", ", keys.Select(k => SqlValue(r[k]))) + ")"));
        return $"INSERT INTO {Plural(kind)} ({string.Join(", ", keys.Select(Snake))})\nVALUES\n{rows};";
    }

    static string YamlValue(object? value) => Plain(value) switch
    {
        bool b => b ? "true" : "false",
        null => "null",
        string s => Py.JsonString(s), // a double-quoted YAML string
        var other => Number(other),
    };

    public static string ToYaml(IReadOnlyList<Record> records)
    {
        var lines = new List<string>();
        foreach (var record in records)
        {
            var first = true;
            foreach (var (key, value) in record)
            {
                lines.Add($"{(first ? "- " : "  ")}{key}: {YamlValue(value)}");
                first = false;
            }
        }
        return string.Join("\n", lines);
    }
}
