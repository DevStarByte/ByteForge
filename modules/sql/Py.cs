using System.Globalization;
using System.Text;

namespace ByteForge.Sql;

/// <summary>Python's string and path rules where the module's output depends on them (it was written in Python).</summary>
public static class Py
{
    /// <summary>str.isspace for one character (also the separators \x1c-\x1f, which .NET doesn't count).</summary>
    public static bool IsSpace(char c) => char.IsWhiteSpace(c) || (c >= '\x1c' && c <= '\x1f');

    public static string Strip(string s) => RStrip(LStrip(s));

    public static string LStrip(string s)
    {
        var i = 0;
        while (i < s.Length && IsSpace(s[i]))
        {
            i++;
        }
        return s[i..];
    }

    public static string RStrip(string s)
    {
        var end = s.Length;
        while (end > 0 && IsSpace(s[end - 1]))
        {
            end--;
        }
        return s[..end];
    }

    /// <summary>str.splitlines(): every line break Python knows, without the breaks.</summary>
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

    /// <summary>len(): code points, not UTF-16 units.</summary>
    public static int Len(string s)
    {
        var n = 0;
        foreach (var _ in s.EnumerateRunes())
        {
            n++;
        }
        return n;
    }

    /// <summary>s[:count] in code points.</summary>
    public static string Take(string s, int count)
    {
        var builder = new StringBuilder();
        foreach (var rune in s.EnumerateRunes())
        {
            if (count-- <= 0)
            {
                break;
            }
            builder.Append(rune.ToString());
        }
        return builder.ToString();
    }

    public static string LJust(string s, int width) => s + new string(' ', Math.Max(0, width - Len(s)));

    public static string RJust(string s, int width) => new string(' ', Math.Max(0, width - Len(s))) + s;

    /// <summary>repr() of a float: the shortest text that reads back the same, Python style (1.0, 1e+16, 1e-05).</summary>
    public static string FloatRepr(double d)
    {
        if (double.IsNaN(d))
        {
            return "nan";
        }
        if (double.IsInfinity(d))
        {
            return d > 0 ? "inf" : "-inf";
        }
        if (d == 0)
        {
            return double.IsNegative(d) ? "-0.0" : "0.0";
        }
        var s = Math.Abs(d).ToString("R", CultureInfo.InvariantCulture);
        var exponent = 0;
        var e = s.IndexOfAny(['E', 'e']);
        if (e >= 0)
        {
            exponent = int.Parse(s[(e + 1)..], NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture);
            s = s[..e];
        }
        var dot = s.IndexOf('.');
        var whole = dot < 0 ? s : s[..dot];
        var digits = whole + (dot < 0 ? "" : s[(dot + 1)..]);
        var point = whole.Length + exponent;
        var lead = 0;
        while (lead < digits.Length - 1 && digits[lead] == '0')
        {
            lead++;
        }
        digits = digits[lead..].TrimEnd('0');
        point -= lead;
        if (digits.Length == 0)
        {
            digits = "0";
        }
        string text;
        if (point <= -4 || point > 16)
        {
            var power = point - 1;
            text = digits[..1] + (digits.Length > 1 ? "." + digits[1..] : "") + "e" + (power < 0 ? "-" : "+") +
                Math.Abs(power).ToString("00", CultureInfo.InvariantCulture);
        }
        else if (point <= 0)
        {
            text = "0." + new string('0', -point) + digits;
        }
        else if (point >= digits.Length)
        {
            text = digits + new string('0', point - digits.Length) + ".0";
        }
        else
        {
            text = digits[..point] + "." + digits[point..];
        }
        return d < 0 ? "-" + text : text;
    }

    /// <summary>os.path.normpath: separators, "." and ".." folded without asking the file system.</summary>
    public static string NormPath(string path)
    {
        if (path.Length == 0)
        {
            return ".";
        }
        string prefix;
        string rest;
        char sep;
        if (OperatingSystem.IsWindows())
        {
            sep = '\\';
            path = path.Replace('/', '\\');
            var drive = "";
            if (path.StartsWith(@"\\", StringComparison.Ordinal))
            {
                // \\server\share
                var server = path.IndexOf('\\', 2);
                var share = server < 0 ? -1 : path.IndexOf('\\', server + 1);
                drive = server < 0 ? path : share < 0 ? path : path[..share];
            }
            else if (path.Length >= 2 && path[1] == ':')
            {
                drive = path[..2];
            }
            rest = path[drive.Length..];
            prefix = drive + (rest.StartsWith('\\') ? "\\" : "");
        }
        else
        {
            sep = '/';
            var slashes = 0;
            while (slashes < path.Length && path[slashes] == '/')
            {
                slashes++;
            }
            prefix = slashes == 0 ? "" : slashes == 2 ? "//" : "/";
            rest = path;
        }
        var rooted = prefix.EndsWith(sep) || prefix == "//";
        var parts = new List<string>();
        foreach (var part in rest.Split(sep))
        {
            if (part is "" or ".")
            {
                continue;
            }
            if (part == ".." && (parts.Count > 0 ? parts[^1] != ".." : rooted))
            {
                if (parts.Count > 0)
                {
                    parts.RemoveAt(parts.Count - 1);
                }
                continue;
            }
            parts.Add(part);
        }
        var result = prefix + string.Join(sep, parts);
        return result.Length == 0 ? "." : result;
    }

    /// <summary>
    /// os.path.isabs (Python 3.13 and later): on Windows only a path with a drive and a root ("C:\x") or a UNC path
    /// is absolute; "\x" and "C:x" aren't.
    /// </summary>
    public static bool IsAbs(string path) => OperatingSystem.IsWindows() ? Path.IsPathFullyQualified(path) : path.StartsWith('/');

    /// <summary>os.path.join of two parts: on Windows "\x" keeps the drive of <paramref name="first"/>, "C:x" its folder on the same drive.</summary>
    public static string Join(string first, string second)
    {
        if (OperatingSystem.IsWindows() && Path.IsPathRooted(second) && !Path.IsPathFullyQualified(second))
        {
            var drive = WindowsDrive(first);
            if (second[0] is '\\' or '/')
            {
                return drive + second;
            }
            // "C:x": relative to the first part when it's on that drive, else as is.
            if (!drive.Equals(second[..2], StringComparison.OrdinalIgnoreCase))
            {
                return second;
            }
            var rest = second[2..];
            return first.Length > drive.Length && first[^1] is not ('\\' or '/') ? first + "\\" + rest : first + rest;
        }
        return Path.Join(first, second);
    }

    /// <summary>The drive ("C:") or UNC share ("\\server\share") a Windows path starts with, or "".</summary>
    static string WindowsDrive(string path)
    {
        if (path.Length >= 2 && path[1] == ':')
        {
            return path[..2];
        }
        var normalized = path.Replace('/', '\\');
        if (normalized.StartsWith(@"\\", StringComparison.Ordinal))
        {
            var server = normalized.IndexOf('\\', 2);
            var share = server < 0 ? -1 : normalized.IndexOf('\\', server + 1);
            return server < 0 ? path : share < 0 ? path : path[..share];
        }
        return "";
    }

    /// <summary>os.path.expanduser for "~" and "~/…".</summary>
    public static string ExpandUser(string path)
    {
        if (path == "~")
        {
            return Home;
        }
        if (path.StartsWith("~/", StringComparison.Ordinal) || (OperatingSystem.IsWindows() && path.StartsWith("~\\", StringComparison.Ordinal)))
        {
            // Like Python: the home folder, then the rest as written ("~/x" -> "<home>/x").
            return Home + path[1..];
        }
        return path;
    }

    static string Home => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
}
