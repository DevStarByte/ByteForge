using System.Globalization;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace ByteForge.GithubActions;

/// <summary>A failed GitHub request: the HTTP status (0: GitHub couldn't be reached) and what to tell the user.</summary>
public sealed class ApiException(int status, string message) : Exception(message)
{
    public int Status { get; } = status;
}

/// <summary>
/// The GitHub REST API. GET answers are kept with their ETag and asked for again with If-None-Match (a 304 doesn't
/// count against the rate limit). Tests derive from it and override <see cref="RequestAsync"/> and <see cref="DownloadAsync"/>.
/// </summary>
public class GitHubApi(string? token = null, CancellationToken cancel = default)
{
    public const string Root = "https://api.github.com";

    static readonly HttpClient Client = new(new SocketsHttpHandler { AllowAutoRedirect = true }) { Timeout = Timeout.InfiniteTimeSpan };
    static readonly HttpClient NoRedirects = new(new SocketsHttpHandler { AllowAutoRedirect = false }) { Timeout = Timeout.InfiniteTimeSpan };

    readonly Dictionary<string, (string ETag, JsonNode? Data)> etags = [];

    /// <summary>The token (null or empty: none).</summary>
    public string? Token { get; } = token;

    public bool HasToken => !string.IsNullOrEmpty(Token);

    /// <summary>X-RateLimit-Remaining of the last answer.</summary>
    public int? Remaining { get; protected set; }

    /// <summary>X-RateLimit-Reset of the last answer (Unix seconds).</summary>
    public long? Reset { get; protected set; }

    void AddHeaders(HttpRequestMessage request)
    {
        request.Headers.TryAddWithoutValidation("Accept", "application/vnd.github+json");
        request.Headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");
        request.Headers.TryAddWithoutValidation("User-Agent", "ByteForge-GitHub-Actions");
        if (HasToken)
        {
            request.Headers.TryAddWithoutValidation("Authorization", $"Bearer {Token}");
        }
    }

