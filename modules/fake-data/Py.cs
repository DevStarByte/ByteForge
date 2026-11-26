using System.Globalization;
using System.Numerics;
using System.Text;

namespace ByteForge.FakeData;

/// <summary>Numbers and strings written exactly as Python writes them (repr, json.dumps), so the output matches the old module.</summary>
public static class Py
{
    /// <summary>Python's repr(float): the shortest digits that read back the same, "1.0", "1e-05", "1e+16".</summary>
    public static string Repr(double value)
    {
        if (double.IsNaN(value))
        {
            return "nan";
        }
        if (double.IsInfinity(value))
        {
            return value > 0 ? "inf" : "-inf";
        }
        var (negative, digits, point) = Shortest(value);
        var sign = negative ? "-" : "";
        if (point <= -4 || point > 16)
        {
            var mantissa = digits.Length == 1 ? digits : digits[..1] + "." + digits[1..];
            return sign + mantissa + Exponent(point - 1);
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

    /// <summary>A float as json.dumps writes it (repr, Infinity, NaN).</summary>
    public static string JsonNumber(double value) =>
        double.IsNaN(value) ? "NaN" : double.IsPositiveInfinity(value) ? "Infinity" : double.IsNegativeInfinity(value) ? "-Infinity" : Repr(value);

    static string Exponent(int exponent) => "e" + (exponent < 0 ? "-" : "+") + Math.Abs(exponent).ToString("00", CultureInfo.InvariantCulture);

    /// <summary>The shortest round-trip digits (no leading or trailing zeros) and where the point goes: value = 0.digits × 10^point.</summary>
    static (bool Negative, string Digits, int Point) Shortest(double value)
    {
        var negative = double.IsNegative(value);
        if (value == 0)
        {
            return (negative, "0", 1);
        }
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
        var fraction = dot >= 0 ? text[(dot + 1)..] : "";
        var all = integerPart + fraction;
        var point = integerPart.Length + exponent;
        var lead = 0;
        while (lead < all.Length - 1 && all[lead] == '0')
        {
            lead++;
        }
        all = all[lead..];
        point -= lead;
        all = all.TrimEnd('0');
        return (negative, all.Length == 0 ? "0" : all, point);
    }

    /// <summary>Python's format(value, ".{precision}g"): rounded exactly (half to even), trailing zeros dropped.</summary>
    public static string FormatG(double value, int precision = 6)
    {
        if (double.IsNaN(value))
        {
            return "nan";
        }
        if (double.IsInfinity(value))
        {
            return value > 0 ? "inf" : "-inf";
        }
        var sign = double.IsNegative(value) ? "-" : "";
        if (value == 0)
        {
            return sign + "0";
        }
        var (digits, point) = Rounded(Math.Abs(value), precision);
        var exponent = point - 1;
        if (exponent < -4 || exponent >= precision)
        {
            var mantissa = digits.Length == 1 ? digits : digits[..1] + "." + digits[1..];
            return sign + mantissa + Exponent(exponent);
        }
        if (point <= 0)
        {
            return sign + "0." + new string('0', -point) + digits;
        }
        if (point >= digits.Length)
        {
            return sign + digits + new string('0', point - digits.Length);
        }
        return sign + digits[..point] + "." + digits[point..];
    }

    /// <summary>The exact decimal value of a positive double rounded half-even to <paramref name="precision"/> digits, trailing zeros removed.</summary>
    static (string Digits, int Point) Rounded(double value, int precision)
    {
        var bits = BitConverter.DoubleToInt64Bits(value);
        var exponentBits = (int)((bits >> 52) & 0x7FF);
        var mantissa = bits & 0xFFFFFFFFFFFFFL;
        int binaryExponent;
        if (exponentBits == 0)
        {
            binaryExponent = -1074;
        }
        else
        {
            mantissa |= 1L << 52;
            binaryExponent = exponentBits - 1075;
        }
        // value = n × 10^scale exactly.
        BigInteger n;
        int scale;
        if (binaryExponent >= 0)
        {
            n = new BigInteger(mantissa) << binaryExponent;
            scale = 0;
        }
        else
        {
            n = new BigInteger(mantissa) * BigInteger.Pow(5, -binaryExponent);
            scale = binaryExponent;
        }
        var digits = n.ToString(CultureInfo.InvariantCulture);
        var point = digits.Length + scale;
        if (digits.Length > precision)
        {
            var kept = BigInteger.Parse(digits[..precision], CultureInfo.InvariantCulture);
            var rest = digits[precision..];
            var up = rest[0] > '5' || (rest[0] == '5' && (rest.AsSpan(1).IndexOfAnyExcept('0') >= 0 || !kept.IsEven));
            if (up)
            {
                kept += 1;
            }
            digits = kept.ToString(CultureInfo.InvariantCulture);
            if (digits.Length > precision)
            {
                digits = digits[..precision];
                point++;
            }
        }
        digits = digits.TrimEnd('0');
        return (digits.Length == 0 ? "0" : digits, point);
    }

    /// <summary>
    /// Python's round(value, digits) for a float: the exact binary value rounded half to even at that decimal,
    /// then the nearest float to that (Math.Round scales by 10^digits first and can land on the other side).
    /// </summary>
    public static double Round(double value, int digits)
    {
        if (!double.IsFinite(value) || value == 0)
        {
            return value;
        }
        var bits = BitConverter.DoubleToInt64Bits(value);
        var exponentBits = (int)((bits >> 52) & 0x7FF);
        var mantissa = bits & 0xFFFFFFFFFFFFFL;
        int binaryExponent;
        if (exponentBits == 0)
        {
            binaryExponent = -1074;
        }
        else
        {
            mantissa |= 1L << 52;
            binaryExponent = exponentBits - 1075;
        }
        // |value| x 10^digits = numerator / denominator, exactly.
        var numerator = new BigInteger(mantissa) * BigInteger.Pow(10, digits);
        var denominator = BigInteger.One;
        if (binaryExponent >= 0)
        {
            numerator <<= binaryExponent;
        }
        else
        {
            denominator <<= -binaryExponent;
        }
        var quotient = BigInteger.DivRem(numerator, denominator, out var remainder);
        var twice = remainder * 2;
        if (twice > denominator || (twice == denominator && !quotient.IsEven))
        {
            quotient += 1;
        }
        var text = quotient.ToString(CultureInfo.InvariantCulture).PadLeft(digits + 1, '0');
        var rounded = double.Parse(text[..^digits] + "." + text[^digits..], CultureInfo.InvariantCulture);
        return value < 0 ? -rounded : rounded;
    }

    /// <summary>A JSON string as json.dumps(..., ensure_ascii=False) writes it.</summary>
    public static string JsonString(string text)
    {
        var json = new StringBuilder(text.Length + 2).Append('"');
        foreach (var c in text)
        {
            switch (c)
            {
                case '"': json.Append("\\\""); break;
                case '\\': json.Append("\\\\"); break;
                case '\n': json.Append("\\n"); break;
                case '\r': json.Append("\\r"); break;
                case '\t': json.Append("\\t"); break;
                case '\b': json.Append("\\b"); break;
                case '\f': json.Append("\\f"); break;
                default:
                    if (c < 0x20)
                    {
                        json.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                    }
                    else
                    {
                        json.Append(c);
                    }
                    break;
            }
        }
        return json.Append('"').ToString();
    }
}
