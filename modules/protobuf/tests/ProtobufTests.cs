using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules;
using ByteForge.Modules.Testing;

namespace ByteForge.Protobuf.Tests;

public sealed class ManifestTests
{
    static JsonObject Manifest() =>
        JsonNode.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "..", "..", "module.json")), documentOptions: new() { CommentHandling = JsonCommentHandling.Skip })!.AsObject();

    static JsonArray Rules() => Manifest()["languages"]![0]!["rules"]!.AsArray();

    /// <summary>The token the earliest rule gives <paramref name="text"/> on <paramref name="line"/> (what the IDE does: the earlier rule wins).</summary>
    static string? FirstToken(JsonArray rules, string line, string text)
    {
        var start = line.IndexOf(text, StringComparison.Ordinal);
        foreach (var rule in rules)
        {
            var group = rule!["group"]?.GetValue<int>() ?? 0;
            foreach (Match m in Regex.Matches(line, rule["match"]!.GetValue<string>()))
            {
                var g = m.Groups[group];
                if (g.Success && g.Index <= start && start < g.Index + g.Length)
                {
                    return rule["token"]!.GetValue<string>();
                }
            }
        }
        return null;
    }

    [Fact]
    public void Rules_compile_without_lookaround()
    {
        var language = Manifest()["languages"]![0]!;
        foreach (var pattern in language["rules"]!.AsArray().Select(r => r!["match"]!.GetValue<string>()).Concat(language["detect"]!.AsArray().Select(d => d!.GetValue<string>())))
        {
            _ = new Regex(pattern);
            Assert.DoesNotMatch(@"\(\?<?[=!]", pattern); // Rust's regex crate has no lookaround
        }
    }

    [Theory]
    [InlineData("service Orders {", "service", "keyword")]
    [InlineData("service Orders {", "Orders", "class name")]
    [InlineData("  rpc Get (GetRequest) returns (stream Order);", "Get", "method name")]
    [InlineData("  rpc Get (GetRequest) returns (stream Order);", "GetRequest", "class name")]
    [InlineData("  rpc Get (GetRequest) returns (stream Order);", "Order", "class name")]
    [InlineData("  repeated string items = 2; // the items", "repeated", "keyword")]
    [InlineData("  repeated string items = 2; // the items", "items", "field name")]
    [InlineData("  repeated string items = 2; // the items", "2", "number")]
    [InlineData("  repeated string items = 2; // the items", "the", "comment")]
    [InlineData("package shop.orders;", "shop.orders", "namespace name")]
    [InlineData("option csharp_namespace = \"Shop.Grpc\";", "csharp_namespace", "property name")]
    [InlineData("option csharp_namespace = \"Shop.Grpc\";", "Shop.Grpc", "string")]
    [InlineData("  google.protobuf.Timestamp created = 3;", "google.protobuf.Timestamp", "class name")]
    public void Highlighting(string line, string text, string token) => Assert.Equal(token, FirstToken(Rules(), line, text));

    [Fact]
    public void Runs_on_dotnet()
    {
        var manifest = Manifest();
        Assert.Equal("dotnet", manifest["runtime"]!.GetValue<string>());
        Assert.Equal("bin/Protobuf.dll", manifest["assembly"]!.GetValue<string>());
        Assert.Null(manifest["command"]);
    }
}

/// <summary>PATH is process-wide: these tests take turns.</summary>
[CollectionDefinition("process environment", DisableParallelization = true)]
public sealed class ProcessEnvironment;

[Collection("process environment")]
public sealed class FormatTests : IDisposable
{
    const string Proto = "syntax = \"proto3\";\noption csharp_namespace=\"Shop.Grpc\";\npackage shop;\nservice Orders{rpc Get(GetRequest)returns(Order);}\nmessage GetRequest{int32 id=1;}\nmessage Order{int32 id=1; repeated string items=2;}\n";

    readonly string dir = Directory.CreateTempSubdirectory("bf-protobuf-").FullName;
    readonly string? path = Environment.GetEnvironmentVariable("PATH");

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("PATH", path);
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    ModuleHarness Start(string formatter)
    {
        var module = ModuleHarness.Start(new ProtobufModule(), Path.Combine(dir, "modules", "protobuf"));
        module.Activate(dir, new JsonObject { ["protobuf.formatter"] = formatter });
        return module;
    }

    static JsonObject Format(string? path, string text) => new() { ["type"] = "format", ["requestId"] = 3, ["path"] = path, ["text"] = text };

