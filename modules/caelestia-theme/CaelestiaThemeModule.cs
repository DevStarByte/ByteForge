using System.Globalization;
using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.CaelestiaTheme;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<CaelestiaThemeModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// Turns the current Caelestia scheme (~/.local/state/caelestia/scheme.json, written by caelestia-shell whenever the
/// wallpaper or scheme changes) into a ByteForge theme called "Caelestia" in the module's own themes/ folder, so it
/// goes away with the module. ByteForge hot-reloads theme files, so while the theme is active the IDE follows every
/// scheme change live.
///
/// The scheme holds Material You roles (surface, primary, onSurface, …), terminal colors (term0-15) and a
/// Catppuccin-style palette (mauve, peach, green, …) derived from the wallpaper. UI chrome uses the Material roles,
/// syntax highlighting the Catppuccin-style palette, so the theme works for dark and light schemes alike.
/// </summary>
public sealed class CaelestiaThemeModule : Module
{
    public const string ThemeName = "Caelestia";
    public const string ThemeFileName = "caelestia.json";

    static readonly JsonSerializerOptions Indented = new()
    {
        WriteIndented = true,
        NewLine = "\n",
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    };

    readonly Lock writing = new();

    /// <summary>The scheme file caelestia-shell writes (settable for tests).</summary>
    public string SchemePath { get; set; } = DefaultSchemePath();

    /// <summary>How often the scheme file is checked for changes (settable for tests).</summary>
    public TimeSpan PollInterval { get; set; } = TimeSpan.FromSeconds(1);

    /// <summary>Where the theme is written: the module's own themes/ folder, which ByteForge lists while the module is enabled.</summary>
    public string ThemeFile => Path.Combine(ModuleDir, "themes", ThemeFileName);

    /// <summary>Where versions before 2.0.0 wrote the theme: ByteForge's general user themes folder.</summary>
    public string LegacyThemeFile => Path.Combine(ConfigDir, "themes", ThemeFileName);

    public static string DefaultSchemePath()
    {
        var state = Environment.GetEnvironmentVariable("XDG_STATE_HOME");
        if (string.IsNullOrEmpty(state))
        {
            state = Path.Combine(Programs.Home, ".local", "state");
        }
        return Path.Combine(state, "caelestia", "scheme.json");
    }

    protected override Task OnStartAsync()
    {
        // Upgrading from a version that wrote to <config>/themes/ isn't a fresh install: don't switch themes then.
        var firstRun = !File.Exists(ThemeFile) && !File.Exists(LegacyThemeFile);
        RemoveLegacyTheme();
        RegisterCommand("caelestia.use", "Use Caelestia Theme", "Caelestia");
        RegisterCommand("caelestia.regenerate", "Regenerate Theme from Scheme", "Caelestia");
        var stamp = Stamp();
        Generate();
        if (firstRun && File.Exists(ThemeFile))
        {
            // Just installed: switch to the theme once. Afterwards the user's theme choice is respected.
            SetTheme(ThemeName);
        }
        _ = WatchAsync(stamp);
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id)
    {
        switch (id)
        {
            case "caelestia.use":
                Generate();
                if (File.Exists(ThemeFile))
                {
                    SetTheme(ThemeName);
                }
                break;
            case "caelestia.regenerate":
                Log(Generate() ? "Caelestia Theme: theme updated from the current scheme." : "Caelestia Theme: theme is up to date.");
                break;
        }
        return Task.CompletedTask;
    }

    /// <summary>Deletes the copy an older version of this module left in the user themes folder (a user's own file is kept).</summary>
    void RemoveLegacyTheme()
    {
        var legacy = LegacyThemeFile;
        try
        {
            if (!File.Exists(legacy) || JsonNode.Parse(File.ReadAllText(legacy)) is not JsonObject theme
                || theme["name"] is not JsonValue name || !name.TryGetValue<string>(out var value) || value != ThemeName)
            {
                return;
            }
            File.Delete(legacy);
            Log($"Caelestia Theme: removed {legacy} left by an older version; the theme now lives in the module's own folder.");
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
        }
    }

