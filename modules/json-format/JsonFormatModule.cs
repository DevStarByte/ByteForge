using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;
using ByteForge.Modules;

namespace ByteForge.JsonFormat;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<JsonFormatModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// JSON formatter: Format Document on .json files. Indentation follows the file (tabs or the width of its first
/// indented line, default 2), key order is kept and the trailing newline too. JSON with comments (JSONC, e.g. some
/// tsconfig/launch files) is left alone rather than losing its comments.
/// </summary>
public sealed class JsonFormatModule : Module
{
    protected override Task OnStartAsync()
    {
        RegisterFormatter("json-format", "JSON Formatter", "json");
        return Task.CompletedTask;
    }

    protected override Task<string> FormatAsync(string? path, string text)
    {
        try
        {
            return Task.FromResult(JsonFormatter.Format(text));
        }
        catch (JsonFormatException e)
        {
            throw new ModuleException($"JSON: {e.Message}");
        }
    }
}

/// <summary>Why a text couldn't be formatted (the message is Python's json module's, as the module used to say).</summary>
public sealed class JsonFormatException(string message) : Exception(message);

/// <summary>
/// The formatting, byte for byte as Python's <c>json.dumps(json.loads(text), indent=…, ensure_ascii=False)</c>, which
/// the module used before: numbers as Python writes them (1.0, 1e+16, Infinity), a repeated key keeps its first
/// place and its last value, and the error messages are Python's.
/// </summary>
public static partial class JsonFormatter
{
    /// <summary>
    /// Deeper nesting is refused with an error (Python, with its stack-based limit, read a few thousand levels and
    /// stopped the module with a RecursionError somewhere below 100 000). Reading and writing are recursive, so
    /// they run on a thread of their own with <see cref="StackSize"/>: a stack overflow would end the module host.
    /// </summary>
    public const int MaxDepth = 10_000;

    const int StackSize = 64 * 1024 * 1024;

    /// <summary>Python's sys.get_int_max_str_digits() default: longer integers aren't read.</summary>
    const int MaxIntDigits = 4300;

    [GeneratedRegex("\"(?:\\\\.|[^\"\\\\])*\"")]
    private static partial Regex StringLiteral();

    /// <summary>Whether the text has // or /* outside strings ("http://…" in a string isn't a comment).</summary>
    public static bool HasComments(string text)
    {
        var withoutStrings = StringLiteral().Replace(text, "\"\"");
        return withoutStrings.Contains("//", StringComparison.Ordinal) || withoutStrings.Contains("/*", StringComparison.Ordinal);
    }

    /// <summary>The file's indentation: a tab, or the spaces of the first indented line after the first; else two spaces.</summary>
    public static string DetectIndent(string text)
    {
        var first = true;
        foreach (var line in SplitLines(text))
        {
            if (first)
            {
                first = false;
                continue;
            }
            if (line.StartsWith('\t'))
            {
                return "\t";
            }
            var spaces = 0;
            while (spaces < line.Length && line[spaces] == ' ')
            {
                spaces++;
            }
            if (spaces > 0)
            {
                return new string(' ', spaces);
            }
        }
        return "  ";
    }

    /// <summary>Python's str.splitlines(): every line break Python knows, without the breaks.</summary>
    static IEnumerable<string> SplitLines(string text)
    {
        var start = 0;
        for (var i = 0; i < text.Length; i++)
        {
            var c = text[i];
            if (c is '\n' or '\r' or '\v' or '\f' or '\x1c' or '\x1d' or '\x1e' or '\x85' or '\u2028' or '\u2029')
            {
                yield return text[start..i];
                if (c == '\r' && i + 1 < text.Length && text[i + 1] == '\n')
                {
                    i++;
                }
                start = i + 1;
            }
        }
        if (start < text.Length)
        {
            yield return text[start..];
        }
    }

    /// <summary>The text formatted; <see cref="JsonFormatException"/> when it has comments or isn't JSON.</summary>
    public static string Format(string text)
    {
        if (HasComments(text))
        {
            throw new JsonFormatException("the file has comments (JSONC); left unchanged");
        }
        string? result = null;
        Exception? failure = null;
        var worker = new Thread(() =>
        {
            try
            {
                var value = new Parser(text).Document();
                var output = new StringBuilder(text.Length + text.Length / 4 + 16);
                Write(output, value, DetectIndent(text), 0);
                if (text.EndsWith('\n') || text.Length == 0)
                {
                    output.Append('\n');
                }
                result = output.ToString();
            }
            catch (Exception e)
            {
                failure = e;
            }
        }, StackSize) { IsBackground = true, Name = "JSON formatter" };
        worker.Start();
        worker.Join();
        if (failure != null)
        {
            System.Runtime.ExceptionServices.ExceptionDispatchInfo.Throw(failure);
        }
        return result!;
    }