    /// <summary>A fake program on PATH: prints "formatted" and its arguments, or fails with "bad input" and "line 2" (CRLF).</summary>
    void Fake(string name, bool fail = false)
    {
        var bin = Directory.CreateDirectory(Path.Combine(dir, "fake-" + Guid.NewGuid().ToString("N")[..6])).FullName;
        if (OperatingSystem.IsWindows())
        {
            File.WriteAllText(Path.Combine(bin, name + ".cmd"), fail ? "@echo off\r\n>&2 echo bad input\r\n>&2 echo line 2\r\nexit /b 3\r\n" : "@more >nul\r\n@echo formatted %*\r\n");
        }
        else
        {
            var script = Path.Combine(bin, name);
            File.WriteAllText(script, fail ? "#!/bin/sh\nprintf 'bad input\\r\\nline 2\\r\\n' >&2\nexit 3\n" : "#!/bin/sh\ncat >/dev/null\necho formatted \"$@\"\n");
            File.SetUnixFileMode(script, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }
        Environment.SetEnvironmentVariable("PATH", bin + Path.PathSeparator + path);
    }

    [Fact]
    public async Task Registers_the_formatter()
    {
        await using var module = ModuleHarness.Start(new ProtobufModule(), Path.Combine(dir, "modules", "protobuf"));
        var formatter = await module.Next("registerFormatter");
        Assert.Equal("protobuf", formatter["id"]!.GetValue<string>());
        Assert.Equal("Protocol Buffers (buf / clang-format)", formatter["name"]!.GetValue<string>());
        Assert.Equal(["proto"], formatter["extensions"]!.AsArray().Select(e => e!.GetValue<string>()));
    }

    [Fact]
    public async Task Missing_formatter_says_what_to_install()
    {
        await using var module = Start("no-such-formatter");
        module.Send(Format(Path.Combine(dir, "a.proto"), Proto));
        var result = await module.Next("formatResult");
        Assert.Equal(3, result["requestId"]!.GetValue<long>());
        Assert.Null(result["text"]);
        Assert.Contains("install", result["error"]!.GetValue<string>());
    }

    [Fact]
    public async Task Clang_format_gets_the_file_name_and_its_errors_are_reported()
    {
        Fake("clang-format");
        await using (var module = Start("clang-format"))
        {
            var file = Path.Combine(dir, "a.proto");
            module.Send(Format(file, Proto));
            var result = await module.Next("formatResult");
            Assert.Null(result["error"]);
            Assert.Equal($"formatted --assume-filename={file}\n", result["text"]!.GetValue<string>().Replace(" \n", "\n"));
            module.Send(Format(null, Proto));
            result = await module.Next("formatResult");
            Assert.Equal("formatted --assume-filename=file.proto\n", result["text"]!.GetValue<string>().Replace(" \n", "\n"));
        }
        Fake("clang-format", fail: true);
        await using (var module = Start("auto"))
        {
            if (Programs.Which("buf") != null)
            {
                return; // auto prefers a real buf here
            }
            module.Send(Format(Path.Combine(dir, "a.proto"), Proto));
            var result = await module.Next("formatResult");
            Assert.Equal("clang-format: bad input\nline 2", result["error"]!.GetValue<string>());
        }
    }

    [Fact]
    public async Task Buf_formats_a_temporary_file_next_to_the_original()
    {
        Fake("buf");
        await using var module = Start("buf");
        var file = Path.Combine(dir, "a.proto");
        module.Send(Format(file, Proto));
        var result = await module.Next("formatResult");
        var text = result["text"]!.GetValue<string>();
        Assert.StartsWith("formatted format " + Path.Combine(dir, "tmp"), text);
        Assert.EndsWith(".proto\n", text.Replace(" \n", "\n"));
        Assert.Empty(Directory.GetFiles(dir, "*.proto")); // the temporary file is gone
    }

    [Fact]
    public async Task Clang_format_when_installed()
    {
        if (Programs.Which("clang-format") == null)
        {
            return; // clang-format isn't installed
        }
        await using var module = Start("clang-format");
        module.Send(Format(null, Proto));
        var result = await module.Next("formatResult");
        Assert.Null(result["error"]);
        var text = result["text"]!.GetValue<string>();
        Assert.Contains("rpc Get(GetRequest) returns (Order);", text);
        Assert.Contains("int32 id = 1;", text);
    }

    [Fact]
    public async Task Buf_when_installed()
    {
        if (Programs.Which("buf") == null)
        {
            return; // buf isn't installed
        }
        await using var module = Start("buf");
        module.Send(Format(null, Proto));
        var result = await module.Next("formatResult");
        Assert.Null(result["error"]);
        Assert.Contains("int32 id = 1;", result["text"]!.GetValue<string>());
    }
}
