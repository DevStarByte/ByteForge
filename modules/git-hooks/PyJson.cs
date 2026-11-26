using System.Globalization;
using System.Numerics;
using System.Text;

namespace ByteForge.GitHooks;

/// <summary>A JSON document that Python's json module couldn't read (json.JSONDecodeError, with its message).</summary>
public sealed class JsonDecodeError(string message) : Exception(message);

/// <summary>
/// Python's json.loads, so .husky/task-runner.json is read (and its values shown with str()) as the Python module
/// did: objects keep their key order (a repeated key keeps its first place and its last value), integers of any size
/// stay integers, floats are shown as repr() shows them, NaN and Infinity are read.
/// Values: null, bool, string, <see cref="BigInteger"/>, double, <see cref="List{T}"/> and
/// <see cref="OrderedDictionary{TKey, TValue}"/> (string keys).
/// </summary>
public static class PyJson
{
    // ───────────── loads ─────────────

    public static object? Loads(string text)
    {
        var parser = new Parser(text);
        var start = parser.SkipSpace(0);
        var (value, end) = parser.Value(start) ?? throw parser.Error("Expecting value", start);
        end = parser.SkipSpace(end);
        if (end != text.Length)
        {
            throw parser.Error("Extra data", end);
        }
        return value;
    }

    sealed class Parser(string s)
    {
        public int SkipSpace(int i)
        {
            while (i < s.Length && s[i] is ' ' or '\t' or '\n' or '\r')
            {
                i++;
            }
            return i;
        }

        public JsonDecodeError Error(string message, int pos)
        {
            var line = 1;
            var lastNewline = -1;
            for (var i = 0; i < pos && i < s.Length; i++)
            {
                if (s[i] == '\n')
                {
                    line++;
                    lastNewline = i;
                }
            }
            var column = lastNewline < 0 ? pos + 1 : pos - lastNewline;
            return new JsonDecodeError($"{message}: line {line} column {column} (char {pos})");
        }

        bool At(int i, string word) => i + word.Length <= s.Length && string.CompareOrdinal(s, i, word, 0, word.Length) == 0;

        /// <summary>The value at <paramref name="i"/> and where it ends; null when there's none (Python's StopIteration).</summary>
        public (object? Value, int End)? Value(int i)
        {
            if (i >= s.Length)
            {
                return null;
            }
            switch (s[i])
            {
                case '"':
                    return String(i + 1);
                case '{':
                    return Object(i + 1);
                case '[':
                    return Array(i + 1);
                case 'n' when At(i, "null"):
                    return (null, i + 4);
                case 't' when At(i, "true"):
                    return (true, i + 4);
                case 'f' when At(i, "false"):
                    return (false, i + 5);
                case 'N' when At(i, "NaN"):
                    return (double.NaN, i + 3);
                case 'I' when At(i, "Infinity"):
                    return (double.PositiveInfinity, i + 8);
                case '-' when At(i, "-Infinity"):
                    return (double.NegativeInfinity, i + 9);
            }
            return Number(i);
        }

        static bool Digit(char c) => c is >= '0' and <= '9';

        (object? Value, int End)? Number(int start)
        {
            // -?(?:0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?
            var i = start;
            if (i < s.Length && s[i] == '-')
            {
                i++;
            }
            if (i < s.Length && s[i] == '0')
            {
                i++;
            }
            else if (i < s.Length && s[i] is >= '1' and <= '9')
            {
                while (i < s.Length && Digit(s[i]))
                {
                    i++;
                }
            }
            else
            {
                return null;
            }
            var isFloat = false;
            if (i + 1 < s.Length && s[i] == '.' && Digit(s[i + 1]))
            {
                isFloat = true;
                i += 2;
                while (i < s.Length && Digit(s[i]))
                {
                    i++;
                }
            }
            if (i < s.Length && s[i] is 'e' or 'E')
            {
                var j = i + 1;
                if (j < s.Length && s[j] is '+' or '-')
                {
                    j++;
                }
                if (j < s.Length && Digit(s[j]))
                {
                    while (j < s.Length && Digit(s[j]))
                    {
                        j++;
                    }
                    isFloat = true;
                    i = j;
                }
            }
            var number = s[start..i];
            return isFloat
                ? (double.Parse(number, NumberStyles.Float, CultureInfo.InvariantCulture), i)
                : (BigInteger.Parse(number, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture), i);
        }