    // ───────────── values ─────────────

    abstract class Value;

    /// <summary>A number, true, false or null: already as Python writes it.</summary>
    sealed class Scalar(string text) : Value
    {
        public string Text { get; } = text;
    }

    sealed class Text(string value) : Value
    {
        public string Value { get; } = value;
    }

    sealed class Array : Value
    {
        public List<Value> Items { get; } = [];
    }

    sealed class Object : Value
    {
        public List<string> Keys { get; } = [];
        public List<Value> Values { get; } = [];
        readonly Dictionary<string, int> index = new(StringComparer.Ordinal);

        /// <summary>As a Python dict: a repeated key keeps its first place and takes the new value.</summary>
        public void Set(string key, Value value)
        {
            if (index.TryGetValue(key, out var at))
            {
                Values[at] = value;
                return;
            }
            index[key] = Keys.Count;
            Keys.Add(key);
            Values.Add(value);
        }
    }

    // ───────────── writing (json.dumps with indent, ensure_ascii=False) ─────────────

    static void Write(StringBuilder output, Value value, string indent, int level)
    {
        switch (value)
        {
            case Scalar scalar:
                output.Append(scalar.Text);
                break;
            case Text text:
                WriteString(output, text.Value);
                break;
            case Array array when array.Items.Count == 0:
                output.Append("[]");
                break;
            case Array array:
                output.Append('[');
                for (var i = 0; i < array.Items.Count; i++)
                {
                    if (i > 0)
                    {
                        output.Append(',');
                    }
                    NewLine(output, indent, level + 1);
                    Write(output, array.Items[i], indent, level + 1);
                }
                NewLine(output, indent, level);
                output.Append(']');
                break;
            case Object obj when obj.Keys.Count == 0:
                output.Append("{}");
                break;
            case Object obj:
                output.Append('{');
                for (var i = 0; i < obj.Keys.Count; i++)
                {
                    if (i > 0)
                    {
                        output.Append(',');
                    }
                    NewLine(output, indent, level + 1);
                    WriteString(output, obj.Keys[i]);
                    output.Append(": ");
                    Write(output, obj.Values[i], indent, level + 1);
                }
                NewLine(output, indent, level);
                output.Append('}');
                break;
        }
    }

    static void NewLine(StringBuilder output, string indent, int level)
    {
        output.Append('\n');
        for (var i = 0; i < level; i++)
        {
            output.Append(indent);
        }
    }

    /// <summary>A string as Python's encoder writes it with ensure_ascii=False: only quotes, backslashes and control characters escaped.</summary>
    static void WriteString(StringBuilder output, string value)
    {
        output.Append('"');
        for (var i = 0; i < value.Length; i++)
        {
            var c = value[i];
            switch (c)
            {
                case '"':
                    output.Append("\\\"");
                    break;
                case '\\':
                    output.Append("\\\\");
                    break;
                case '\n':
                    output.Append("\\n");
                    break;
                case '\r':
                    output.Append("\\r");
                    break;
                case '\t':
                    output.Append("\\t");
                    break;
                case '\b':
                    output.Append("\\b");
                    break;
                case '\f':
                    output.Append("\\f");
                    break;
                default:
                    if (c < 0x20)
                    {
                        output.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                    }
                    else if (char.IsHighSurrogate(c) && i + 1 < value.Length && char.IsLowSurrogate(value[i + 1]))
                    {
                        output.Append(c).Append(value[++i]);
                    }
                    else if (char.IsSurrogate(c))
                    {
                        // A lone surrogate ("\ud800" in the file) can't be sent as text: it stays an escape.
                        output.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                    }
                    else
                    {
                        output.Append(c);
                    }
                    break;
            }
        }
        output.Append('"');
    }

