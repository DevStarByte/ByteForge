using System.Globalization;
using System.Runtime.Versioning;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;
using Microsoft.Win32;

namespace ByteForge.WindowsTheme;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<WindowsThemeModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// A theme that looks like Windows 11 and follows its settings live. Writes a theme called "Windows" into the
/// module's own themes/ folder (ByteForge lists the themes of installed, enabled modules, and drops them with the
/// module) from what Windows is set to right now: the app mode (Light or Dark), the accent color (with the light
/// and dark shades Windows derives from it), transparency effects, "Show accent color on title bars and window
/// borders" and high contrast. ByteForge hot-reloads theme files, so while the theme is active the IDE follows
/// every change in Settings &gt; Personalization (polled every 3 seconds).
/// On another system the theme is still written (dark, the default blue, or what the settings say).
/// </summary>
public sealed class WindowsThemeModule : Module
{
    public const string UseCommand = "windowsTheme.use";
    public const string RegenerateCommand = "windowsTheme.regenerate";

    readonly Func<SystemLook?> readSystem;
    readonly bool followWindows;
    readonly TimeSpan pollInterval;
    readonly Lock writing = new();

    public WindowsThemeModule() : this(SystemReader.Read, OperatingSystem.IsWindows(), TimeSpan.FromSeconds(3))
    {
    }

    /// <param name="readSystem">What Windows is set to (null: not on Windows).</param>
    /// <param name="followWindows">Whether Windows' settings are polled for changes (on Windows).</param>
    /// <param name="pollInterval">How often they are.</param>
    public WindowsThemeModule(Func<SystemLook?> readSystem, bool followWindows, TimeSpan pollInterval)
    {
        this.readSystem = readSystem;
        this.followWindows = followWindows;
        this.pollInterval = pollInterval;
    }

    /// <summary>Where the theme is written: the module's own themes/ folder.</summary>
    public string ThemeFile => Path.Combine(ModuleDir, "themes", "windows.json");

    /// <summary>Where versions before 0.2.0 wrote it: ByteForge's user themes folder.</summary>
    public string LegacyThemeFile => Path.Combine(ConfigDir, "themes", "windows.json");

    protected override Task OnStartAsync()
    {
        var upgraded = RemoveLegacyTheme();
        var firstRun = !File.Exists(ThemeFile) && !upgraded;
        RegisterCommand(UseCommand, "Use Windows Theme", "Windows Theme");
        RegisterCommand(RegenerateCommand, "Regenerate from Windows Settings", "Windows Theme");
        if (!followWindows)
        {
            Log("Windows Theme: not on Windows, so the theme uses the settings (or Dark with the default blue).");
        }
        Generate();
        if (firstRun && File.Exists(ThemeFile))
        {
            // Just installed: switch to the theme once. Afterwards the user's theme choice is respected.
            SetTheme(ThemeBuilder.ThemeName);
        }
        if (followWindows)
        {
            _ = Task.Run(WatchAsync);
        }
        return Task.CompletedTask;
    }

    protected override Task OnActivateAsync()
    {
        Generate();
        return Task.CompletedTask;
    }