        (object? Value, int End) String(int begin)
        {
            var text = new StringBuilder();
            var i = begin;
            while (true)
            {
                if (i >= s.Length)
                {
                    throw Error("Unterminated string starting at", begin - 1);
                }
                var c = s[i];
                if (c == '"')
                {
                    return (text.ToString(), i + 1);
                }
                if (c < 0x20)
                {
                    throw Error("Invalid control character at", i);
                }
                if (c != '\\')
                {
                    text.Append(c);
                    i++;
                    continue;
                }
                i++;
                if (i >= s.Length)
                {
                    throw Error("Unterminated string starting at", begin - 1);
                }
                var escape = s[i];
                switch (escape)
                {
                    case '"': text.Append('"'); break;
                    case '\\': text.Append('\\'); break;
                    case '/': text.Append('/'); break;
                    case 'b': text.Append('\b'); break;
                    case 'f': text.Append('\f'); break;
                    case 'n': text.Append('\n'); break;
                    case 'r': text.Append('\r'); break;
                    case 't': text.Append('\t'); break;
                    case 'u':
                        // As Python's C scanner: four hex digits, and the string can't end right after them.
                        if (i + 5 >= s.Length || !ushort.TryParse(s.AsSpan(i + 1, 4), NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out var code))
                        {
                            throw Error("Invalid \\uXXXX escape", i);
                        }
                        // A surrogate pair written as two escapes ends up as the same two UTF-16 units.
                        text.Append((char)code);
                        i += 4;
                        break;
                    default:
                        throw Error("Invalid \\escape", i - 1);
                }
                i++;
            }
        }

        (object? Value, int End) Object(int i)
        {
            var result = new OrderedDictionary<string, object?>();
            i = SkipSpace(i);
            if (i < s.Length && s[i] == '}')
            {
                return (result, i + 1);
            }
            while (true)
            {
                if (i >= s.Length || s[i] != '"')
                {
                    throw Error("Expecting property name enclosed in double quotes", i);
                }
                var (key, end) = String(i + 1);
                i = SkipSpace(end);
                if (i >= s.Length || s[i] != ':')
                {
                    throw Error("Expecting ':' delimiter", i);
                }
                i = SkipSpace(i + 1);
                var (value, after) = Value(i) ?? throw Error("Expecting value", i);
                result[(string)key!] = value;
                i = SkipSpace(after);
                if (i < s.Length && s[i] == '}')
                {
                    return (result, i + 1);
                }
                if (i >= s.Length || s[i] != ',')
                {
                    throw Error("Expecting ',' delimiter", i);
                }
                i = SkipSpace(i + 1);
            }
        }

        (object? Value, int End) Array(int i)
        {
            var result = new List<object?>();
            i = SkipSpace(i);
            if (i < s.Length && s[i] == ']')
            {
                return (result, i + 1);
            }
            while (true)
            {
                var (value, after) = Value(i) ?? throw Error("Expecting value", i);
                result.Add(value);
                i = SkipSpace(after);
                if (i < s.Length && s[i] == ']')
                {
                    return (result, i + 1);
                }
                if (i >= s.Length || s[i] != ',')
                {
                    throw Error("Expecting ',' delimiter", i);
                }
                i = SkipSpace(i + 1);
            }
        }
    }

    /// <summary>A float as json.dumps writes it: repr(), or NaN / Infinity / -Infinity.</summary>
    public static string FloatText(double value)
    {
        if (double.IsNaN(value))
        {
            return "NaN";
        }
        if (double.IsInfinity(value))
        {
            return value > 0 ? "Infinity" : "-Infinity";
        }
        var sign = double.IsNegative(value) ? "-" : "";
        if (value == 0)
        {
            return sign + "0.0";
        }
        // The shortest digits that read back the same, and where the point goes: value = 0.digits × 10^point.
        var text = Math.Abs(value).ToString("R", CultureInfo.InvariantCulture);
        var exponent = 0;
        var e = text.IndexOfAny(['E', 'e']);
        if (e >= 0)
        {
            exponent = int.Parse(text[(e + 1)..], NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture);
            text = text[..e];
        }
        var dot = text.IndexOf('.');
        var integerPart = dot >= 0 ? text[..dot] : text;
        var digits = integerPart + (dot >= 0 ? text[(dot + 1)..] : "");
        var point = integerPart.Length + exponent;
        var lead = 0;
        while (lead < digits.Length - 1 && digits[lead] == '0')
        {
            lead++;
        }
        digits = digits[lead..].TrimEnd('0');
        point -= lead;
        if (point <= -4 || point > 16)
        {
            var mantissa = digits.Length == 1 ? digits : digits[..1] + "." + digits[1..];
            var power = point - 1;
            return sign + mantissa + "e" + (power < 0 ? "-" : "+") + Math.Abs(power).ToString("00", CultureInfo.InvariantCulture);
        }
        if (point <= 0)
        {
            return sign + "0." + new string('0', -point) + digits;
        }
        if (point >= digits.Length)
        {
            return sign + digits + new string('0', point - digits.Length) + ".0";
        }
        return sign + digits[..point] + "." + digits[point..];
    }
}
