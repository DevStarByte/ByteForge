using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.MacosTheme;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<MacosThemeModule>(input, output, moduleDir, cancel);
}

/// <summary>What macOS is set to (System Settings).</summary>
public sealed record SystemLook(bool Dark, string Accent, bool ReduceTransparency, bool IncreaseContrast);

/// <summary>The look to build: from macOS and the module's settings.</summary>
public sealed record Look(bool Dark, string Accent, bool Translucent, bool Contrast);

/// <summary>
/// A theme that looks like macOS and follows its settings live: writes a theme called "macOS" into the module's own
/// themes/ folder from what macOS is set to right now: Light or Dark appearance (also when it switches
/// automatically), the accent color, Reduce Transparency and Increase Contrast. ByteForge hot-reloads theme files,
/// so while the theme is active the IDE follows every change in System Settings.
///
/// The look: Xcode's Default (Dark) / Default (Light) syntax colors, SF Pro and SF Mono, a translucent sidebar and
/// title bar (window vibrancy), the accent in selections, buttons and menus, rounded controls.
///
/// On another system the theme is still written (dark, blue accent, or what the settings say), so the look can be
/// used anywhere.
/// </summary>
public class MacosThemeModule : Module
{
    public const string ThemeName = "macOS";
    public const string ThemeFileName = "macos.json";
    static readonly SystemLook Fallback = new(true, "blue", false, false);

    /// <summary>NSColor system accent colors: (dark appearance, light appearance), in the order of the settings.</summary>
    public static readonly IReadOnlyList<(string Name, string Dark, string Light)> Accents =
    [
        ("blue", "#0a84ff", "#007aff"),
        ("purple", "#bf5af2", "#af52de"),
        ("pink", "#ff375f", "#ff2d55"),
        ("red", "#ff453a", "#ff3b30"),
        ("orange", "#ff9f0a", "#ff9500"),
        ("yellow", "#ffd60a", "#ffcc00"),
        ("green", "#30d158", "#34c759"),
        ("graphite", "#8e8e93", "#8e8e93"),
    ];

    /// <summary>AppleAccentColor: -1 graphite, 0 red, 1 orange, 2 yellow, 3 green, (absent) blue or multicolor, 5 purple, 6 pink.</summary>
    static readonly Dictionary<string, string> AccentCodes = new()
    {
        ["-1"] = "graphite", ["0"] = "red", ["1"] = "orange", ["2"] = "yellow", ["3"] = "green", ["4"] = "blue", ["5"] = "purple", ["6"] = "pink",
    };

    static readonly Dictionary<string, string> DarkPalette = new()
    {
        ["window"] = "#1e1e1e", ["chrome"] = "#2a2a2d", ["sidebar"] = "#232325", ["editor"] = "#1f1f24", ["widget"] = "#2c2c2e",
        ["field"] = "#ffffff14", ["border"] = "#ffffff1a", ["strong"] = "#ffffff38", ["fg"] = "#dfdfe0", ["muted"] = "#98989d",
        ["faint"] = "#6e6e73", ["line"] = "#23252b", ["selection"] = "#646f83", ["hover"] = "#ffffff12",
        ["red"] = "#ff453a", ["orange"] = "#ff9f0a", ["yellow"] = "#ffd60a", ["green"] = "#30d158", ["teal"] = "#40c8e0",
        ["blue"] = "#0a84ff", ["purple"] = "#bf5af2", ["pink"] = "#ff375f",
    };

    static readonly Dictionary<string, string> LightPalette = new()
    {
        ["window"] = "#ececec", ["chrome"] = "#f6f6f6", ["sidebar"] = "#e8e8e8", ["editor"] = "#ffffff", ["widget"] = "#f6f6f6",
        ["field"] = "#ffffff", ["border"] = "#0000001a", ["strong"] = "#00000040", ["fg"] = "#262626", ["muted"] = "#6e6e73",
        ["faint"] = "#a1a1a6", ["line"] = "#ecf5ff", ["selection"] = "#b2d7ff", ["hover"] = "#0000000d",
        ["red"] = "#ff3b30", ["orange"] = "#ff9500", ["yellow"] = "#ffcc00", ["green"] = "#34c759", ["teal"] = "#30b0c7",
        ["blue"] = "#007aff", ["purple"] = "#af52de", ["pink"] = "#ff2d55",
    };

