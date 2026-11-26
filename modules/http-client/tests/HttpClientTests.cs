using System.Net;
using System.Net.Sockets;
using System.Numerics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.HttpClientModule;
using ByteForge.Modules.Testing;

namespace ByteForge.HttpClientModule.Tests;

static class Sample
{
    public const string Text = """
        @host = http://localhost:5000
        @api = {{host}}/api

        ### Log in
        # @name login
        POST {{api}}/login HTTP/1.1
        Content-Type: application/json
        # a comment between headers

        {
          "user": "admin"
        }

        > {%
            client.global.set("token", response.body.token);
        %}

        ###
        GET {{api}}/orders
            ?page=1
            &size=20
        Authorization: Bearer {{login.response.body.$.token}}

        ###
        // @name upload
        PUT https://example.com/files
        Content-Type: application/octet-stream

        < ./data.bin

        ### Just a URL
        https://example.com/health

        """;

    public static string Dir(string name) => Directory.CreateTempSubdirectory($"bf-http-{name}-").FullName;

    public static void Delete(string dir)
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
        catch (UnauthorizedAccessException)
        {
        }
    }
}

public sealed class ParseTests
{
    readonly OrderedDictionary<string, string> variables;
    readonly List<HttpRequestEntry> requests;

    public ParseTests() => (variables, requests) = HttpFiles.Parse(Sample.Text);

    [Fact]
    public void Variables()
    {
        Assert.Equal([new("host", "http://localhost:5000"), new("api", "{{host}}/api")], variables.ToList());
    }

    [Fact]
    public void Requests()
    {
        Assert.Equal(["POST", "GET", "PUT", "GET"], requests.Select(r => r.Method));
        var (login, orders, upload, health) = (requests[0], requests[1], requests[2], requests[3]);
        Assert.Equal("login", login.Name);
        Assert.Equal("Log in", login.Title);
        Assert.Equal("{{api}}/login", login.Url);
        Assert.Equal([("Content-Type", "application/json")], login.Headers);
        Assert.Equal("{\n  \"user\": \"admin\"\n}", login.Body);
        Assert.Equal("{{api}}/orders?page=1&size=20", orders.Url);
        Assert.Equal("Authorization", orders.Headers[0].Name);
        Assert.Null(orders.Body);
        Assert.Equal("upload", upload.Name);
        Assert.Equal("./data.bin", upload.BodyFile);
        Assert.Null(upload.Body);
        Assert.Equal("https://example.com/health", health.Url);
    }

    [Fact]
    public void Lines_are_zero_based_request_lines()
    {
        var lines = Sample.Text.Split('\n');
        foreach (var r in requests)
        {
            Assert.Contains(r.Url.Split('?')[0], lines[r.Line]);
        }
    }

    [Fact]
    public void Labels()
    {
        Assert.Equal(["Log in", "GET /orders?page=1&size=20", "upload", "Just a URL"], requests.Select(HttpFiles.Label));
    }

    [Fact]
    public void Empty_and_comment_only_blocks()
    {
        Assert.Empty(HttpFiles.Parse("# nothing\n\n###\n// still nothing\n").Requests);
    }

    [Fact]
    public void Crlf_files_and_form_bodies()
    {
        var (_, parsed) = HttpFiles.Parse("POST https://h/form\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\na=1\r\n&b=2\r\n");
        var resolver = new HttpFiles.Resolver(new Dictionary<string, string>(), new Dictionary<string, object?>(), new Dictionary<string, NamedResponse>(), "x.http", null);
        var (_, _, _, body) = HttpFiles.Build(parsed[0], resolver, "x.http");
        Assert.Equal("a=1&b=2", Encoding.UTF8.GetString(body!));
    }
}

public sealed class ResolverTests
{
    static HttpFiles.Resolver Resolver(Dictionary<string, string>? variables = null, Dictionary<string, object?>? env = null, Dictionary<string, NamedResponse>? responses = null) =>
        new(variables ?? [], env ?? [], responses ?? [], Path.Combine(AppContext.BaseDirectory, "x.http"), AppContext.BaseDirectory);

