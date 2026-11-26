using System.Globalization;
using ByteForge.Modules;
using System.Numerics;
using System.Text;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using static ByteForge.HttpClientModule.Py;

namespace ByteForge.HttpClientModule;

/// <summary>One request of an .http file. <see cref="Line"/>: the 0-based line of its request line.</summary>
public sealed record HttpRequestEntry(string? Name, string? Title, string Method, string Url, List<(string Name, string Value)> Headers,
    string? Body, string? BodyFile, int Line);

/// <summary>A response as the module keeps it (Python's perform() result).</summary>
public sealed record HttpResult(int Status, string Reason, string Version, List<(string Name, string Value)> Headers, byte[] Data, double Elapsed, string Url);

/// <summary>What a named request left for {{name.response…}} / {{name.request…}}.</summary>
public sealed record NamedResponse(List<(string Name, string Value)> Headers, string Text, List<(string Name, string Value)> RequestHeaders, string RequestBody);

/// <summary>A variable that can't be filled in (the message is for the user).</summary>
public sealed class VariableError(string message) : Exception(message);

/// <summary>The .http file format: parsing, environments, variables, and the response documents written.</summary>
public static partial class HttpFiles
{
    public static readonly string[] Extensions = [".http", ".rest"];
    public static readonly string[] Methods = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"];
    public static readonly HashSet<string> SkipDirs = ["node_modules", "bin", "obj", ".git", "target", "dist", ".vs", ".idea", "TestResults"];
    public const int KeepResponses = 100;
    public const int MaxPretty = 5_000_000;

    const RegexOptions I = RegexOptions.IgnoreCase | RegexOptions.CultureInvariant;

    static readonly Regex RequestLine = new($@"^({string.Join("|", Methods)}){S}+({NS}.*?)(?:{S}+HTTP/\d(?:\.\d)?)?{S}*$", I);
    static readonly Regex UrlOnly = new($@"^(https?://{NS}.*?|\{{\{{[^}}]+\}}\}}{NS}*)(?:{S}+HTTP/\d(?:\.\d)?)?{S}*$", I);
    static readonly Regex FileVariable = new($@"^@([A-Za-z_][{WChars}.-]*){S}*={S}?(.*)$");
    static readonly Regex NameComment = new($@"^(?:#|//){S}*@name{S}*=?{S}*({NS}+)");
    static readonly Regex Header = new($@"^([!#$%&'*+.^_`|~0-9A-Za-z-]+){S}*:{S}?(.*)$");
    static readonly Regex ResponseScript = new(@"^> \{%.*?%\}[ \t]*$", RegexOptions.Singleline | RegexOptions.Multiline);
    static readonly Regex ResponseReference = new($@"^(?:> |<> ){NS}.*$", RegexOptions.Multiline);
    static readonly Regex BodyFileLine = new($@"\A(?:<@?{S}+{NS}.*)\z");

    public static bool IsHttpFile(string? path) => path != null && Extensions.Any(e => path.ToLowerInvariant().EndsWith(e, StringComparison.Ordinal));

    static bool IsComment(string line)
    {
        var s = LStrip(line);
        return s.StartsWith('#') || s.StartsWith("//", StringComparison.Ordinal);
    }

