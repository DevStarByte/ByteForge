using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.Modules.Testing;
using ByteForge.WindowsTheme;

namespace ByteForge.WindowsTheme.Tests;

public sealed partial class WindowsThemeTests : IDisposable
{
    [GeneratedRegex("^(#[0-9a-f]{6}([0-9a-f]{2})?|transparent)$")]
    private static partial Regex Color();

    /// <summary>An AccentPalette as Windows stores it (REG_BINARY: eight RGBA shades, base blue 4th).</summary>
    static readonly byte[] AccentPalette = Convert.FromHexString("99ebff0049d8ff00009cff000078d4000059a100004280000026440000b7c300");

    static readonly SystemLook DarkBlue = new(true, ThemeBuilder.Shades("#0078d4"), true, false, false, true);

    /// <summary>A config folder laid out like ByteForge's: the module in &lt;tmp&gt;/modules/windows-theme.</summary>
    readonly string config = Directory.CreateTempSubdirectory("bf-windows-theme-").FullName;

    string ModuleDir => Path.Combine(config, "modules", "windows-theme");
    string ThemeFile => Path.Combine(ModuleDir, "themes", "windows.json");
    string LegacyFile => Path.Combine(config, "themes", "windows.json");

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

    /// <summary>The module with Windows' settings from <paramref name="system"/>, never from this machine.</summary>
    static WindowsThemeModule Module(Func<SystemLook?>? system = null, bool follow = false, int pollMs = 3000) =>
        new(system ?? (() => DarkBlue), follow, TimeSpan.FromMilliseconds(pollMs));

    /// <summary>Waits for everything sent before it (activate, settings) to be handled: commands come after.</summary>
    static async Task<string> Regenerate(ModuleHarness module)
    {
        module.Send(new() { ["type"] = "executeCommand", ["id"] = WindowsThemeModule.RegenerateCommand });
        var log = await module.Next("log", m => m["text"]!.GetValue<string>() is "Windows Theme: updated from Windows' settings." or "Windows Theme: already up to date.");
        return log["text"]!.GetValue<string>();
    }

    static bool Sent(ModuleHarness module, string type)
    {
        lock (module.Sent)
        {
            return module.Sent.Any(m => m["type"]!.GetValue<string>() == type);
        }
    }