    [Fact]
    public void Nested_file_and_environment_variables()
    {
        var r = Resolver(new() { ["api"] = "{{host}}/api" }, new() { ["host"] = "https://h", ["port"] = new BigInteger(5) });
        Assert.Equal("https://h/api/x:5", r.Resolve("{{api}}/x:{{port}}"));
    }

    [Fact]
    public void File_variables_win_over_environment()
    {
        Assert.Equal("file", Resolver(new() { ["a"] = "file" }, new() { ["a"] = "env" }).Resolve("{{a}}"));
    }

    [Fact]
    public void Dynamic_variables()
    {
        var r = Resolver();
        Assert.Matches("^[0-9a-f-]{36}$", r.Resolve("{{$guid}}"));
        var n = int.Parse(r.Resolve("{{$randomInt 10 20}}")!);
        Assert.InRange(n, 10, 19);
        Assert.Matches(@"^\d{10}$", r.Resolve("{{$timestamp}}"));
        Assert.Matches(@"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$", r.Resolve("{{$datetime iso8601}}"));
        Assert.Matches("GMT$", r.Resolve("{{$datetime rfc1123}}"));
        Assert.Matches(@"^\d{4}-\d\d-\d\d$", r.Resolve("{{$datetime \"YYYY-MM-DD\" 1 d}}"));
        Environment.SetEnvironmentVariable("BF_HTTP_TEST", "secret");
        Assert.Equal("secret", r.Resolve("{{$processEnv BF_HTTP_TEST}}"));
    }

    [Fact]
    public void Dates_on_a_fixed_clock()
    {
        var r = new HttpFiles.Resolver(new Dictionary<string, string>(), new Dictionary<string, object?>(), new Dictionary<string, NamedResponse>(), "x.http", null)
        {
            Now = () => new DateTimeOffset(2024, 2, 28, 22, 5, 9, TimeSpan.FromHours(1)),
        };
        Assert.Equal("2024-02-28T21:05:09Z", r.Resolve("{{$datetime iso8601}}"));
        Assert.Equal("2024-02-28T21:05:09Z", r.Resolve("{{$isoTimestamp}}"));
        Assert.Equal("Wed, 28 Feb 2024 21:05:09 GMT", r.Resolve("{{$datetime rfc1123}}"));
        Assert.Equal("2024-03-01 21:05", r.Resolve("{{$datetime 'YYYY-MM-DD HH:mm' 2 d}}"));
        Assert.Equal("1709154309", r.Resolve("{{$timestamp}}"));
        Assert.Equal("1709157909", r.Resolve("{{$timestamp 1 h}}"));
        Assert.Throws<VariableError>(() => r.Resolve("{{$timestamp 1 q}}"));
        Assert.Equal("invalid literal for int() with base 10: 'x'", Assert.Throws<VariableError>(() => r.Resolve("{{$randomInt x 3}}")).Message);
    }

    [Fact]
    public void Undefined_and_recursive_variables_fail()
    {
        Assert.Equal("{{nope}} isn't defined", Assert.Throws<VariableError>(() => Resolver().Resolve("{{nope}}")).Message);
        Assert.Equal("{{a}} refers to itself", Assert.Throws<VariableError>(() => Resolver(new() { ["a"] = "{{a}}" }).Resolve("{{a}}")).Message);
    }

