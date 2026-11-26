using System.Diagnostics;
using System.Net.Http.Headers;
using System.Net.Sockets;
using System.Security.Authentication;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace ByteForge.HttpClientModule;

/// <summary>A request that got no response (connection refused, timeout, TLS, bad URL): the reason, for the log.</summary>
public sealed class SendError(string message, Exception? inner = null) : Exception(message, inner);

/// <summary>
/// Sends a request the way the Python module did with urllib: 4xx/5xx are responses too; redirects (301, 302, 303,
/// 307, 308) are followed as urllib follows them (a POST becomes a GET on 301-303, other methods are not redirected,
/// at most 10 hops); Content-Type application/x-www-form-urlencoded for a body without one; User-Agent
/// ByteForge-HTTP-Client unless the request sets one; the timeout applies to connecting and to each read.
/// </summary>
public static class Sender
{
    const string InfiniteLoop = "The HTTP server returned a redirect error that would lead to an infinite loop.\nThe last 30x error message was:\n";
    const int MaxRedirections = 10;
    const int MaxRepeats = 4;

    static bool IsFalse(JsonObject settings, string key) => settings[key] is JsonValue v && v.GetValueKind() == JsonValueKind.False;

    static double Timeout(JsonObject settings)
    {
        double seconds = 0;
        if (settings["http.timeout"] is JsonValue v)
        {
            switch (v.GetValueKind())
            {
                case JsonValueKind.Number:
                    seconds = v.GetValue<double>();
                    break;
                case JsonValueKind.True:
                    seconds = 1;
                    break;
                case JsonValueKind.String:
                    double.TryParse(v.GetValue<string>(), System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out seconds);
                    break;
            }
        }
        return seconds > 0 ? seconds : 30;
    }

    /// <summary>The handler requests go through when none is injected: no automatic redirects or decompression, the system proxy.</summary>
    public static HttpMessageHandler DefaultHandler(bool acceptAnyCertificate, TimeSpan connectTimeout) => DefaultHandler(acceptAnyCertificate, connectTimeout, null);

    /// <summary><see cref="DefaultHandler(bool, TimeSpan)"/>; with <paramref name="heads"/>, the head of each response as received.</summary>
    static SocketsHttpHandler DefaultHandler(bool acceptAnyCertificate, TimeSpan connectTimeout, ResponseHeads? heads)
    {
        var handler = new SocketsHttpHandler
        {
            AllowAutoRedirect = false,
            AutomaticDecompression = System.Net.DecompressionMethods.None,
            UseCookies = false,
            ConnectTimeout = connectTimeout,
        };
        if (acceptAnyCertificate)
        {
            handler.SslOptions.RemoteCertificateValidationCallback = (_, _, _, _) => true;
        }
        if (heads != null)
        {
            // Above TLS, so this sees what http.client parsed: header order, names and values exactly as sent.
            handler.PlaintextStreamFilter = (context, _) => ValueTask.FromResult<Stream>(new CaptureStream(context.PlaintextStream, heads));
        }
        return handler;
    }

