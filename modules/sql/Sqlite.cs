using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;
using Microsoft.Data.Sqlite;

namespace ByteForge.Sql;

/// <summary>SQLite through Microsoft.Data.Sqlite: tables, running scripts, a small shell.</summary>
public static partial class Sqlite
{
    /// <summary>Opens a database file (read-only unless <paramref name="readOnly"/> is false); it must exist.</summary>
    public static SqliteConnection Open(string? path, bool readOnly = true)
    {
        if (string.IsNullOrEmpty(path) || (path != ":memory:" && !File.Exists(path)))
        {
            throw new SqlException($"{(string.IsNullOrEmpty(path) ? "the database file" : path)} doesn't exist");
        }
        var builder = new SqliteConnectionStringBuilder
        {
            DataSource = path,
            Mode = readOnly && path != ":memory:" ? SqliteOpenMode.ReadOnly : SqliteOpenMode.ReadWriteCreate,
            DefaultTimeout = 5,
            // Nothing stays open (or locked) once a connection is closed.
            Pooling = false,
        };
        var db = new SqliteConnection(builder.ToString());
        try
        {
            db.Open();
        }
        catch (SqliteException error)
        {
            db.Dispose();
            throw new SqlException(Message(error));
        }
        return db;
    }

    [GeneratedRegex(@"^SQLite Error \d+: '(.*)'\.$", RegexOptions.Singleline)]
    private static partial Regex ErrorText();

    /// <summary>SQLite's own message ("no such table: x"), as Python's sqlite3 shows it.</summary>
    public static string Message(SqliteException error) => ErrorText().Match(error.Message) is { Success: true } m ? m.Groups[1].Value : error.Message;

    public static List<Table> Tables(string? path)
    {
        using var db = Open(path);
        try
        {
            var tables = new List<Table>();
            using (var list = db.CreateCommand())
            {
                list.CommandText = "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY type, name";
                using var reader = list.ExecuteReader();
                while (reader.Read())
                {
                    tables.Add(new Table { Name = reader.GetString(0), View = reader.GetString(1) == "view" });
                }
            }
            foreach (var table in tables)
            {
                using var info = db.CreateCommand();
                info.CommandText = $"PRAGMA table_info({Clients.QuoteIdentifier("sqlite", table.Name)})";
                using var reader = info.ExecuteReader();
                while (reader.Read())
                {
                    // cid, name, type, notnull, dflt_value, pk
                    var notNull = !reader.IsDBNull(3) && reader.GetInt64(3) != 0;
                    var pk = !reader.IsDBNull(5) && reader.GetInt64(5) != 0;
                    table.Columns.Add(new Column(reader.GetString(1), reader.IsDBNull(2) ? "" : reader.GetString(2), !notNull && !pk, pk));
                }
            }
            return tables;
        }
        catch (SqliteException error)
        {
            throw new SqlException(Message(error));
        }
    }

    /// <summary>Whether <paramref name="text"/> ends a complete statement (sqlite3_complete).</summary>
    public static bool Complete(string text)
    {
        SQLitePCL.Batteries_V2.Init();
        return SQLitePCL.raw.sqlite3_complete(text) != 0;
    }

    /// <summary>SQLite statements of a script, each ending with its ';' (the last one may not have one).</summary>
    public static List<string> SplitStatements(string text)
    {
        var statements = new List<string>();
        var start = 0;
        for (var i = 0; i < text.Length; i++)
        {
            if (text[i] == ';' && Complete(text[start..(i + 1)]))
            {
                statements.Add(text[start..(i + 1)]);
                start = i + 1;
            }
        }
        var rest = text[start..];
        if (Py.Strip(SqlFormatter.StripComments(rest)).Length > 0)
        {
            statements.Add(rest);
        }
        return statements.Where(s => SqlFormatter.StripComments(s).Trim(' ', '\t', '\r', '\n', ';').Length > 0).Select(Py.Strip).ToList();
    }

    static bool IsNumber(object? value) => value is long or int or double or float or decimal;

    static string Cell(object? value, int maxWidth)
    {
        switch (value)
        {
            case null or DBNull:
                return "NULL";
            case byte[] bytes:
                return "0x" + Convert.ToHexStringLower(bytes.AsSpan(0, Math.Min(32, bytes.Length))) + (bytes.Length > 32 ? "…" : "");
        }
        var text = value switch
        {
            double d => Py.FloatRepr(d),
            IFormattable f => f.ToString(null, CultureInfo.InvariantCulture),
            _ => value.ToString() ?? "",
        };
        text = text.Replace("\r", "").Replace("\n", "⏎");
        return Py.Len(text) <= maxWidth ? text : Py.Take(text, maxWidth - 1) + "…";
    }