    [Fact]
    public void Request_variables()
    {
        var responses = new Dictionary<string, NamedResponse>
        {
            ["login"] = new([("Location", "/x")], "{\"token\": \"t1\", \"items\": [{\"id\": 1}, {\"id\": 2}]}", [], ""),
        };
        var r = Resolver(responses: responses);
        Assert.Equal("t1", r.Resolve("{{login.response.body.$.token}}"));
        Assert.Equal("2", r.Resolve("{{login.response.body.$.items[1].id}}"));
        Assert.Equal("2", r.Resolve("{{login.response.body.$.items[-1].id}}"));
        Assert.Equal("[1, 2]", r.Resolve("{{login.response.body.$.items[*].id}}"));
        Assert.Equal("{\"id\": 1}", r.Resolve("{{login.response.body.$['items'][0]}}"));
        Assert.Equal("/x", r.Resolve("{{login.response.headers.location}}"));
        Assert.Equal("send the request named other first", Assert.Throws<VariableError>(() => r.Resolve("{{other.response.body.$.a}}")).Message);
        Assert.Equal("$.nope matches nothing in the response", Assert.Throws<VariableError>(() => r.Resolve("{{login.response.body.$.nope}}")).Message);
    }
}

public sealed class EnvironmentTests : IDisposable
{
    readonly string root = Sample.Dir("env");

    public void Dispose() => Sample.Delete(root);

    [Fact]
    public void Nearest_env_file_with_shared_and_private()
    {
        var api = Directory.CreateDirectory(Path.Combine(root, "src", "Api")).FullName;
        File.WriteAllText(Path.Combine(root, "http-client.env.json"), """{"$shared": {"version": "v1"}, "dev": {"host": "http://localhost"}, "prod": {"host": "https://p"}}""");
        File.WriteAllText(Path.Combine(root, "http-client.private.env.json"), """{"dev": {"password": "pw"}}""");
        var envs = HttpFiles.LoadEnvironments(Path.Combine(api, "api.http"), root);
        Assert.Equal([new("version", "v1"), new("host", "http://localhost"), new("password", "pw")], envs["dev"].ToList());
        Assert.Equal([new("version", "v1"), new("host", "https://p")], envs["prod"].ToList());
        Assert.Equal("dev", new ByteForge.HttpClientModule.HttpClientModule().CurrentEnvironment(envs));
    }

    [Fact]
    public void Broken_env_file_is_logged()
    {
        File.WriteAllText(Path.Combine(root, "http-client.env.json"), "{\"dev\": }");
        var logged = new List<string>();
        Assert.Empty(HttpFiles.LoadEnvironments(Path.Combine(root, "a.http"), root, logged.Add));
        Assert.EndsWith("http-client.env.json: Expecting value: line 1 column 9 (char 8)", Assert.Single(logged));
    }

    [Fact]
    public void Dotenv()
    {
        File.WriteAllText(Path.Combine(root, ".env"), "export TOKEN=\"abc\"\n# comment\nOTHER=1\n");
        var r = new HttpFiles.Resolver(new Dictionary<string, string>(), new Dictionary<string, object?>(), new Dictionary<string, NamedResponse>(), Path.Combine(root, "a.http"), root);
        Assert.Equal("abc-1", r.Resolve("{{$dotenv TOKEN}}-{{$dotenv OTHER}}"));
    }
}

public sealed class FormatTests
{
    static HttpResult Response(string contentType, byte[] data, int status = 200, string reason = "OK") =>
        new(status, reason, "HTTP/1.1", [("Content-Type", contentType), ("Content-Length", data.Length.ToString())], data, 12.5, "http://h/x");

    [Fact]
    public void Json_is_written_as_python_writes_it()
    {
        var value = PyJson.Loads("""{"a": 1.0, "b": [1e16, 1.5e-5, 0.0001, 100, 12345678901234567890], "c": "é\u0001", "a": 2, "d": {}, "e": [], "f": NaN}""");
        Assert.Equal("""{"a": 2, "b": [1e+16, 1.5e-05, 0.0001, 100, 12345678901234567890], "c": "\u00e9\u0001", "d": {}, "e": [], "f": NaN}""", PyJson.Dumps(value));
        Assert.Equal("{\n  \"a\": 2,\n  \"b\": [\n    1e+16,\n    1.5e-05,\n    0.0001,\n    100,\n    12345678901234567890\n  ],\n  \"c\": \"é\\u0001\",\n  \"d\": {},\n  \"e\": [],\n  \"f\": NaN\n}", PyJson.DumpsIndented(value));
        Assert.Equal("Extra data: line 1 column 3 (char 2)", Assert.Throws<JsonDecodeError>(() => PyJson.Loads("1 2")).Message);
    }

