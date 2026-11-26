using System.Globalization;
using System.Numerics;
using System.Text;

namespace ByteForge.Csv;

/// <summary>
/// Python's semantics where the module's output depends on them (the module was Python before): whitespace as
/// str.isspace sees it, lengths in code points, str.splitlines, os.path.splitext, float repr, "%.6g", sum().
/// </summary>
public static class Py
{
    /// <summary>str.isspace for one character (the BMP: every Python whitespace character is in it).</summary>
    public static bool IsSpace(char c) => c switch
    {
        >= '\t' and <= '\r' => true,
        >= '\x1c' and <= ' ' => true,
        '\x85' or '\xa0' or '\u1680' or '\u2028' or '\u2029' or '\u202f' or '\u205f' or '\u3000' => true,
        >= '\u2000' and <= '\u200a' => true,
        _ => false,
    };

    /// <summary>The same characters as a regex class (Python's \s on str).</summary>
    public const string SpaceClass = @"[\t-\r\x1c- \x85\xa0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]";

    public static string Strip(string s)
    {
        int start = 0, end = s.Length;
        while (start < end && IsSpace(s[start]))
        {
            start++;
        }
        while (end > start && IsSpace(s[end - 1]))
        {
            end--;
        }
        return s[start..end];
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

    public static bool IsBlank(string s) => s.All(IsSpace);

    /// <summary>len(): code points, not UTF-16 units.</summary>
    public static int Len(string s)
    {
        var n = s.Length;
        for (var i = 1; i < s.Length; i++)
        {
            if (char.IsLowSurrogate(s[i]) && char.IsHighSurrogate(s[i - 1]))
            {
                n--;
            }
        }
        return n;
    }

    /// <summary>s[:count] in code points.</summary>
    public static string Prefix(string s, int count)
    {
        var i = 0;
        for (var taken = 0; i < s.Length && taken < count; taken++)
        {
            i += char.IsHighSurrogate(s[i]) && i + 1 < s.Length && char.IsLowSurrogate(s[i + 1]) ? 2 : 1;
        }
        return s[..i];
    }

    public static string LJust(string s, int width) => s + new string(' ', Math.Max(0, width - Len(s)));

    public static string RJust(string s, int width) => new string(' ', Math.Max(0, width - Len(s))) + s;

    /// <summary>str.splitlines() (no line ends kept).</summary>
    public static List<string> SplitLines(string s)
    {
        var lines = new List<string>();
        var start = 0;
        for (var i = 0; i < s.Length; i++)
        {
            var c = s[i];
            if (c is '\n' or '\r' or '\v' or '\f' or '\x1c' or '\x1d' or '\x1e' or '\x85' or '\u2028' or '\u2029')
            {
                lines.Add(s[start..i]);
                if (c == '\r' && i + 1 < s.Length && s[i + 1] == '\n')
                {
                    i++;
                }
                start = i + 1;
            }
        }
        if (start < s.Length)
        {
            lines.Add(s[start..]);
        }
        return lines;
    }

    static string Separators => OperatingSystem.IsWindows() ? "\\/" : "/";

    /// <summary>os.path.splitext: (root, ext); leading dots of the name don't start an extension.</summary>
    public static (string Root, string Ext) SplitExt(string path)
    {
        var sep = path.LastIndexOfAny(Separators.ToCharArray());
        var dot = path.LastIndexOf('.');
        if (dot > sep)
        {
            for (var i = sep + 1; i < dot; i++)
            {
                if (path[i] != '.')
                {
                    return (path[..dot], path[dot..]);
                }
            }
        }
        return (path, "");
    }

    /// <summary>os.path.basename.</summary>
    public static string Basename(string path) => path[(path.LastIndexOfAny(Separators.ToCharArray()) + 1)..];

    /// <summary>Unicode decimal digits as ASCII (what Python's int() and float() accept).</summary>
    public static string AsciiDigits(string s)
    {
        if (s.All(char.IsAscii))
        {
            return s;
        }
        var b = new StringBuilder(s.Length);
        foreach (var c in s)
        {
            b.Append(char.IsDigit(c) ? (char)('0' + (int)char.GetNumericValue(c)) : c);
        }
        return b.ToString();
    }

    /// <summary>
    /// Decimal digits outside the BMP (U+1D7CE…) as a BMP digit of the same value (Arabic-Indic), one character
    /// for one code point: Python's \d, isdigit() and float() take them, .NET's regex sees two surrogates.
    /// </summary>
    public static string BmpDigits(string s)
    {
        if (!s.Any(char.IsSurrogate))
        {
            return s;
        }
        var b = new StringBuilder(s.Length);
        foreach (var rune in s.EnumerateRunes())
        {
            if (rune.IsBmp || Rune.GetUnicodeCategory(rune) != System.Globalization.UnicodeCategory.DecimalDigitNumber)
            {
                b.Append(rune.ToString());
            }
            else
            {
                b.Append((char)(0x0660 + (int)Rune.GetNumericValue(rune)));
            }
        }
        return b.ToString();
    }

    /// <summary>float(s) for the number shapes the module produces; null where Python raises ValueError.</summary>
    public static double? Float(string s)
    {
        // float() trims ASCII whitespace (not 0x1C-0x1F) and the Unicode spaces.
        static bool Trimmed(char c) => c < 128 ? c is ' ' or (>= (char)9 and <= (char)13) : IsSpace(c);
        var start = 0;
        var end = s.Length;
        while (start < end && Trimmed(s[start]))
        {
            start++;
        }
        while (end > start && Trimmed(s[end - 1]))
        {
            end--;
        }
        var ascii = AsciiDigits(s[start..end]);
        if (ascii.Length == 0 || ascii.Any(c => !(char.IsAsciiDigit(c) || c is '+' or '-' or '.' or 'e' or 'E')))
        {
            return null;
        }
        return double.TryParse(ascii, NumberStyles.Float, CultureInfo.InvariantCulture, out var value) ? value : null;
    }

    /// <summary>int(x) for a float: truncated, exact (Python's ints have no size limit).</summary>
    public static BigInteger Int(double x)
    {
        if (double.IsNaN(x))
        {
            throw new PyException("ValueError", "cannot convert float NaN to integer");
        }
        if (double.IsInfinity(x))
        {
            throw new PyException("OverflowError", "cannot convert float infinity to integer");
        }
        return new BigInteger(Math.Truncate(x));
    }

    /// <summary>sum() of floats: Neumaier's compensated sum, as CPython 3.12+ does it.</summary>
    public static double Sum(IEnumerable<double> values)
    {
        double total = 0, c = 0;
        foreach (var x in values)
        {
            var t = total + x;
            if (Math.Abs(total) >= Math.Abs(x))
            {
                c += (total - t) + x;
            }
            else
            {
                c += (x - t) + total;
            }
            total = t;
        }
        if (c != 0 && double.IsFinite(c))
        {
            total += c;
        }
        return total;
    }

    /// <summary>
    /// The iteration order of set(values) for non-negative ints (hash(n) == n): CPython's open-addressing table
    /// (8 slots, 9 linear probes then perturbed jumps, grown to the next power of two above used×4 when 3/5 full).
    /// </summary>
    public static List<int> SetOrder(IEnumerable<int> values)
    {
        const int LinearProbes = 9, PerturbShift = 5;
        var table = new int?[8];
        int fill = 0, used = 0;

        static void InsertClean(int?[] table, int key)
        {
            var mask = (ulong)table.Length - 1;
            var perturb = (ulong)key;
            var i = (ulong)key & mask;
            while (true)
            {
                if (table[i] == null)
                {
                    table[i] = key;
                    return;
                }
                if (i + LinearProbes <= mask)
                {
                    for (ulong j = 1; j <= LinearProbes; j++)
                    {
                        if (table[i + j] == null)
                        {
                            table[i + j] = key;
                            return;
                        }
                    }
                }
                perturb >>= PerturbShift;
                i = (i * 5 + 1 + perturb) & mask;
            }
        }

        foreach (var key in values)
        {
            var mask = (ulong)table.Length - 1;
            var perturb = (ulong)key;
            var i = (ulong)key & mask;
            var found = false;
            while (!found)
            {
                var probes = i + LinearProbes <= mask ? LinearProbes : 0;
                for (ulong j = 0; j <= (ulong)probes; j++)
                {
                    if (table[i + j] == null)
                    {
                        table[i + j] = key;
                        fill++;
                        used++;
                        if ((ulong)fill * 5 >= mask * 3)
                        {
                            var size = 8;
                            while (size <= used * 4)
                            {
                                size <<= 1;
                            }
                            var old = table;
                            table = new int?[size];
                            foreach (var k in old)
                            {
                                if (k is { } existing)
                                {
                                    InsertClean(table, existing);
                                }
                            }
                            fill = used;
                        }
                        found = true;
                        break;
                    }
                    if (table[i + j] == key)
                    {
                        found = true;
                        break;
                    }
                }
                if (!found)
                {
                    perturb >>= PerturbShift;
                    i = (i * 5 + 1 + perturb) & mask;
                }
            }
        }
        return [.. table.OfType<int>()];
    }

    /// <summary>The digits (no leading/trailing zeros) and decimal exponent of a finite, non-zero |x|.</summary>
    static (string Digits, int Exponent) Decompose(string formatted)
    {
        // formatted: [-]d[.ddd][E[+-]ddd] or [-]ddd.ddd
        var s = formatted.TrimStart('-');
        var exponent = 0;
        var e = s.IndexOfAny(['E', 'e']);
        if (e >= 0)
        {
            exponent = int.Parse(s[(e + 1)..], NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture);
            s = s[..e];
        }
        var point = s.IndexOf('.');
        var intDigits = point < 0 ? s.Length : point;
        var digits = s.Replace(".", "");
        var lead = 0;
        while (lead < digits.Length - 1 && digits[lead] == '0')
        {
            lead++;
        }
        digits = digits[lead..];
        // value = 0.digits × 10^(intDigits - lead + exponent) → scientific exponent one less
        var sci = intDigits - lead + exponent - 1;
        digits = digits.TrimEnd('0');
        return (digits.Length == 0 ? "0" : digits, sci);
    }

    static string Scientific(string digits, int exponent, bool negative)
    {
        var mantissa = digits.Length > 1 ? digits[0] + "." + digits[1..] : digits;
        var sign = exponent < 0 ? "-" : "+";
        return (negative ? "-" : "") + mantissa + "e" + sign + Math.Abs(exponent).ToString("00", CultureInfo.InvariantCulture);
    }

    static string Fixed(string digits, int exponent, bool negative, bool addDotZero)
    {
        string text;
        if (exponent < 0)
        {
            text = "0." + new string('0', -exponent - 1) + digits;
        }
        else if (digits.Length <= exponent + 1)
        {
            text = digits + new string('0', exponent + 1 - digits.Length) + (addDotZero ? ".0" : "");
        }
        else
        {
            text = digits[..(exponent + 1)] + "." + digits[(exponent + 1)..];
        }
        return (negative ? "-" : "") + text;
    }

    /// <summary>repr(float) as json.dumps writes it (Infinity, NaN; 1e+16, 1.5, 2.0).</summary>
    public static string Repr(double x)
    {
        if (double.IsNaN(x))
        {
            return "NaN";
        }
        if (double.IsInfinity(x))
        {
            return x > 0 ? "Infinity" : "-Infinity";
        }
        var negative = double.IsNegative(x);
        if (x == 0)
        {
            return negative ? "-0.0" : "0.0";
        }
        var (digits, exponent) = Decompose(x.ToString("R", CultureInfo.InvariantCulture));
        return exponent < -4 || exponent >= 16 ? Scientific(digits, exponent, negative) : Fixed(digits, exponent, negative, addDotZero: true);
    }

    /// <summary>f"{x:.6g}".</summary>
    public static string G6(double x)
    {
        if (double.IsNaN(x))
        {
            return "nan";
        }
        if (double.IsInfinity(x))
        {
            return x > 0 ? "inf" : "-inf";
        }
        var negative = double.IsNegative(x);
        if (x == 0)
        {
            return negative ? "-0" : "0";
        }
        var (digits, exponent) = Decompose(x.ToString("E5", CultureInfo.InvariantCulture));
        return exponent < -4 || exponent >= 6 ? Scientific(digits, exponent, negative) : Fixed(digits, exponent, negative, addDotZero: false);
    }
}

/// <summary>An error Python would have raised, with its type name (the module logs "csv: Type: message").</summary>
public class PyException(string type, string message) : Exception(message)
{
    public string Type { get; } = type;
}

/// <summary>csv.Error.</summary>
public sealed class CsvError(string message) : PyException("Error", message);
