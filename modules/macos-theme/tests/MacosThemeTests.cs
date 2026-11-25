using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.MacosTheme;
using ByteForge.Modules.Testing;

namespace ByteForge.MacosTheme.Tests;

/// <summary>The module with its System Settings faked: "macOS" or not, and what <c>defaults read</c> answers.</summary>
sealed class FakeMac(bool mac = true, Dictionary<string, string>? values = null) : MacosThemeModule
{
    public Dictionary<string, string> Values { get; } = values ?? [];

    protected override bool IsMac => mac;

    protected override TimeSpan PollInterval => TimeSpan.FromMilliseconds(50);

    protected override Task<string?> DefaultsAsync(string domain, string key)
    {
        lock (Values)
        {
            return Task.FromResult(Values.GetValueOrDefault(key));
        }
    }
}

public sealed partial class ReadTests
{
    [Fact]
    public async Task Reads_appearance_accent_and_accessibility()
    {
        var module = new FakeMac(values: new() { ["AppleInterfaceStyle"] = "Dark", ["AppleAccentColor"] = "5", ["reduceTransparency"] = "1" });
        Assert.Equal(new SystemLook(true, "purple", true, false), await module.ReadSystemAsync());
    }

    [Fact]
    public async Task Missing_keys_mean_light_and_blue()
    {
        var system = await new FakeMac().ReadSystemAsync();
        Assert.NotNull(system);
        Assert.False(system.Dark);
        Assert.Equal("blue", system.Accent);
    }

    [Fact]
    public async Task Off_macos() => Assert.Null(await new FakeMac(mac: false).ReadSystemAsync());
}

public sealed class LookTests
{
    static readonly SystemLook System = new(false, "purple", false, false);

    [Fact]
    public void System_wins_unless_settings_say_otherwise()
    {
        var look = MacosThemeModule.Effective(System, []);
        Assert.Equal((false, "purple", true), (look.Dark, look.Accent, look.Translucent));
        look = MacosThemeModule.Effective(System, new() { ["macosTheme.appearance"] = "dark", ["macosTheme.accent"] = "green", ["macosTheme.translucent"] = false });
        Assert.Equal((true, "green", false), (look.Dark, look.Accent, look.Translucent));
    }

    [Fact]
    public void Reduce_transparency_turns_translucency_off() =>
        Assert.False(MacosThemeModule.Effective(System with { ReduceTransparency = true }, []).Translucent);

    [Fact]
    public void Off_macos_defaults_to_dark_blue()
    {
        var look = MacosThemeModule.Effective(null, []);
        Assert.Equal((true, "blue"), (look.Dark, look.Accent));
    }

    [Fact]
    public void Unknown_or_empty_settings_fall_back()
    {
        var look = MacosThemeModule.Effective(System, new() { ["macosTheme.appearance"] = "", ["macosTheme.accent"] = "teal", ["macosTheme.translucent"] = null });
        Assert.Equal((false, "blue", true), (look.Dark, look.Accent, look.Translucent));
    }
}

public sealed partial class ThemeTests
{
    [GeneratedRegex("^(#[0-9a-f]{6}([0-9a-f]{2})?|transparent)$")]
    private static partial Regex Color();

    static void Check(JsonObject theme)
    {
        foreach (var (key, value) in theme["colors"]!.AsObject())
        {
            Assert.True(Color().IsMatch(value!.GetValue<string>()), key);
        }
        foreach (var (key, value) in theme["tokenColors"]!.AsObject())
        {
            var color = value is JsonObject style ? style["foreground"]! : value!;
            Assert.True(Color().IsMatch(color.GetValue<string>()), key);
        }
    }

    [Fact]
    public void Every_variant_is_valid()
    {
        foreach (var dark in new[] { true, false })
        {
            foreach (var (accent, _, _) in MacosThemeModule.Accents)
            {
                foreach (var translucent in new[] { true, false })
                {
                    foreach (var contrast in new[] { true, false })
                    {
                        Check(MacosThemeModule.BuildTheme(new Look(dark, accent, translucent, contrast)));
                    }
                }
            }
        }
    }

    [Fact]
    public void Light_purple_translucent()
    {
        var theme = MacosThemeModule.BuildTheme(new Look(false, "purple", true, false));
        Assert.Equal("light", theme["type"]!.GetValue<string>());
        Assert.Equal("ByteForge Light", theme["inherits"]!.GetValue<string>());
        Assert.Equal("#af52de", theme["colors"]!["accent"]!.GetValue<string>());
        Assert.Equal("#ffffff", theme["colors"]!["button.foreground"]!.GetValue<string>());
        Assert.Equal(9, theme["colors"]!["sideBar.background"]!.GetValue<string>().Length); // translucent
        Assert.Equal(7, theme["colors"]!["editor.background"]!.GetValue<string>().Length); // the editor stays opaque
        Assert.Equal("blur", theme["effects"]!["window.backdrop"]!.GetValue<string>());
        Assert.True(JsonNode.DeepEquals(new JsonObject { ["foreground"] = "#9b2393", ["fontStyle"] = "bold" }, theme["tokenColors"]!["keyword"]));
    }

