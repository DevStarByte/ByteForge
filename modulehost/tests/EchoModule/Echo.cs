using System.Text.Json.Nodes;

namespace EchoModule;

/// Registers a command, answers `activate` with a log line, echoes `executeCommand`, and fails on "echo.crash".
public static class Echo
{
    public static async Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel)
    {
        await output.WriteLineAsync(new JsonObject { ["type"] = "registerCommand", ["id"] = "echo.hello", ["title"] = "Hello" }.ToJsonString());
        string? line;
        while ((line = await input.ReadLineAsync(cancel)) != null)
        {
            var message = JsonNode.Parse(line);
            switch (message?["type"]?.GetValue<string>())
            {
                case "activate":
                    var folder = Path.GetFileName(moduleDir);
                    await output.WriteLineAsync(new JsonObject { ["type"] = "log", ["text"] = $"activated in {folder}" }.ToJsonString());
                    break;
                case "executeCommand" when message["id"]?.GetValue<string>() == "echo.crash":
                    throw new InvalidOperationException("crashed on purpose");
                case "executeCommand":
                    await output.WriteLineAsync(new JsonObject { ["type"] = "log", ["text"] = "ran " + message["id"] }.ToJsonString());
                    break;
            }
        }
    }
}