    /// <summary>repr() of a float, as Python's json writes it: the shortest text that reads back the same.</summary>
    public static string FloatRepr(double d)
    {
        if (double.IsNaN(d))
        {
            return "NaN";
        }
        if (double.IsInfinity(d))
        {
            return d > 0 ? "Infinity" : "-Infinity";
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

    // ───────────── reading (json.loads) ─────────────

    /// <summary>Python's JSON decoder (the C scanner), with its error messages and positions.</summary>
    sealed class Parser(string s)
    {
        int depth;

        static bool IsWhitespace(char c) => c is ' ' or '\t' or '\n' or '\r';

        int SkipWhitespace(int i)
        {
            while (i < s.Length && IsWhitespace(s[i]))
            {
                i++;
            }
            return i;
        }

        /// <summary>JSONDecodeError's text: "msg: line L column C (char P)", counted in code points as Python does.</summary>
        JsonFormatException Error(string message, int index)
        {
            int Points(int end)
            {
                var n = end;
                for (var i = 1; i < end && i < s.Length; i++)
                {
                    if (char.IsLowSurrogate(s[i]) && char.IsHighSurrogate(s[i - 1]))
                    {
                        n--;
                    }
                }
                return n;
            }
            index = Math.Min(index, s.Length);
            var pos = Points(index);
            var line = 1;
            var lastNewline = -1;
            for (var i = 0; i < index; i++)
            {
                if (s[i] == '\n')
                {
                    line++;
                    lastNewline = i;
                }
            }
            var column = lastNewline < 0 ? pos + 1 : pos - Points(lastNewline);
            return new JsonFormatException(string.Create(CultureInfo.InvariantCulture, $"{message}: line {line} column {column} (char {pos})"));
        }

        /// <summary>No value here: Python's StopIteration, which becomes "Expecting value".</summary>
        sealed class NoValue(int index) : Exception
        {
            public int Index { get; } = index;
        }

        public Value Document()
        {
            if (s.StartsWith('\uFEFF'))
            {
                throw Error("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0);
            }
            Value value;
            int end;
            try
            {
                (value, end) = ScanOnce(SkipWhitespace(0));
            }
            catch (NoValue none)
            {
                throw Error("Expecting value", none.Index);
            }
            end = SkipWhitespace(end);
            if (end != s.Length)
            {
                throw Error("Extra data", end);
            }
            return value;
        }

        bool At(int i, string literal) => string.CompareOrdinal(s, i, literal, 0, literal.Length) == 0 && i + literal.Length <= s.Length;

        (Value, int) ScanOnce(int i)
        {
            if (i >= s.Length)
            {
                throw new NoValue(i);
            }
            switch (s[i])
            {
                case '"':
                    var (text, end) = ScanString(i + 1);
                    return (new Text(text), end);
                case '{':
                    return Nested(() => ParseObject(i + 1));
                case '[':
                    return Nested(() => ParseArray(i + 1));
                case 'n' when At(i, "null"):
                    return (new Scalar("null"), i + 4);
                case 't' when At(i, "true"):
                    return (new Scalar("true"), i + 4);
                case 'f' when At(i, "false"):
                    return (new Scalar("false"), i + 5);
                case 'N' when At(i, "NaN"):
                    return (new Scalar("NaN"), i + 3);
                case 'I' when At(i, "Infinity"):
                    return (new Scalar("Infinity"), i + 8);
                case '-' when At(i, "-Infinity"):
                    return (new Scalar("-Infinity"), i + 9);
            }
            return MatchNumber(i);
        }

        (Value, int) Nested(Func<(Value, int)> parse)
        {
            if (++depth > MaxDepth)
            {
                throw new JsonFormatException("maximum recursion depth exceeded while decoding a JSON document");
            }
            try
            {
                return parse();
            }
            finally
            {
                depth--;
            }
        }

        (Value, int) MatchNumber(int start)
        {
            var i = start;
            var end = s.Length - 1;
            if (s[i] == '-')
            {
                i++;
                if (i > end)
                {
                    throw new NoValue(start);
                }
            }
            if (s[i] is >= '1' and <= '9')
            {
                i++;
                while (i <= end && char.IsAsciiDigit(s[i]))
                {
                    i++;
                }
            }
            else if (s[i] == '0')
            {
                i++;
            }
            else
            {
                throw new NoValue(start);
            }
            var isFloat = false;
            if (i < end && s[i] == '.' && char.IsAsciiDigit(s[i + 1]))
            {
                isFloat = true;
                i += 2;
                while (i <= end && char.IsAsciiDigit(s[i]))
                {
                    i++;
                }
            }
            if (i < end && (s[i] == 'e' || s[i] == 'E'))
            {
                var exponentStart = i;
                i++;
                if (i < end && (s[i] == '-' || s[i] == '+'))
                {
                    i++;
                }
                while (i <= end && char.IsAsciiDigit(s[i]))
                {
                    i++;
                }
                if (char.IsAsciiDigit(s[i - 1]))
                {
                    isFloat = true;
                }
                else
                {
                    i = exponentStart;
                }
            }
            var number = s[start..i];
            if (isFloat)
            {
                return (new Scalar(FloatRepr(double.Parse(number, NumberStyles.Float, CultureInfo.InvariantCulture))), i);
            }
            var digits = number.TrimStart('-');
            if (digits.Length > MaxIntDigits)
            {
                throw new JsonFormatException(string.Create(CultureInfo.InvariantCulture,
                    $"Exceeds the limit ({MaxIntDigits} digits) for integer string conversion: value has {digits.Length} digits; use sys.set_int_max_str_digits() to increase the limit"));
            }
            // int("-0") is 0.
            return (new Scalar(number == "-0" ? "0" : number), i);
        }

        (string, int) ScanString(int end)
        {
            var begin = end - 1;
            var builder = new StringBuilder();
            while (true)
            {
                var next = end;
                var c = '\0';
                for (; next < s.Length; next++)
                {
                    c = s[next];
                    if (c is '"' or '\\')
                    {
                        break;
                    }
                    if (c <= 0x1f)
                    {
                        throw Error("Invalid control character at", next);
                    }
                }
                if (next >= s.Length || (c != '"' && c != '\\'))
                {
                    throw Error("Unterminated string starting at", begin);
                }
                builder.Append(s, end, next - end);
                next++;
                if (c == '"')
                {
                    return (builder.ToString(), next);
                }
                if (next == s.Length)
                {
                    throw Error("Unterminated string starting at", begin);
                }
                c = s[next];
                if (c != 'u')
                {
                    end = next + 1;
                    var escaped = c switch
                    {
                        '"' => '"',
                        '\\' => '\\',
                        '/' => '/',
                        'b' => '\b',
                        'f' => '\f',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        _ => '\0',
                    };
                    if (escaped == '\0')
                    {
                        throw Error("Invalid \\escape", end - 2);
                    }
                    builder.Append(escaped);
                    continue;
                }
                next++;
                end = next + 4;
                if (end >= s.Length)
                {
                    throw Error("Invalid \\uXXXX escape", next - 1);
                }
                var code = Hex4(next, end);
                next = end;
                // A high surrogate followed by \u and a low one: one character.
                if (code is >= 0xD800 and <= 0xDBFF && end + 6 < s.Length && s[next] == '\\' && s[next + 1] == 'u')
                {
                    var low = Hex4(next + 2, end + 6);
                    if (low is >= 0xDC00 and <= 0xDFFF)
                    {
                        builder.Append((char)code).Append((char)low);
                        end += 6;
                        continue;
                    }
                }
                builder.Append((char)code);
            }
        }

        int Hex4(int from, int end)
        {
            var value = 0;
            for (var i = from; i < end; i++)
            {
                var digit = s[i];
                value <<= 4;
                value |= digit switch
                {
                    >= '0' and <= '9' => digit - '0',
                    >= 'a' and <= 'f' => digit - 'a' + 10,
                    >= 'A' and <= 'F' => digit - 'A' + 10,
                    _ => throw Error("Invalid \\uXXXX escape", end - 5),
                };
            }
            return value;
        }

        (Value, int) ParseObject(int i)
        {
            var obj = new Object();
            var end = s.Length - 1;
            i = SkipWhitespace(i);
            if (i > end || s[i] != '}')
            {
                while (true)
                {
                    if (i > end || s[i] != '"')
                    {
                        throw Error("Expecting property name enclosed in double quotes", i);
                    }
                    var (key, next) = ScanString(i + 1);
                    i = SkipWhitespace(next);
                    if (i > end || s[i] != ':')
                    {
                        throw Error("Expecting ':' delimiter", i);
                    }
                    i = SkipWhitespace(i + 1);
                    var (value, after) = ScanValue(i);
                    obj.Set(key, value);
                    i = SkipWhitespace(after);
                    if (i <= end && s[i] == '}')
                    {
                        break;
                    }
                    if (i > end || s[i] != ',')
                    {
                        throw Error("Expecting ',' delimiter", i);
                    }
                    var comma = i;
                    i = SkipWhitespace(i + 1);
                    if (i <= end && s[i] == '}')
                    {
                        throw Error("Illegal trailing comma before end of object", comma);
                    }
                }
            }
            return (obj, i + 1);
        }

        (Value, int) ParseArray(int i)
        {
            var array = new Array();
            var end = s.Length - 1;
            i = SkipWhitespace(i);
            if (i > end || s[i] != ']')
            {
                while (true)
                {
                    var (value, after) = ScanValue(i);
                    array.Items.Add(value);
                    i = SkipWhitespace(after);
                    if (i <= end && s[i] == ']')
                    {
                        break;
                    }
                    if (i > end || s[i] != ',')
                    {
                        throw Error("Expecting ',' delimiter", i);
                    }
                    var comma = i;
                    i = SkipWhitespace(i + 1);
                    if (i <= end && s[i] == ']')
                    {
                        throw Error("Illegal trailing comma before end of array", comma);
                    }
                }
            }
            return (array, i + 1);
        }

        /// <summary>A value inside an object or array; none there is "Expecting value" right away.</summary>
        (Value, int) ScanValue(int i)
        {
            try
            {
                return ScanOnce(i);
            }
            catch (NoValue none)
            {
                throw Error("Expecting value", none.Index);
            }
        }
    }
}
