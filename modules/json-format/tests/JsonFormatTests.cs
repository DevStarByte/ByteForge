using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.JsonFormat;
using ByteForge.Modules.Testing;

namespace ByteForge.JsonFormat.Tests;

public sealed class JsonFormatTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-json-format-").FullName;

    public void Dispose()
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    // Expected outputs below are what the Python module (json.dumps(json.loads(text), indent=…, ensure_ascii=False)) produced.
    [Theory]
    [InlineData("{\"a\": 1, \"b\": [1, 2, {\"c\": null}], \"d\": {}}\n", "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2,\n    {\n      \"c\": null\n    }\n  ],\n  \"d\": {}\n}\n")]
    [InlineData("{\"a\":1}", "{\n  \"a\": 1\n}")]
    [InlineData("{\n    \"name\": \"x\",\n    \"list\": [1,2]\n}\n", "{\n    \"name\": \"x\",\n    \"list\": [\n        1,\n        2\n    ]\n}\n")]
    [InlineData("{\n\t\"tabs\": true,\n\t\"k\": [ ]\n}\n", "{\n\t\"tabs\": true,\n\t\"k\": []\n}\n")]
    [InlineData("{\"z\": 1, \"a\": 2}", "{\n  \"z\": 1,\n  \"a\": 2\n}")]
    [InlineData("{\"dup\": 1, \"other\": 2, \"dup\": 3}\n", "{\n  \"dup\": 3,\n  \"other\": 2\n}\n")]
    [InlineData("{\r\n  \"crlf\": [1]\r\n}\r\n", "{\n  \"crlf\": [\n    1\n  ]\n}\n")]
    [InlineData("\"just a string\"\n", "\"just a string\"\n")]
    [InlineData("  true  ", "true")]
    public void Formats_like_python(string text, string expected) => Assert.Equal(expected, JsonFormatter.Format(text));

    [Fact]
    public void Numbers_are_written_as_python_writes_them()
    {
        Assert.Equal("[\n  1.0,\n  100,\n  0,\n  -0.0,\n  100000.0,\n  1e-05,\n  1e+16,\n  1000000000000000.0,\n  123456789012345678901234567890,\n  0.1,\n  Infinity,\n  -Infinity,\n  0.0,\n  NaN\n]",
            JsonFormatter.Format("[1.0, 100, -0, -0.0, 1e5, 1e-5, 1e16, 1e15, 123456789012345678901234567890, 0.10, 1e400, -1e999999999999999999, 1e-400, NaN]"));
    }

    [Fact]
    public void Non_ascii_stays_and_control_characters_are_escaped()
    {
        Assert.Equal("{\n  \"t\": \"café ☃ 😀 é 😀 \u2028 \\u0000 \\u001f \u007f \\\" \\\\ / \\b\\f\\n\\r\\t\"\n}",
            JsonFormatter.Format("{\"t\": \"café ☃ 😀 \\u00e9 \\ud83d\\ude00 \\u2028 \\u0000 \\u001f \\u007f \\\" \\\\ \\/ \\b\\f\\n\\r\\t\"}"));
        // A lone surrogate can't travel as text: it stays an escape (Python wrote it raw).
        Assert.Equal("{\n  \"a\": \"\\ud800A\"\n}", JsonFormatter.Format("{\"a\": \"\\ud800\\u0041\"}"));
    }

    [Fact]
    public void Indentation_comes_from_the_first_indented_line()
    {
        Assert.Equal("    ", JsonFormatter.DetectIndent("{\n    \n  \"a\": 1\n}"));
        Assert.Equal("\t", JsonFormatter.DetectIndent("{\n\t\"a\": 1\n}"));
        Assert.Equal("  ", JsonFormatter.DetectIndent("   {\"a\": 1}"));
        Assert.Equal("   ", JsonFormatter.DetectIndent("{\u2028   \"a\": 1}"));
    }

    [Theory]
    [InlineData("{\n  // a comment\n  \"a\": 1\n}\n", "the file has comments (JSONC); left unchanged")]
    [InlineData("{\"a\": 1 /* c */}", "the file has comments (JSONC); left unchanged")]
    [InlineData("", "Expecting value: line 1 column 1 (char 0)")]
    [InlineData("{\"a\": 1,}", "Illegal trailing comma before end of object: line 1 column 8 (char 7)")]
    [InlineData("[1, 2,]", "Illegal trailing comma before end of array: line 1 column 6 (char 5)")]
    [InlineData("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)")]
    [InlineData("{\"a\" 1}", "Expecting ':' delimiter: line 1 column 6 (char 5)")]
    [InlineData("{1: 2}", "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)")]
    [InlineData("\"abc", "Unterminated string starting at: line 1 column 1 (char 0)")]
    [InlineData("[\"a\nb\"]", "Invalid control character at: line 1 column 4 (char 3)")]
    [InlineData("\"a\\xb\"", "Invalid \\escape: line 1 column 3 (char 2)")]
    [InlineData("\"\\u12\"", "Invalid \\uXXXX escape: line 1 column 3 (char 2)")]
    [InlineData("1 2", "Extra data: line 1 column 3 (char 2)")]
    [InlineData("\uFEFF{}", "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)")]
    [InlineData("{\"😀\": x}", "Expecting value: line 1 column 7 (char 6)")]
    [InlineData("[1,\n  2,\n  oops]", "Expecting value: line 3 column 3 (char 11)")]
    public void Errors_are_pythons(string text, string message)
    {
        Assert.Equal(message, Assert.Throws<JsonFormatException>(() => JsonFormatter.Format(text)).Message);
    }

    [Fact]
    public void Urls_in_strings_are_not_comments()
    {
        Assert.Equal("{\n  \"url\": \"http://example.com/a//b\"\n}", JsonFormatter.Format("{\"url\": \"http://example.com/a//b\"}"));
    }

    [Fact]
    public void Deep_nesting_is_formatted_and_too_deep_refused_without_crashing()
    {
        var deep = new string('[', 5000) + new string(']', 5000);
        Assert.StartsWith("[\n  [\n    [", JsonFormatter.Format(deep));
        var tooDeep = new string('[', 100_000) + new string(']', 100_000);
        Assert.Contains("recursion", Assert.Throws<JsonFormatException>(() => JsonFormatter.Format(tooDeep)).Message);
    }

    [Fact]
    public void Very_long_integers_are_refused_as_python_did()
    {
        Assert.Equal("Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit",
            Assert.Throws<JsonFormatException>(() => JsonFormatter.Format("[" + new string('9', 4301) + "]")).Message);
    }

    [Fact]
    public async Task Registers_and_answers_format_requests()
    {
        await using var module = ModuleHarness.Start(new JsonFormatModule(), Path.Combine(dir, "modules", "json-format"));
        Assert.Equal("""{"type":"registerFormatter","id":"json-format","name":"JSON Formatter","extensions":["json"]}""", (await module.Next("registerFormatter")).ToJsonString());
        module.Send(new() { ["type"] = "format", ["requestId"] = 4, ["path"] = "a.json", ["text"] = "{\"a\":[1]}\n" });
        var result = await module.Next("formatResult");
        Assert.Equal(4, result["requestId"]!.GetValue<int>());
        Assert.Equal("{\n  \"a\": [\n    1\n  ]\n}\n", result["text"]!.GetValue<string>());
        module.Send(new() { ["type"] = "format", ["requestId"] = 5, ["path"] = "a.json", ["text"] = "{\"a\":" });
        result = await module.Next("formatResult");
        Assert.Null(result["text"]);
        Assert.Equal("JSON: Expecting value: line 1 column 6 (char 5)", result["error"]!.GetValue<string>());
    }

    [Fact]
    public void Manifest_names_the_assembly()
    {
        var manifest = JsonNode.Parse(File.ReadAllText(Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "..", "module.json"))),
            documentOptions: new JsonDocumentOptions { CommentHandling = JsonCommentHandling.Skip })!;
        Assert.Equal("byteforge.json-format", manifest["id"]!.GetValue<string>());
        Assert.Equal(("dotnet", "bin/JsonFormat.dll"), (manifest["runtime"]!.GetValue<string>(), manifest["assembly"]!.GetValue<string>()));
        Assert.Null(manifest["command"]);
    }
}
