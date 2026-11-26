using System.Buffers.Binary;
using System.Text;
using System.Text.Json.Nodes;
using System.Threading.Channels;
using ByteForge.Modules.Testing;

namespace ByteForge.Discord.Tests;

/// <summary>One end of an in-memory, two-way byte stream.</summary>
sealed class MemoryPipe(Channel<byte[]> incoming, Channel<byte[]> outgoing) : Stream
{
    byte[] current = [];
    int offset;

    public static (MemoryPipe Client, MemoryPipe Server) Pair()
    {
        var up = Channel.CreateUnbounded<byte[]>();
        var down = Channel.CreateUnbounded<byte[]>();
        return (new MemoryPipe(down, up), new MemoryPipe(up, down));
    }

    public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
    {
        while (offset == current.Length)
        {
            if (!await incoming.Reader.WaitToReadAsync(cancellationToken))
            {
                return 0;
            }
            if (incoming.Reader.TryRead(out var next))
            {
                (current, offset) = (next, 0);
            }
        }
        var count = Math.Min(buffer.Length, current.Length - offset);
        current.AsMemory(offset, count).CopyTo(buffer);
        offset += count;
        return count;
    }

    public override ValueTask WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken cancellationToken = default)
    {
        if (!outgoing.Writer.TryWrite(buffer.ToArray()))
        {
            throw new IOException("the other end is closed");
        }
        return ValueTask.CompletedTask;
    }

    public override int Read(byte[] buffer, int offset, int count) => ReadAsync(buffer.AsMemory(offset, count)).AsTask().GetAwaiter().GetResult();
    public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) => ReadAsync(buffer.AsMemory(offset, count), cancellationToken).AsTask();
    public override void Write(byte[] buffer, int offset, int count) => WriteAsync(buffer.AsMemory(offset, count)).AsTask().GetAwaiter().GetResult();
    public override Task WriteAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) => WriteAsync(buffer.AsMemory(offset, count), cancellationToken).AsTask();
    public override void Flush() { }
    public override bool CanRead => true;
    public override bool CanWrite => true;
    public override bool CanSeek => false;
    public override long Length => throw new NotSupportedException();
    public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
    public override void SetLength(long value) => throw new NotSupportedException();

    protected override void Dispose(bool disposing)
    {
        outgoing.Writer.TryComplete();
        base.Dispose(disposing);
    }
}

/// <summary>
/// Stands in for the Discord app: answers the handshake (or refuses it), pings once before READY, and records
/// every SET_ACTIVITY.
/// </summary>
sealed class FakeDiscord
{
    public string? Refuse { get; init; }
    public Channel<JsonObject> Commands { get; } = Channel.CreateUnbounded<JsonObject>();
    public Channel<JsonObject> Pongs { get; } = Channel.CreateUnbounded<JsonObject>();
    public Channel<JsonObject> Handshakes { get; } = Channel.CreateUnbounded<JsonObject>();
    public int Connections;
    MemoryPipe? server;
    CancellationTokenSource? serving;

    public Stream Accept()
    {
        Interlocked.Increment(ref Connections);
        var (client, server) = MemoryPipe.Pair();
        this.server = server;
        serving = new CancellationTokenSource();
        var token = serving.Token;
        _ = Task.Run(() => Serve(server, token));
        return client;
    }

    /// <summary>Discord quits: the current connection ends.</summary>
    public void Hangup()
    {
        serving!.Cancel();
        server!.Dispose();
    }