    [Fact]
    public void Sizes_round_half_to_even()
    {
        Assert.Equal("0 B", HttpFiles.Size(0));
        Assert.Equal("1023 B", HttpFiles.Size(1023));
        Assert.Equal("1.0 KB", HttpFiles.Size(1024));
        Assert.Equal("1.2 KB", HttpFiles.Size(1280));
        Assert.Equal("1.3 KB", HttpFiles.Size(1331));
        Assert.Equal("1024.0 KB", HttpFiles.Size(1024 * 1024 - 1));
        Assert.Equal("1.0 MB", HttpFiles.Size(1024 * 1024));
    }

    [Fact]
    public void Response_document()
    {
        var response = Response("application/json; charset=utf-8", Encoding.UTF8.GetBytes("{\"x\":[1,{\"y\":\"ü\"}]}"));
        var text = HttpFiles.BodyText(response);
        Assert.Equal(
            "GET http://h/x\n# 200 OK · 12 ms · 20 B\n\nHTTP/1.1 200 OK\nContent-Type: application/json; charset=utf-8\nContent-Length: 20\n\n{\n  \"x\": [\n    1,\n    {\n      \"y\": \"ü\"\n    }\n  ]\n}\n",
            HttpFiles.ResponseDocument("GET", "http://h/x", response, text));
        Assert.Equal("{\"x\":[1,{\"y\":\"ü\"}]}", HttpFiles.BodyText(response, pretty: false));

        var image = Response("image/png", [0x89, 0x50, 0, 0]) with { Url = "http://h/y", Reason = "" };
        Assert.Null(HttpFiles.BodyText(image));
        Assert.Equal("GET http://h/x\n# 200 · 12 ms · 4 B\n# redirected to http://h/y\n\nHTTP/1.1 200\nContent-Type: image/png\nContent-Length: 4\n\n# 4 B of image/png, saved next to this file\n",
            HttpFiles.ResponseDocument("GET", "http://h/x", image, null));
        Assert.Equal("caf\uFFFD", HttpFiles.BodyText(Response("text/plain; charset=ascii", [0x63, 0x61, 0x66, 0xE9])));
        Assert.Equal("café", HttpFiles.BodyText(Response("text/plain; charset=latin-1", [0x63, 0x61, 0x66, 0xE9])));
        // Python's utf-16 / utf-32 codecs take the byte order from the BOM and drop it; utf-16le keeps it.
        Assert.Equal("hé", HttpFiles.BodyText(Response("text/plain; charset=utf-16", [0xFE, 0xFF, 0, 0x68, 0, 0xE9])));
        Assert.Equal("hé", HttpFiles.BodyText(Response("text/plain; charset=UTF-16", [0x68, 0, 0xE9, 0])));
        Assert.Equal("﻿h", HttpFiles.BodyText(Response("text/plain; charset=utf-16le", [0xFF, 0xFE, 0x68, 0])));
        Assert.Equal("h", HttpFiles.BodyText(Response("text/plain; charset=utf-32", [0, 0, 0xFE, 0xFF, 0, 0, 0, 0x68])));
        Assert.Equal("{\n  \"a\": 1\n}", HttpFiles.BodyText(Response("application/json; charset=utf-16", [0xFF, 0xFE, .. Encoding.Unicode.GetBytes("{\"a\":1}")])));
    }

    [Fact]
    public void Curl_command()
    {
        Assert.Equal("curl -X POST 'https://h/a' -H 'X: it'\\''s' --data-binary '{}'",
            HttpFiles.CurlCommand("POST", "https://h/a", [("X", "it's")], "{}"u8.ToArray()));
    }
}

public sealed class PruneTests : IDisposable
{
    readonly string folder = Sample.Dir("prune");

    public void Dispose() => Sample.Delete(folder);

