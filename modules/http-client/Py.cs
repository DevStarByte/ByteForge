using System.Globalization;
using System.Numerics;
using System.Text;
using System.Text.RegularExpressions;

namespace ByteForge.HttpClientModule;

/// <summary>
/// The bits of Python's behavior the module's file format and output depend on, so .http files are read and
/// responses written exactly as the Python module did: str.splitlines / strip, int(), regex classes, strftime.
/// </summary>
public static class Py
{
    /// <summary>Python's \w (letters, numbers, underscore) for regex patterns.</summary>
    public const string W = "[" + WChars + "]";

    /// <summary>What Python's \w matches, for use inside a character class.</summary>
    public const string WChars = @"\p{L}\p{N}_";

    /// <summary>Python's \s (also the information separators \x1c-\x1f).</summary>
    public const string S = @"[\s\x1c-\x1f]";

    /// <summary>Python's \S.</summary>
    public const string NS = @"[^\s\x1c-\x1f]";

    public static bool IsSpace(char c) => char.IsWhiteSpace(c) || c is >= '\x1c' and <= '\x1f';

    public static string Strip(string s) => s.Trim(SpaceChars);

    public static string LStrip(string s) => s.TrimStart(SpaceChars);

    public static string RStrip(string s) => s.TrimEnd(SpaceChars);

    static readonly char[] SpaceChars = [.. Enumerable.Range(0, 0x10000).Select(i => (char)i).Where(IsSpace)];

    /// <summary>str.splitlines(): \n, \r, \r\n, \v, \f, \x1c-\x1e, \x85, U+2028 and U+2029 end lines; no empty last line.</summary>
    public static List<string> SplitLines(string text)
    {
        var lines = new List<string>();
        var start = 0;
        for (var i = 0; i < text.Length; i++)
        {
            var c = text[i];
            if (c is '\n' or '\r' or '\v' or '\f' or '\x1c' or '\x1d' or '\x1e' or '\x85' or '\u2028' or '\u2029')
            {
                lines.Add(text[start..i]);
                if (c == '\r' && i + 1 < text.Length && text[i + 1] == '\n')
                {
                    i++;
                }
                start = i + 1;
            }
        }
        if (start < text.Length)
        {
            lines.Add(text[start..]);
        }
        return lines;
    }

    /// <summary>What reading a file in Python's text mode gives: UTF-8 (a BOM dropped), bad bytes replaced, universal newlines.</summary>
    public static string ReadText(string path)
    {
        var bytes = File.ReadAllBytes(path);
        var skip = bytes.AsSpan().StartsWith((ReadOnlySpan<byte>)[0xEF, 0xBB, 0xBF]) ? 3 : 0;
        return Universal(new UTF8Encoding(false).GetString(bytes, skip, bytes.Length - skip));
    }

    /// <summary>Universal newlines: \r\n and \r become \n.</summary>
    public static string Universal(string text) => text.Replace("\r\n", "\n").Replace('\r', '\n');

    /// <summary>
    /// int(text): surrounding whitespace, a sign, digits (any script) with single underscores between them. Throws
    /// <see cref="FormatException"/> with Python's message otherwise.
    /// </summary>
    public static long Int(string text)
    {
        var s = Strip(text);
        var digits = new StringBuilder();
        var i = 0;
        var negative = false;
        if (s.Length > 0 && s[0] is '+' or '-')
        {
            negative = s[0] == '-';
            i = 1;
        }
        var lastUnderscore = true;
        for (; i < s.Length; i++)
        {
            var value = CharUnicodeInfo.GetDecimalDigitValue(s[i]);
            if (value >= 0)
            {
                digits.Append((char)('0' + value));
                lastUnderscore = false;
            }
            else if (s[i] == '_' && !lastUnderscore)
            {
                lastUnderscore = true;
            }
            else
            {
                digits.Clear();
                break;
            }
        }
        if (digits.Length == 0 || lastUnderscore)
        {
            throw new FormatException($"invalid literal for int() with base 10: {Repr(text)}");
        }
        var number = BigInteger.Parse(digits.ToString(), CultureInfo.InvariantCulture);
        return (long)(negative ? -number : number);
    }

