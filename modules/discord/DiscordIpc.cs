using System.Buffers.Binary;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace ByteForge.Discord;

/// <summary>A failure Discord reported (or a connection it closed); the message is shown as is.</summary>
public sealed class DiscordException(string message) : Exception(message);

/// <summary>
/// Discord's local RPC connection (a named pipe on Windows, a unix socket elsewhere). Every frame is an opcode
/// and the payload's length (both 32-bit little-endian) followed by the JSON payload. Opcodes: 0 handshake,
/// 1 frame, 2 close, 3 ping, 4 pong.
/// </summary>
public sealed class DiscordIpc(Stream stream, TimeSpan? timeout = null) : IDisposable
{
    public const int Handshake = 0, FrameOp = 1, Close = 2, Ping = 3, Pong = 4;

    /// <summary>How long one read or write may take before the connection counts as lost.</summary>
    public TimeSpan Timeout { get; } = timeout ?? TimeSpan.FromSeconds(5);

    /// <summary>One frame as sent over the wire.</summary>
    public static byte[] Frame(int op, JsonNode? payload)
    {
        var body = Encoding.UTF8.GetBytes(payload?.ToJsonString() ?? "null");
        var frame = new byte[8 + body.Length];
        BinaryPrimitives.WriteInt32LittleEndian(frame.AsSpan(0, 4), op);
        BinaryPrimitives.WriteInt32LittleEndian(frame.AsSpan(4, 4), body.Length);
        body.CopyTo(frame, 8);
        return frame;
    }

    /// <summary>
    /// Connects to the first candidate Discord answers on and shakes hands. Null when none answers; throws
    /// <see cref="DiscordException"/> when Discord refuses (an unknown client id, for example).
    /// </summary>
    public static async Task<DiscordIpc?> ConnectAsync(IEnumerable<string> candidates, Func<string, CancellationToken, Task<Stream?>> open, string clientId,
        CancellationToken cancel, TimeSpan? timeout = null)
    {
        foreach (var path in candidates)
        {
            Stream? stream;
            try
            {
                stream = await open(path, cancel);
            }
            catch (Exception e) when (IsConnectionError(e))
            {
                continue;
            }
            if (stream == null)
            {
                continue;
            }
            var ipc = new DiscordIpc(stream, timeout);
            int op;
            JsonObject data;
            try
            {
                await ipc.SendAsync(Handshake, new JsonObject { ["v"] = 1, ["client_id"] = clientId }, cancel);
                (op, data) = await ipc.ReceiveAsync(cancel);
            }
            catch (Exception e) when (IsConnectionError(e))
            {
                ipc.Dispose();
                continue;
            }
            if (op == Close || Text(data["evt"]) == "ERROR")
            {
                var message = Text(data["data"]?["message"]) ?? Text(data["message"]) ?? "Discord refused the connection";
                ipc.Dispose();
                throw new DiscordException(message);
            }
            return ipc;
        }
        return null;
    }

    /// <summary>Errors that mean the connection is gone (or never was).</summary>
    public static bool IsConnectionError(Exception e) =>
        e is IOException or SocketException or ObjectDisposedException or UnauthorizedAccessException or TimeoutException or DiscordException
            or JsonException or InvalidOperationException;

    public Task SendAsync(int op, JsonNode? payload, CancellationToken cancel) => Limited(async token =>
    {
        await stream.WriteAsync(Frame(op, payload), token);
        await stream.FlushAsync(token);
    }, cancel);

    /// <summary>The next frame that isn't a ping (pings are answered on the way).</summary>
    public async Task<(int Op, JsonObject Data)> ReceiveAsync(CancellationToken cancel)
    {
        while (true)
        {
            var header = new byte[8];
            await Read(header, cancel);
            var op = BinaryPrimitives.ReadInt32LittleEndian(header.AsSpan(0, 4));
            var length = BinaryPrimitives.ReadUInt32LittleEndian(header.AsSpan(4, 4));
            if (length > 64 * 1024 * 1024)
            {
                throw new DiscordException("Discord sent a frame that is too big");
            }
            var data = new JsonObject();
            if (length > 0)
            {
                var body = new byte[length];
                await Read(body, cancel);
                data = JsonNode.Parse(body) as JsonObject ?? [];
            }
            if (op == Ping)
            {
                await SendAsync(Pong, data, cancel);
                continue;
            }
            return (op, data);
        }
    }

    /// <summary>Sets (or with null clears) the activity of the process <paramref name="pid"/>, and waits for Discord's answer.</summary>
    public async Task SetActivityAsync(JsonObject? activity, int pid, CancellationToken cancel)
    {
        var nonce = Guid.NewGuid().ToString();
        await SendAsync(FrameOp, new JsonObject
        {
            ["cmd"] = "SET_ACTIVITY",
            ["args"] = new JsonObject { ["pid"] = pid, ["activity"] = activity },
            ["nonce"] = nonce,
        }, cancel);
        while (true)
        {
            var (op, data) = await ReceiveAsync(cancel);
            if (op == Close)
            {
                throw new DiscordException(Text(data["message"]) ?? "Discord closed the connection");
            }
            if (Text(data["nonce"]) == nonce)
            {
                if (Text(data["evt"]) == "ERROR")
                {
                    throw new DiscordException(Text(data["data"]?["message"]) ?? "SET_ACTIVITY failed");
                }
                return;
            }
        }
    }

    async Task Read(byte[] buffer, CancellationToken cancel)
    {
        try
        {
            await Limited(token => stream.ReadExactlyAsync(buffer, token).AsTask(), cancel);
        }
        catch (EndOfStreamException)
        {
            throw new DiscordException("Discord closed the connection");
        }
    }

    async Task Limited(Func<CancellationToken, Task> work, CancellationToken cancel)
    {
        using var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel);
        limit.CancelAfter(Timeout);
        try
        {
            await work(limit.Token);
        }
        catch (OperationCanceledException) when (!cancel.IsCancellationRequested)
        {
            throw new TimeoutException("Discord didn't answer in time");
        }
    }

    static string? Text(JsonNode? node) =>
        node is JsonValue value && value.GetValueKind() == JsonValueKind.String && value.GetValue<string>() is { Length: > 0 } text ? text : null;

    public void Dispose()
    {
        try
        {
            stream.Dispose();
        }
        catch (Exception e) when (IsConnectionError(e))
        {
        }
    }
}
