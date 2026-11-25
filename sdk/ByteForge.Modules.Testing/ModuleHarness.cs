using System.Text;
using System.Text.Json.Nodes;
using System.Threading.Channels;

namespace ByteForge.Modules.Testing;

/// <summary>
/// A module running in memory, as the IDE would run it: <see cref="Send"/> the IDE's messages, <see cref="Next"/>
/// for what the module sends. Dispose stops it (input ends, <see cref="Module.Stopping"/> is cancelled).
/// </summary>
public sealed class ModuleHarness : IAsyncDisposable
{
    readonly Channel<string> input = Channel.CreateUnbounded<string>();
    readonly Channel<JsonObject> output = Channel.CreateUnbounded<JsonObject>();
    readonly CancellationTokenSource stop = new();
    readonly Task run;

    /// <summary>Everything the module has sent so far (also the messages <see cref="Next"/> skipped).</summary>
    public List<JsonObject> Sent { get; } = [];

    ModuleHarness(Module module, string moduleDir)
    {
        var writer = new LineWriter(line =>
        {
            if (JsonNode.Parse(line) is JsonObject message)
            {
                lock (Sent)
                {
                    Sent.Add(message);
                }
                output.Writer.TryWrite(message);
            }
        });
        run = Task.Run(() => ModuleRunner.RunAsync(module, new LineReader(input.Reader), writer, moduleDir, stop.Token));
    }

    /// <summary>Starts <paramref name="module"/> as if installed in <paramref name="moduleDir"/>.</summary>
    public static ModuleHarness Start(Module module, string moduleDir) => new(module, moduleDir);

    /// <summary>Sends a message from the IDE.</summary>
    public void Send(JsonObject message) => input.Writer.TryWrite(message.ToJsonString());

    /// <summary>Sends activate with a workspace and settings.</summary>
    public void Activate(string? workspace = null, JsonObject? settings = null) =>
        Send(new() { ["type"] = "activate", ["workspace"] = workspace, ["settings"] = settings ?? [] });

    /// <summary>The next message of <paramref name="type"/> (others are skipped) that also satisfies <paramref name="where"/>.</summary>
    public async Task<JsonObject> Next(string type, Func<JsonObject, bool>? where = null, TimeSpan? timeout = null)
    {
        using var limit = new CancellationTokenSource(timeout ?? TimeSpan.FromSeconds(30));
        try
        {
            while (true)
            {
                var message = await output.Reader.ReadAsync(limit.Token);
                if (message["type"]?.GetValue<string>() == type && (where == null || where(message)))
                {
                    return message;
                }
            }
        }
        catch (OperationCanceledException)
        {
            string seen;
            lock (Sent)
            {
                seen = string.Join("\n", Sent.Select(m => m.ToJsonString()));
            }
            throw new TimeoutException($"no {type} message from the module; it sent:\n{seen}");
        }
    }

    public async ValueTask DisposeAsync()
    {
        input.Writer.TryComplete();
        stop.Cancel();
        try
        {
            await run.WaitAsync(TimeSpan.FromSeconds(5));
        }
        catch (OperationCanceledException)
        {
        }
        stop.Dispose();
    }

    sealed class LineReader(ChannelReader<string> lines) : TextReader
    {
        public override async ValueTask<string?> ReadLineAsync(CancellationToken cancellationToken)
        {
            try
            {
                return await lines.ReadAsync(cancellationToken);
            }
            catch (ChannelClosedException)
            {
                return null;
            }
        }

        public override string? ReadLine() => ReadLineAsync(CancellationToken.None).AsTask().GetAwaiter().GetResult();
    }

    sealed class LineWriter(Action<string> line) : TextWriter
    {
        readonly StringBuilder pending = new();

        public override Encoding Encoding => Encoding.UTF8;

        public override void Write(char value)
        {
            if (value == '\n')
            {
                line(pending.ToString());
                pending.Clear();
            }
            else if (value != '\r')
            {
                pending.Append(value);
            }
        }
    }
}