    /// <summary>A request to the API (a path below https://api.github.com, or a full URL); its JSON answer.</summary>
    public virtual async Task<JsonNode?> RequestAsync(string method, string path, JsonNode? body = null)
    {
        var url = path.StartsWith("http", StringComparison.Ordinal) ? path : Root + path;
        using var request = new HttpRequestMessage(new HttpMethod(method), url);
        AddHeaders(request);
        (string ETag, JsonNode? Data)? cached = null;
        if (method == "GET")
        {
            lock (etags)
            {
                if (etags.TryGetValue(url, out var hit))
                {
                    cached = hit;
                }
            }
        }
        if (cached != null)
        {
            request.Headers.TryAddWithoutValidation("If-None-Match", cached.Value.ETag);
        }
        if (body != null)
        {
            request.Content = new StringContent(body.ToJsonString(), Encoding.UTF8);
            request.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json");
        }
        using var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel);
        limit.CancelAfter(TimeSpan.FromSeconds(20));
        try
        {
            using var response = await Client.SendAsync(request, limit.Token);
            NoteLimits(response);
            var raw = await response.Content.ReadAsByteArrayAsync(limit.Token);
            if (response.IsSuccessStatusCode)
            {
                var result = raw.Length > 0 ? JsonNode.Parse(raw) : null;
                if (method == "GET" && Header(response, "ETag") is { } etag)
                {
                    lock (etags)
                    {
                        etags[url] = (etag, result);
                    }
                }
                return result;
            }
            var status = (int)response.StatusCode;
            if (status == 304 && cached != null)
            {
                return cached.Value.Data;
            }
            throw new ApiException(status, Explain(status, Encoding.UTF8.GetString(raw), name => Header(response, name)));
        }
        catch (Exception e) when (e is HttpRequestException or IOException || (e is OperationCanceledException && !cancel.IsCancellationRequested))
        {
            throw new ApiException(0, $"can't reach GitHub: {Reason(e)}");
        }
    }

    /// <summary>A job log: the API answers with a redirect to a storage URL, which must be fetched without our token.</summary>
    public virtual async Task<string> DownloadAsync(string path)
    {
        Uri location;
        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, Root + path);
            AddHeaders(request);
            using var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel);
            limit.CancelAfter(TimeSpan.FromSeconds(20));
            using var response = await NoRedirects.SendAsync(request, limit.Token);
            var status = (int)response.StatusCode;
            if (response.IsSuccessStatusCode)
            {
                return Decode(await response.Content.ReadAsByteArrayAsync(limit.Token));
            }
            if (status is not (301 or 302 or 303 or 307 or 308))
            {
                var raw = await response.Content.ReadAsByteArrayAsync(limit.Token);
                throw new ApiException(status, Explain(status, Encoding.UTF8.GetString(raw), name => Header(response, name)));
            }
            location = new Uri(new Uri(Root + path), Header(response, "Location") ?? "");
        }
        catch (Exception e) when (e is HttpRequestException or IOException || (e is OperationCanceledException && !cancel.IsCancellationRequested))
        {
            throw new ApiException(0, $"can't reach GitHub: {Reason(e)}");
        }
        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, location);
            request.Headers.TryAddWithoutValidation("User-Agent", "ByteForge");
            using var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel);
            limit.CancelAfter(TimeSpan.FromSeconds(60));
            using var response = await Client.SendAsync(request, limit.Token);
            if (!response.IsSuccessStatusCode)
            {
                throw new ApiException(0, $"can't download the log: {response.ReasonPhrase}");
            }
            return Decode(await response.Content.ReadAsByteArrayAsync(limit.Token));
        }
        catch (Exception e) when (e is HttpRequestException or IOException || (e is OperationCanceledException && !cancel.IsCancellationRequested))
        {
            throw new ApiException(0, $"can't download the log: {Reason(e)}");
        }
    }

    static string Decode(byte[] raw) => new UTF8Encoding(false, false).GetString(raw);

    static string Reason(Exception e) => e is OperationCanceledException ? "timed out" : e.Message;

    static string? Header(HttpResponseMessage response, string name) =>
        response.Headers.TryGetValues(name, out var values) || response.Content.Headers.TryGetValues(name, out values) ? values.FirstOrDefault() : null;

    void NoteLimits(HttpResponseMessage response)
    {
        if (Header(response, "X-RateLimit-Remaining") is { } remaining && int.TryParse(remaining, CultureInfo.InvariantCulture, out var left))
        {
            Remaining = left;
            Reset = long.TryParse(Header(response, "X-RateLimit-Reset"), CultureInfo.InvariantCulture, out var reset) ? reset : 0;
        }
    }

    /// <summary>What to tell the user about a failed request (its status, body and headers).</summary>
    public string Explain(int status, string raw, Func<string, string?> header)
    {
        var message = "";
        try
        {
            message = JsonNode.Parse(raw).Str("message") ?? "";
        }
        catch (JsonException)
        {
        }
        if (status is 403 or 429 && header("X-RateLimit-Remaining") == "0")
        {
            var reset = long.TryParse(header("X-RateLimit-Reset"), CultureInfo.InvariantCulture, out var r) ? r : 0;
            var at = DateTimeOffset.FromUnixTimeSeconds(reset).ToLocalTime().ToString("HH:mm", CultureInfo.InvariantCulture);
            var hint = HasToken ? "" : " Set \"github.token\" (or GH_TOKEN) for 5000 an hour.";
            return $"GitHub's rate limit is used up until {at}.{hint}";
        }
        if (status == 401)
        {
            return "GitHub didn't accept the token (expired or revoked?).";
        }
        if (status == 404 && !HasToken)
        {
            return "repository not found: private repositories need a token (\"github.token\", GH_TOKEN or gh).";
        }
        if (status == 403 && !HasToken)
        {
            return $"{(message.Length > 0 ? message : "forbidden")}: this needs a token (\"github.token\", GH_TOKEN or gh).";
        }
        return message.Length > 0 ? message : $"HTTP {status}";
    }
}
