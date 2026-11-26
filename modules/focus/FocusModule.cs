using ByteForge.Modules;

namespace ByteForge.Focus;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<FocusModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// A Pomodoro timer in the status bar: 25 minutes of focus, 5 minutes of break (every 4th: 15). Clicking it starts,
/// pauses or resumes; when a phase ends the Output says so and, where notify-send is on PATH, a desktop
/// notification does too.
/// </summary>
public sealed class FocusModule : Module
{
    public const int FocusSeconds = 25 * 60;
    public const int ShortBreakSeconds = 5 * 60;
    public const int LongBreakSeconds = 15 * 60;
    public const string StatusId = "focus.timer";

    public static readonly (string Id, string Title)[] Commands =
    [
        ("focus.toggle", "Start / Pause"),
        ("focus.skip", "Skip to Next Phase"),
        ("focus.reset", "Stop and Reset"),
    ];

    readonly TimeSpan tickInterval;
    readonly Action<string> desktopNotify;
    readonly Lock state = new();

    string phase = "idle";
    int remaining = FocusSeconds;
    bool running;
    int done;

    public FocusModule() : this(TimeSpan.FromSeconds(1), NotifySend)
    {
    }

    /// <param name="tickInterval">How long a second of the timer takes (one second; shorter in tests).</param>
    /// <param name="desktopNotify">Shows a desktop notification with this text.</param>
    public FocusModule(TimeSpan tickInterval, Action<string> desktopNotify)
    {
        this.tickInterval = tickInterval;
        this.desktopNotify = desktopNotify;
    }

    protected override Task OnStartAsync()
    {
        foreach (var (id, title) in Commands)
        {
            RegisterCommand(id, title, "Focus Timer");
        }
        Render();
        _ = Task.Run(TickAsync);
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id)
    {
        lock (state)
        {
            switch (id)
            {
                case "focus.toggle":
                    if (phase == "idle")
                    {
                        (phase, remaining, running) = ("focus", FocusSeconds, true);
                    }
                    else
                    {
                        running = !running;
                    }
                    Render();
                    break;
                case "focus.skip":
                    if (phase != "idle")
                    {
                        NextPhase();
                        running = true;
                    }
                    Render();
                    break;
                case "focus.reset":
                    (phase, remaining, running) = ("idle", FocusSeconds, false);
                    Render();
                    break;
            }
        }
        return Task.CompletedTask;
    }

    async Task TickAsync()
    {
        try
        {
            while (true)
            {
                await Task.Delay(tickInterval, Stopping);
                lock (state)
                {
                    if (running)
                    {
                        remaining--;
                        if (remaining <= 0)
                        {
                            NextPhase();
                        }
                        Render();
                    }
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    void NextPhase()
    {
        if (phase == "focus")
        {
            done++;
            var isLong = done % 4 == 0;
            (phase, remaining) = ("break", isLong ? LongBreakSeconds : ShortBreakSeconds);
            Notify($"Session {done} done. Take a {(isLong ? "15" : "5")}-minute break.");
        }
        else
        {
            (phase, remaining) = ("focus", FocusSeconds);
            Notify("Break's over. Back to focus.");
        }
    }

    void Render()
    {
        string text, tip;
        if (phase == "idle")
        {
            (text, tip) = ("◷ Focus", "Start a 25-minute focus session");
        }
        else
        {
            var left = Math.Max(remaining, 0);
            var icon = phase == "focus" ? "◉" : "☕";
            text = $"{icon} {left / 60:00}:{left % 60:00}" + (running ? "" : " ⏸");
            tip = $"{(phase == "focus" ? "Focus" : "Break")} · {done} sessions done today. Click to {(running ? "pause" : "resume")}.";
        }
        SetStatus(StatusId, text, tip, "focus.toggle", right: true);
    }

    void Notify(string text)
    {
        Log($"Focus Timer: {text}");
        try
        {
            desktopNotify(text);
        }
        catch (Exception e)
        {
            Log($"Focus Timer: the desktop notification failed: {e.Message}");
        }
    }

    /// <summary>A desktop notification through notify-send, when it's on PATH (as before: nothing otherwise).</summary>
    public static void NotifySend(string text)
    {
        if (Programs.Which("notify-send") is not { } program)
        {
            return;
        }
        _ = Task.Run(async () =>
        {
            try
            {
                await Programs.RunAsync(program, ["--app-name=ByteForge", "Focus Timer", text], timeout: TimeSpan.FromSeconds(30));
            }
            catch (Exception e) when (e is ModuleException or TimeoutException)
            {
            }
        });
    }
}