    async Task Serve(MemoryPipe stream, CancellationToken cancel)
    {
        try
        {
            while (true)
            {
                var (op, data) = await ReadFrame(stream, cancel);
                switch (op)
                {
                    case 0:
                        await Handshakes.Writer.WriteAsync(data);
                        if (Refuse != null)
                        {
                            await WriteFrame(stream, 2, new JsonObject { ["code"] = 4000, ["message"] = Refuse });
                            stream.Dispose();
                            return;
                        }
                        await WriteFrame(stream, 3, new JsonObject { ["ping"] = 1 });
                        await WriteFrame(stream, 1, new JsonObject { ["cmd"] = "DISPATCH", ["evt"] = "READY", ["data"] = new JsonObject { ["v"] = 1 } });
                        break;
                    case 4:
                        await Pongs.Writer.WriteAsync(data);
                        break;
                    case 1:
                        await Commands.Writer.WriteAsync(data);
                        await WriteFrame(stream, 1, new JsonObject { ["cmd"] = "SOMETHING_ELSE", ["nonce"] = "other" });
                        await WriteFrame(stream, 1, new JsonObject { ["cmd"] = "SET_ACTIVITY", ["evt"] = null, ["nonce"] = data["nonce"]!.GetValue<string>(), ["data"] = new JsonObject() });
                        break;
                }
            }
        }
        catch (Exception e) when (e is EndOfStreamException or IOException or OperationCanceledException)
        {
        }
    }

    public static async Task<(int Op, JsonObject Data)> ReadFrame(Stream stream, CancellationToken cancel)
    {
        var header = new byte[8];
        await stream.ReadExactlyAsync(header, cancel);
        var body = new byte[BinaryPrimitives.ReadInt32LittleEndian(header.AsSpan(4))];
        await stream.ReadExactlyAsync(body, cancel);
        return (BinaryPrimitives.ReadInt32LittleEndian(header), (JsonObject)JsonNode.Parse(body)!);
    }

    public static async Task WriteFrame(Stream stream, int op, JsonObject data) => await stream.WriteAsync(DiscordIpc.Frame(op, data));
}

/// <summary>The module with Discord's IPC replaced by <see cref="FakeDiscord"/> (or by nothing).</summary>
sealed class TestDiscordModule(FakeDiscord? discord) : DiscordModule
{
    protected override IEnumerable<string> SocketCandidates() => ["fake-0", "fake-1"];

    protected override Task<Stream?> OpenAsync(string path, CancellationToken cancel) =>
        Task.FromResult(discord != null && path == "fake-1" ? discord.Accept() : null);

    protected override int ProcessId => 4242;
}

/// <summary>The real transport (named pipe / unix socket), opened directly.</summary>
sealed class TransportModule : DiscordModule
{
    public Task<Stream?> Open(string path) => OpenAsync(path, CancellationToken.None);
}

