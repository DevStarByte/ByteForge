using System.Text.Json.Nodes;
using ByteForge.CaelestiaTheme;
using ByteForge.Modules.Testing;

namespace ByteForge.CaelestiaTheme.Tests;

public sealed class CaelestiaThemeTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-caelestia-").FullName;
    readonly string moduleDir;
    readonly string schemePath;

    public CaelestiaThemeTests()
    {
        moduleDir = Directory.CreateDirectory(Path.Combine(dir, "modules", "caelestia-theme")).FullName;
        schemePath = Path.Combine(dir, "state", "caelestia", "scheme.json");
    }

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

    string ThemeFile => Path.Combine(moduleDir, "themes", "caelestia.json");
    string LegacyFile => Path.Combine(dir, "themes", "caelestia.json");

    /// <summary>A scheme.json as caelestia-shell writes it (Material You roles, terminal colors, a Catppuccin-style palette).</summary>
    public static JsonObject SampleScheme(string mode = "dark", string primary = "CBBEFF") => new()
    {
        ["name"] = "dynamic",
        ["flavour"] = "default",
        ["mode"] = mode,
        ["variant"] = "tonalspot",
        ["colours"] = new JsonObject
        {
            ["primary_paletteKeyColor"] = "7A6EB0",
            ["background"] = "141318",
            ["onBackground"] = "E6E1E9",
            ["surface"] = "141318",
            ["surfaceDim"] = "141318",
            ["surfaceBright"] = "3A383E",
            ["surfaceContainerLowest"] = "0F0D13",
            ["surfaceContainerLow"] = "1C1B20",
            ["surfaceContainer"] = "201F24",
            ["surfaceContainerHigh"] = "2B292F",
            ["surfaceContainerHighest"] = "36343A",
            ["onSurface"] = "E6E1E9",
            ["surfaceVariant"] = "48454E",
            ["onSurfaceVariant"] = "C9C4D0",
            ["inverseSurface"] = "E6E1E9",
            ["inverseOnSurface"] = "312F36",
            ["outline"] = "938F99",
            ["outlineVariant"] = "48454E",
            ["shadow"] = "000000",
            ["scrim"] = "000000",
            ["surfaceTint"] = primary,
            ["primary"] = primary,
            ["onPrimary"] = "33275E",
            ["primaryContainer"] = "4A3E76",
            ["onPrimaryContainer"] = "E7DEFF",
            ["inversePrimary"] = "62568F",
            ["secondary"] = "CAC3DC",
            ["onSecondary"] = "322E41",
            ["secondaryContainer"] = "494458",
            ["onSecondaryContainer"] = "E6DFF9",
            ["tertiary"] = "EEB8CA",
            ["onTertiary"] = "492534",
            ["tertiaryContainer"] = "B38394",
            ["onTertiaryContainer"] = "000000",
            ["error"] = "FFB4AB",
            ["onError"] = "690005",
            ["errorContainer"] = "93000A",
            ["onErrorContainer"] = "FFDAD6",
            ["success"] = "B5CCBA",
            ["onSuccess"] = "213528",
            ["term0"] = "353434",
            ["term1"] = "FF4C8A",
            ["term15"] = "FFFFFF",
            ["rosewater"] = "F5E0DC",
            ["flamingo"] = "F2CDCD",
            ["pink"] = "F5C2E7",
            ["mauve"] = "CBA6F7",
            ["red"] = "F38BA8",
            ["maroon"] = "EBA0AC",
            ["peach"] = "FAB387",
            ["yellow"] = "F9E2AF",
            ["green"] = "A6E3A1",
            ["teal"] = "94E2D5",
            ["sky"] = "89DCEB",
            ["sapphire"] = "74C7EC",
            ["blue"] = "89B4FA",
            ["lavender"] = "B4BEFE",
            ["klink"] = "7A6EB0",
            ["text"] = "E6E1E9",
            ["subtext1"] = "C9C4D0",
            ["overlay0"] = "938F99",
            ["base"] = "141318",
            ["mantle"] = "0F0D13",
            ["crust"] = "0A090D",
        },
    };

    void WriteScheme(JsonObject scheme)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(schemePath)!);
        File.WriteAllText(schemePath, scheme.ToJsonString());
    }

    CaelestiaThemeModule NewModule() => new() { SchemePath = schemePath, PollInterval = TimeSpan.FromMilliseconds(50) };

    /// <summary>Waits until the module has handled its start (commands run after it) and returns what it sent.</summary>
    static async Task<List<JsonObject>> Started(ModuleHarness module)
    {
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "caelestia.regenerate" });
        await module.Next("log", m => m["text"]!.GetValue<string>() is "Caelestia Theme: theme is up to date." or "Caelestia Theme: theme updated from the current scheme.");
        lock (module.Sent)
        {
            return [.. module.Sent];
        }
    }

    static bool SetTheme(List<JsonObject> sent) => sent.Any(m => m["type"]!.GetValue<string>() == "setTheme");

    static async Task Eventually(Func<bool> condition)
    {
        for (var waited = TimeSpan.Zero; !condition(); waited += TimeSpan.FromMilliseconds(50))
        {
            Assert.True(waited < TimeSpan.FromSeconds(15), "timed out");
            await Task.Delay(50);
        }
    }

    [Fact]
    public void Builds_the_theme_from_a_scheme()
    {
        var theme = CaelestiaThemeModule.BuildTheme(SampleScheme());
        Assert.Equal("Caelestia", theme["name"]!.GetValue<string>());
        Assert.Equal("dark", theme["type"]!.GetValue<string>());
        Assert.Equal("Follows your Caelestia scheme (dynamic · default · tonalspot). Generated by the Caelestia Theme module; edits are overwritten.",
            theme["description"]!.GetValue<string>());
        var colors = theme["colors"]!.AsObject();
        Assert.Equal(151, colors.Count);
        Assert.Equal("#cbbeff", colors["accent"]!.GetValue<string>());
        Assert.Equal("#0f0d13", colors["titleBar.background"]!.GetValue<string>()); // surfaceContainerLowest when dark
        Assert.Equal("#1c1b20", colors["editor.background"]!.GetValue<string>()); // surfaceContainerLow when dark
        Assert.Equal("#e6e1e91a", colors["toolbar.hoverBackground"]!.GetValue<string>());
        Assert.Equal("#fab387", colors["editorWarning.foreground"]!.GetValue<string>()); // peach when dark
        Assert.Equal("#cbbeff", colors["button.hoverBackground"]!.GetValue<string>()); // no primaryDim: primary
        Assert.Equal("transparent", colors["button.border"]!.GetValue<string>());
        var tokens = theme["tokenColors"]!.AsObject();
        Assert.Equal(49, tokens.Count);
        Assert.Equal("#cba6f7", tokens["keyword"]!.GetValue<string>());
        Assert.Equal("#938f99", tokens["comment"]!["foreground"]!.GetValue<string>());
        Assert.Equal("italic", tokens["comment"]!["fontStyle"]!.GetValue<string>());
    }

    [Fact]
    public void Light_schemes_and_missing_colours()
    {
        var scheme = SampleScheme(mode: "light");
        scheme["colours"]!.AsObject().Remove("error");
        scheme["colours"]!["primaryDim"] = "AABBCC";
        scheme["colours"]!["peach"] = 5; // not a string: ignored
        scheme["flavour"] = "";
        var theme = CaelestiaThemeModule.BuildTheme(scheme);
        var colors = theme["colors"]!.AsObject();
        Assert.Equal("light", theme["type"]!.GetValue<string>());
        Assert.Equal("#1c1b20", colors["titleBar.background"]!.GetValue<string>()); // surfaceContainerLow when light
        Assert.Equal("#141318", colors["editor.background"]!.GetValue<string>()); // surface when light
        Assert.Equal("#f9e2af", colors["editorWarning.foreground"]!.GetValue<string>()); // yellow when light
        Assert.Equal("#ff00ff", colors["errorForeground"]!.GetValue<string>());
        Assert.Equal("#ff00ff", colors["icon.markup"]!.GetValue<string>());
        Assert.Equal("#aabbcc", colors["button.hoverBackground"]!.GetValue<string>());
        Assert.Contains("(dynamic · tonalspot)", theme["description"]!.GetValue<string>());

        var empty = CaelestiaThemeModule.BuildTheme([]);
        Assert.Equal("dark", empty["type"]!.GetValue<string>());
        Assert.Contains("scheme ()", empty["description"]!.GetValue<string>());
    }

    [Fact]
    public async Task Fresh_install_registers_commands_writes_the_theme_into_its_own_folder_and_switches_once()
    {
        WriteScheme(SampleScheme());
        await using (var module = ModuleHarness.Start(NewModule(), moduleDir))
        {
            await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "caelestia.use" && m["title"]!.GetValue<string>() == "Use Caelestia Theme"
                && m["category"]!.GetValue<string>() == "Caelestia");
            await module.Next("registerCommand", m => m["id"]!.GetValue<string>() == "caelestia.regenerate" && m["title"]!.GetValue<string>() == "Regenerate Theme from Scheme"
                && m["category"]!.GetValue<string>() == "Caelestia");
            var set = await module.Next("setTheme");
            Assert.Equal("Caelestia", set["name"]!.GetValue<string>());
        }
        Assert.True(File.Exists(ThemeFile));
        Assert.False(File.Exists(LegacyFile));
        Assert.Empty(Directory.GetFiles(Path.GetDirectoryName(ThemeFile)!, "*.tmp"));
        var written = JsonNode.Parse(File.ReadAllText(ThemeFile))!;
        Assert.True(JsonNode.DeepEquals(CaelestiaThemeModule.BuildTheme(SampleScheme()), written));

        // The next start isn't a first run: the user's theme choice is respected.
        await using (var module = ModuleHarness.Start(NewModule(), moduleDir))
        {
            Assert.False(SetTheme(await Started(module)));
        }
    }

    [Fact]
    public async Task Upgrading_removes_the_old_copy_from_the_user_themes_folder_without_switching()
    {
        WriteScheme(SampleScheme());
        Directory.CreateDirectory(Path.GetDirectoryName(LegacyFile)!);
        File.WriteAllText(LegacyFile, CaelestiaThemeModule.BuildTheme(SampleScheme()).ToJsonString());
        await using var module = ModuleHarness.Start(NewModule(), moduleDir);
        await module.Next("log", m => m["text"]!.GetValue<string>().Contains("removed") && m["text"]!.GetValue<string>().Contains(LegacyFile));
        Assert.False(SetTheme(await Started(module)));
        Assert.False(File.Exists(LegacyFile));
        Assert.True(File.Exists(ThemeFile));
    }

    [Fact]
    public async Task A_users_own_theme_with_another_name_is_kept()
    {
        WriteScheme(SampleScheme());
        Directory.CreateDirectory(Path.GetDirectoryName(LegacyFile)!);
        const string mine = """{ "name": "My Caelestia", "type": "dark", "colors": {} }""";
        File.WriteAllText(LegacyFile, mine);
        await using var module = ModuleHarness.Start(NewModule(), moduleDir);
        var sent = await Started(module);
        Assert.Equal(mine, File.ReadAllText(LegacyFile));
        Assert.DoesNotContain(sent, m => m["type"]!.GetValue<string>() == "log" && m["text"]!.GetValue<string>().Contains("removed"));
        Assert.True(File.Exists(ThemeFile));
    }

    [Fact]
    public async Task Unchanged_content_is_not_rewritten()
    {
        WriteScheme(SampleScheme());
        var module = NewModule();
        await using var harness = ModuleHarness.Start(module, moduleDir);
        await Started(harness);
        var before = DateTime.UtcNow.AddHours(-1);
        File.SetLastWriteTimeUtc(ThemeFile, before);
        Assert.False(module.Generate());
        Assert.Equal(before, File.GetLastWriteTimeUtc(ThemeFile));

        harness.Send(new() { ["type"] = "executeCommand", ["id"] = "caelestia.regenerate" });
        await harness.Next("log", m => m["text"]!.GetValue<string>() == "Caelestia Theme: theme is up to date.");

        File.WriteAllText(ThemeFile, "edited");
        Assert.True(module.Generate());
        Assert.True(JsonNode.DeepEquals(CaelestiaThemeModule.BuildTheme(SampleScheme()), JsonNode.Parse(File.ReadAllText(ThemeFile))));
    }

    [Fact]
    public async Task A_changed_scheme_regenerates_the_theme()
    {
        WriteScheme(SampleScheme());
        await using var module = ModuleHarness.Start(NewModule(), moduleDir);
        await Started(module);
        Assert.Contains("#cbbeff", File.ReadAllText(ThemeFile));

        WriteScheme(SampleScheme(primary: "123ABC"));
        File.SetLastWriteTimeUtc(schemePath, DateTime.UtcNow.AddMinutes(1)); // a new time even on coarse file systems
        await Eventually(() => File.ReadAllText(ThemeFile).Contains("#123abc"));
        Assert.True(JsonNode.DeepEquals(CaelestiaThemeModule.BuildTheme(SampleScheme(primary: "123ABC")), JsonNode.Parse(File.ReadAllText(ThemeFile))));
    }

    [Fact]
    public async Task A_missing_scheme_is_reported_and_picked_up_once_it_appears()
    {
        await using var module = ModuleHarness.Start(NewModule(), moduleDir);
        var log = await module.Next("log");
        Assert.Equal($"Caelestia Theme: {schemePath} not found. Is caelestia-shell installed and running?", log["text"]!.GetValue<string>());

        // "Use Caelestia Theme" doesn't switch to a theme that couldn't be written.
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "caelestia.use" });
        await module.Next("log", m => m["text"]!.GetValue<string>().Contains("not found"));
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "caelestia.regenerate" });
        await module.Next("log", m => m["text"]!.GetValue<string>() == "Caelestia Theme: theme is up to date.");
        lock (module.Sent)
        {
            Assert.False(SetTheme(module.Sent));
        }
        Assert.False(File.Exists(ThemeFile));

        WriteScheme(SampleScheme());
        await Eventually(() => File.Exists(ThemeFile));
        module.Send(new() { ["type"] = "executeCommand", ["id"] = "caelestia.use" });
        Assert.Equal("Caelestia", (await module.Next("setTheme"))["name"]!.GetValue<string>());
    }

    [Fact]
    public async Task An_unreadable_scheme_is_reported()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(schemePath)!);
        File.WriteAllText(schemePath, "{ not json");
        await using var module = ModuleHarness.Start(NewModule(), moduleDir);
        var log = await module.Next("log");
        Assert.StartsWith($"Caelestia Theme: could not read {schemePath}: ", log["text"]!.GetValue<string>());
        Assert.False(File.Exists(ThemeFile));
    }

    [Fact]
    public void The_scheme_is_read_from_the_xdg_state_folder()
    {
        var path = CaelestiaThemeModule.DefaultSchemePath();
        var state = Environment.GetEnvironmentVariable("XDG_STATE_HOME");
        var expected = string.IsNullOrEmpty(state)
            ? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".local", "state", "caelestia", "scheme.json")
            : Path.Combine(state, "caelestia", "scheme.json");
        Assert.Equal(expected, path);
    }
}