    protected override Task OnSettingsChangedAsync()
    {
        Generate();
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case UseCommand:
                Generate();
                if (File.Exists(ThemeFile))
                {
                    SetTheme(ThemeBuilder.ThemeName);
                }
                break;
            case RegenerateCommand:
                Log(Generate() ? "Windows Theme: updated from Windows' settings." : "Windows Theme: already up to date.");
                break;
        }
        return Task.CompletedTask;
    }

    /// <summary>
    /// Deletes the copy of the theme that versions before 0.2.0 wrote into ByteForge's user themes folder (where it
    /// outlived the module). Only a file named "Windows" is taken for it. Returns whether there was one.
    /// </summary>
    bool RemoveLegacyTheme()
    {
        var legacy = LegacyThemeFile;
        try
        {
            if (!File.Exists(legacy))
            {
                return false;
            }
            var theme = JsonNode.Parse(File.ReadAllText(legacy), documentOptions: new() { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true });
            if (theme?["name"] is not JsonValue name || !name.TryGetValue<string>(out var text) || text != ThemeBuilder.ThemeName)
            {
                return false;
            }
            File.Delete(legacy);
            Log($"Windows Theme: removed the old copy of the theme from {legacy} (the theme now lives in the module's folder).");
            return true;
        }
        catch (JsonException)
        {
            return false;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"Windows Theme: could not remove the old copy of the theme {legacy}: {e.Message}");
            return true;
        }
    }

    /// <summary>Builds the theme from Windows (or <paramref name="system"/>) and the settings and writes it; true when it changed.</summary>
    public bool Generate(SystemLook? system = null) =>
        WriteTheme(ThemeBuilder.Build(ThemeBuilder.Effective(system ?? readSystem(), Settings)));

    /// <summary>Writes the theme file atomically. Returns true when it changed.</summary>
    public bool WriteTheme(JsonObject theme)
    {
        var text = ThemeBuilder.Serialize(theme);
        var file = ThemeFile;
        lock (writing)
        {
            var tmp = file + ".tmp";
            try
            {
                if (File.Exists(file) && File.ReadAllText(file) == text)
                {
                    return false;
                }
                Directory.CreateDirectory(Path.GetDirectoryName(file)!);
                File.WriteAllText(tmp, text);
                File.Move(tmp, file, overwrite: true); // ByteForge never reads a half-written theme
                return true;
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                Log($"Windows Theme: could not write {file}: {e.Message}");
                return false;
            }
        }
    }

    /// <summary>Polls Windows' settings: the mode and accent change without telling us (and with Auto accent, by the wallpaper).</summary>
    async Task WatchAsync()
    {
        try
        {
            var last = readSystem();
            while (true)
            {
                await Task.Delay(pollInterval, Stopping);
                var now = readSystem();
                if (now != last)
                {
                    last = now;
                    Generate(now);
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception e)
        {
            Log($"Windows Theme: stopped following Windows' settings: {e.Message}");
        }
    }
}

/// <summary>Reads what Windows is set to from the registry (HKEY_CURRENT_USER).</summary>
public static class SystemReader
{
    const string Personalize = @"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    const string Accent = @"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent";
    const string Dwm = @"Software\Microsoft\Windows\DWM";
    const string HighContrast = @"Control Panel\Accessibility\HighContrast";

    /// <summary>What Windows is set to, or null off Windows.</summary>
    public static SystemLook? Read() =>
        OperatingSystem.IsWindows() ? FromRegistry(Registry, Environment.OSVersion.Version.Build) : null;

    [SupportedOSPlatform("windows")]
    static object? Registry(string path, string name)
    {
        try
        {
            using var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(path);
            return key?.GetValue(name);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or System.Security.SecurityException)
        {
            return null;
        }
    }

    /// <summary>What Windows is set to, from registry values (<paramref name="registry"/>: key path, value name → value or null).</summary>
    public static SystemLook FromRegistry(Func<string, string, object?> registry, int build)
    {
        var palette = PaletteFromBytes(registry(Accent, "AccentPalette"));
        if (palette == null)
        {
            palette = ThemeBuilder.Shades(registry(Dwm, "AccentColor") is { } color && Integer(color) is { } value ? Abgr(value) : ThemeBuilder.Accents["blue"]);
        }
        var flags = Integer(registry(HighContrast, "Flags")) ?? 0;
        return new SystemLook(
            Integer(registry(Personalize, "AppsUseLightTheme")) == 0,
            palette,
            Integer(registry(Personalize, "EnableTransparency")) != 0,
            Integer(registry(Dwm, "ColorPrevalence")) == 1,
            (flags & 1) != 0,
            build >= 22000);
    }

    /// <summary>A DWORD (int) as is; a number in a string (HighContrast's Flags is REG_SZ) parsed; anything else null.</summary>
    static long? Integer(object? value) => value switch
    {
        int i => i,
        long l => l,
        uint u => u,
        string s when long.TryParse(s.Trim(), NumberStyles.Integer, CultureInfo.InvariantCulture, out var n) => n,
        _ => null,
    };

    /// <summary>AccentPalette: eight RGBA colors, from the lightest shade to the darkest (the base color is the 4th).</summary>
    public static Palette? PaletteFromBytes(object? data)
    {
        if (data is not byte[] bytes || bytes.Length < 28)
        {
            return null;
        }
        var shades = Enumerable.Range(0, 7).Select(i => $"#{bytes[i * 4]:x2}{bytes[i * 4 + 1]:x2}{bytes[i * 4 + 2]:x2}").ToArray();
        return new Palette(shades[0], shades[1], shades[2], shades[3], shades[4], shades[5], shades[6]);
    }

    /// <summary>A DWORD color as Windows stores AccentColor (0xAABBGGRR).</summary>
    public static string Abgr(long value) => $"#{value & 0xff:x2}{(value >> 8) & 0xff:x2}{(value >> 16) & 0xff:x2}";
}