    /// <summary>Rows as a text table: numbers line up on the right, everything else on the left.</summary>
    public static string FormatTable(IReadOnlyList<string> columns, IReadOnlyList<object?[]> rows, int maxWidth = 60)
    {
        var cells = rows.Select(row => row.Select(v => Cell(v, maxWidth)).ToArray()).ToList();
        var widths = columns.Select(Py.Len).ToList();
        foreach (var row in cells)
        {
            widths = widths.Zip(row, (w, v) => Math.Max(w, Py.Len(v))).ToList();
        }
        var lines = new List<string>
        {
            Py.RStrip(string.Join(" | ", columns.Zip(widths, (c, w) => Py.LJust(c, w)))),
            string.Join("-+-", widths.Select(w => new string('-', w))),
        };
        for (var r = 0; r < rows.Count; r++)
        {
            var parts = rows[r].Zip(cells[r]).Zip(widths, (vt, w) => IsNumber(vt.First) ? Py.RJust(vt.Second, w) : Py.LJust(vt.Second, w));
            lines.Add(Py.RStrip(string.Join(" | ", parts)));
        }
        return string.Join("\n", lines);
    }

    static void Print(TextWriter output, string text)
    {
        output.Write(text + "\n");
        output.Flush();
    }

    /// <summary>Python's sqlite3 counts changed rows only for INSERT, UPDATE, DELETE and REPLACE.</summary>
    static bool IsDml(string statement)
    {
        var text = Py.LStrip(SqlFormatter.StripComments(statement)).ToLowerInvariant();
        return text.StartsWith("insert", StringComparison.Ordinal) || text.StartsWith("update", StringComparison.Ordinal) ||
            text.StartsWith("delete", StringComparison.Ordinal) || text.StartsWith("replace", StringComparison.Ordinal);
    }

    static string Plural(long n, string word) => $"{n} {word}{(n != 1 ? "s" : "")}";

    /// <summary>Runs a script statement by statement and prints each result. Returns the exit code.</summary>
    public static int RunScript(string? path, string text, TextWriter output)
    {
        SqliteConnection db;
        try
        {
            db = Open(path, readOnly: false);
        }
        catch (SqlException error)
        {
            Print(output, $"Error: {error.Message}");
            return 1;
        }
        var started = Stopwatch.StartNew();
        using (db)
        {
            var number = 0;
            foreach (var statement in SplitStatements(text))
            {
                number++;
                try
                {
                    using var command = db.CreateCommand();
                    command.CommandText = statement;
                    using var reader = command.ExecuteReader();
                    if (reader.FieldCount > 0)
                    {
                        var columns = Enumerable.Range(0, reader.FieldCount).Select(reader.GetName).ToList();
                        var rows = new List<object?[]>();
                        while (reader.Read())
                        {
                            var row = new object?[reader.FieldCount];
                            for (var i = 0; i < row.Length; i++)
                            {
                                row[i] = reader.IsDBNull(i) ? null : reader.GetValue(i);
                            }
                            rows.Add(row);
                        }
                        Print(output, FormatTable(columns, rows));
                        Print(output, $"({Plural(rows.Count, "row")})\n");
                    }
                    else if (IsDml(statement))
                    {
                        Print(output, $"OK, {Plural(Math.Max(0, reader.RecordsAffected), "row")} affected\n");
                    }
                }
                catch (SqliteException error)
                {
                    var first = Py.Take(Py.SplitLines(statement).FirstOrDefault() ?? "", 80);
                    Print(output, $"Error in statement {number} ({first}): {Message(error)}");
                    return 1;
                }
            }
            Print(output, $"Done in {started.Elapsed.TotalSeconds.ToString("0.00", CultureInfo.InvariantCulture)}s");
            return 0;
        }
    }

    /// <summary>A small interactive SQLite shell (used when the sqlite3 program isn't installed).</summary>
    public static int Shell(string path, TextReader input, TextWriter output)
    {
        SQLitePCL.Batteries_V2.Init();
        Print(output, $"SQLite {SQLitePCL.raw.sqlite3_libversion().utf8_to_string()} · {path}\nEnd statements with ';'. .tables lists the tables, .quit exits.");
        var buffer = new StringBuilder();
        while (true)
        {
            output.Write(buffer.Length == 0 ? "sqlite> " : "   ...> ");
            output.Flush();
            var line = input.ReadLine();
            if (line == null)
            {
                Print(output, "");
                return 0;
            }
            if (buffer.Length == 0 && Py.Strip(line).StartsWith('.'))
            {
                var command = Py.Strip(line).Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries)[0];
                if (command is ".quit" or ".exit" or ".q")
                {
                    return 0;
                }
                if (command == ".tables")
                {
                    try
                    {
                        foreach (var table in Tables(path))
                        {
                            Print(output, table.Name + (table.View ? " (view)" : ""));
                        }
                    }
                    catch (SqlException error)
                    {
                        Print(output, $"Error: {error.Message}");
                    }
                }
                else
                {
                    Print(output, "Commands: .tables, .quit");
                }
                continue;
            }
            buffer.Append(line).Append('\n');
            if (Complete(buffer.ToString()))
            {
                RunScript(path, buffer.ToString(), output);
                buffer.Clear();
            }
        }
    }
}