    /// <summary>repr() of a string (the common cases).</summary>
    public static string Repr(string s) => s.Contains('\'') && !s.Contains('"') ? $"\"{s}\"" : "'" + s.Replace("\\", "\\\\").Replace("'", "\\'") + "'";

    // ───────────── strftime ─────────────

    static readonly string[] Days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    static readonly string[] Months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

    /// <summary>datetime.strftime with the C locale's names; unknown directives stay as they are (as glibc does).</summary>
    public static string Strftime(DateTimeOffset moment, string format, string zoneName)
    {
        var d = moment.DateTime;
        var result = new StringBuilder();
        for (var i = 0; i < format.Length; i++)
        {
            if (format[i] != '%' || i + 1 >= format.Length)
            {
                result.Append(format[i]);
                continue;
            }
            var directive = format[++i];
            result.Append(directive switch
            {
                'a' => Days[(int)d.DayOfWeek][..3],
                'A' => Days[(int)d.DayOfWeek],
                'b' or 'h' => Months[d.Month - 1][..3],
                'B' => Months[d.Month - 1],
                'c' => $"{Days[(int)d.DayOfWeek][..3]} {Months[d.Month - 1][..3]} {d.Day,2} {d:HH:mm:ss} {d.Year}",
                'd' => d.Day.ToString("00", CultureInfo.InvariantCulture),
                'e' => d.Day.ToString(CultureInfo.InvariantCulture).PadLeft(2),
                'f' => (d.Ticks % TimeSpan.TicksPerSecond / 10).ToString("000000", CultureInfo.InvariantCulture),
                'F' => d.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture),
                'H' => d.Hour.ToString("00", CultureInfo.InvariantCulture),
                'I' => ((d.Hour + 11) % 12 + 1).ToString("00", CultureInfo.InvariantCulture),
                'j' => d.DayOfYear.ToString("000", CultureInfo.InvariantCulture),
                'm' => d.Month.ToString("00", CultureInfo.InvariantCulture),
                'M' => d.Minute.ToString("00", CultureInfo.InvariantCulture),
                'p' => d.Hour < 12 ? "AM" : "PM",
                'S' => d.Second.ToString("00", CultureInfo.InvariantCulture),
                'T' => d.ToString("HH:mm:ss", CultureInfo.InvariantCulture),
                'u' => (d.DayOfWeek == DayOfWeek.Sunday ? 7 : (int)d.DayOfWeek).ToString(CultureInfo.InvariantCulture),
                'w' => ((int)d.DayOfWeek).ToString(CultureInfo.InvariantCulture),
                'x' => d.ToString("MM/dd/yy", CultureInfo.InvariantCulture),
                'X' => d.ToString("HH:mm:ss", CultureInfo.InvariantCulture),
                'y' => (d.Year % 100).ToString("00", CultureInfo.InvariantCulture),
                'Y' => d.Year.ToString("0000", CultureInfo.InvariantCulture),
                'z' => Offset(moment.Offset, ""),
                'Z' => zoneName,
                '%' => "%",
                _ => "%" + directive,
            });
        }
        return result.ToString();
    }

    /// <summary>A UTC offset as +HH:MM (<paramref name="separator"/> ":") or +HHMM, with seconds when it has any.</summary>
    public static string Offset(TimeSpan offset, string separator)
    {
        var sign = offset < TimeSpan.Zero ? "-" : "+";
        var o = offset.Duration();
        var text = $"{sign}{o.Hours + o.Days * 24:00}{separator}{o.Minutes:00}";
        return o.Seconds != 0 ? $"{text}{separator}{o.Seconds:00}" : text;
    }

    /// <summary>datetime.isoformat(timespec="seconds") of an aware datetime.</summary>
    public static string IsoFormat(DateTimeOffset moment) =>
        moment.ToString("yyyy-MM-dd'T'HH:mm:ss", CultureInfo.InvariantCulture) + Offset(moment.Offset, ":");

}
