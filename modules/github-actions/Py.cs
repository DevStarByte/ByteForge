using System.Text.Json;
using System.Text.Json.Nodes;

namespace ByteForge.GithubActions;

/// <summary>
/// Reading GitHub's JSON the way the Python version of this module read its dicts: a missing key or another type
/// is null, "truthy" is Python's truthiness, and values put into text are written as Python's str() would.
/// </summary>
static class Py
{
    /// <summary>obj.get(key): the value of a key of an object, else null.</summary>
    public static JsonNode? Get(this JsonNode? obj, string key) => obj is JsonObject o && o.TryGetPropertyValue(key, out var value) ? value : null;

    /// <summary>A string value, or null for anything else.</summary>
    public static string? Str(this JsonNode? obj, string key) => obj.Get(key) is JsonValue v && v.TryGetValue<string>(out var s) ? s : null;

    /// <summary>A whole number value (ids, numbers), or null.</summary>
    public static long? Long(this JsonNode? obj, string key) => obj.Get(key) is JsonValue v && v.GetValueKind() == JsonValueKind.Number && v.TryGetValue<long>(out var n) ? n : null;

    /// <summary>A list value, or an empty one (Python's <c>x.get(key) or []</c>).</summary>
    public static IEnumerable<JsonNode?> List(this JsonNode? obj, string key) => obj.Get(key) as JsonArray ?? [];

    /// <summary>Python's truthiness of a JSON value.</summary>
    public static bool Truthy(JsonNode? node) => node switch
    {
        null => false,
        JsonArray a => a.Count > 0,
        JsonObject o => o.Count > 0,
        JsonValue v => v.GetValueKind() switch
        {
            JsonValueKind.String => v.GetValue<string>().Length > 0,
            JsonValueKind.False or JsonValueKind.Null => false,
            JsonValueKind.Number => v.GetValue<double>() != 0,
            _ => true,
        },
        _ => true,
    };

    /// <summary>Python's str() of a JSON value (None for a missing one).</summary>
    public static string S(JsonNode? node) => node switch
    {
        null => "None",
        JsonValue v when v.GetValueKind() == JsonValueKind.String => v.GetValue<string>(),
        JsonValue v when v.GetValueKind() == JsonValueKind.True => "True",
        JsonValue v when v.GetValueKind() == JsonValueKind.False => "False",
        JsonValue v when v.GetValueKind() == JsonValueKind.Null => "None",
        _ => node.ToJsonString(),
    };

    /// <summary>str(obj.get(key)).</summary>
    public static string S(this JsonNode? obj, string key) => S(obj.Get(key));

    /// <summary><c>obj.get(key) or fallback</c>, as text.</summary>
    public static string Or(this JsonNode? obj, string key, string fallback = "") => obj.Get(key) is var v && Truthy(v) ? S(v) : fallback;

    /// <summary><c>obj.get(key) or None</c>, as text.</summary>
    public static string? OrNull(this JsonNode? obj, string key) => obj.Get(key) is var v && Truthy(v) ? S(v) : null;

    /// <summary>Python's str.strip(chars).</summary>
    public static string StripChars(string text, string chars) => text.Trim(chars.ToCharArray());

    /// <summary>Python's s[:n] (in UTF-16 units).</summary>
    public static string Head(string text, int n) => text.Length > n ? text[..n] : text;
}