    [Fact]
    public void Keeps_the_newest_responses_and_their_binaries()
    {
        for (var i = 0; i < HttpFiles.KeepResponses + 3; i++)
        {
            File.WriteAllText(Path.Combine(folder, $"{i:0000}-r.http"), "");
        }
        File.WriteAllText(Path.Combine(folder, "0000-r.png"), "");
        HttpFiles.Prune(folder);
        var names = Directory.GetFiles(folder).Select(Path.GetFileName).Order(StringComparer.Ordinal).ToList();
        Assert.Equal(HttpFiles.KeepResponses, names.Count);
        Assert.Equal("0003-r.http", names[0]);
    }
}

/// <summary>Requests through an injected handler: what the module sends, and which handler it asks for.</summary>
public sealed class InjectedHandlerTests : IAsyncLifetime
{
    sealed class Fixed(List<(bool Insecure, HttpRequestMessage Request, string? Body)> seen, bool insecure) : HttpMessageHandler
    {
        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            var body = request.Content != null ? await request.Content.ReadAsStringAsync(cancellationToken) : null;
            lock (seen)
            {
                seen.Add((insecure, request, body));
            }
            var response = new HttpResponseMessage(HttpStatusCode.Created) { ReasonPhrase = "Created", Content = new StringContent("{\"id\":7}", Encoding.UTF8, "application/json") };
            response.Headers.TryAddWithoutValidation("X-Trace", "abc");
            return response;
        }
    }

    readonly string tmp = Sample.Dir("inject");
    readonly List<(bool Insecure, HttpRequestMessage Request, string? Body)> seen = [];
    ModuleHarness module = null!;

    public Task InitializeAsync()
    {
        File.WriteAllText(Path.Combine(tmp, "api.http"), "POST https://localhost:5001/items\nX-Api: 1\nX-Api: 2\n\nname=a\n\n###\nPUT https://example.com/x\nContent-Type: application/json\nUser-Agent: mine\n\n< ./body.json\n");
        File.WriteAllText(Path.Combine(tmp, "body.json"), "{\"a\": 1}");
        module = ModuleHarness.Start(new ByteForge.HttpClientModule.HttpClientModule(Path.Combine(tmp, "cache"), (insecure, _) => new Fixed(seen, insecure)), Path.Combine(tmp, "modules", "http-client"));
        module.Activate(tmp);
        return Task.CompletedTask;
    }

    public async Task DisposeAsync()
    {
        await module.DisposeAsync();
        Sample.Delete(tmp);
    }

    [Fact]
    public async Task Sends_through_the_injected_handler()
    {
        var file = Path.Combine(tmp, "api.http");
        module.Send(new() { ["type"] = "treeAction", ["view"] = "http", ["item"] = file, ["action"] = "sendAll" });
        await module.Next("log", m => m["text"]!.GetValue<string>().StartsWith("http: sent 2 requests from api.http", StringComparison.Ordinal));
        Assert.Equal(2, seen.Count);
        var (insecure, post, body) = seen[0];
        Assert.True(insecure);  // https on localhost
        Assert.Equal("name=a", body);
        Assert.Equal(["2"], post.Headers.GetValues("X-Api"));  // urllib keeps the last value of a repeated header
        Assert.Equal("application/x-www-form-urlencoded", post.Content!.Headers.ContentType!.MediaType);
        Assert.Equal("ByteForge-HTTP-Client", post.Headers.UserAgent.ToString());
        (insecure, var put, body) = seen[1];
        Assert.False(insecure);
        Assert.Equal("{\"a\": 1}", body);
        Assert.Equal("mine", put.Headers.UserAgent.ToString());
        Assert.Equal("application/json", put.Content!.Headers.ContentType!.MediaType);
        var saved = Directory.GetFiles(Path.Combine(tmp, "cache"), "*.http").Select(File.ReadAllText).ToList();
        Assert.Contains(saved, d => d.Contains("# 201 Created") && d.Contains("X-Trace: abc") && d.Contains("\"id\": 7"));
    }
}