public sealed class DiscordTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-discord-").FullName;

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

    string ModuleDir => Path.Combine(dir, "modules", "discord");

    static async Task<T> Read<T>(Channel<T> channel)
    {
        using var limit = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        return await channel.Reader.ReadAsync(limit.Token);
    }

    static JsonObject Event(string name, string? path = null) => new() { ["type"] = "event", ["name"] = name, ["path"] = path };

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = ModuleHarness.Start(new TestDiscordModule(null), ModuleDir);
        var toggle = await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "discord.toggle");
        Assert.Equal("Pause or Resume Sharing", toggle["title"]!.GetValue<string>());
        Assert.Equal("Discord", toggle["category"]!.GetValue<string>());
        var reconnect = await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "discord.reconnect");
        Assert.Equal("Reconnect", reconnect["title"]!.GetValue<string>());
    }

    [Fact]
    public void Frames_are_opcode_and_length_little_endian_then_the_JSON()
    {
        var frame = DiscordIpc.Frame(1, new JsonObject { ["a"] = "ü" });
        var body = Encoding.UTF8.GetBytes("""{"a":"\u00FC"}""");
        Assert.Equal([1, 0, 0, 0, (byte)body.Length, 0, 0, 0, .. body], frame);
        Assert.Equal([3, 0, 0, 0, 4, 0, 0, 0, .. "null"u8.ToArray()], DiscordIpc.Frame(3, null));
    }

    [Fact]
    public async Task Shares_the_activity_after_the_handshake()
    {
        var discord = new FakeDiscord();
        var before = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        await using var module = ModuleHarness.Start(new TestDiscordModule(discord) { MinInterval = TimeSpan.Zero }, ModuleDir);
        var workspace = Path.Combine(dir, "MyGame");
        module.Activate(workspace);

        var handshake = await Read(discord.Handshakes);
        Assert.Equal("""{"v":1,"client_id":"1569037494551322826"}""", handshake.ToJsonString());
        Assert.Equal("""{"ping":1}""", (await Read(discord.Pongs)).ToJsonString());

        var first = await Read(discord.Commands);
        Assert.Equal("SET_ACTIVITY", first["cmd"]!.GetValue<string>());
        Assert.True(Guid.TryParse(first["nonce"]!.GetValue<string>(), out _));
        Assert.Equal(4242, first["args"]!["pid"]!.GetValue<int>());
        var activity = first["args"]!["activity"]!.AsObject();
        Assert.Equal("Browsing the project", activity["details"]!.GetValue<string>());
        Assert.Equal("In MyGame", activity["state"]!.GetValue<string>());
        var start = activity["timestamps"]!["start"]!.GetValue<long>();
        Assert.InRange(start, before, DateTimeOffset.UtcNow.ToUnixTimeSeconds());

        module.Send(Event("editorChanged", Path.Combine(workspace, "src", "Player.cs")));
        var second = (await Read(discord.Commands))["args"]!["activity"]!.AsObject();
        Assert.Equal("Editing Player.cs", second["details"]!.GetValue<string>());
        Assert.Equal("ByteForge, writing C#", second["assets"]!["large_text"]!.GetValue<string>());
        Assert.Equal(DiscordModule.Logo, second["assets"]!["large_image"]!.GetValue<string>());
        Assert.Equal(start, second["timestamps"]!["start"]!.GetValue<long>());

        // Paused: the activity is cleared (null) and the status bar says so.
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "discord.toggle" });
        await module.Next("log", m => m["text"]!.GetValue<string>() == "Discord: paused: your activity is hidden.");
        var cleared = await Read(discord.Commands);
        Assert.True(cleared["args"]!.AsObject().ContainsKey("activity"));
        Assert.Null(cleared["args"]!["activity"]);
        var paused = await module.Next("status", m => m["text"]!.GetValue<string>() == "Discord: paused");
        Assert.Equal("discord.toggle", paused["command"]!.GetValue<string>());
        Assert.Equal("right", paused["alignment"]!.GetValue<string>());
        Assert.Equal(1, discord.Connections);
    }

    [Fact]
    public async Task Settings_hide_the_file_and_the_project()
    {
        var discord = new FakeDiscord();
        await using var module = ModuleHarness.Start(new TestDiscordModule(discord) { MinInterval = TimeSpan.Zero }, ModuleDir);
        module.Activate(Path.Combine(dir, "Secret"), new() { ["discord.showFileName"] = false, ["discord.showProject"] = false });
        module.Send(Event("editorChanged", Path.Combine(dir, "Secret", "main.rs")));
        JsonObject activity;
        do
        {
            activity = (await Read(discord.Commands))["args"]!["activity"]!.AsObject();
        }
        while (activity["details"]!.GetValue<string>() != "Editing a Rust file");
        Assert.Equal("Writing Rust", activity["state"]!.GetValue<string>());
        Assert.Equal("ByteForge", activity["assets"]!["large_text"]!.GetValue<string>());
    }

    [Fact]
    public async Task Without_Discord_it_says_offline_and_a_refusal_is_an_error()
    {
        await using (var module = ModuleHarness.Start(new TestDiscordModule(null), ModuleDir))
        {
            module.Activate(dir);
            var status = await module.Next("status", m => m["id"]!.GetValue<string>() == "discord.status");
            Assert.Equal("Discord: offline", status["text"]!.GetValue<string>());
            Assert.Equal("The Discord app isn't running. ByteForge connects when it starts.", status["tooltip"]!.GetValue<string>());
            Assert.Equal("discord.reconnect", status["command"]!.GetValue<string>());
        }
        var discord = new FakeDiscord { Refuse = "Invalid Client ID" };
        await using (var module = ModuleHarness.Start(new TestDiscordModule(discord), ModuleDir))
        {
            module.Activate(dir);
            await module.Next("log", m => m["text"]!.GetValue<string>() == "Discord: Discord refused the connection: Invalid Client ID.");
            var status = await module.Next("status", m => m["id"]!.GetValue<string>() == "discord.status");
            Assert.Equal("Discord: error", status["text"]!.GetValue<string>());
            Assert.Equal("Discord refused the connection: Invalid Client ID. Click to retry.", status["tooltip"]!.GetValue<string>());
        }
    }

    [Fact]
    public async Task A_lost_connection_is_logged_and_retried()
    {
        var discord = new FakeDiscord();
        await using var module = ModuleHarness.Start(new TestDiscordModule(discord) { MinInterval = TimeSpan.Zero, Retry = TimeSpan.FromMilliseconds(200) }, ModuleDir);
        module.Activate(dir);
        await Read(discord.Commands);
        discord.Hangup();
        module.Send(Event("editorChanged", Path.Combine(dir, "a.lua")));
        await module.Next("log", m => m["text"]!.GetValue<string>().StartsWith("Discord: lost the connection (Discord closed the connection); retrying."));
        var again = (await Read(discord.Commands))["args"]!["activity"]!.AsObject();
        Assert.Equal("Editing a.lua", again["details"]!.GetValue<string>());
        Assert.Equal(2, discord.Connections);
    }

    [Fact]
    public async Task Talks_over_a_real_pipe_or_socket()
    {
        var module = new TransportModule();
        Stream server;
        string path;
        if (OperatingSystem.IsWindows())
        {
            var name = "bf-discord-test-" + Guid.NewGuid().ToString("N");
            path = $@"\\?\pipe\{name}";
            await Assert.ThrowsAsync<TimeoutException>(() => module.Open(path));
            var pipe = new System.IO.Pipes.NamedPipeServerStream(name, System.IO.Pipes.PipeDirection.InOut, 1, System.IO.Pipes.PipeTransmissionMode.Byte,
                System.IO.Pipes.PipeOptions.Asynchronous);
            var waiting = pipe.WaitForConnectionAsync();
            using var client = await module.Open(path);
            await waiting;
            server = pipe;
            await Exchange(client!, server);
        }
        else
        {
            path = Path.Combine(dir, "discord-ipc-0");
            Assert.Null(await module.Open(path));
            using var listener = new System.Net.Sockets.Socket(System.Net.Sockets.AddressFamily.Unix, System.Net.Sockets.SocketType.Stream, System.Net.Sockets.ProtocolType.Unspecified);
            listener.Bind(new System.Net.Sockets.UnixDomainSocketEndPoint(path));
            listener.Listen();
            var accepting = listener.AcceptAsync();
            using var client = await module.Open(path);
            server = new System.Net.Sockets.NetworkStream(await accepting, ownsSocket: true);
            await Exchange(client!, server);
        }
        server.Dispose();
    }

    static async Task Exchange(Stream client, Stream server)
    {
        using var ipc = new DiscordIpc(client);
        var answering = Task.Run(async () =>
        {
            var (op, data) = await FakeDiscord.ReadFrame(server, CancellationToken.None);
            Assert.Equal(1, op);
            await FakeDiscord.WriteFrame(server, 1, new JsonObject { ["nonce"] = data["nonce"]!.GetValue<string>(), ["evt"] = "ERROR", ["data"] = new JsonObject { ["message"] = "bad activity" } });
        });
        var error = await Assert.ThrowsAsync<DiscordException>(() => ipc.SetActivityAsync(new JsonObject { ["details"] = "x" }, 1, CancellationToken.None));
        Assert.Equal("bad activity", error.Message);
        await answering;
    }

    [Fact]
    public void Socket_candidates_follow_the_platform()
    {
        Assert.Equal(@"\\?\pipe\discord-ipc-0", DiscordModule.SocketCandidates(true, _ => null).First());
        Assert.Equal(10, DiscordModule.SocketCandidates(true, _ => null).Count());
        var unix = DiscordModule.SocketCandidates(false, name => name == "XDG_RUNTIME_DIR" ? "/run/user/1000" : null).ToList();
        Assert.Equal(70, unix.Count);
        Assert.Equal("/run/user/1000/discord-ipc-0", unix[0]);
        Assert.Equal("/run/user/1000/app/com.discordapp.Discord/discord-ipc-0", unix[10]);
        Assert.Equal("/run/user/1000/.flatpak/dev.vencord.Vesktop/xdg-run/discord-ipc-9", unix[59]);
        Assert.Equal("/tmp/discord-ipc-9", unix[69]);
        Assert.Equal("/var/tmp/x/discord-ipc-0", DiscordModule.SocketCandidates(false, name => name switch { "XDG_RUNTIME_DIR" => "", "TMPDIR" => "/var/tmp/x", _ => null }).First());
        Assert.Equal("/tmp/discord-ipc-0", DiscordModule.SocketCandidates(false, _ => null).First());
    }

    [Theory]
    [InlineData("Program.cs", "C#")]
    [InlineData("App.CSPROJ", "MSBuild")]
    [InlineData("Dockerfile", "Docker")]
    [InlineData("dockerfile.dev", "Docker")]
    [InlineData("build.dockerfile", "Docker")]
    [InlineData(".bashrc", null)]
    [InlineData(".lua", null)]
    [InlineData("name.", null)]
    [InlineData("README", null)]
    [InlineData("archive.tar.gz", null)]
    [InlineData("notes.txt", "plain text")]
    public void Knows_the_language_of_a_file(string name, string? language) => Assert.Equal(language, DiscordModule.LanguageOf(Path.Combine("src", name)));

    [Fact]
    public void Clips_texts_to_Discords_limits()
    {
        Assert.Null(DiscordModule.Clip(" a "));
        Assert.Equal("ab", DiscordModule.Clip(" ab "));
        Assert.Equal(new string('x', 128), DiscordModule.Clip(new string('x', 128)));
        Assert.Equal(new string('x', 127) + "…", DiscordModule.Clip(new string('x', 129)));
        // Characters, not UTF-16 units: an emoji counts once.
        Assert.Equal(string.Concat(Enumerable.Repeat("😀", 127)) + "…", DiscordModule.Clip(string.Concat(Enumerable.Repeat("😀", 130))));
    }

    [Fact]
    public void Builds_the_activity()
    {
        JsonObject none = [];
        Assert.Equal("""{"details":"Idle","timestamps":{"start":100},"assets":{"large_image":"LOGO","large_text":"ByteForge"}}""",
            DiscordModule.BuildActivity(null, null, 100, none).ToJsonString().Replace(DiscordModule.Logo, "LOGO"));
        var ws = Path.Combine(dir, "Proj") + Path.DirectorySeparatorChar;
        Assert.Equal("""{"details":"Editing main.py","timestamps":{"start":5},"assets":{"large_image":"LOGO","large_text":"ByteForge, writing Python"},"state":"In Proj"}""",
            DiscordModule.BuildActivity(ws, Path.Combine(ws, "main.py"), 5, none).ToJsonString().Replace(DiscordModule.Logo, "LOGO"));
        Assert.Equal("""{"details":"Editing a file","timestamps":{"start":5},"assets":{"large_image":"LOGO","large_text":"ByteForge"},"state":"In Proj"}""",
            DiscordModule.BuildActivity(ws, Path.Combine(ws, "x.unknown"), 5, new() { ["discord.showFileName"] = false }).ToJsonString().Replace(DiscordModule.Logo, "LOGO"));
        // Only false hides: other values count as the default.
        Assert.Equal("""{"details":"Editing x.go","timestamps":{"start":5},"assets":{"large_image":"LOGO","large_text":"ByteForge"},"state":"Writing Go"}""",
            DiscordModule.BuildActivity(null, Path.Combine(ws, "x.go"), 5, new() { ["discord.showFileName"] = 0, ["discord.showProject"] = "no" }).ToJsonString().Replace(DiscordModule.Logo, "LOGO"));
    }
}
