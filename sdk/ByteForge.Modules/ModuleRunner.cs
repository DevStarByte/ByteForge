using System.Text.Json;
using System.Text.Json.Nodes;

namespace ByteForge.Modules;

/// <summary>Runs a <see cref="Module"/>: what a module's ByteForgeMain hands its arguments to.</summary>
public static class ModuleRunner
{
    public static Task RunAsync<T>(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) where T : Module, new() =>
        RunAsync(new T(), input, output, moduleDir, cancel);

    public static async Task RunAsync(Module module, TextReader input, TextWriter output, string moduleDir, CancellationToken cancel)
    {
        module.Attach(output, moduleDir, cancel);
        await module.StartAsync();
        var running = new List<Task>();
        while (await ReadLine(input, cancel) is { } line)
        {
            JsonObject? message;
            try
            {
                message = JsonNode.Parse(line) as JsonObject;
            }
            catch (JsonException)
            {
                continue;
            }
            if (message == null)
            {
                continue;
            }
            try
            {
                if (await module.HandleAsync(message) is { } work)
                {
                    running.RemoveAll(t => t.IsCompleted);
                    running.Add(work);
                }
            }
            catch (Exception e) when (e is not OperationCanceledException)
            {
                module.Log($"{message["type"]} failed: {e}");
            }
        }
        // Input ended (the module is being unloaded): let commands and formatting in flight finish briefly.
        await Task.WhenAny(Task.WhenAll(running), Task.Delay(TimeSpan.FromSeconds(1), CancellationToken.None));
    }

    static async Task<string?> ReadLine(TextReader input, CancellationToken cancel)
    {
        try
        {
            return await input.ReadLineAsync(cancel);
        }
        catch (OperationCanceledException)
        {
            return null;
        }
    }
}