public sealed class HeadTests
{
    [Fact]
    public void Response_heads_are_read_as_http_client_reads_them()
    {
        var (version, reason, headers) = Sender.ParseHead("HTTP/1.0 200  Fine  \r\nContent-Type: text/plain\r\nX-Pad:   v \r\nSet-Cookie: a\r\nSet-Cookie: b")!.Value;
        Assert.Equal("HTTP/1.0", version);
        Assert.Equal("Fine", reason);
        Assert.Equal([("Content-Type", "text/plain"), ("X-Pad", "v "), ("Set-Cookie", "a"), ("Set-Cookie", "b")], headers);
        Assert.Equal("", Sender.ParseHead("HTTP/1.1 204 ")!.Value.Reason);
    }
}

public sealed class ManifestTests
{
    static JsonNode Manifest()
    {
        var dir = AppContext.BaseDirectory;
        while (!File.Exists(Path.Combine(dir, "module.json")))
        {
            dir = Path.GetDirectoryName(Path.TrimEndingDirectorySeparator(dir))!;
        }
        return JsonNode.Parse(File.ReadAllText(Path.Combine(dir, "module.json")), documentOptions: new() { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true })!;
    }

    [Fact]
    public void Manifest_rules_compile_without_lookaround()
    {
        var manifest = Manifest();
        var language = manifest["languages"]![0]!;
        var patterns = language["rules"]!.AsArray().Select(r => r!["match"]!.GetValue<string>()).Concat(language["detect"]!.AsArray().Select(d => d!.GetValue<string>()));
        foreach (var pattern in patterns)
        {
            _ = new Regex(pattern);
            Assert.DoesNotMatch(@"\(\?<?[=!]", pattern);
        }
        Assert.Equal(["http"], manifest["settings"]!.AsArray().Select(s => s!["key"]!.GetValue<string>().Split('.')[0]).Distinct());
        Assert.Equal("dotnet", manifest["runtime"]!.GetValue<string>());
        Assert.Equal("bin/HttpClientModule.dll", manifest["assembly"]!.GetValue<string>());
        Assert.Null(manifest["command"]);
    }
}

/// <summary>A tiny HTTP/1.1 server on 127.0.0.1 answering as the Python tests' handler did.</summary>
public sealed class TestServer : IDisposable
{
    readonly TcpListener listener = new(IPAddress.Loopback, 0);
    readonly CancellationTokenSource stop = new();

    public string Base { get; }

    public TestServer()
    {
        listener.Start();
        Base = $"http://127.0.0.1:{((IPEndPoint)listener.LocalEndpoint).Port}";
        _ = Task.Run(Serve);
    }

    async Task Serve()
    {
        while (!stop.IsCancellationRequested)
        {
            TcpClient client;
            try
            {
                client = await listener.AcceptTcpClientAsync(stop.Token);
            }
            catch (Exception)
            {
                return;
            }
            _ = Task.Run(() => Handle(client));
        }
    }

