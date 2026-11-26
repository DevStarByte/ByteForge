using System.Collections.Concurrent;
using System.Text.Json.Nodes;
using ByteForge.Focus;
using ByteForge.Modules.Testing;

namespace ByteForge.Focus.Tests;

public sealed class FocusTests : IDisposable
{
    readonly string config = Directory.CreateTempSubdirectory("bf-focus-").FullName;
    readonly ConcurrentQueue<string> notified = new();

    string ModuleDir => Path.Combine(config, "modules", "focus");

    public void Dispose()
    {
        try
        {
            Directory.Delete(config, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    /// <summary>The module with desktop notifications recorded, not shown; a "second" lasts <paramref name="tickMs"/>.</summary>
    ModuleHarness Start(int tickMs = 60_000) => ModuleHarness.Start(new FocusModule(TimeSpan.FromMilliseconds(tickMs), notified.Enqueue), ModuleDir);

    static JsonObject Run(string id) => new() { ["type"] = "executeCommand", ["id"] = id };

    static string S(JsonNode? node) => node!.GetValue<string>();

    static Task<JsonObject> Status(ModuleHarness module, string text) =>
        module.Next("status", m => S(m["id"]) == FocusModule.StatusId && S(m["text"]) == text);

    [Fact]
    public async Task Registers_its_commands_and_shows_the_idle_timer()
    {
        await using var module = Start();
        foreach (var (id, title) in FocusModule.Commands)
        {
            var command = await module.Next("registerCommand", m => S(m["id"]) == id);
            Assert.Equal(title, S(command["title"]));
            Assert.Equal("Focus Timer", S(command["category"]));
        }
        var status = await Status(module, "◷ Focus");
        Assert.Equal("Start a 25-minute focus session", S(status["tooltip"]));
        Assert.Equal("focus.toggle", S(status["command"]));
        Assert.Equal("right", S(status["alignment"]));
    }

    [Fact]
    public async Task Toggle_starts_pauses_and_resumes_and_reset_stops()
    {
        await using var module = Start();
        await Status(module, "◷ Focus");
        module.Send(Run("focus.toggle"));
        var status = await Status(module, "◉ 25:00");
        Assert.Equal("Focus · 0 sessions done today. Click to pause.", S(status["tooltip"]));
        module.Send(Run("focus.toggle"));
        status = await Status(module, "◉ 25:00 ⏸");
        Assert.Equal("Focus · 0 sessions done today. Click to resume.", S(status["tooltip"]));
        module.Send(Run("focus.reset"));
        await Status(module, "◷ Focus");
    }

    [Fact]
    public async Task Skip_moves_through_the_phases_with_a_long_break_every_fourth_session()
    {
        await using var module = Start();
        await Status(module, "◷ Focus");
        module.Send(Run("focus.skip")); // idle: nothing to skip, the timer is shown again
        await Status(module, "◷ Focus");
        module.Send(Run("focus.toggle"));
        await Status(module, "◉ 25:00");
        module.Send(Run("focus.skip"));
        await module.Next("log", m => S(m["text"]) == "Focus Timer: Session 1 done. Take a 5-minute break.");
        var status = await Status(module, "☕ 05:00");
        Assert.Equal("Break · 1 sessions done today. Click to pause.", S(status["tooltip"]));
        module.Send(Run("focus.skip"));
        await module.Next("log", m => S(m["text"]) == "Focus Timer: Break's over. Back to focus.");
        await Status(module, "◉ 25:00");
        for (var i = 0; i < 5; i++)
        {
            module.Send(Run("focus.skip"));
        }
        status = await Status(module, "☕ 15:00");
        Assert.Equal("Break · 4 sessions done today. Click to pause.", S(status["tooltip"]));
        Assert.Equal(
            [
                "Session 1 done. Take a 5-minute break.", "Break's over. Back to focus.",
                "Session 2 done. Take a 5-minute break.", "Break's over. Back to focus.",
                "Session 3 done. Take a 5-minute break.", "Break's over. Back to focus.",
                "Session 4 done. Take a 15-minute break.",
            ],
            notified);
    }

    [Fact]
    public async Task Counts_down_while_running_only()
    {
        await using var module = Start(tickMs: 20);
        module.Send(Run("focus.toggle"));
        await Status(module, "◉ 24:59");
        await Status(module, "◉ 24:58");
        module.Send(Run("focus.toggle"));
        var paused = await module.Next("status", m => S(m["text"]).EndsWith(" ⏸"));
        await Task.Delay(200);
        lock (module.Sent)
        {
            Assert.Equal(paused, module.Sent.Last(m => S(m["type"]) == "status"));
        }
    }

    [Fact]
    public async Task A_failing_desktop_notification_is_logged_not_fatal()
    {
        await using var module = ModuleHarness.Start(new FocusModule(TimeSpan.FromMinutes(1), _ => throw new InvalidOperationException("no bus")), ModuleDir);
        module.Send(Run("focus.toggle"));
        await Status(module, "◉ 25:00");
        module.Send(Run("focus.skip"));
        await module.Next("log", m => S(m["text"]) == "Focus Timer: the desktop notification failed: no bus");
        await Status(module, "☕ 05:00");
    }
}