    // Xcode's Default (Dark) and Default (Light) themes.
    static readonly Dictionary<string, string> XcodeDark = new()
    {
        ["plain"] = "#dfdfe0", ["comment"] = "#6c7986", ["doc"] = "#92a1b1", ["string"] = "#fc6a5d", ["number"] = "#d0bf69",
        ["keyword"] = "#fc5fa3", ["preprocessor"] = "#fd8f3f", ["attribute"] = "#bf8555", ["typeDecl"] = "#5dd8ff",
        ["otherDecl"] = "#41a1c0", ["type"] = "#9ef1dd", ["method"] = "#67b7a4", ["member"] = "#67b7a4", ["otherType"] = "#d0a8ff",
        ["otherMethod"] = "#a167e6", ["url"] = "#5482ff",
    };

    static readonly Dictionary<string, string> XcodeLight = new()
    {
        ["plain"] = "#262626", ["comment"] = "#5d6c79", ["doc"] = "#4a5560", ["string"] = "#c41a16", ["number"] = "#1c00cf",
        ["keyword"] = "#9b2393", ["preprocessor"] = "#643820", ["attribute"] = "#815f03", ["typeDecl"] = "#0b4f79",
        ["otherDecl"] = "#0f68a0", ["type"] = "#1c464a", ["method"] = "#326d74", ["member"] = "#326d74", ["otherType"] = "#3900a0",
        ["otherMethod"] = "#6c36a9", ["url"] = "#0e0eff",
    };

    static readonly JsonSerializerOptions Indented = new()
    {
        WriteIndented = true,
        IndentSize = 2,
        NewLine = "\n",
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    };

    /// <summary>Generating and writing take turns: settings, commands and the watcher can all ask at once.</summary>
    readonly SemaphoreSlim writing = new(1, 1);

    /// <summary>The theme file: in the module's own themes/ folder, so it goes away with the module.</summary>
    public string ThemeFile => Path.Combine(ModuleDir, "themes", ThemeFileName);

    /// <summary>Where versions before 0.2.0 wrote the theme: the user's themes folder (left behind on uninstall).</summary>
    public string LegacyThemeFile => Path.Combine(ConfigDir, "themes", ThemeFileName);

    /// <summary>How often macOS's settings are read: the appearance switches by itself with Auto, and nothing tells us.</summary>
    protected virtual TimeSpan PollInterval => TimeSpan.FromSeconds(3);

    /// <summary>Whether System Settings can be read (macOS); tests override it.</summary>
    protected virtual bool IsMac => OperatingSystem.IsMacOS();

    // ───────────── module ─────────────

    protected override async Task OnStartAsync()
    {
        var legacyExisted = File.Exists(LegacyThemeFile);
        var firstRun = !File.Exists(ThemeFile) && !legacyExisted;
        RemoveLegacyTheme();
        RegisterCommand("macosTheme.use", "Use macOS Theme", "macOS Theme");
        RegisterCommand("macosTheme.regenerate", "Regenerate from System Settings", "macOS Theme");
        if (!IsMac)
        {
            Log("macOS Theme: not on macOS, so the theme uses the settings (or Dark with the blue accent).");
        }
        await GenerateAsync();
        if (firstRun && File.Exists(ThemeFile))
        {
            // Just installed: switch to the theme once. Afterwards the user's theme choice is respected.
            SetTheme(ThemeName);
        }
        if (IsMac)
        {
            _ = Task.Run(WatchAsync);
        }
    }

    protected override Task OnActivateAsync() => GenerateAsync();

    protected override Task OnSettingsChangedAsync() => GenerateAsync();