    static async Task Handle(TcpClient client)
    {
        using var _ = client;
        var stream = client.GetStream();
        var buffer = new List<byte>();
        var chunk = new byte[4096];
        int headerEnd;
        while ((headerEnd = IndexOf(buffer, "\r\n\r\n"u8.ToArray())) < 0)
        {
            var n = await stream.ReadAsync(chunk);
            if (n == 0)
            {
                return;
            }
            buffer.AddRange(chunk.AsSpan(0, n).ToArray());
        }
        var head = Encoding.ASCII.GetString([.. buffer.Take(headerEnd)]).Split("\r\n");
        var (method, path) = (head[0].Split(' ')[0], head[0].Split(' ')[1]);
        var headers = head.Skip(1).Select(h => h.Split(':', 2)).ToDictionary(h => h[0].Trim(), h => h[1].Trim(), StringComparer.OrdinalIgnoreCase);
        var length = headers.TryGetValue("Content-Length", out var l) ? int.Parse(l) : 0;
        while (buffer.Count < headerEnd + 4 + length)
        {
            var n = await stream.ReadAsync(chunk);
            if (n == 0)
            {
                break;
            }
            buffer.AddRange(chunk.AsSpan(0, n).ToArray());
        }
        var body = buffer.Skip(headerEnd + 4).Take(length).ToArray();

        async Task Reply(int status, string reason, byte[] data, string type, params (string, string)[] extra)
        {
            var text = new StringBuilder($"HTTP/1.1 {status} {reason}\r\nServer: test\r\nContent-Type: {type}\r\nContent-Length: {data.Length}\r\n");
            foreach (var (k, v) in extra)
            {
                text.Append($"{k}: {v}\r\n");
            }
            text.Append("Connection: close\r\n\r\n");
            await stream.WriteAsync(Encoding.ASCII.GetBytes(text.ToString()));
            await stream.WriteAsync(data);
        }

        if (method == "POST")
        {
            var user = JsonNode.Parse(body.Length > 0 ? body : "{}"u8.ToArray())?["user"]?.GetValue<string>() ?? "?";
            await Reply(200, "OK", Encoding.UTF8.GetBytes($"{{\"token\": \"tok-{user}\"}}"), "application/json");
        }
        else if (path == "/missing")
        {
            await Reply(404, "Not Found", "{\"title\":\"Not Found\"}"u8.ToArray(), "application/problem+json");
        }
        else if (path == "/redirect")
        {
            await Reply(302, "Found", [], "text/plain", ("Location", "/orders"));
        }
        else if (path == "/image")
        {
            await Reply(200, "OK", [0x89, (byte)'P', (byte)'N', (byte)'G', 13, 10, 0x1a, 10, 0, 0], "image/png");
        }
        else
        {
            var auth = headers.TryGetValue("Authorization", out var a) ? JsonValue.Create(a) : null;
            await Reply(200, "OK", Encoding.UTF8.GetBytes(new JsonObject { ["auth"] = auth, ["path"] = path }.ToJsonString()), "application/json");
        }
    }

    static int IndexOf(List<byte> data, byte[] needle)
    {
        for (var i = 0; i + needle.Length <= data.Count; i++)
        {
            if (data.Skip(i).Take(needle.Length).SequenceEqual(needle))
            {
                return i;
            }
        }
        return -1;
    }

    public void Dispose()
    {
        stop.Cancel();
        listener.Stop();
    }
}

public sealed class SendTests : IClassFixture<TestServer>, IAsyncLifetime
{
    readonly TestServer server;
    readonly string tmp = Sample.Dir("send");
    readonly string path;
    readonly string cache;
    List<HttpRequestEntry> requests = [];
    ModuleHarness module = null!;

    public SendTests(TestServer server)
    {
        this.server = server;
        path = Path.Combine(tmp, "api.http");
        cache = Path.Combine(tmp, "cache");
        File.WriteAllText(path, $$$"""
            @host = {{{server.Base}}}

            # @name login
            POST {{host}}/login
            Content-Type: application/json

            {"user": "{{$processEnv USER_FOR_TEST}}"}

            ###
            GET {{host}}/orders?x=1
            Authorization: Bearer {{login.response.body.$.token}}

            ###
            GET {{host}}/missing

            ###
            GET {{host}}/redirect

            ###
            GET {{host}}/image

            """);
        Environment.SetEnvironmentVariable("USER_FOR_TEST", "ada");
    }

    public Task InitializeAsync()
    {
        requests = HttpFiles.Parse(Py.ReadText(path)).Requests;
        module = ModuleHarness.Start(new ByteForge.HttpClientModule.HttpClientModule(cache, null), Path.Combine(tmp, "modules", "http-client"));
        module.Activate(tmp);
        return Task.CompletedTask;
    }

    public async Task DisposeAsync()
    {
        await module.DisposeAsync();
        Sample.Delete(tmp);
    }

    static string Text(JsonObject m) => m["text"]?.GetValue<string>() ?? "";