    /// <summary>Polls the scheme file; caelestia-shell rewrites it on every wallpaper or scheme change.</summary>
    async Task WatchAsync(long? last)
    {
        try
        {
            while (true)
            {
                await Task.Delay(PollInterval, Stopping);
                var stamp = Stamp();
                if (stamp != null && stamp != last)
                {
                    last = stamp;
                    Generate();
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    /// <summary>The scheme file's modification time, or null when it doesn't exist.</summary>
    long? Stamp()
    {
        try
        {
            var info = new FileInfo(SchemePath);
            return info.Exists ? info.LastWriteTimeUtc.Ticks : null;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException)
        {
            return null;
        }
    }

    /// <summary>Writes the theme from the current scheme. Returns true when the file changed.</summary>
    public bool Generate()
    {
        var path = SchemePath;
        JsonObject theme;
        try
        {
            if (JsonNode.Parse(File.ReadAllText(path)) is not JsonObject scheme)
            {
                throw new JsonException("the scheme isn't a JSON object");
            }
            theme = BuildTheme(scheme);
        }
        catch (Exception e) when (e is FileNotFoundException or DirectoryNotFoundException)
        {
            Log($"Caelestia Theme: {path} not found. Is caelestia-shell installed and running?");
            return false;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
            Log($"Caelestia Theme: could not read {path}: {e.Message}");
            return false;
        }
        var text = theme.ToJsonString(Indented) + "\n";
        var target = ThemeFile;
        lock (writing)
        {
            try
            {
                if (File.Exists(target) && File.ReadAllText(target) == text)
                {
                    return false;
                }
                var dir = Path.GetDirectoryName(target)!;
                Directory.CreateDirectory(dir);
                var tmp = Path.Combine(dir, $"{ThemeFileName}.{Guid.NewGuid():N}.tmp");
                try
                {
                    File.WriteAllText(tmp, text);
                    File.Move(tmp, target, overwrite: true); // atomic, so ByteForge never reads a half-written theme
                }
                finally
                {
                    File.Delete(tmp);
                }
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                Log($"Caelestia Theme: could not write {target}: {e.Message}");
                return false;
            }
        }
        return true;
    }

    /// <summary>The theme for a Caelestia scheme (the content of scheme.json).</summary>
    public static JsonObject BuildTheme(JsonObject scheme)
    {
        var c = new Dictionary<string, string>();
        if (scheme["colours"] is JsonObject colours)
        {
            foreach (var (key, value) in colours)
            {
                if (value is JsonValue v && v.TryGetValue<string>(out var s))
                {
                    c[key] = s.ToLowerInvariant();
                }
            }
        }
        var light = scheme["mode"] is JsonValue mode && mode.TryGetValue<string>(out var m) && m == "light";

        string col(string key, string fallback = "ff00ff") =>
            "#" + (c.TryGetValue(key, out var v) ? v : c.TryGetValue(fallback, out var f) ? f : fallback);
        string alpha(string key, string a) => col(key) + a;

        var bg = col("surface");
        var chrome = col(!light ? "surfaceContainerLowest" : "surfaceContainerLow");
        var editor = col(!light ? "surfaceContainerLow" : "surface");
        var widget = col("surfaceContainerHigh");
        var field = col("surfaceContainerHighest");
        var border = col("outlineVariant");
        var fg = col("onSurface");
        var muted = col("onSurfaceVariant");
        var accent = col("primary");
        var warning = col(light ? "yellow" : "peach");

        var colors = new JsonObject
        {
            ["accent"] = accent,
            ["focusBorder"] = accent,
            ["window.background"] = bg,
            ["window.foreground"] = fg,
            ["foreground.muted"] = muted,
            ["icon.foreground"] = muted,
            ["toolbar.hoverBackground"] = alpha("onSurface", "1a"),
            ["sash.hoverBorder"] = accent,

            ["titleBar.background"] = chrome,
            ["titleBar.foreground"] = fg,
            ["titleBar.inactiveForeground"] = muted,
            ["titleBar.border"] = border,
            ["titleBar.logo"] = accent,
            ["titleIcon.activeForeground"] = accent,
            ["commandCenter.background"] = widget,
            ["commandCenter.foreground"] = muted,
            ["commandCenter.activeForeground"] = fg,
            ["commandCenter.border"] = border,
            ["commandCenter.hoverBorder"] = col("outline"),
            ["commandCenter.hoverBackground"] = field,
            ["windowControl.foreground"] = fg,
            ["windowControl.hoverBackground"] = alpha("onSurface", "1a"),
            ["windowControl.closeHoverBackground"] = col("error"),
            ["windowControl.closeHoverForeground"] = col("onError"),
            ["menu.background"] = widget,
            ["menu.foreground"] = fg,
            ["menu.selectionBackground"] = col("secondaryContainer"),
            ["menu.border"] = border,

            ["activityBar.background"] = chrome,
            ["activityBar.foreground"] = accent,
            ["activityBar.inactiveForeground"] = muted,
            ["activityBar.activeBorder"] = accent,
            ["activityBar.border"] = border,
            ["activityBarBadge.background"] = accent,
            ["activityBarBadge.foreground"] = col("onPrimary"),

            ["sideBar.background"] = bg,
            ["sideBar.foreground"] = fg,
            ["sideBar.border"] = border,
            ["sideBarTitle.foreground"] = accent,
            ["sideBarSectionHeader.foreground"] = col("secondary"),
            ["list.hoverBackground"] = alpha("onSurface", "12"),
            ["list.activeSelectionBackground"] = col("secondaryContainer"),
            ["list.activeSelectionForeground"] = col("onSecondaryContainer"),
            ["list.activeFileBackground"] = col("secondaryContainer"),
            ["list.activeFileForeground"] = col("onSecondaryContainer"),
            ["list.activeFileIndicator"] = accent,
            ["terminalCursor.foreground"] = accent,
            ["terminal.selectionBackground"] = alpha("primary", "40"),

            ["icon.folder"] = col("tertiary"),
            ["icon.file"] = muted,
            ["icon.csharp"] = col("primary"),
            ["icon.project"] = col("secondary"),
            ["icon.json"] = col("yellow"),
            ["icon.markup"] = col("peach"),
            ["icon.markdown"] = col("sapphire"),
            ["icon.docker"] = col("sky"),

            ["editorGroupHeader.background"] = bg,
            ["editorGroupHeader.border"] = border,
            ["tab.activeBackground"] = editor,
            ["tab.activeForeground"] = fg,
            ["tab.activeBorder"] = border,
            ["tab.inactiveBackground"] = bg,
            ["tab.inactiveForeground"] = muted,
            ["tab.hoverBackground"] = editor,
            ["tab.border"] = border,
            ["tab.activeBorderTop"] = accent,
            ["tab.modifiedIndicator"] = accent,
            ["breadcrumb.background"] = editor,
            ["breadcrumb.foreground"] = muted,
            ["breadcrumb.activeForeground"] = fg,

            ["editor.background"] = editor,
            ["editor.foreground"] = fg,
            ["editor.lineHighlightBackground"] = alpha("onSurface", "0d"),
            ["editor.lineHighlightBorder"] = "transparent",
            ["editor.selectionBackground"] = alpha("primary", "40"),
            ["editor.inactiveSelectionBackground"] = alpha("primary", "20"),
            ["editor.findMatchBackground"] = alpha("tertiary", "50"),
            ["editor.findMatchBorder"] = col("tertiary"),
            ["editor.wordHighlightBackground"] = alpha("onSurface", "1f"),
            ["editor.wordHighlightStrongBackground"] = alpha("primary", "30"),
            ["editorInlayHint.foreground"] = col("outline"),
            ["editorInlayHint.background"] = alpha("onSurface", "0f"),
            ["editorCursor.foreground"] = accent,
            ["editorCursor.background"] = editor,
            ["editorLineNumber.foreground"] = col("outline"),
            ["editorLineNumber.activeForeground"] = accent,
            ["editorGutter.background"] = editor,
            ["editorIndentGuide.background"] = border,
            ["editorWhitespace.foreground"] = alpha("outline", "60"),
            ["editorRuler.foreground"] = border,
            ["editorBracketMatch.background"] = alpha("primary", "20"),
            ["editorBracketMatch.border"] = accent,
            ["editorBracketHighlight.foreground1"] = col("primary"),
            ["editorBracketHighlight.foreground2"] = col("tertiary"),
            ["editorBracketHighlight.foreground3"] = col("secondary"),
            ["editorBracketHighlight.unexpectedBracket.foreground"] = col("error"),
            ["editorError.foreground"] = col("error"),
            ["editorWarning.foreground"] = warning,
            ["editorInfo.foreground"] = col("sapphire"),
            ["editorWidget.background"] = widget,
            ["editorWidget.border"] = border,
            ["editorHoverWidget.background"] = widget,
            ["editorHoverWidget.foreground"] = fg,
            ["editorHoverWidget.border"] = border,

            ["editorGutter.addedBackground"] = col("success"),
            ["editorGutter.modifiedBackground"] = col("primary"),
            ["editorGutter.deletedBackground"] = col("error"),
            ["diffEditor.insertedLineBackground"] = alpha("success", "26"),
            ["diffEditor.removedLineBackground"] = alpha("error", "26"),
            ["diffEditor.hunkHeaderBackground"] = alpha("primary", "1f"),
            ["gitDecoration.addedResourceForeground"] = col("success"),
            ["gitDecoration.modifiedResourceForeground"] = warning,
            ["gitDecoration.stageModifiedResourceForeground"] = warning,
            ["gitDecoration.deletedResourceForeground"] = col("error"),
            ["gitDecoration.renamedResourceForeground"] = col("teal"),
            ["gitDecoration.untrackedResourceForeground"] = col("success"),
            ["gitDecoration.conflictingResourceForeground"] = col("error"),
            ["badge.background"] = col("secondaryContainer"),
            ["badge.foreground"] = col("onSecondaryContainer"),
            ["errorForeground"] = col("error"),

            ["panel.background"] = bg,
            ["panel.border"] = border,
            ["panelTitle.activeForeground"] = fg,
            ["panelTitle.inactiveForeground"] = muted,
            ["panelTitle.activeBorder"] = accent,
            ["problemsErrorIcon.foreground"] = col("error"),
            ["problemsWarningIcon.foreground"] = warning,
            ["problemsInfoIcon.foreground"] = col("sapphire"),

            ["statusBar.background"] = chrome,
            ["statusBar.debuggingBackground"] = col("tertiaryContainer"),
            ["statusBar.foreground"] = muted,
            ["statusBar.border"] = border,
            ["statusBarItem.hoverBackground"] = alpha("onSurface", "1a"),

            ["input.background"] = field,
            ["input.foreground"] = fg,
            ["input.border"] = border,
            ["input.placeholderForeground"] = col("outline"),
            ["button.background"] = accent,
            ["button.foreground"] = col("onPrimary"),
            ["button.hoverBackground"] = col("primaryDim", "primary"),
            ["button.secondaryBackground"] = field,
            ["button.border"] = "transparent",
            ["scrollbarSlider.background"] = alpha("outline", "50"),
            ["scrollbarSlider.hoverBackground"] = alpha("outline", "80"),
            ["scrollbarSlider.activeBackground"] = alpha("primary", "80"),
            ["quickInput.background"] = widget,
            ["quickInput.foreground"] = fg,
            ["quickInput.border"] = border,

            ["welcome.logo"] = alpha("primary", "40"),
            ["welcome.titleForeground"] = accent,
            ["welcome.foreground"] = muted,
        };

        string keyword = col("mauve"), control = col("tertiary");
        string types = col("yellow"), method = col("blue");
        string str = col("green"), number = col("peach");
        string member = col("lavender"), param = col("maroon");
        var comment = col("outline");
        var tokens = new JsonObject
        {
            ["keyword"] = keyword,
            ["keyword - control"] = control,
            ["preprocessor keyword"] = muted,
            ["preprocessor text"] = muted,
            ["comment"] = new JsonObject { ["foreground"] = comment, ["fontStyle"] = "italic" },
            ["xml doc comment"] = comment,
            ["xml doc comment - name"] = types,
            ["xml doc comment - attribute name"] = member,
            ["excluded code"] = col("outlineVariant"),
            ["string"] = str,
            ["string - verbatim"] = str,
            ["string - escape character"] = col("pink"),
            ["number"] = number,
            ["operator"] = col("sky"),
            ["operator - overloaded"] = method,
            ["punctuation"] = muted,
            ["identifier"] = fg,
            ["namespace name"] = col("rosewater"),
            ["class name"] = types,
            ["record class name"] = types,
            ["struct name"] = col("teal"),
            ["record struct name"] = col("teal"),
            ["interface name"] = col("sapphire"),
            ["enum name"] = col("sapphire"),
            ["delegate name"] = types,
            ["type parameter name"] = col("teal"),
            ["module name"] = types,
            ["method name"] = method,
            ["extension method name"] = method,
            ["property name"] = member,
            ["field name"] = member,
            ["event name"] = member,
            ["local name"] = fg,
            ["parameter name"] = param,
            ["label name"] = muted,
            ["constant name"] = number,
            ["enum member name"] = number,
            ["regex - character class"] = col("pink"),
            ["regex - anchor"] = control,
            ["regex - quantifier"] = number,
            ["json - property name"] = accent,
            ["json - string"] = str,
            ["html element name"] = keyword,
            ["html attribute name"] = member,
            ["html attribute value"] = str,
            ["html tag delimiter"] = muted,
            ["razor transition"] = control,
            ["razor directive"] = control,
            ["xml - msbuild reference"] = method,
        };

        var flavour = string.Join(" · ", new[] { "name", "flavour", "variant" }.Where(k => Truthy(scheme[k])).Select(k => Text(scheme[k]!)));
        return new JsonObject
        {
            ["name"] = ThemeName,
            ["type"] = light ? "light" : "dark",
            ["description"] = $"Follows your Caelestia scheme ({flavour}). Generated by the Caelestia Theme module; edits are overwritten.",
            ["colors"] = colors,
            ["tokenColors"] = tokens,
        };
    }

    /// <summary>Whether a JSON value counts as set (Python truthiness: not null, "", 0, false, [] or {}).</summary>
    static bool Truthy(JsonNode? node) => node switch
    {
        null => false,
        JsonObject o => o.Count > 0,
        JsonArray a => a.Count > 0,
        JsonValue v when v.GetValueKind() == JsonValueKind.String => v.GetValue<string>().Length > 0,
        JsonValue v when v.GetValueKind() == JsonValueKind.True => true,
        JsonValue v when v.GetValueKind() == JsonValueKind.False => false,
        JsonValue v when v.GetValueKind() == JsonValueKind.Number => v.GetValue<double>() != 0,
        _ => true,
    };

    /// <summary>A scheme value as text (strings as they are, like Python's str()).</summary>
    static string Text(JsonNode node) => node.GetValueKind() switch
    {
        JsonValueKind.String => node.GetValue<string>(),
        JsonValueKind.True => "True",
        JsonValueKind.False => "False",
        JsonValueKind.Number => node.GetValue<double>().ToString(CultureInfo.InvariantCulture),
        _ => node.ToJsonString(),
    };
}