    protected override async Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "macosTheme.use":
                await GenerateAsync();
                if (File.Exists(ThemeFile))
                {
                    SetTheme(ThemeName);
                }
                break;
            case "macosTheme.regenerate":
                Log(await GenerateAsync() ? "macOS Theme: updated from System Settings." : "macOS Theme: already up to date.");
                break;
        }
    }

    /// <summary>Deletes the copy an older version wrote into the user's themes folder (only if it's this module's theme).</summary>
    void RemoveLegacyTheme()
    {
        var legacy = LegacyThemeFile;
        try
        {
            if (!File.Exists(legacy) || JsonNode.Parse(File.ReadAllText(legacy))?["name"] is not JsonValue name
                || !name.TryGetValue<string>(out var value) || value != ThemeName)
            {
                return;
            }
            File.Delete(legacy);
            Log($"macOS Theme: removed the old copy of the theme from {legacy}; it now lives in the module's folder.");
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
            // Not readable or not ours: leave it alone.
        }
    }

    /// <summary>Builds the theme from what macOS is set to now and the settings; true when the file changed.</summary>
    public async Task<bool> GenerateAsync()
    {
        var system = await ReadSystemAsync();
        return await GenerateAsync(system);
    }

    async Task<bool> GenerateAsync(SystemLook? system)
    {
        await writing.WaitAsync();
        try
        {
            return WriteTheme(BuildTheme(Effective(system, Settings)));
        }
        finally
        {
            writing.Release();
        }
    }

    /// <summary>Polls macOS's settings and regenerates when they change; stops with the module.</summary>
    async Task WatchAsync()
    {
        try
        {
            var last = await ReadSystemAsync();
            while (true)
            {
                await Task.Delay(PollInterval, Stopping);
                var now = await ReadSystemAsync();
                if (now != last)
                {
                    last = now;
                    await GenerateAsync(now);
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception e)
        {
            Log($"macOS Theme: watching System Settings failed: {e}");
        }
    }

    /// <summary>Writes the theme file atomically. Returns true when it changed.</summary>
    public bool WriteTheme(JsonObject theme)
    {
        var file = ThemeFile;
        var text = Serialize(theme);
        try
        {
            if (File.Exists(file) && File.ReadAllText(file) == text)
            {
                return false;
            }
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            var tmp = file + ".tmp";
            File.WriteAllText(tmp, text);
            File.Move(tmp, file, overwrite: true); // ByteForge never reads a half-written theme
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"macOS Theme: could not write {file}: {e.Message}");
            return false;
        }
        return true;
    }

    /// <summary>The theme file's text: indented JSON with a final newline.</summary>
    public static string Serialize(JsonObject theme) => theme.ToJsonString(Indented) + "\n";

    // ───────────── reading macOS ─────────────

    /// <summary>What macOS is set to, or null off macOS.</summary>
    public async Task<SystemLook?> ReadSystemAsync()
    {
        if (!IsMac)
        {
            return null;
        }
        var accentCode = await DefaultsAsync("-g", "AppleAccentColor");
        return new SystemLook(
            // Only set when the appearance is Dark at the moment (with Auto too).
            Dark: await DefaultsAsync("-g", "AppleInterfaceStyle") == "Dark",
            // Absent for blue and for multicolor (which shows blue in apps without their own accent).
            Accent: AccentCodes.GetValueOrDefault(string.IsNullOrEmpty(accentCode) ? "4" : accentCode, "blue"),
            ReduceTransparency: await DefaultsAsync("com.apple.universalaccess", "reduceTransparency") == "1",
            IncreaseContrast: await DefaultsAsync("com.apple.universalaccess", "increaseContrast") == "1");
    }

    /// <summary><c>defaults read domain key</c>: the value, or null when it isn't set (or can't be read).</summary>
    protected virtual async Task<string?> DefaultsAsync(string domain, string key)
    {
        var program = Programs.Which("defaults") ?? (File.Exists("/usr/bin/defaults") ? "/usr/bin/defaults" : null);
        if (program == null)
        {
            return null;
        }
        try
        {
            var result = await Programs.RunAsync(program, ["read", domain, key], timeout: TimeSpan.FromSeconds(5), cancel: Stopping);
            return result.Success ? result.Output.Trim() : null;
        }
        catch (Exception e) when (e is ModuleException or TimeoutException)
        {
            return null;
        }
    }

    // ───────────── the look ─────────────

    /// <summary>The look to build, from macOS and the module's settings (which win when they aren't "system").</summary>
    public static Look Effective(SystemLook? system, JsonObject settings)
    {
        system ??= Fallback;
        var appearance = Choice(settings["macosTheme.appearance"]);
        var accent = Choice(settings["macosTheme.accent"]);
        // Only an explicit false turns translucency off.
        var translucentSetting = !(settings["macosTheme.translucent"] is JsonValue t && t.TryGetValue<bool>(out var on) && !on);
        return new Look(
            Dark: appearance == "system" ? system.Dark : appearance == "dark",
            Accent: accent == "system" ? system.Accent : Accents.Any(a => a.Name == accent) ? accent : "blue",
            Translucent: translucentSetting && !system.ReduceTransparency,
            Contrast: system.IncreaseContrast);
    }

    /// <summary>A choice setting: "system" when missing or empty (or another false-like value), "" for a value of another type.</summary>
    static string Choice(JsonNode? node) => node switch
    {
        null => "system",
        JsonValue v when v.TryGetValue<string>(out var s) => s.Length == 0 ? "system" : s,
        JsonValue v when v.TryGetValue<bool>(out var b) => b ? "" : "system",
        JsonValue v when v.TryGetValue<double>(out var d) => d == 0 ? "system" : "",
        JsonArray a => a.Count == 0 ? "system" : "",
        JsonObject o => o.Count == 0 ? "system" : "",
        _ => "",
    };

    // ───────────── colors ─────────────

    static (int R, int G, int B) Rgb(string color)
    {
        var c = color.TrimStart('#');
        return (Convert.ToInt32(c[..2], 16), Convert.ToInt32(c[2..4], 16), Convert.ToInt32(c[4..6], 16));
    }

    public static double Luminance(string color)
    {
        static double Channel(int value)
        {
            var v = value / 255.0;
            return v <= 0.03928 ? v / 12.92 : Math.Pow((v + 0.055) / 1.055, 2.4);
        }
        var (r, g, b) = Rgb(color);
        return 0.2126 * Channel(r) + 0.7152 * Channel(g) + 0.0722 * Channel(b);
    }

    /// <summary>Black or white text, whichever reads better on the color.</summary>
    public static string On(string color) => Luminance(color) > 0.4 ? "#000000" : "#ffffff";

    /// <summary><paramref name="a"/> blended toward <paramref name="b"/> by <paramref name="t"/> (0..1).</summary>
    public static string Mix(string a, string b, double t)
    {
        var (ar, ag, ab) = Rgb(a);
        var (br, bg, bb) = Rgb(b);
        return "#" + Blend(ar, br) + Blend(ag, bg) + Blend(ab, bb);

        // Python's round(): halves to even, as Math.Round does.
        string Blend(int x, int y) => ((int)Math.Round(x + (y - x) * t)).ToString("x2");
    }

    public static string Alpha(string color, double a) => color[..7] + ((int)Math.Round(a * 255)).ToString("x2");

    // ───────────── the theme ─────────────

    public static JsonObject BuildTheme(Look look)
    {
        var dark = look.Dark;
        var p = new Dictionary<string, string>(dark ? DarkPalette : LightPalette);
        var accentEntry = Accents.First(a => a.Name == look.Accent);
        var accent = dark ? accentEntry.Dark : accentEntry.Light;
        if (look.Contrast)
        {
            (p["border"], p["muted"]) = (p["strong"], Mix(p["muted"], p["fg"], 0.4));
        }
        var translucent = look.Translucent;
        // With vibrancy the window shows the desktop through the chrome; the editor stays opaque.
        string Glass(string c, double a) => translucent ? Alpha(c, a) : c[..7];
        string window = Glass(p["window"], 0.72), chrome = Glass(p["chrome"], 0.78), sidebar = Glass(p["sidebar"], 0.62);
        var selectionList = Alpha(accent, dark ? 0.35 : 0.22);
        var xc = dark ? XcodeDark : XcodeLight;
        var blue = Accents[0];

        var colors = new JsonObject
        {
            ["accent"] = accent,
            ["focusBorder"] = Alpha(accent, 0.7),
            ["window.background"] = window,
            ["window.foreground"] = p["fg"],
            ["foreground.muted"] = p["muted"],
            ["icon.foreground"] = p["muted"],
            ["toolbar.hoverBackground"] = p["hover"],
            ["sash.hoverBorder"] = accent,
            ["overlay.background"] = dark ? "#00000059" : "#00000026",

            ["titleBar.background"] = chrome,
            ["titleBar.foreground"] = p["fg"],
            ["titleBar.inactiveForeground"] = p["faint"],
            ["titleBar.border"] = p["border"],
            ["titleBar.logo"] = accent,
            ["titleIcon.activeForeground"] = accent,
            ["commandCenter.background"] = dark ? p["field"] : "#00000008",
            ["commandCenter.foreground"] = p["muted"],
            ["commandCenter.activeForeground"] = p["fg"],
            ["commandCenter.border"] = p["border"],
            ["commandCenter.hoverBorder"] = p["strong"],
            ["commandCenter.hoverBackground"] = p["hover"],
            ["windowControl.foreground"] = p["fg"],
            ["windowControl.hoverBackground"] = p["hover"],
            ["windowControl.closeHoverBackground"] = p["red"],
            ["windowControl.closeHoverForeground"] = "#ffffff",
            // macOS menus highlight the hovered item with the accent color.
            ["menu.background"] = Alpha(p["widget"], 0.96),
            ["menu.foreground"] = p["fg"],
            ["menu.selectionBackground"] = accent,
            ["menu.border"] = p["border"],

            ["activityBar.background"] = sidebar,
            ["activityBar.foreground"] = accent,
            ["activityBar.activeIconForeground"] = accent,
            ["activityBar.inactiveForeground"] = p["muted"],
            ["activityBar.activeBorder"] = "transparent",
            ["activityBar.activeBackground"] = p["hover"],
            ["activityBar.hoverBackground"] = p["hover"],
            ["activityBar.border"] = p["border"],
            ["activityBarBadge.background"] = p["red"],
            ["activityBarBadge.foreground"] = "#ffffff",

            ["sideBar.background"] = sidebar,
            ["sideBar.foreground"] = p["fg"],
            ["sideBar.border"] = p["border"],
            ["sideBarTitle.foreground"] = p["muted"],
            ["sideBarSectionHeader.foreground"] = !look.Contrast ? p["faint"] : p["muted"],
            ["list.hoverBackground"] = p["hover"],
            ["list.activeSelectionBackground"] = selectionList,
            ["list.activeSelectionForeground"] = p["fg"],
            ["list.activeFileBackground"] = selectionList,
            ["list.activeFileForeground"] = p["fg"],
            ["list.activeFileIndicator"] = "transparent",

            ["icon.folder"] = dark ? blue.Dark : blue.Light,
            ["icon.file"] = p["muted"],
            ["icon.csharp"] = p["purple"],
            ["icon.project"] = p["blue"],
            ["icon.solution"] = p["purple"],
            ["icon.json"] = p["yellow"],
            ["icon.markup"] = p["orange"],
            ["icon.markdown"] = p["teal"],
            ["icon.razor"] = p["pink"],
            ["icon.style"] = p["blue"],
            ["icon.yaml"] = p["red"],
            ["icon.docker"] = p["teal"],

            ["editorGroupHeader.background"] = chrome,
            ["editorGroupHeader.border"] = p["border"],
            ["tab.activeBackground"] = p["editor"],
            ["tab.activeForeground"] = p["fg"],
            ["tab.activeBorder"] = p["border"],
            ["tab.activeBorderTop"] = "transparent",
            ["tab.inactiveBackground"] = "transparent",
            ["tab.inactiveForeground"] = p["muted"],
            ["tab.hoverBackground"] = p["hover"],
            ["tab.border"] = p["border"],
            ["tab.modifiedIndicator"] = p["muted"],
            ["breadcrumb.background"] = p["editor"],
            ["breadcrumb.foreground"] = p["muted"],
            ["breadcrumb.activeForeground"] = p["fg"],
            ["breadcrumb.border"] = p["border"],

            ["editor.background"] = p["editor"],
            ["editor.foreground"] = xc["plain"],
            ["editor.lineHighlightBackground"] = p["line"],
            ["editor.lineHighlightBorder"] = "transparent",
            ["editor.selectionBackground"] = !dark ? p["selection"] : Alpha(p["selection"], 0.8),
            ["editor.inactiveSelectionBackground"] = Alpha(p["selection"], 0.45),
            ["editor.findMatchBackground"] = dark ? "#ffd60a80" : "#ffe50080",
            ["editor.findMatchBorder"] = p["yellow"],
            ["editor.wordHighlightBackground"] = Alpha(p["selection"], 0.35),
            ["editor.wordHighlightStrongBackground"] = Alpha(accent, 0.3),
            ["editorInlayHint.foreground"] = p["faint"],
            ["editorInlayHint.background"] = p["hover"],
            ["editorCursor.foreground"] = xc["plain"],
            ["editorCursor.background"] = p["editor"],
            ["editorLineNumber.foreground"] = dark ? "#747478" : "#a6a6a6",
            ["editorLineNumber.activeForeground"] = p["fg"],
            ["editorGutter.background"] = p["editor"],
            ["editorIndentGuide.background"] = p["border"],
            ["editorWhitespace.foreground"] = Alpha(p["faint"], 0.5),
            ["editorRuler.foreground"] = p["border"],
            ["editorBracketMatch.background"] = Alpha(accent, 0.2),
            ["editorBracketMatch.border"] = Alpha(accent, 0.6),
            ["editorBracketHighlight.foreground1"] = xc["keyword"],
            ["editorBracketHighlight.foreground2"] = xc["typeDecl"],
            ["editorBracketHighlight.foreground3"] = xc["number"],
            ["editorBracketHighlight.unexpectedBracket.foreground"] = p["red"],
            ["editorError.foreground"] = p["red"],
            ["editorWarning.foreground"] = dark ? p["yellow"] : p["orange"],
            ["editorInfo.foreground"] = p["blue"],
            ["editorWidget.background"] = Alpha(p["widget"], 0.97),
            ["editorWidget.border"] = p["border"],
            ["editorHoverWidget.background"] = Alpha(p["widget"], 0.97),
            ["editorHoverWidget.foreground"] = p["fg"],
            ["editorHoverWidget.border"] = p["border"],

            ["editorGutter.addedBackground"] = p["green"],
            ["editorGutter.modifiedBackground"] = p["blue"],
            ["editorGutter.deletedBackground"] = p["red"],
            ["diffEditor.insertedLineBackground"] = Alpha(p["green"], 0.15),
            ["diffEditor.removedLineBackground"] = Alpha(p["red"], 0.15),
            ["diffEditor.hunkHeaderBackground"] = Alpha(p["blue"], 0.12),
            ["gitDecoration.addedResourceForeground"] = p["green"],
            ["gitDecoration.modifiedResourceForeground"] = p["orange"],
            ["gitDecoration.stageModifiedResourceForeground"] = p["orange"],
            ["gitDecoration.deletedResourceForeground"] = p["red"],
            ["gitDecoration.renamedResourceForeground"] = p["teal"],
            ["gitDecoration.untrackedResourceForeground"] = p["green"],
            ["gitDecoration.conflictingResourceForeground"] = p["red"],
            ["badge.background"] = Alpha(p["fg"], 0.15),
            ["badge.foreground"] = p["fg"],
            ["errorForeground"] = p["red"],

            ["panel.background"] = p["editor"],
            ["panel.border"] = p["border"],
            ["panelTitle.activeForeground"] = p["fg"],
            ["panelTitle.activeBackground"] = p["hover"],
            ["panelTitle.inactiveForeground"] = p["muted"],
            ["panelTitle.activeBorder"] = "transparent",
            ["problemsErrorIcon.foreground"] = p["red"],
            ["problemsWarningIcon.foreground"] = dark ? p["yellow"] : p["orange"],
            ["problemsInfoIcon.foreground"] = p["blue"],

            ["statusBar.background"] = chrome,
            ["statusBar.debuggingBackground"] = p["orange"],
            ["statusBar.foreground"] = p["muted"],
            ["statusBar.border"] = p["border"],
            ["statusBarItem.hoverBackground"] = p["hover"],
            ["statusBarItem.branch.background"] = "transparent",
            ["statusBarItem.branch.foreground"] = p["fg"],

            ["input.background"] = p["field"],
            ["input.foreground"] = p["fg"],
            ["input.border"] = p["border"],
            ["input.placeholderForeground"] = p["faint"],
            ["button.background"] = accent,
            ["button.foreground"] = On(accent),
            ["button.hoverBackground"] = Mix(accent, dark ? "#ffffff" : "#000000", 0.12),
            ["button.secondaryBackground"] = dark ? p["field"] : "#0000000d",
            ["button.border"] = "transparent",
            ["scrollbarSlider.background"] = Alpha(p["fg"], 0.25),
            ["scrollbarSlider.hoverBackground"] = Alpha(p["fg"], 0.4),
            ["scrollbarSlider.activeBackground"] = Alpha(p["fg"], 0.5),
            ["quickInput.background"] = Alpha(p["widget"], 0.97),
            ["quickInput.foreground"] = p["fg"],
            ["quickInput.border"] = p["border"],
            ["terminalCursor.foreground"] = p["fg"],
            ["terminal.selectionBackground"] = Alpha(p["selection"], 0.6),

            ["welcome.logo"] = Alpha(accent, 0.3),
            ["welcome.titleForeground"] = p["fg"],
            ["welcome.foreground"] = p["muted"],
        };

        static JsonObject Bold(string c) => new() { ["foreground"] = c, ["fontStyle"] = "bold" };
        var tokens = new JsonObject
        {
            ["keyword"] = Bold(xc["keyword"]),
            ["keyword - control"] = Bold(xc["keyword"]),
            ["preprocessor keyword"] = xc["preprocessor"],
            ["preprocessor text"] = xc["preprocessor"],
            ["comment"] = xc["comment"],
            ["xml doc comment"] = xc["doc"],
            ["xml doc comment - name"] = Bold(xc["doc"]),
            ["xml doc comment - attribute name"] = xc["doc"],
            ["excluded code"] = xc["comment"],
            ["string"] = xc["string"],
            ["string - verbatim"] = xc["string"],
            ["string - escape character"] = xc["number"],
            ["number"] = xc["number"],
            ["operator"] = xc["plain"],
            ["operator - overloaded"] = xc["method"],
            ["punctuation"] = xc["plain"],
            ["identifier"] = xc["plain"],
            ["namespace name"] = xc["plain"],
            ["class name"] = xc["type"],
            ["record class name"] = xc["type"],
            ["struct name"] = xc["type"],
            ["record struct name"] = xc["type"],
            ["interface name"] = xc["type"],
            ["enum name"] = xc["type"],
            ["delegate name"] = xc["type"],
            ["type parameter name"] = xc["otherType"],
            ["module name"] = xc["type"],
            ["method name"] = xc["method"],
            ["extension method name"] = xc["method"],
            ["property name"] = xc["member"],
            ["field name"] = xc["member"],
            ["event name"] = xc["member"],
            ["local name"] = xc["plain"],
            ["parameter name"] = xc["plain"],
            ["label name"] = xc["plain"],
            ["constant name"] = xc["member"],
            ["enum member name"] = xc["member"],
            ["regex - character class"] = xc["number"],
            ["regex - anchor"] = xc["keyword"],
            ["regex - quantifier"] = xc["number"],
            ["json - property name"] = xc["typeDecl"],
            ["json - string"] = xc["string"],
            ["html element name"] = xc["keyword"],
            ["html attribute name"] = xc["attribute"],
            ["html attribute value"] = xc["string"],
            ["html tag delimiter"] = xc["plain"],
            ["razor transition"] = xc["preprocessor"],
            ["razor directive"] = xc["preprocessor"],
            ["xml - msbuild reference"] = xc["otherDecl"],
        };

        var fonts = new JsonObject
        {
            ["ui.family"] = "SF Pro Text, SF Pro, .AppleSystemUIFont, Helvetica Neue, default",
            ["ui.size"] = 13,
            ["ui.smallSize"] = 11,
            ["tab.fontSize"] = 12,
            ["tab.activeWeight"] = "medium",
            ["list.fontSize"] = 13,
            ["statusBar.fontSize"] = 11,
            ["sideBarTitle.fontSize"] = 11,
            ["sideBarTitle.weight"] = "semibold",
            ["sideBarTitle.textTransform"] = "none",
            ["sideBarSectionHeader.fontSize"] = 11,
            ["sideBarSectionHeader.weight"] = "bold",
            ["sideBarSectionHeader.textTransform"] = "none",
            ["editor.family"] = "SF Mono, Menlo, Monaco, JetBrains Mono, default",
            ["editor.size"] = 13,
            ["editor.lineHeight"] = 1.45,
            ["editor.lineNumberSize"] = 12,
        };
        var metrics = new JsonObject
        {
            ["commandCenter.radius"] = 6,
            ["windowControl.radius"] = 6,
            ["activityBar.itemRadius"] = 6,
            ["activityBar.indicatorLeft.border"] = 0,
            ["activityBar.indicatorRight.border"] = 0,
            ["list.radius"] = 5,
            ["list.itemMargin"] = "8,1",
            ["tab.radius"] = 6,
            ["tab.indicatorSize"] = 0,
            ["panelTab.radius"] = 5,
            ["statusBarItem.radius"] = 4,
            ["widget.radius"] = 10,
            ["palette.radius"] = 12,
            ["input.radius"] = 6,
            ["button.radius"] = 6,
            ["scrollbar.size"] = 10,
            ["scrollbar.radius"] = 5,
            ["scrollbar.thumbMargin"] = 2,
            ["titleBar.borderThickness"] = 1,
        };
        return new JsonObject
        {
            ["name"] = ThemeName,
            ["type"] = dark ? "dark" : "light",
            ["inherits"] = dark ? "ByteForge Dark" : "ByteForge Light",
            ["description"] = $"macOS {(dark ? "Dark" : "Light")} with the {look.Accent} accent"
                + $"{(translucent ? ", translucent" : "")}{(look.Contrast ? ", increased contrast" : "")}. "
                + "Generated by the macOS Theme module from System Settings; edits are overwritten.",
            ["colors"] = colors,
            ["tokenColors"] = tokens,
            ["fonts"] = fonts,
            ["metrics"] = metrics,
            ["editor"] = new JsonObject { ["selectionRadius"] = 3, ["lineHighlight"] = "fill", ["cursorStyle"] = "line", ["cursorWidth"] = 2 },
            ["effects"] = new JsonObject
            {
                ["window.backdrop"] = translucent ? "blur" : "none",
                ["palette.shadow"] = dark ? "0 20 50 0 #00000080" : "0 20 50 0 #00000038",
                ["widget.shadow"] = dark ? "0 8 24 0 #00000060" : "0 8 24 0 #00000026",
            },
        };
    }
}