    /// <summary>Sends request <paramref name="index"/>; the log line that ends it (" → status", or the error).</summary>
    async Task<string> Send(int index)
    {
        module.Send(new() { ["type"] = "treeAction", ["view"] = "http", ["item"] = $"{path}\n{requests[index].Line}", ["action"] = "send" });
        return Text(await module.Next("log", m => Text(m).StartsWith("http: ", StringComparison.Ordinal) && !Text(m).Contains("environment")));
    }

    async Task<(string Path, string Text)> Opened()
    {
        var file = (await module.Next("openFile"))["path"]!.GetValue<string>();
        return (file, File.ReadAllText(file));
    }

    [Fact]
    public async Task Chained_requests()
    {
        Assert.Contains("→ 200 OK", await Send(0));
        var (_, document) = await Opened();
        Assert.Matches(@"(?m)^HTTP/1\.[01] 200 OK\r?$", document);
        Assert.Contains("\"token\": \"tok-ada\"", document);  // pretty-printed
        Assert.Matches(@"Server: test\r?\nContent-Type: application/json\r?\nContent-Length: \d+\r?\nConnection: close\r?\n", document);  // headers as received
        Assert.Contains("→ 200 OK", await Send(1));
        (_, document) = await Opened();
        Assert.Contains("\"auth\": \"Bearer tok-ada\"", document);
        Assert.Contains("\"path\": \"/orders?x=1\"", document);
        if (OperatingSystem.IsWindows())
        {
            Assert.Contains("\r\n", document);  // written in text mode, as Python did
        }
    }

    [Fact]
    public async Task Missing_named_response_is_a_variable_error()
    {
        Assert.Equal("http: GET /orders?x=1: send the request named login first", await Send(1));
    }

    [Fact]
    public async Task Error_status_and_redirects()
    {
        Assert.Contains("→ 404 Not Found", await Send(2));
        var line = await Send(3);
        Assert.Contains("→ 200 OK", line);
        var (_, document) = await Opened();
        Assert.Contains("# redirected to " + server.Base + "/orders", document);
        module.Send(new() { ["type"] = "event", ["name"] = "settingsChanged", ["settings"] = new JsonObject { ["http.followRedirects"] = false } });
        Assert.Contains("→ 302 Found", await Send(3));
    }

    [Fact]
    public async Task Binary_body_is_saved_next_to_the_response()
    {
        await Send(4);
        var (file, document) = await Opened();
        Assert.Contains("image/png", document);
        Assert.True(File.Exists(file[..^5] + ".png"));
        Assert.Equal(cache, Path.GetDirectoryName(file));
    }

    [Fact]
    public async Task Connection_failure_is_logged()
    {
        File.WriteAllText(path, "GET http://127.0.0.1:9/nothing\n");
        requests = HttpFiles.Parse(Py.ReadText(path)).Requests;
        Assert.StartsWith("http: GET http://127.0.0.1:9/nothing:", await Send(0));
    }

    [Fact]
    public async Task Tree_lists_requests_with_results()
    {
        await Send(0);
        module.Send(new() { ["type"] = "treeAction", ["view"] = "http", ["item"] = null, ["action"] = "refresh" });
        var tree = await module.Next("setTree", m => m["items"]!.AsArray().Count > 0
            && m["items"]![0]!["children"]![0]!["description"]!.GetValue<string>().StartsWith("200 OK", StringComparison.Ordinal));
        var file = tree["items"]![0]!;
        Assert.Equal("api.http", file["label"]!.GetValue<string>());
        Assert.Equal(5, file["count"]!.GetValue<int>());
    }

    [Fact]
    public async Task Registers_commands_and_shows_curl()
    {
        await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "http.send" && m["keybinding"]!.GetValue<string>() == "ctrl-alt-enter");
        module.Send(new() { ["type"] = "treeAction", ["view"] = "http", ["item"] = $"{path}\n{requests[2].Line}", ["action"] = "curl" });
        Assert.Equal($"curl -X GET '{server.Base}/missing'", Text(await module.Next("log", m => Text(m).StartsWith("curl", StringComparison.Ordinal))));
    }
}