    /// <summary>The file variables (@name = value, raw) and the requests of an .http file.</summary>
    public static (OrderedDictionary<string, string> Variables, List<HttpRequestEntry> Requests) Parse(string text)
    {
        var variables = new OrderedDictionary<string, string>();
        var requests = new List<HttpRequestEntry>();
        var lines = SplitLines(text);
        var blocks = new List<(string? Title, int Start, List<string> Lines)>();
        string? title = null;
        var start = 0;
        var block = new List<string>();
        for (var n = 0; n < lines.Count; n++)
        {
            if (lines[n].StartsWith("###", StringComparison.Ordinal))
            {
                blocks.Add((title, start, block));
                var rest = Strip(lines[n][3..]);
                (title, start, block) = (rest.Length > 0 ? rest : null, n + 1, []);
            }
            else
            {
                block.Add(lines[n]);
            }
        }
        blocks.Add((title, start, block));

        foreach (var (blockTitle, blockStart, blockLines) in blocks)
        {
            string? name = null;
            var i = 0;
            // Before the request line: blank lines, comments (# @name), file variables.
            for (; i < blockLines.Count; i++)
            {
                var line = Strip(blockLines[i]);
                var m = FileVariable.Match(line);
                if (line.Length == 0)
                {
                }
                else if (IsComment(line))
                {
                    var n = NameComment.Match(line);
                    if (n.Success)
                    {
                        name = n.Groups[1].Value;
                    }
                }
                else if (m.Success)
                {
                    variables[m.Groups[1].Value] = Strip(m.Groups[2].Value);
                }
                else
                {
                    break;
                }
            }
            if (i >= blockLines.Count)
            {
                continue;
            }
            var requestText = Strip(blockLines[i]);
            string method, url;
            var r = RequestLine.Match(requestText);
            if (r.Success)
            {
                (method, url) = (r.Groups[1].Value.ToUpperInvariant(), r.Groups[2].Value);
            }
            else
            {
                var u = UrlOnly.Match(requestText);
                if (!u.Success)
                {
                    continue;
                }
                (method, url) = ("GET", u.Groups[1].Value);
            }
            var requestLine = blockStart + i;
            i++;
            // Query continuation lines (?a=1 / &b=2) on the lines below the URL.
            while (i < blockLines.Count && Strip(blockLines[i]) is [ '?' or '&', ..] && blockLines[i] is [' ' or '\t' or '?' or '&', ..])
            {
                url += Strip(blockLines[i]);
                i++;
            }
            var headers = new List<(string, string)>();
            while (i < blockLines.Count && Strip(blockLines[i]).Length > 0)
            {
                var h = Strip(blockLines[i]);
                if (!IsComment(h) && Header.Match(h) is { Success: true } hm)
                {
                    headers.Add((hm.Groups[1].Value, Strip(hm.Groups[2].Value)));
                }
                i++;
            }
            var bodyText = i < blockLines.Count ? string.Join("\n", blockLines.Skip(i + 1)) : "";
            // Rider's response handlers (> {% … %}, > handler.js) and response references (<> file) aren't body.
            bodyText = ResponseScript.Replace(bodyText, "");
            bodyText = RStrip(ResponseReference.Replace(bodyText, ""));
            string? body = Strip(bodyText).Length > 0 ? bodyText : null;
            string? bodyFile = null;
            if (body != null && BodyFileLine.IsMatch(Strip(body)))
            {
                (bodyFile, body) = (Strip(Strip(body).TrimStart('<', '@')), null);
            }
            requests.Add(new HttpRequestEntry(name, blockTitle != null && !blockTitle.StartsWith('#') ? blockTitle : null,
                method, url, headers, body, bodyFile, requestLine));
        }
        return (variables, requests);
    }

    [GeneratedRegex(@"^\{\{[^}]+\}\}")]
    private static partial Regex LeadingVariable();

    [GeneratedRegex(@"^https?://[^/]+")]
    private static partial Regex SchemeAndHost();

    /// <summary>What a request is called in lists: its title, its @name, else METHOD /path.</summary>
    public static string Label(HttpRequestEntry request)
    {
        if (!string.IsNullOrEmpty(request.Title))
        {
            return request.Title;
        }
        if (!string.IsNullOrEmpty(request.Name))
        {
            return request.Name;
        }
        var path = SchemeAndHost().Replace(LeadingVariable().Replace(request.Url, ""), "");
        return $"{request.Method} {(path.Length > 0 ? path : request.Url)}";
    }

    // ───────────── environments ─────────────

    public static readonly string[] EnvFiles = ["http-client.env.json", "http-client.private.env.json", "http-client.env.json.user"];

    static bool SamePath(string a, string b) =>
        string.Equals(a, b, OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal);

    /// <summary>Folders searched for environment files: the .http file's folder and those above it, up to the root.</summary>
    public static List<string> EnvDirs(string httpPath, string? root)
    {
        var dir = Path.GetDirectoryName(Path.GetFullPath(httpPath)) ?? Path.GetFullPath(httpPath);
        var stop = !string.IsNullOrEmpty(root) ? Path.TrimEndingDirectorySeparator(Path.GetFullPath(root)) : null;
        var dirs = new List<string>();
        while (true)
        {
            dirs.Add(dir);
            if (stop == null || SamePath(Path.TrimEndingDirectorySeparator(dir), stop))
            {
                break;
            }
            var parent = Path.GetDirectoryName(dir);
            if (parent == null || parent == dir)
            {
                break;
            }
            dir = parent;
        }
        return dirs;
    }