    public static async Task<HttpResult> PerformAsync(string method, string url, List<(string Name, string Value)> headers, byte[]? body, JsonObject settings,
        Func<bool, TimeSpan, HttpMessageHandler>? handlers = null, CancellationToken cancel = default)
    {
        var timeout = TimeSpan.FromSeconds(Timeout(settings));
        Uri uri;
        try
        {
            uri = MakeUri(url);
        }
        catch (UriFormatException e)
        {
            throw new SendError(e.Message, e);
        }
        var host = uri.Host.ToLowerInvariant();
        var insecure = url.StartsWith("https", StringComparison.OrdinalIgnoreCase)
            && (IsFalse(settings, "http.verifyCertificates") || (HttpFiles.IsLocal(host) && !IsFalse(settings, "http.insecureLocalhost")));
        var follow = !IsFalse(settings, "http.followRedirects");

        // urllib's Request.add_header: one value per header name (the last), in the order the names first came.
        var sent = new List<(string Name, string Value)>();
        foreach (var (name, value) in headers)
        {
            var at = sent.FindIndex(h => string.Equals(h.Name, name, StringComparison.OrdinalIgnoreCase));
            if (at >= 0)
            {
                sent[at] = (sent[at].Name, value);
            }
            else
            {
                sent.Add((name, value));
            }
        }
        if (!sent.Any(h => h.Name.Equals("user-agent", StringComparison.OrdinalIgnoreCase)))
        {
            sent.Add(("User-Agent", "ByteForge-HTTP-Client"));
        }

        var heads = handlers == null ? new ResponseHeads() : null;
        var handler = handlers != null ? handlers(insecure, timeout) : DefaultHandler(insecure, timeout, heads);
        using var client = new HttpClient(handler, disposeHandler: true) { Timeout = System.Threading.Timeout.InfiniteTimeSpan };
        var watch = Stopwatch.StartNew();
        var visited = new Dictionary<string, int>();
        var currentUrl = url;
        var currentMethod = method;
        var currentBody = body;
        var defaultContentType = true;
        try
        {
            while (true)
            {
                using var request = new HttpRequestMessage(new HttpMethod(currentMethod), uri);
                AddHeaders(request, sent, currentBody, defaultContentType);
                HttpResponseMessage response;
                using (var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel))
                {
                    limit.CancelAfter(timeout);
                    try
                    {
                        response = await client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, limit.Token);
                    }
                    catch (OperationCanceledException) when (!cancel.IsCancellationRequested)
                    {
                        throw new SendError("timed out");
                    }
                }
                using (response)
                {
                    var status = (int)response.StatusCode;
                    var reason = response.ReasonPhrase ?? "";
                    var version = response.Version == System.Net.HttpVersion.Version10 ? "HTTP/1.0" : "HTTP/1.1";
                    var responseHeaders = Headers(response);
                    if (heads?.Take() is { } head && ParseHead(head) is { } raw)
                    {
                        (version, reason, responseHeaders) = raw;
                    }
                    var data = await ReadAll(response, timeout, cancel);
                    if (status is 301 or 302 or 303 or 307 or 308 && follow)
                    {
                        var location = Header(responseHeaders, "location") ?? Header(responseHeaders, "uri");
                        var redirect = location == null ? null : Redirect(currentUrl, location, status, currentMethod);
                        if (redirect is { Method: null })
                        {
                            if (redirect.Url.StartsWith("ftp:", StringComparison.OrdinalIgnoreCase))
                            {
                                throw new SendError($"redirected to {redirect.Url}, and FTP isn't supported");
                            }
                            return Result(status, $"{reason} - Redirection to url '{location}' is not allowed", version, responseHeaders, data, watch, redirect.Url);
                        }
                        if (redirect != null)
                        {
                            visited.TryGetValue(redirect.Url, out var seen);
                            if (seen >= MaxRepeats || visited.Count >= MaxRedirections)
                            {
                                return Result(status, InfiniteLoop + reason, version, responseHeaders, data, watch, currentUrl);
                            }
                            visited[redirect.Url] = seen + 1;
                            // The redirected request: no body, and no Content-Type / Content-Length of the first one.
                            sent.RemoveAll(h => h.Name.Equals("content-type", StringComparison.OrdinalIgnoreCase) || h.Name.Equals("content-length", StringComparison.OrdinalIgnoreCase));
                            (currentUrl, currentMethod, currentBody, defaultContentType) = (redirect.Url, redirect.Method, null, false);
                            uri = MakeUri(currentUrl);
                            continue;
                        }
                    }
                    return Result(status, reason, version, responseHeaders, data, watch, currentUrl);
                }
            }
        }
        catch (HttpRequestException e)
        {
            throw new SendError(Reason(e), e);
        }
        catch (Exception e) when (e is UriFormatException or InvalidOperationException or NotSupportedException or ArgumentException or IOException)
        {
            throw new SendError(Reason(e), e);
        }
    }

    static HttpResult Result(int status, string reason, string version, List<(string, string)> headers, byte[] data, Stopwatch watch, string url) =>
        new(status, reason, version, headers, data, watch.Elapsed.TotalMilliseconds, url);

    /// <summary>
    /// A response head as http.client reads it: version, reason (stripped) and the headers in order, values with
    /// the blanks after the colon removed (email's compat32 policy), Latin-1.
    /// </summary>
    public static (string Version, string Reason, List<(string Name, string Value)> Headers)? ParseHead(string head)
    {
        var lines = head.Split('\n').Select(l => l.TrimEnd('\r')).ToList();
        var parts = lines[0].Split((char[]?)null, 3, StringSplitOptions.RemoveEmptyEntries);
        if (parts.Length < 2 || !parts[0].StartsWith("HTTP/", StringComparison.Ordinal))
        {
            return null;
        }
        var reason = parts.Length > 2 ? Py.Strip(parts[2]) : "";
        var headers = new List<(string Name, string Value)>();
        foreach (var line in lines.Skip(1))
        {
            if (line.Length > 0 && line[0] is ' ' or '\t' && headers.Count > 0)
            {
                headers[^1] = (headers[^1].Name, headers[^1].Value + "\r\n" + line);
                continue;
            }
            var colon = line.IndexOf(':');
            if (colon > 0)
            {
                headers.Add((line[..colon], line[(colon + 1)..].TrimStart(' ', '\t')));
            }
        }
        return (parts[0] == "HTTP/1.0" ? "HTTP/1.0" : "HTTP/1.1", reason, headers);
    }

    /// <summary>
    /// A URI for HttpClient that keeps the path and query as written (urllib doesn't collapse "/a/../b" or re-escape),
    /// unless they hold characters a request line can't carry; those are escaped.
    /// </summary>
    static Uri MakeUri(string url)
    {
        var plain = url.All(c => c is > ' ' and < '\x7f');
        return plain ? new Uri(url, new UriCreationOptions { DangerousDisablePathAndQueryCanonicalization = true }) : new Uri(url, UriKind.Absolute);
    }

    sealed record Redirection(string Url, string? Method);

    /// <summary>
    /// urllib's HTTPRedirectHandler: where a redirect goes, or null when urllib doesn't follow it. A redirect to
    /// another scheme comes back without a method (urllib refuses it with its own message).
    /// </summary>
    static Redirection? Redirect(string from, string location, int status, string method)
    {
        Uri target;
        try
        {
            target = new Uri(new Uri(from), location.Replace(" ", "%20"));
        }
        catch (UriFormatException)
        {
            return null;
        }
        if (target.Scheme is not ("http" or "https"))
        {
            return new Redirection(location, null);
        }
        if (!((status is 301 or 302 or 303 or 307 or 308 && method is "GET" or "HEAD") || (status is 301 or 302 or 303 && method == "POST")))
        {
            return null;
        }
        return new Redirection(target.AbsoluteUri, method == "HEAD" ? "HEAD" : "GET");
    }

    static void AddHeaders(HttpRequestMessage request, List<(string Name, string Value)> headers, byte[]? body, bool defaultContentType)
    {
        if (body != null)
        {
            request.Content = new ByteArrayContent(body);
        }
        foreach (var (name, value) in headers)
        {
            if (name.Equals("content-length", StringComparison.OrdinalIgnoreCase))
            {
                continue;  // HttpClient writes it from the body
            }
            if (name.Equals("host", StringComparison.OrdinalIgnoreCase))
            {
                request.Headers.Host = value;
                continue;
            }
            if (!request.Headers.TryAddWithoutValidation(name, value))
            {
                request.Content ??= new ByteArrayContent([]);
                request.Content.Headers.TryAddWithoutValidation(name, value);
            }
        }
        if (body != null && defaultContentType && !headers.Any(h => h.Name.Equals("content-type", StringComparison.OrdinalIgnoreCase)))
        {
            request.Content!.Headers.ContentType = new MediaTypeHeaderValue("application/x-www-form-urlencoded");
        }
        // What Python's http.client sends on its own.
        if (!headers.Any(h => h.Name.Equals("accept-encoding", StringComparison.OrdinalIgnoreCase)))
        {
            request.Headers.TryAddWithoutValidation("Accept-Encoding", "identity");
        }
        request.Headers.ConnectionClose = true;
    }

    /// <summary>The response's headers, each value on its own; content headers after the others.</summary>
    static List<(string, string)> Headers(HttpResponseMessage response)
    {
        var list = new List<(string, string)>();
        foreach (var header in response.Headers.NonValidated)
        {
            foreach (var value in header.Value)
            {
                list.Add((header.Key, value));
            }
        }
        foreach (var header in response.Content.Headers.NonValidated)
        {
            foreach (var value in header.Value)
            {
                list.Add((header.Key, value));
            }
        }
        return list;
    }

    static string? Header(List<(string Name, string Value)> headers, string name) =>
        headers.Where(h => h.Name.Equals(name, StringComparison.OrdinalIgnoreCase)).Select(h => h.Value).FirstOrDefault();

    /// <summary>The whole body; each read may take up to <paramref name="timeout"/>, as a socket timeout would.</summary>
    static async Task<byte[]> ReadAll(HttpResponseMessage response, TimeSpan timeout, CancellationToken cancel)
    {
        using var buffer = new MemoryStream();
        await using var stream = await response.Content.ReadAsStreamAsync(cancel);
        var chunk = new byte[81920];
        while (true)
        {
            using var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel);
            limit.CancelAfter(timeout);
            int read;
            try
            {
                read = await stream.ReadAsync(chunk, limit.Token);
            }
            catch (OperationCanceledException) when (!cancel.IsCancellationRequested)
            {
                throw new SendError("timed out");
            }
            if (read == 0)
            {
                return buffer.ToArray();
            }
            buffer.Write(chunk, 0, read);
        }
    }

    /// <summary>Why a request failed, as short as urllib's reason: the socket or TLS error when there is one.</summary>
    static string Reason(Exception e)
    {
        for (var inner = e; inner != null; inner = inner.InnerException)
        {
            if (inner is SocketException or AuthenticationException)
            {
                return inner.Message;
            }
            if (inner is OperationCanceledException or TimeoutException)
            {
                return "timed out";
            }
        }
        return e.Message;
    }
}