    [Fact]
    public void Yellow_accent_gets_dark_button_text()
    {
        var theme = MacosThemeModule.BuildTheme(new Look(true, "yellow", false, false));
        Assert.Equal("#000000", theme["colors"]!["button.foreground"]!.GetValue<string>());
        Assert.Equal("none", theme["effects"]!["window.backdrop"]!.GetValue<string>());
        Assert.Equal(7, theme["colors"]!["sideBar.background"]!.GetValue<string>().Length);
    }

    [Fact]
    public void Colors_round_like_python()
    {
        // round() halves to even: 0.7 * 255 = 178.5 → 178 (b2); 0.12 blend of #0a84ff toward white.
        Assert.Equal("#0a84ffb2", MacosThemeModule.Alpha("#0a84ff", 0.7));
        Assert.Equal("#2793ff", MacosThemeModule.Mix("#0a84ff", "#ffffff", 0.12));
        Assert.Equal("#000000", MacosThemeModule.On("#ffd60a"));
        Assert.Equal("#ffffff", MacosThemeModule.On("#0a84ff"));
    }

    [Fact]
    public void Manifest_settings_match()
    {
        var manifest = JsonNode.Parse(File.ReadAllText(ModuleFolder.Manifest), documentOptions: new JsonDocumentOptions { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true })!;
        Assert.Equal("dotnet", manifest["runtime"]!.GetValue<string>());
        Assert.Equal("bin/MacosTheme.dll", manifest["assembly"]!.GetValue<string>());
        var accents = manifest["settings"]!.AsArray().Single(s => s!["key"]!.GetValue<string>() == "macosTheme.accent")!["choices"]!.AsArray().Select(c => c!.GetValue<string>());
        Assert.Equal(new HashSet<string>(["system", .. MacosThemeModule.Accents.Select(a => a.Name)]), accents.ToHashSet());
    }
}

static class ModuleFolder
{
    /// <summary>The module's module.json, found upwards from the test binaries.</summary>
    public static string Manifest
    {
        get
        {
            for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir != null; dir = dir.Parent)
            {
                var candidate = Path.Combine(dir.FullName, "module.json");
                if (File.Exists(candidate) && File.ReadAllText(candidate).Contains("\"byteforge.macos-theme\""))
                {
                    return candidate;
                }
            }
            throw new FileNotFoundException("module.json of the macOS Theme module");
        }
    }
}

public sealed class ModuleTests : IDisposable
{
    readonly string config = Directory.CreateTempSubdirectory("bf-macos-theme-").FullName;

    string ModuleDir => Path.Combine(config, "modules", "macos-theme");
    string ThemeFile => Path.Combine(ModuleDir, "themes", "macos.json");
    string LegacyFile => Path.Combine(config, "themes", "macos.json");

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

    ModuleHarness Start(MacosThemeModule? module = null)
    {
        Directory.CreateDirectory(ModuleDir);
        return ModuleHarness.Start(module ?? new FakeMac(mac: false), ModuleDir);
    }