    /// <summary>{environment: {variable: value}} from the nearest folder with an environment file ($shared merged in).</summary>
    public static OrderedDictionary<string, OrderedDictionary<string, object?>> LoadEnvironments(string httpPath, string? root, Action<string>? log = null)
    {
        foreach (var dir in EnvDirs(httpPath, root))
        {
            var found = EnvFiles.Select(n => Path.Combine(dir, n)).Where(File.Exists).ToList();
            if (found.Count == 0)
            {
                continue;
            }
            var envs = new OrderedDictionary<string, OrderedDictionary<string, object?>>();
            foreach (var path in found)
            {
                object? data;
                try
                {
                    data = PyJson.Loads(ReadStrict(path));
                }
                catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonDecodeError or DecoderFallbackException)
                {
                    log?.Invoke($"http: {path}: {OsError(e, path)}");
                    continue;
                }
                if (data is not OrderedDictionary<string, object?> map)
                {
                    continue;
                }
                foreach (var (env, values) in map)
                {
                    if (values is OrderedDictionary<string, object?> v)
                    {
                        if (!envs.TryGetValue(env, out var into))
                        {
                            envs[env] = into = [];
                        }
                        foreach (var (key, value) in v)
                        {
                            into[key] = value;
                        }
                    }
                }
            }
            envs.Remove("$shared", out var shared);
            shared ??= [];
            var result = new OrderedDictionary<string, OrderedDictionary<string, object?>>();
            foreach (var (env, values) in envs)
            {
                var merged = new OrderedDictionary<string, object?>(shared);
                foreach (var (key, value) in values)
                {
                    merged[key] = value;
                }
                result[env] = merged;
            }
            if (result.Count == 0 && shared.Count > 0)
            {
                result["$shared"] = shared;
            }
            return result;
        }
        return [];
    }

    /// <summary>A text file read as Python's open(encoding="utf-8-sig") reads it: a bad byte is an error.</summary>
    static string ReadStrict(string path)
    {
        var bytes = File.ReadAllBytes(path);
        var skip = bytes.AsSpan().StartsWith((ReadOnlySpan<byte>)[0xEF, 0xBB, 0xBF]) ? 3 : 0;
        return Universal(new UTF8Encoding(false, throwOnInvalidBytes: true).GetString(bytes, skip, bytes.Length - skip));
    }

    /// <summary>An I/O error worded as Python words the common ones.</summary>
    public static string OsError(Exception e, string path) => e switch
    {
        FileNotFoundException or DirectoryNotFoundException => $"[Errno 2] No such file or directory: {Repr(path)}",
        UnauthorizedAccessException => $"[Errno 13] Permission denied: {Repr(path)}",
        _ => e.Message,
    };

    [GeneratedRegex(@"^\s*(?:export\s+)?([A-Za-z_][\p{L}\p{N}_]*)\s*=\s*(.*?)\s*$")]
    private static partial Regex DotenvLine();

    /// <summary>The variables of the .env files from the root down to the .http file's folder (nearer ones win).</summary>
    public static Dictionary<string, string> ReadDotenv(string httpPath, string? root)
    {
        var values = new Dictionary<string, string>();
        foreach (var dir in Enumerable.Reverse(EnvDirs(httpPath, root)))
        {
            string text;
            try
            {
                text = ReadText(Path.Combine(dir, ".env"));
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                continue;
            }
            foreach (var line in text.Split('\n'))
            {
                var m = DotenvLine().Match(line);
                if (m.Success)
                {
                    var v = m.Groups[2].Value;
                    if (v.Length >= 2 && v[0] == v[^1] && v[0] is '"' or '\'')
                    {
                        v = v[1..^1];
                    }
                    values[m.Groups[1].Value] = v;
                }
            }
        }
        return values;
    }

    // ───────────── variables ─────────────

    [GeneratedRegex(@"\.([^.\[\]]+)|\[\s*'([^']*)'\s*\]|\[\s*""([^""]*)""\s*\]|\[\s*(-?\d+|\*)\s*\]")]
    private static partial Regex JsonPathToken();

    /// <summary>A small JSONPath: $.a.b[0].c, $['a'], $.items[*].id (a list), * for the whole value.</summary>
    public static object? JsonPath(object? value, string path)
    {
        if (path is "*" or "$")
        {
            return value;
        }
        if (!path.StartsWith('$'))
        {
            throw new VariableError($"not a JSONPath: {path}");
        }
        var current = new List<object?> { value };
        var multi = false;
        foreach (Match token in JsonPathToken().Matches(path[1..]))
        {
            var key = token.Groups[1].Value is { Length: > 0 } dotted ? dotted : token.Groups[2].Value is { Length: > 0 } single ? single : token.Groups[3].Value;
            var index = token.Groups[4].Value;
            var next = new List<object?>();
            foreach (var v in current)
            {
                if (index == "*" || key == "*")
                {
                    multi = true;
                    if (v is OrderedDictionary<string, object?> map)
                    {
                        next.AddRange(map.Values);
                    }
                    else if (v is List<object?> list)
                    {
                        next.AddRange(list);
                    }
                }
                else if (index.Length > 0)
                {
                    if (v is List<object?> list)
                    {
                        long n;
                        try
                        {
                            n = Int(index);
                        }
                        catch (OverflowException)
                        {
                            continue;
                        }
                        if (-list.Count <= n && n < list.Count)
                        {
                            next.Add(list[(int)(n < 0 ? n + list.Count : n)]);
                        }
                    }
                }
                else if (v is OrderedDictionary<string, object?> map && map.TryGetValue(key, out var item))
                {
                    next.Add(item);
                }
            }
            current = next;
        }
        if (current.Count == 0)
        {
            throw new VariableError($"{path} matches nothing in the response");
        }
        return multi ? current : current[0];
    }

    /// <summary>Python's as_text: strings as they are, None empty, anything else as JSON.</summary>
    public static string AsText(object? value) => value switch
    {
        string s => s,
        null => "",
        _ => PyJson.Dumps(value),
    };

    /// <summary>datetime.timedelta for an amount of a unit (ms, s, m, h, d, w, M = 30 days, y = 365 days).</summary>
    public static TimeSpan Offset(long amount, string unit)
    {
        try
        {
            return unit switch
            {
                "s" => TimeSpan.FromSeconds(amount),
                "m" => TimeSpan.FromMinutes(amount),
                "h" => TimeSpan.FromHours(amount),
                "d" => TimeSpan.FromDays(amount),
                "w" => TimeSpan.FromDays(7.0 * amount),
                "ms" => TimeSpan.FromMilliseconds(amount),
                "M" => TimeSpan.FromDays(30.0 * amount),
                "y" => TimeSpan.FromDays(365.0 * amount),
                _ => throw new VariableError($"unknown time unit {unit}"),
            };
        }
        catch (Exception e) when (e is OverflowException or ArgumentException)
        {
            throw new VariableError("date value out of range");
        }
    }

    /// <summary>int(text), with Python's error as a <see cref="VariableError"/>.</summary>
    static long ToInt(string text)
    {
        try
        {
            return Int(text);
        }
        catch (Exception e) when (e is FormatException or OverflowException)
        {
            throw new VariableError(e is FormatException ? e.Message : "Python int too large to convert to C long");
        }
    }

    /// <summary>{{$datetime …}}: iso8601, rfc1123 or a "YYYY-MM-DD"-style format, optionally with an offset (1 d).</summary>
    public static string FormatDateTime(DateTimeOffset moment, IReadOnlyList<string> args, string zoneName)
    {
        var format = args.Count > 0 ? args[0] : "iso8601";
        if (args.Count >= 3)
        {
            try
            {
                moment += Offset(ToInt(args[1]), args[2]);
            }
            catch (ArgumentOutOfRangeException)
            {
                throw new VariableError("date value out of range");
            }
        }
        if (format == "iso8601")
        {
            return IsoFormat(moment).Replace("+00:00", "Z");
        }
        if (format == "rfc1123")
        {
            return Strftime(moment.ToUniversalTime(), "%a, %d %b %Y %H:%M:%S GMT", "UTC");
        }
        format = format.Trim('"', '\'');
        // .NET / Day.js tokens to strftime, for the common ones.
        foreach (var (a, b) in new[] { ("YYYY", "%Y"), ("yyyy", "%Y"), ("MM", "%m"), ("DD", "%d"), ("dd", "%d"), ("HH", "%H"), ("mm", "%M"), ("ss", "%S") })
        {
            format = format.Replace(a, b, StringComparison.Ordinal);
        }
        return Strftime(moment, format, zoneName);
    }

    [GeneratedRegex(@"\{\{(.+?)\}\}")]
    private static partial Regex Placeholder();

    [GeneratedRegex(@"""[^""]*""|'[^']*'|[^\s\x1c-\x1f]+")]
    private static partial Regex DynamicPart();

    [GeneratedRegex(@"^([\p{L}\p{N}_-]+)\.(response|request)\.(body|headers)(?:\.(.+))?$")]
    private static partial Regex RequestVariable();

    /// <summary>Fills in {{variables}}: file variables, then the environment, then named responses; {{$…}} are dynamic.</summary>
    public sealed class Resolver(IReadOnlyDictionary<string, string> fileVariables, IReadOnlyDictionary<string, object?> environment,
        IReadOnlyDictionary<string, NamedResponse> responses, string httpPath, string? root)
    {
        Dictionary<string, string>? dotenv;

        /// <summary>The clock ({{$timestamp}}, {{$datetime}}); tests fix it.</summary>
        public Func<DateTimeOffset> Now { get; init; } = () => DateTimeOffset.Now;

        string Dynamic(string name, List<string> args)
        {
            switch (name)
            {
                case "$guid" or "$uuid" or "$random.uuid":
                    return Guid.NewGuid().ToString();
                case "$randomInt" or "$random.integer":
                    var (low, high) = args.Count >= 2 ? (ToInt(args[0]), ToInt(args[1])) : (0L, 1000L);
                    return (high > low ? Random.Shared.NextInt64(low, high) : low).ToString(CultureInfo.InvariantCulture);
                case "$timestamp":
                    var seconds = Now().ToUnixTimeMilliseconds() / 1000.0;
                    if (args.Count >= 2)
                    {
                        seconds += Offset(ToInt(args[0]), args[1]).TotalSeconds;
                    }
                    return ((BigInteger)Math.Truncate(seconds)).ToString(CultureInfo.InvariantCulture);
                case "$isoTimestamp":
                    return IsoFormat(Now().ToUniversalTime()).Replace("+00:00", "Z");
                case "$datetime":
                    return FormatDateTime(Now().ToUniversalTime(), args, "UTC");
                case "$localDatetime":
                    var local = Now().ToLocalTime();
                    var zone = TimeZoneInfo.Local;
                    return FormatDateTime(local, args, zone.IsDaylightSavingTime(local) ? zone.DaylightName : zone.StandardName);
                case "$processEnv":
                    {
                        if (args.Count == 0)
                        {
                            throw new VariableError("$processEnv needs a variable name");
                        }
                        var key = args[0];
                        if (key.StartsWith('%'))  // %name: the name is itself a variable (REST Client)
                        {
                            key = Lookup(key[1..], 0);
                        }
                        return Environment.GetEnvironmentVariable(key) ?? throw new VariableError($"environment variable {key} isn't set");
                    }
                case "$dotenv":
                    {
                        dotenv ??= ReadDotenv(httpPath, root);
                        var key = args.Count > 0 ? args[0] : "";
                        if (key.StartsWith('%'))
                        {
                            key = Lookup(key[1..], 0);
                        }
                        return dotenv.TryGetValue(key, out var value) ? value : throw new VariableError($"{key} isn't in a .env file");
                    }
            }
            throw new VariableError($"unknown variable {name}");
        }

        string? RequestValue(string expression)
        {
            // name.response.body.<path> | name.response.headers.<Header>
            var m = RequestVariable().Match(expression);
            if (!m.Success)
            {
                return null;
            }
            var (name, part, kind) = (m.Groups[1].Value, m.Groups[2].Value, m.Groups[3].Value);
            var rest = m.Groups[4].Success ? m.Groups[4].Value : null;
            if (!responses.TryGetValue(name, out var response))
            {
                throw new VariableError($"send the request named {name} first");
            }
            if (kind == "headers")
            {
                var wanted = (rest ?? "").ToLowerInvariant();
                foreach (var (k, v) in part == "request" ? response.RequestHeaders : response.Headers)
                {
                    if (k.ToLowerInvariant() == wanted)
                    {
                        return v;
                    }
                }
                throw new VariableError($"{name}'s {part} has no header {rest ?? "None"}");
            }
            var body = part == "response" ? response.Text : response.RequestBody;
            if (string.IsNullOrEmpty(rest) || rest == "*")
            {
                return body ?? "";
            }
            object? data;
            try
            {
                data = PyJson.Loads(body ?? "");
            }
            catch (JsonDecodeError)
            {
                throw new VariableError($"{name}'s {part} body isn't JSON");
            }
            return AsText(JsonPath(data, rest));
        }

        public string Lookup(string expression, int depth)
        {
            expression = Strip(expression);
            if (depth > 20)
            {
                throw new VariableError($"{{{{{expression}}}}} refers to itself");
            }
            if (expression.StartsWith('$'))
            {
                var parts = DynamicPart().Matches(expression).Select(p => p.Value).ToList();
                return Dynamic(parts[0], [.. parts.Skip(1).Select(p => p[0] is '"' or '\'' ? p.Trim('"', '\'') : p)]);
            }
            if (fileVariables.TryGetValue(expression, out var raw))
            {
                return Resolve(raw, depth + 1)!;
            }
            if (environment.TryGetValue(expression, out var value))
            {
                return Resolve(AsText(value), depth + 1)!;
            }
            return RequestValue(expression) ?? throw new VariableError($"{{{{{expression}}}}} isn't defined");
        }

        public string? Resolve(string? text, int depth = 0) =>
            text == null ? null : Placeholder().Replace(text, m => Lookup(m.Groups[1].Value, depth));
    }

    // ───────────── building a request ─────────────

    /// <summary>The request with its variables filled in: method, URL, headers, body bytes (null: none).</summary>
    public static (string Method, string Url, List<(string Name, string Value)> Headers, byte[]? Body) Build(HttpRequestEntry request, Resolver resolver, string httpPath)
    {
        var url = Strip(resolver.Resolve(request.Url)!);
        if (!Regex.IsMatch(url, "^https?://", I))
        {
            url = (url.StartsWith("localhost", StringComparison.Ordinal) || url.StartsWith("127.", StringComparison.Ordinal) ? "http://" : "https://") + url.TrimStart('/');
        }
        var headers = request.Headers.Select(h => (h.Name, resolver.Resolve(h.Value)!)).ToList();
        byte[]? body = null;
        if (request.BodyFile != null)
        {
            var path = resolver.Resolve(request.BodyFile)!;
            if (!Path.IsPathRooted(path))
            {
                path = Path.Combine(Path.GetDirectoryName(httpPath) ?? "", path);
            }
            try
            {
                body = File.ReadAllBytes(path);
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                throw new IOException(OsError(e, path), e);
            }
        }
        else if (request.Body != null)
        {
            var text = resolver.Resolve(request.Body)!;
            var contentType = headers.FirstOrDefault(h => h.Name.ToLowerInvariant() == "content-type").Item2 ?? "";
            if (contentType.Contains("x-www-form-urlencoded", StringComparison.Ordinal))
            {
                // Form bodies may be written on several lines (a=1\n&b=2).
                text = string.Concat(SplitLines(text).Select(Strip));
            }
            body = Encoding.UTF8.GetBytes(text);
        }
        return (request.Method, url, headers, body);
    }

    public static bool IsLocal(string? host) =>
        host is "localhost" or "127.0.0.1" or "::1" or "[::1]" || (host ?? "").EndsWith(".localhost", StringComparison.Ordinal);

    // ───────────── responses ─────────────

    [GeneratedRegex(@"charset=([\p{L}\p{N}_.-]+)", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant)]
    private static partial Regex Charset();

    static string? HeaderValue(List<(string Name, string Value)> headers, string name) =>
        headers.Where(h => h.Name.ToLowerInvariant() == name).Select(h => h.Value).FirstOrDefault();

    public static string CharsetOf(List<(string Name, string Value)> headers)
    {
        foreach (var (k, v) in headers)
        {
            if (k.ToLowerInvariant() == "content-type" && Charset().Match(v) is { Success: true } m)
            {
                return m.Groups[1].Value;
            }
        }
        return "utf-8";
    }

    public static string ContentType(List<(string Name, string Value)> headers) =>
        HeaderValue(headers, "content-type") is { } v ? Strip(v.Split(';')[0]).ToLowerInvariant() : "";

    public static bool IsText(string contentType, byte[] data)
    {
        if (contentType.StartsWith("text/", StringComparison.Ordinal)
            || new[] { "json", "xml", "javascript", "html", "yaml", "csv", "x-www-form-urlencoded", "problem" }.Any(t => contentType.Contains(t, StringComparison.Ordinal)))
        {
            return true;
        }
        if (contentType.Length > 0)
        {
            return false;
        }
        return !data.AsSpan(0, Math.Min(2048, data.Length)).Contains((byte)0);
    }

    static readonly bool CodePages = Register();

    static bool Register()
    {
        Encoding.RegisterProvider(CodePagesEncodingProvider.Instance);
        return true;
    }

    static Encoding? EncodingFor(string name)
    {
        _ = CodePages;
        var key = name.ToLowerInvariant().Replace('_', '-');
        key = key switch
        {
            "utf8" or "u8" or "utf" or "cp65001" => "utf-8",
            "latin1" or "latin-1" or "l1" or "iso8859-1" or "8859" => "iso-8859-1",
            "ascii" or "us-ascii" or "646" => "us-ascii",
            "utf16" => "utf-16",
            "utf32" => "utf-32",
            _ when key.StartsWith("cp", StringComparison.Ordinal) && int.TryParse(key[2..], out var page) => "windows-" + page,
            _ => key,
        };
        try
        {
            return Encoding.GetEncoding(key, EncoderFallback.ReplacementFallback, new DecoderReplacementFallback("�"));
        }
        catch (ArgumentException)
        {
            return null;
        }
    }

    /// <summary>bytes.decode(charset, errors="replace"); an unknown charset reads as UTF-8.</summary>
    public static string Decode(byte[] data, string charset)
    {
        var encoding = EncodingFor(charset) ?? EncodingFor("utf-8")!;
        var key = charset.ToLowerInvariant().Replace('_', '-');
        // Python's utf-16 / utf-32 codecs read the byte order from a BOM and drop it (no BOM: little-endian).
        if (key is "utf-16" or "utf16" or "u16")
        {
            var (big, skip) = data.AsSpan().StartsWith((ReadOnlySpan<byte>)[0xFE, 0xFF]) ? (true, 2) : data.AsSpan().StartsWith((ReadOnlySpan<byte>)[0xFF, 0xFE]) ? (false, 2) : (false, 0);
            return EncodingFor(big ? "utf-16be" : "utf-16le")!.GetString(data, skip, data.Length - skip);
        }
        if (key is "utf-32" or "utf32" or "u32")
        {
            var (big, skip) = data.AsSpan().StartsWith((ReadOnlySpan<byte>)[0, 0, 0xFE, 0xFF]) ? (true, 4) : data.AsSpan().StartsWith((ReadOnlySpan<byte>)[0xFF, 0xFE, 0, 0]) ? (false, 4) : (false, 0);
            return EncodingFor(big ? "utf-32be" : "utf-32le")!.GetString(data, skip, data.Length - skip);
        }
        return encoding.GetString(data);
    }

    /// <summary>The body as text (pretty JSON when it is JSON), or null for binary content.</summary>
    public static string? BodyText(HttpResult response, bool pretty = true)
    {
        var data = response.Data;
        var contentType = ContentType(response.Headers);
        if (!IsText(contentType, data))
        {
            return null;
        }
        var text = Decode(data, CharsetOf(response.Headers));
        var start = LStrip(text);
        if (pretty && data.Length <= MaxPretty && (contentType.Contains("json", StringComparison.Ordinal) || (contentType.Length == 0 && (start.Length == 0 || start[0] is '{' or '['))))
        {
            try
            {
                return PyJson.DumpsIndented(PyJson.Loads(text));
            }
            catch (JsonDecodeError)
            {
            }
        }
        return text;
    }

    /// <summary>A byte count as "512 B", "1.5 KB", "2.0 MB" (rounded half to even, as Python's format does).</summary>
    public static string Size(long n)
    {
        if (n < 1024)
        {
            return $"{n} B";
        }
        var (shift, unit) = n < 1024L * 1024 ? (10, "KB") : (20, "MB");
        // n / 2^shift to one decimal, exactly: tenths = round(n * 10 / 2^shift).
        var scaled = (BigInteger)n * 10;
        var tenths = scaled >> shift;
        var remainder = scaled - (tenths << shift);
        var half = BigInteger.One << (shift - 1);
        if (remainder > half || (remainder == half && !tenths.IsEven))
        {
            tenths++;
        }
        return $"{tenths / 10}.{tenths % 10} {unit}";
    }

    public static string Summary(HttpResult response) =>
        $"{response.Status} {response.Reason} · {Math.Round(response.Elapsed, MidpointRounding.ToEven).ToString("0", CultureInfo.InvariantCulture)} ms · {Size(response.Data.Length)}".Replace("  ", " ");

    /// <summary>The text of the editor a response opens in.</summary>
    public static string ResponseDocument(string method, string url, HttpResult response, string? text)
    {
        var lines = new List<string> { $"{method} {url}", $"# {Summary(response)}" };
        if (response.Url != url)
        {
            lines.Add($"# redirected to {response.Url}");
        }
        lines.Add("");
        lines.Add(RStrip($"{response.Version} {response.Status} {response.Reason}"));
        lines.AddRange(response.Headers.Select(h => $"{h.Name}: {h.Value}"));
        lines.Add("");
        if (text == null)
        {
            var type = ContentType(response.Headers);
            lines.Add($"# {Size(response.Data.Length)} of {(type.Length > 0 ? type : "binary data")}, saved next to this file");
        }
        else
        {
            lines.Add(text);
        }
        return string.Join("\n", lines) + "\n";
    }

    [GeneratedRegex(@"[^\p{L}\p{N}_.-]+")]
    private static partial Regex NotSlug();

    public static string Slug(string text)
    {
        var slug = NotSlug().Replace(text, "-").Trim('-');
        slug = slug.Length > 60 ? slug[..60] : slug;
        return slug.Length > 0 ? slug : "request";
    }

    static readonly Dictionary<string, string> ExtensionByType = new()
    {
        ["image/png"] = ".png", ["image/jpeg"] = ".jpg", ["image/gif"] = ".gif", ["image/webp"] = ".webp",
        ["application/pdf"] = ".pdf", ["application/zip"] = ".zip", ["image/svg+xml"] = ".svg",
    };

    /// <summary>
    /// Writes the response document (and a binary body next to it) into <paramref name="folder"/>, keeps the newest
    /// <see cref="KeepResponses"/>; the document's path. Written in text mode as Python did: CRLF on Windows.
    /// </summary>
    public static string SaveResponse(string folder, string name, string document, HttpResult response, bool binary)
    {
        Directory.CreateDirectory(folder);
        var stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture);
        var baseName = $"{stamp}-{Slug(name)}";
        var n = 1;
        while (Path.Exists(Path.Combine(folder, baseName + ".http")))
        {
            n++;
            baseName = $"{stamp}-{Slug(name)}-{n}";
        }
        var path = Path.Combine(folder, baseName + ".http");
        File.WriteAllText(path, OperatingSystem.IsWindows() ? document.Replace("\n", "\r\n") : document, new UTF8Encoding(false));
        if (binary)
        {
            var ext = ExtensionByType.GetValueOrDefault(ContentType(response.Headers), ".bin");
            File.WriteAllBytes(Path.Combine(folder, baseName + ext), response.Data);
        }
        Prune(folder);
        return path;
    }

    /// <summary>Deletes all but the newest <see cref="KeepResponses"/> responses (and their binaries).</summary>
    public static void Prune(string folder)
    {
        List<string> names;
        try
        {
            names = [.. Directory.EnumerateFileSystemEntries(folder).Select(p => Path.GetFileName(p))];
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return;
        }
        names.Sort(StringComparer.Ordinal);
        var responses = names.Where(n => n.EndsWith(".http", StringComparison.Ordinal)).ToList();
        foreach (var old in responses.Take(Math.Max(0, responses.Count - KeepResponses)))
        {
            var stem = old[..^5];
            foreach (var n in names)
            {
                if (n == old || (n.StartsWith(stem + ".", StringComparison.Ordinal) && !n.EndsWith(".http", StringComparison.Ordinal)))
                {
                    try
                    {
                        File.Delete(Path.Combine(folder, n));
                    }
                    catch (Exception e) when (e is IOException or UnauthorizedAccessException)
                    {
                    }
                }
            }
        }
    }

    /// <summary>The request as a curl command line (POSIX shell quoting).</summary>
    public static string CurlCommand(string method, string url, List<(string Name, string Value)> headers, byte[]? body)
    {
        static string Q(string s) => "'" + s.Replace("'", "'\\''") + "'";
        var parts = new List<string> { "curl", "-X", method, Q(url) };
        foreach (var (k, v) in headers)
        {
            parts.AddRange(["-H", Q($"{k}: {v}")]);
        }
        if (body != null)
        {
            parts.AddRange(["--data-binary", Q(Decode(body, "utf-8"))]);
        }
        return string.Join(" ", parts);
    }

    /// <summary>The .http / .rest files under <paramref name="root"/>, as os.walk finds them (a folder's files, then its folders).</summary>
    public static List<string> FindHttpFiles(string root)
    {
        var found = new List<string>();
        Walk(root, found);
        return found;
    }

    static void Walk(string directory, List<string> found)
    {
        List<FileSystemInfo> entries;
        try
        {
            entries = [.. new DirectoryInfo(directory).EnumerateFileSystemInfos()];
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or System.Security.SecurityException)
        {
            return;
        }
        bool IsDir(FileSystemInfo e)
        {
            if (e is DirectoryInfo)
            {
                return true;
            }
            // A link to a folder counts as a folder (listed, not walked into).
            return e.LinkTarget != null && Directory.Exists(e.FullName);
        }
        var dirs = entries.Where(IsDir).Select(e => e.Name).Where(d => !SkipDirs.Contains(d) && !d.StartsWith('.')).Order(StringComparer.Ordinal).ToList();
        var files = entries.Where(e => !IsDir(e)).Select(e => e.Name).Order(StringComparer.Ordinal);
        foreach (var name in files)
        {
            if (IsHttpFile(name))
            {
                found.Add(Path.Combine(directory, name));
            }
        }
        foreach (var dir in dirs)
        {
            var full = Path.Combine(directory, dir);
            if (new DirectoryInfo(full).LinkTarget == null)
            {
                Walk(full, found);
            }
        }
    }
}