    /// <summary>Reads the theme without keeping the module from replacing it meanwhile (as the polling test does).</summary>
    JsonObject ReadTheme()
    {
        for (var attempt = 0; ; attempt++)
        {
            try
            {
                using var stream = new FileStream(ThemeFile, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
                return JsonNode.Parse(stream)!.AsObject();
            }
            catch (IOException) when (attempt < 20)
            {
                Thread.Sleep(20);
            }
        }
    }

    static string S(JsonNode? node) => node!.GetValue<string>();

    // ───────────── reading Windows ─────────────

    [Fact]
    public void Accent_palette()
    {
        var palette = SystemReader.PaletteFromBytes(AccentPalette)!;
        Assert.Equal("#0078d4", palette.Base);
        Assert.Equal("#49d8ff", palette.Light2);
        Assert.Equal("#0059a1", palette.Dark1);
        Assert.Null(SystemReader.PaletteFromBytes(new byte[] { 0, 1 }));
    }

    [Fact]
    public void Accent_color_dword_is_abgr()
    {
        Assert.Equal("#0078d4", SystemReader.Abgr(0xffd47800));
        Assert.Equal("#0078d4", SystemReader.Abgr(unchecked((int)0xffd47800)));
    }

    [Fact]
    public void Reads_the_registry()
    {
        var values = new Dictionary<(string, string), object>
        {
            [("Personalize", "AppsUseLightTheme")] = 1,
            [("Personalize", "EnableTransparency")] = 0,
            [("Accent", "AccentPalette")] = AccentPalette,
            [("DWM", "ColorPrevalence")] = 1,
            [("HighContrast", "Flags")] = "126",
        };
        var system = SystemReader.FromRegistry((path, name) => values.GetValueOrDefault((path[(path.LastIndexOf('\\') + 1)..], name)), 22631);
        Assert.Equal(new SystemLook(false, SystemReader.PaletteFromBytes(AccentPalette)!, false, true, false, true), system);
    }

    [Fact]
    public void Registry_fallbacks()
    {
        // No AccentPalette: the shades of AccentColor; nothing at all: dark is false, transparency on, the default blue.
        var fromColor = SystemReader.FromRegistry((_, name) => name == "AccentColor" ? unchecked((int)0xff107c10) : null, 19045);
        Assert.Equal(ThemeBuilder.Shades("#107c10"), fromColor.Palette);
        Assert.False(fromColor.Windows11);
        Assert.Equal(new SystemLook(false, ThemeBuilder.Shades("#0078d4"), true, false, false, false), SystemReader.FromRegistry((_, _) => null, 0));
        Assert.True(SystemReader.FromRegistry((_, name) => name == "Flags" ? "127" : null, 0).HighContrast);
    }

    [Fact]
    public void Off_windows()
    {
        // Off Windows there is nothing to read; on Windows something always is (whatever this machine is set to).
        Assert.Equal(OperatingSystem.IsWindows(), SystemReader.Read() != null);
    }

    // ───────────── the theme ─────────────

    static void Check(JsonObject theme)
    {
        foreach (var (key, value) in theme["colors"]!.AsObject())
        {
            Assert.True(Color().IsMatch(S(value)), $"{key}: {value}");
        }
        foreach (var (key, value) in theme["tokenColors"]!.AsObject())
        {
            Assert.True(Color().IsMatch(S(value)), $"{key}: {value}");
        }
        JsonNode.Parse(ThemeBuilder.Serialize(theme));
    }

    [Fact]
    public void Every_variant_is_valid()
    {
        foreach (var dark in new[] { true, false })
        {
            foreach (var accent in ThemeBuilder.Accents.Values)
            {
                for (var flags = 0; flags < 8; flags++)
                {
                    Check(ThemeBuilder.Build(new Look(dark, ThemeBuilder.Shades(accent), (flags & 1) != 0, (flags & 2) != 0, (flags & 4) != 0, true)));
                }
            }
        }
    }

    [Fact]
    public void Dark_uses_the_light_accent_shade_and_mica()
    {
        var look = ThemeBuilder.Effective(new SystemLook(true, SystemReader.PaletteFromBytes(AccentPalette)!, true, false, false, true), []);
        var theme = ThemeBuilder.Build(look);
        Assert.Equal("#49d8ff", S(theme["colors"]!["accent"]));
        Assert.Equal("#49d8ff", S(theme["colors"]!["list.activeFileIndicator"]));
        Assert.Equal("mica", S(theme["effects"]!["window.backdrop"]));
        Assert.Equal("#569cd6", S(theme["tokenColors"]!["keyword"]));
        Assert.Equal("ByteForge Dark", S(theme["inherits"]));
    }

    [Fact]
    public void Light_accent_title_bar_on_windows_10()
    {
        var look = ThemeBuilder.Effective(new SystemLook(false, SystemReader.PaletteFromBytes(AccentPalette)!, true, true, false, false), []);
        var theme = ThemeBuilder.Build(look);
        Assert.Equal("#0078d4", S(theme["colors"]!["titleBar.background"]));
        Assert.Equal("#ffffff", S(theme["colors"]!["titleBar.foreground"]));
        Assert.Equal("acrylic", S(theme["effects"]!["window.backdrop"]));
        Assert.Equal("#0000ff", S(theme["tokenColors"]!["keyword"]));
    }

    [Fact]
    public void Settings_override_windows()
    {
        var system = new SystemLook(true, ThemeBuilder.Shades("#0078d4"), false, true, false, true);
        var look = ThemeBuilder.Effective(system, new()
        {
            ["windowsTheme.appearance"] = "light", ["windowsTheme.accent"] = "green", ["windowsTheme.accentTitleBar"] = "off",
        });
        Assert.False(look.Dark);
        Assert.Equal("#107c10", look.Palette.Base);
        Assert.False(look.AccentTitleBar);
        Assert.False(look.Translucent); // Windows' transparency effects are off
    }

    [Fact]
    public void Settings_system_and_translucent()
    {
        var look = ThemeBuilder.Effective(DarkBlue, new() { ["windowsTheme.appearance"] = "system", ["windowsTheme.accent"] = "", ["windowsTheme.translucent"] = false });
        Assert.True(look.Dark);
        Assert.Equal("#0078d4", look.Palette.Base);
        Assert.False(look.Translucent);
        Assert.Equal("none", S(ThemeBuilder.Build(look)["effects"]!["window.backdrop"]));
        Assert.True(ThemeBuilder.Effective(DarkBlue, new() { ["windowsTheme.translucent"] = true }).Translucent);
        Assert.Equal("#0078d4", ThemeBuilder.Effective(DarkBlue, new() { ["windowsTheme.accent"] = "nonsense" }).Palette.Base);
        // Not on Windows: Dark with the default blue.
        Assert.Equal(ThemeBuilder.Effective(DarkBlue, []), ThemeBuilder.Effective(null, []));
    }

    [Fact]
    public async Task Theme_file()
    {
        var module = Module();
        await using var harness = ModuleHarness.Start(module, ModuleDir);
        await harness.Next("registerCommand");
        var theme = ThemeBuilder.Build(ThemeBuilder.Effective(null, []));
        theme["description"] = "changed";
        Assert.True(module.WriteTheme(theme));
        Assert.False(module.WriteTheme(theme));
        Assert.Equal(ThemeBuilder.Serialize(theme), File.ReadAllText(ThemeFile));
        Assert.False(File.Exists(ThemeFile + ".tmp"));
    }

    [Fact]
    public void Manifest_settings_match()
    {
        var dir = AppContext.BaseDirectory;
        while (!File.Exists(Path.Combine(dir, "module.json")))
        {
            dir = Path.GetDirectoryName(dir) ?? throw new FileNotFoundException("module.json");
        }
        var manifest = JsonNode.Parse(File.ReadAllText(Path.Combine(dir, "module.json")), documentOptions: new() { CommentHandling = JsonCommentHandling.Skip })!;
        Assert.Equal("byteforge.windows-theme", S(manifest["id"]));
        Assert.Equal("dotnet", S(manifest["runtime"]));
        Assert.Equal("bin/WindowsTheme.dll", S(manifest["assembly"]));
        var accents = manifest["settings"]!.AsArray().Single(s => S(s!["key"]) == "windowsTheme.accent")!["choices"]!.AsArray().Select(S);
        Assert.Equal(new[] { "system" }.Concat(ThemeBuilder.Accents.Keys).Order(), accents.Order());
    }

    // ───────────── the module ─────────────

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = ModuleHarness.Start(Module(), ModuleDir);
        var use = await module.Next("registerCommand", m => S(m["id"]) == "windowsTheme.use");
        Assert.Equal("Use Windows Theme", S(use["title"]));
        Assert.Equal("Windows Theme", S(use["category"]));
        var regenerate = await module.Next("registerCommand", m => S(m["id"]) == "windowsTheme.regenerate");
        Assert.Equal("Regenerate from Windows Settings", S(regenerate["title"]));
        Assert.Equal("Windows Theme", S(regenerate["category"]));
    }