    /// <summary>Runs "Regenerate" and waits for its answer: once it's there, start has finished.</summary>
    static async Task<string> Regenerate(ModuleHarness module)
    {
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "macosTheme.regenerate" });
        var log = await module.Next("log", m => m["text"]!.GetValue<string>() is "macOS Theme: updated from System Settings." or "macOS Theme: already up to date.");
        return log["text"]!.GetValue<string>();
    }

    JsonObject Theme() => JsonNode.Parse(ReadShared(ThemeFile))!.AsObject();

    /// <summary>Reads without blocking the module's replacing of the file (Windows).</summary>
    static string ReadShared(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    }

    void WriteLegacy(string name)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(LegacyFile)!);
        File.WriteAllText(LegacyFile, new JsonObject { ["name"] = name, ["type"] = "dark", ["colors"] = new JsonObject() }.ToJsonString());
    }

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = Start();
        foreach (var (id, title) in new[] { ("macosTheme.use", "Use macOS Theme"), ("macosTheme.regenerate", "Regenerate from System Settings") })
        {
            var command = await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == id);
            Assert.Equal(title, command["title"]!.GetValue<string>());
            Assert.Equal("macOS Theme", command["category"]!.GetValue<string>());
        }
        await module.Next("log", m => m["text"]!.GetValue<string>().StartsWith("macOS Theme: not on macOS"));
    }

    [Fact]
    public async Task Writes_the_theme_into_the_modules_own_themes_folder()
    {
        await using var module = Start();
        await Regenerate(module);
        Assert.True(File.Exists(ThemeFile));
        Assert.False(File.Exists(LegacyFile));
        Assert.Empty(Directory.GetFiles(Path.GetDirectoryName(ThemeFile)!, "*.tmp"));
        var theme = Theme();
        Assert.Equal("macOS", theme["name"]!.GetValue<string>());
        Assert.Equal("dark", theme["type"]!.GetValue<string>()); // not on macOS: Dark with the blue accent
        Assert.Equal("#0a84ff", theme["colors"]!["accent"]!.GetValue<string>());
    }

    [Fact]
    public async Task First_run_switches_to_the_theme_once()
    {
        await using (var module = Start())
        {
            var setTheme = await module.Next("setTheme");
            Assert.Equal("macOS", setTheme["name"]!.GetValue<string>());
        }
        // Started again (the theme file is there now): the user's theme choice is respected.
        await using (var module = Start())
        {
            await Regenerate(module);
            Assert.DoesNotContain(module.Sent, m => m["type"]!.GetValue<string>() == "setTheme");
        }
    }

    [Fact]
    public async Task Upgrading_removes_the_old_copy_without_switching_themes()
    {
        WriteLegacy("macOS");
        await using var module = Start();
        await Regenerate(module);
        Assert.False(File.Exists(LegacyFile));
        Assert.True(File.Exists(ThemeFile));
        Assert.Contains(module.Sent, m => m["type"]!.GetValue<string>() == "log" && m["text"]!.GetValue<string>().Contains("removed the old copy"));
        Assert.DoesNotContain(module.Sent, m => m["type"]!.GetValue<string>() == "setTheme");
    }

    [Fact]
    public async Task A_users_own_theme_with_that_file_name_is_kept()
    {
        WriteLegacy("My macOS");
        await using var module = Start();
        await Regenerate(module);
        Assert.True(File.Exists(LegacyFile));
        Assert.Equal("My macOS", JsonNode.Parse(File.ReadAllText(LegacyFile))!["name"]!.GetValue<string>());
        Assert.DoesNotContain(module.Sent, m => m["type"]!.GetValue<string>() == "log" && m["text"]!.GetValue<string>().Contains("removed the old copy"));
    }

    [Fact]
    public async Task Unchanged_content_isnt_rewritten()
    {
        await using var module = Start();
        Assert.Equal("macOS Theme: already up to date.", await Regenerate(module));
        var written = File.GetLastWriteTimeUtc(ThemeFile);
        File.SetLastWriteTimeUtc(ThemeFile, written.AddHours(-1));
        Assert.Equal("macOS Theme: already up to date.", await Regenerate(module));
        Assert.Equal(written.AddHours(-1), File.GetLastWriteTimeUtc(ThemeFile));
    }

    [Fact]
    public async Task Theme_file_is_rewritten_only_when_it_changes()
    {
        var module = new FakeMac(mac: false);
        await using var harness = Start(module);
        await Regenerate(harness);
        var theme = MacosThemeModule.BuildTheme(MacosThemeModule.Effective(new SystemLook(false, "purple", false, false), []));
        Assert.True(module.WriteTheme(theme));
        Assert.False(module.WriteTheme(theme));
        Assert.Equal("macOS", Theme()["name"]!.GetValue<string>());
    }

    [Fact]
    public async Task Settings_change_the_theme()
    {
        await using var module = Start();
        module.Activate(config, new JsonObject { ["macosTheme.appearance"] = "light", ["macosTheme.accent"] = "green", ["macosTheme.translucent"] = false });
        Assert.Equal("macOS Theme: already up to date.", await Regenerate(module));
        var theme = Theme();
        Assert.Equal("light", theme["type"]!.GetValue<string>());
        Assert.Equal("#34c759", theme["colors"]!["accent"]!.GetValue<string>());
        Assert.Equal("none", theme["effects"]!["window.backdrop"]!.GetValue<string>());

        module.Send(new() { ["type"] = "event", ["name"] = "settingsChanged", ["settings"] = new JsonObject { ["macosTheme.appearance"] = "dark" } });
        Assert.Equal("macOS Theme: already up to date.", await Regenerate(module));
        Assert.Equal("dark", Theme()["type"]!.GetValue<string>());
    }

    [Fact]
    public async Task Use_command_switches_to_the_theme()
    {
        await using var module = Start();
        await module.Next("setTheme"); // first run
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "macosTheme.use" });
        Assert.Equal("macOS", (await module.Next("setTheme"))["name"]!.GetValue<string>());
    }

    [Fact]
    public async Task Follows_system_settings_on_macos()
    {
        var mac = new FakeMac(values: new() { ["AppleAccentColor"] = "3" });
        await using var module = Start(mac);
        await Regenerate(module);
        Assert.DoesNotContain(module.Sent, m => m["type"]!.GetValue<string>() == "log" && m["text"]!.GetValue<string>().StartsWith("macOS Theme: not on macOS"));
        Assert.Equal("light", Theme()["type"]!.GetValue<string>());
        Assert.Equal("#34c759", Theme()["colors"]!["accent"]!.GetValue<string>());

        // Auto switched to Dark: the watcher notices without being told.
        lock (mac.Values)
        {
            mac.Values["AppleInterfaceStyle"] = "Dark";
        }
        var deadline = DateTime.UtcNow.AddSeconds(10);
        while (DateTime.UtcNow < deadline && !ReadShared(ThemeFile).Contains("\"type\": \"dark\""))
        {
            await Task.Delay(50);
        }
        Assert.Equal("dark", Theme()["type"]!.GetValue<string>());
        Assert.Equal("#30d158", Theme()["colors"]!["accent"]!.GetValue<string>());
    }
}