/// <summary>The last response head (status line and headers, Latin-1) read through a <see cref="CaptureStream"/>.</summary>
sealed class ResponseHeads
{
    readonly Lock gate = new();
    string? last;

    public void Add(string head)
    {
        lock (gate)
        {
            last = head;
        }
    }

    public string? Take()
    {
        lock (gate)
        {
            var head = last;
            last = null;
            return head;
        }
    }
}

/// <summary>A connection's plaintext stream that notes the head of the response read through it (1xx heads skipped).</summary>
sealed class CaptureStream(Stream inner, ResponseHeads heads) : Stream
{
    const int MaxHead = 1 << 20;
    MemoryStream? pending = new();

    void Observe(ReadOnlySpan<byte> data)
    {
        if (pending == null || data.IsEmpty)
        {
            return;
        }
        pending.Write(data);
        while (pending != null)
        {
            var bytes = pending.GetBuffer().AsSpan(0, (int)pending.Length);
            var end = bytes.IndexOf("\r\n\r\n"u8);
            if (end < 0)
            {
                if (pending.Length > MaxHead)
                {
                    pending = null;
                }
                return;
            }
            var head = System.Text.Encoding.Latin1.GetString(bytes[..end]);
            var rest = bytes[(end + 4)..].ToArray();
            var space = head.IndexOf(' ');
            if (head.StartsWith("HTTP/", StringComparison.Ordinal) && space > 0 && space + 1 < head.Length && head[space + 1] == '1')
            {
                // An interim 1xx response: the real head follows.
                pending = new MemoryStream();
                pending.Write(rest);
                continue;
            }
            heads.Add(head);
            pending = null;
        }
    }