    [Fact]
    public async Task Writes_the_theme_into_the_modules_own_themes_folder()
    {
        await using var module = ModuleHarness.Start(Module(), ModuleDir);
        module.Activate();
        await Regenerate(module);
        var theme = ReadTheme();
        Assert.Equal("Windows", S(theme["name"]));
        Assert.Equal("dark", S(theme["type"]));
        Assert.False(File.Exists(LegacyFile));
        Assert.False(Directory.Exists(Path.Combine(config, "themes")));
    }

    [Fact]
    public async Task First_run_switches_to_the_theme_once()
    {
        await using (var module = ModuleHarness.Start(Module(), ModuleDir))
        {
            Assert.Equal("Windows", S((await module.Next("setTheme"))["name"]));
        }
        // Started again: the theme file is there, so the user's theme choice is respected.
        await using (var module = ModuleHarness.Start(Module(), ModuleDir))
        {
            await Regenerate(module);
            Assert.False(Sent(module, "setTheme"));
        }
    }

    [Fact]
    public async Task Upgrading_removes_the_old_copy_and_keeps_the_users_theme()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(LegacyFile)!);
        File.WriteAllText(LegacyFile, ThemeBuilder.Serialize(ThemeBuilder.Build(ThemeBuilder.Effective(null, []))));
        await using var module = ModuleHarness.Start(Module(), ModuleDir);
        var log = await module.Next("log", m => S(m["text"]).Contains("old copy"));
        Assert.Contains(LegacyFile, S(log["text"]));
        await Regenerate(module);
        Assert.False(File.Exists(LegacyFile));
        Assert.True(File.Exists(ThemeFile));
        Assert.False(Sent(module, "setTheme"));
        Assert.Single(module.Sent, m => S(m["type"]) == "log" && S(m["text"]).Contains("old copy"));
    }

    [Fact]
    public async Task A_users_own_windows_json_is_kept()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(LegacyFile)!);
        const string mine = "{\"name\": \"My Windows\", \"type\": \"dark\"}";
        File.WriteAllText(LegacyFile, mine);
        await using var module = ModuleHarness.Start(Module(), ModuleDir);
        // Not an upgrade (the file isn't this module's), so it's a fresh install.
        await module.Next("setTheme");
        await Regenerate(module);
        Assert.Equal(mine, File.ReadAllText(LegacyFile));
        Assert.DoesNotContain(module.Sent, m => S(m["type"]) == "log" && S(m["text"]).Contains("old copy"));
    }

    [Fact]
    public async Task Unchanged_content_is_not_rewritten()
    {
        await using var module = ModuleHarness.Start(Module(), ModuleDir);
        module.Activate();
        Assert.Equal("Windows Theme: already up to date.", await Regenerate(module));
        var written = File.GetLastWriteTimeUtc(ThemeFile);
        File.SetLastWriteTimeUtc(ThemeFile, written.AddHours(-1));
        Assert.Equal("Windows Theme: already up to date.", await Regenerate(module));
        Assert.Equal(written.AddHours(-1), File.GetLastWriteTimeUtc(ThemeFile));
    }

    [Fact]
    public async Task Settings_changes_regenerate_the_theme()
    {
        await using var module = ModuleHarness.Start(Module(), ModuleDir);
        module.Activate(settings: new() { ["windowsTheme.appearance"] = "light" });
        await Regenerate(module);
        Assert.Equal("light", S(ReadTheme()["type"]));
        module.Send(new() { ["type"] = "event", ["name"] = "settingsChanged", ["settings"] = new JsonObject { ["windowsTheme.accent"] = "green" } });
        await Regenerate(module);
        var theme = ReadTheme();
        Assert.Equal("dark", S(theme["type"]));
        Assert.Equal(ThemeBuilder.Shades("#107c10").Light2, S(theme["colors"]!["accent"]));
    }

    [Fact]
    public async Task Use_command_switches_to_the_theme()
    {
        File.WriteAllText(Directory.CreateDirectory(Path.GetDirectoryName(ThemeFile)!).FullName + "/windows.json", "{}");
        await using var module = ModuleHarness.Start(Module(), ModuleDir);
        await module.Next("registerCommand");
        Assert.False(Sent(module, "setTheme"));
        module.Send(new() { ["type"] = "executeCommand", ["id"] = WindowsThemeModule.UseCommand });
        Assert.Equal("Windows", S((await module.Next("setTheme"))["name"]));
        Assert.Equal("Windows", S(ReadTheme()["name"]));
    }

    [Fact]
    public async Task Follows_windows_settings()
    {
        var system = DarkBlue;
        await using var module = ModuleHarness.Start(Module(() => Volatile.Read(ref system), follow: true, pollMs: 20), ModuleDir);
        await module.Next("setTheme");
        Assert.Equal("dark", S(ReadTheme()["type"]));
        Assert.DoesNotContain(module.Sent, m => S(m["type"]) == "log" && S(m["text"]).Contains("not on Windows"));
        Volatile.Write(ref system, system with { Dark = false });
        var deadline = DateTime.UtcNow.AddSeconds(10);
        while (S(ReadTheme()["type"]) != "light" && DateTime.UtcNow < deadline)
        {
            await Task.Delay(20);
        }
        Assert.Equal("light", S(ReadTheme()["type"]));
    }

    [Fact]
    public async Task Says_so_when_not_following_windows()
    {
        await using var module = ModuleHarness.Start(Module(() => null, follow: false), ModuleDir);
        await module.Next("log", m => S(m["text"]) == "Windows Theme: not on Windows, so the theme uses the settings (or Dark with the default blue).");
    }
}
