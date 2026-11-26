using System.Text;
using System.Text.RegularExpressions;

namespace ByteForge.Sql;

/// <summary>The .sql formatter: keyword case, no trailing whitespace, one final newline.</summary>
public static partial class SqlFormatter
{
    public static readonly HashSet<string> Keywords = """
        ADD ALL ALTER AND ANY AS ASC AUTOINCREMENT AUTO_INCREMENT BEGIN BETWEEN BIGINT BINARY BIT BLOB BOOLEAN BOTH BY CASCADE
        CASE CAST CHAR CHARACTER CHECK CLUSTERED COALESCE COLLATE COLUMN COMMIT CONFLICT CONSTRAINT CONTINUE CREATE CROSS
        CURRENT_DATE CURRENT_TIME CURRENT_TIMESTAMP CURSOR DATABASE DATE DATETIME DATETIME2 DECIMAL DECLARE DEFAULT DEFERRABLE
        DELETE DESC DISTINCT DO DOUBLE DROP EACH ELSE ELSIF END ESCAPE EXCEPT EXEC EXECUTE EXISTS EXPLAIN FALSE FETCH FILTER
        FIRST FLOAT FOR FOREIGN FROM FULL FUNCTION GO GRANT GROUP HAVING IDENTITY IF IGNORE ILIKE IN INDEX INNER INSERT INT
        INTEGER INTERSECT INTERVAL INTO IS JOIN JSON JSONB KEY LANGUAGE LAST LATERAL LEADING LEFT LIKE LIMIT MATERIALIZED MERGE
        NATURAL NCHAR NEXT NO NOCOUNT NONCLUSTERED NOT NOTHING NULL NULLS NUMERIC NVARCHAR OF OFFSET ON ONLY OR ORDER OUTER OUTPUT
        OVER PARTITION PRAGMA PRECISION PRIMARY PROCEDURE RAISE REAL RECURSIVE REFERENCES REINDEX RELEASE RENAME REPLACE
        RESTRICT RETURN RETURNING RETURNS REVOKE RIGHT ROLLBACK ROW ROWS SAVEPOINT SCHEMA SELECT SEQUENCE SERIAL SET SMALLINT
        TABLE TEMP TEMPORARY TEXT THEN TIME TIMESTAMP TIMESTAMPTZ TINYINT TO TOP TRAILING TRAN TRANSACTION TRIGGER TRUE
        TRUNCATE UNION UNIQUE UNIQUEIDENTIFIER UNSIGNED UPDATE UPSERT USE USING UUID VACUUM VALUES VARBINARY VARCHAR VIEW WHEN
        WHERE WHILE WINDOW WITH WITHOUT
        """.Split((char[])[' ', '\n', '\r'], StringSplitOptions.RemoveEmptyEntries).ToHashSet(StringComparer.Ordinal);

    // Python's \Z (the very end) is .NET's \z.
    [GeneratedRegex(
        @"(?<comment>--[^\n]*|/\*.*?(?:\*/|\z))" +
        @"|(?<dollar>\$\$.*?(?:\$\$|\z)|\$(?<tag>[A-Za-z_][A-Za-z0-9_]*)\$.*?(?:\$\k<tag>\$|\z))" +
        @"|(?<string>[NnEeXxBb]?'(?:[^']|'')*(?:'|\z))" +
        @"|(?<quoted>""(?:[^""]|"""")*(?:""|\z)|\[[^\]\n]*(?:\]|\z)|`(?:[^`]|``)*(?:`|\z))" +
        @"|(?<word>[A-Za-z_][A-Za-z0-9_$]*)" +
        @"|(?<other>\s+|.)",
        RegexOptions.Singleline)]
    private static partial Regex TokenRe();

    static readonly string[] Kinds = ["comment", "dollar", "string", "quoted", "word", "other"];

    /// <summary>(kind, text) tokens: comment, string (also dollar-quoted bodies), quoted (identifier), word or other.</summary>
    public static IEnumerable<(string Kind, string Text)> Tokenize(string text)
    {
        foreach (Match match in TokenRe().Matches(text))
        {
            var kind = Kinds.First(k => match.Groups[k].Success);
            yield return (kind == "dollar" ? "string" : kind, match.Value);
        }
    }

    public static string StripComments(string text)
    {
        var builder = new StringBuilder();
        foreach (var (kind, token) in Tokenize(text))
        {
            if (kind != "comment")
            {
                builder.Append(token);
            }
        }
        return builder.ToString();
    }

    /// <summary>
    /// Keywords in one case (outside strings, comments and quoted names), no trailing whitespace, one final
    /// newline. Everything else stays as written. <paramref name="keywordCase"/>: upper, lower or preserve.
    /// </summary>
    public static string Format(string text, string keywordCase = "upper")
    {
        var builder = new StringBuilder(text.Length + 16);
        var previous = "";
        foreach (var (kind, original) in Tokenize(text))
        {
            var token = original;
            // A word after '.' or '@' / ':' is a name (t.key, @user), not a keyword.
            if (kind == "word" && Keywords.Contains(token.ToUpperInvariant()) && previous is not ("." or "@" or ":" or "#"))
            {
                if (keywordCase == "upper")
                {
                    token = token.ToUpperInvariant();
                }
                else if (keywordCase == "lower")
                {
                    token = token.ToLowerInvariant();
                }
            }
            builder.Append(token);
            if (kind != "other" || Py.Strip(token).Length > 0)
            {
                previous = token[^1..];
            }
        }
        var lines = builder.ToString().Replace("\r\n", "\n").Split('\n').Select(Py.RStrip).ToList();
        while (lines.Count > 0 && lines[^1].Length == 0)
        {
            lines.RemoveAt(lines.Count - 1);
        }
        return lines.Count > 0 ? string.Join("\n", lines) + "\n" : "";
    }
}