    public override int Read(byte[] buffer, int offset, int count)
    {
        var n = inner.Read(buffer, offset, count);
        Observe(buffer.AsSpan(offset, n));
        return n;
    }

    public override int Read(Span<byte> buffer)
    {
        var n = inner.Read(buffer);
        Observe(buffer[..n]);
        return n;
    }

    public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
    {
        var n = await inner.ReadAsync(buffer, cancellationToken);
        Observe(buffer.Span[..n]);
        return n;
    }

    public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) =>
        ReadAsync(buffer.AsMemory(offset, count), cancellationToken).AsTask();

    public override void Write(byte[] buffer, int offset, int count) => inner.Write(buffer, offset, count);

    public override void Write(ReadOnlySpan<byte> buffer) => inner.Write(buffer);

    public override ValueTask WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken cancellationToken = default) => inner.WriteAsync(buffer, cancellationToken);

    public override Task WriteAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) => inner.WriteAsync(buffer, offset, count, cancellationToken);

    public override void Flush() => inner.Flush();

    public override Task FlushAsync(CancellationToken cancellationToken) => inner.FlushAsync(cancellationToken);

    public override bool CanRead => inner.CanRead;
    public override bool CanWrite => inner.CanWrite;
    public override bool CanSeek => false;
    public override long Length => throw new NotSupportedException();
    public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
    public override void SetLength(long value) => throw new NotSupportedException();

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            inner.Dispose();
        }
        base.Dispose(disposing);
    }

    public override async ValueTask DisposeAsync()
    {
        await inner.DisposeAsync();
        GC.SuppressFinalize(this);
    }
}
