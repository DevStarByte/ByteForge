using System.Globalization;
using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace ByteForge.WindowsTheme;

/// <summary>An accent color and the shades Windows derives from it, from the lightest to the darkest.</summary>
public sealed record Palette(string Light3, string Light2, string Light1, string Base, string Dark1, string Dark2, string Dark3);

/// <summary>What Windows is set to (see <see cref="SystemReader"/>).</summary>
public sealed record SystemLook(bool Dark, Palette Palette, bool Transparency, bool AccentTitleBar, bool HighContrast, bool Windows11);

/// <summary>The look to build: Windows' settings with the module's settings applied.</summary>
public sealed record Look(bool Dark, Palette Palette, bool Translucent, bool AccentTitleBar, bool Contrast, bool Windows11);

/// <summary>
/// The "Windows" theme: Fluent colors on Mica (Windows 11; acrylic on Windows 10), Visual Studio 2022's Dark / Light
/// syntax colors, Segoe UI Variable and Cascadia Mono, rounded controls and wide caption buttons.
/// </summary>
public static class ThemeBuilder
{
    public const string ThemeName = "Windows";

    /// <summary>Windows' own accent presets (Settings &gt; Personalization &gt; Colors), for the "accent" setting.</summary>
    public static readonly IReadOnlyDictionary<string, string> Accents = new Dictionary<string, string>
    {
        ["blue"] = "#0078d4", ["purple"] = "#8764b8", ["pink"] = "#e3008c", ["red"] = "#e81123", ["orange"] = "#ca5010",
        ["yellow"] = "#ffb900", ["green"] = "#107c10", ["teal"] = "#038387", ["gray"] = "#69797e",
    };

    // ───────────── look ─────────────

    /// <summary>What's used off Windows (or when Windows can't be read): Dark with the default blue.</summary>
    public static SystemLook DefaultSystem => new(true, Shades(Accents["blue"]), true, false, false, true);

    /// <summary>The look to build, from Windows and the module's settings (which win when they aren't "system").</summary>
    public static Look Effective(SystemLook? system, JsonObject settings)
    {
        system ??= DefaultSystem;
        var appearance = Choice(settings, "windowsTheme.appearance");
        var accent = Choice(settings, "windowsTheme.accent");
        var title = Choice(settings, "windowsTheme.accentTitleBar");
        var translucentSetting = !(settings["windowsTheme.translucent"] is JsonValue t && t.GetValueKind() == JsonValueKind.False);
        return new Look(
            appearance == "system" ? system.Dark : appearance == "dark",
            accent == "system" ? system.Palette : Shades(Accents.GetValueOrDefault(accent, Accents["blue"])),
            translucentSetting && system.Transparency,
            title == "system" ? system.AccentTitleBar : title == "on",
            system.HighContrast,
            system.Windows11);
    }

    /// <summary>
    /// A choice setting as the Python module read it (<c>settings.get(key) or "system"</c>): missing, null, false,
    /// 0 and "" mean "system"; another non-string value matches no choice.
    /// </summary>
    static string Choice(JsonObject settings, string key)
    {
        if (settings[key] is not JsonValue value)
        {
            return settings[key] is null ? "system" : "\0";
        }
        switch (value.GetValueKind())
        {
            case JsonValueKind.String:
                var text = value.GetValue<string>();
                return text.Length == 0 ? "system" : text;
            case JsonValueKind.False or JsonValueKind.Null:
                return "system";
            case JsonValueKind.Number:
                return value.TryGetValue<double>(out var number) && number == 0 ? "system" : "\0";
            default:
                return "\0";
        }
    }

    // ───────────── colors ─────────────

    static (int R, int G, int B) Rgb(string color)
    {
        var c = color.TrimStart('#');
        return (Hex(c, 0), Hex(c, 2), Hex(c, 4));
    }

    static int Hex(string text, int at) => int.Parse(text.AsSpan(at, 2), NumberStyles.HexNumber, CultureInfo.InvariantCulture);

    public static double Luminance(string color)
    {
        static double Channel(double v)
        {
            v /= 255;
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
        // Math.Round rounds half to even, like Python's round().
        static string Channel(int x, int y, double t) => ((int)Math.Round(x + (y - x) * t)).ToString("x2", CultureInfo.InvariantCulture);
        return "#" + Channel(ar, br, t) + Channel(ag, bg, t) + Channel(ab, bb, t);
    }

    public static string Alpha(string color, double a) => color[..Math.Min(7, color.Length)] + ((int)Math.Round(a * 255)).ToString("x2", CultureInfo.InvariantCulture);

    static string Opaque(string color) => color[..Math.Min(7, color.Length)];

    /// <summary>The shades Windows derives from an accent color (close to its AccentPalette).</summary>
    public static Palette Shades(string @base) => new(
        Mix(@base, "#ffffff", 0.75), Mix(@base, "#ffffff", 0.5), Mix(@base, "#ffffff", 0.25), @base,
        Mix(@base, "#000000", 0.2), Mix(@base, "#000000", 0.4), Mix(@base, "#000000", 0.6));

    static readonly Dictionary<string, string> DarkColors = new()
    {
        ["window"] = "#202020", ["layer"] = "#2b2b2b", ["editor"] = "#1f1f1f", ["widget"] = "#2c2c2c", ["field"] = "#ffffff0f",
        ["border"] = "#ffffff14", ["strong"] = "#ffffff33", ["fg"] = "#ffffff", ["muted"] = "#c5c5c5", ["faint"] = "#9d9d9d",
        ["line"] = "#282828", ["selection"] = "#264f78", ["hover"] = "#ffffff0f",
        ["red"] = "#ff99a4", ["orange"] = "#fce100", ["yellow"] = "#fce100", ["green"] = "#6ccb5f", ["teal"] = "#4cc2c4",
        ["blue"] = "#60cdff", ["purple"] = "#b4a0ff", ["pink"] = "#ff8fd4",
    };

    static readonly Dictionary<string, string> LightColors = new()
    {
        ["window"] = "#f3f3f3", ["layer"] = "#fbfbfb", ["editor"] = "#ffffff", ["widget"] = "#f9f9f9", ["field"] = "#ffffffb3",
        ["border"] = "#0000000f", ["strong"] = "#00000029", ["fg"] = "#1b1b1b", ["muted"] = "#5f5f5f", ["faint"] = "#8a8a8a",
        ["line"] = "#f5f5f5", ["selection"] = "#add6ff", ["hover"] = "#0000000a",
        ["red"] = "#c42b1c", ["orange"] = "#9d5d00", ["yellow"] = "#9d5d00", ["green"] = "#0f7b0f", ["teal"] = "#007f7f",
        ["blue"] = "#005fb8", ["purple"] = "#6b4fbb", ["pink"] = "#bf0077",
    };

    // Visual Studio 2022's Dark and Light themes.
    static readonly Dictionary<string, string> VsDark = new()
    {
        ["plain"] = "#dcdcdc", ["keyword"] = "#569cd6", ["control"] = "#d8a0df", ["preprocessor"] = "#9b9b9b", ["comment"] = "#57a64a",
        ["doc"] = "#608b4e", ["docTag"] = "#808080", ["string"] = "#d69d85", ["escape"] = "#ffd68f", ["number"] = "#b5cea8",
        ["operator"] = "#b4b4b4", ["type"] = "#4ec9b0", ["struct"] = "#86c691", ["interface"] = "#b8d7a3", ["method"] = "#dcdcaa",
        ["local"] = "#9cdcfe", ["member"] = "#dcdcdc", ["html"] = "#569cd6", ["attribute"] = "#9cdcfe", ["excluded"] = "#9b9b9b",
    };

    static readonly Dictionary<string, string> VsLight = new()
    {
        ["plain"] = "#000000", ["keyword"] = "#0000ff", ["control"] = "#8f08c4", ["preprocessor"] = "#808080", ["comment"] = "#008000",
        ["doc"] = "#008000", ["docTag"] = "#808080", ["string"] = "#a31515", ["escape"] = "#b776fb", ["number"] = "#000000",
        ["operator"] = "#000000", ["type"] = "#2b91af", ["struct"] = "#2b91af", ["interface"] = "#2b91af", ["method"] = "#74531f",
        ["local"] = "#1f377f", ["member"] = "#000000", ["html"] = "#800000", ["attribute"] = "#ff0000", ["excluded"] = "#808080",
    };

    // ───────────── the theme ─────────────

    public static JsonObject Build(Look look)
    {
        var dark = look.Dark;
        var p = new Dictionary<string, string>(dark ? DarkColors : LightColors);
        var shade = look.Palette;
        // Fluent's AccentFillColorDefault: Light 2 on dark, Dark 1 on light; AccentTextFillColor alike.
        var accent = dark ? shade.Light2 : shade.Dark1;
        if (look.Contrast)
        {
            (p["border"], p["muted"]) = (p["strong"], Mix(p["muted"], p["fg"], 0.5));
        }
        var translucent = look.Translucent;
        string Glass(string c, double a) => translucent ? Alpha(c, a) : Opaque(c);
        var window = Glass(p["window"], 0.7);
        var chrome = Glass(p["window"], 0.7);
        var sidebar = Glass(p["window"], 0.55);
        var accentTitle = look.AccentTitleBar;
        if (accentTitle)
        {
            chrome = dark ? shade.Dark2 : shade.Base;
        }
        var titleFg = accentTitle ? On(Opaque(chrome)) : p["fg"];
        var vs = dark ? VsDark : VsLight;
        var selected = Alpha(dark ? "#ffffff" : "#000000", 0.06);

        var colors = new JsonObject
        {
            ["accent"] = accent,
            ["focusBorder"] = accent,
            ["window.background"] = window,
            ["window.foreground"] = p["fg"],
            ["foreground.muted"] = p["muted"],
            ["icon.foreground"] = p["muted"],
            ["toolbar.hoverBackground"] = p["hover"],
            ["sash.hoverBorder"] = accent,
            ["overlay.background"] = dark ? "#0000004d" : "#0000001f",

            ["titleBar.background"] = chrome,
            ["titleBar.foreground"] = titleFg,
            ["titleBar.inactiveForeground"] = Alpha(titleFg, 0.55),
            ["titleBar.border"] = accentTitle ? "transparent" : p["border"],
            ["titleBar.logo"] = accentTitle ? titleFg : accent,
            ["titleIcon.activeForeground"] = accentTitle ? titleFg : accent,
            ["commandCenter.background"] = accentTitle ? Alpha(titleFg, 0.08) : p["field"],
            ["commandCenter.foreground"] = accentTitle ? Alpha(titleFg, 0.75) : p["muted"],
            ["commandCenter.activeForeground"] = titleFg,
            ["commandCenter.border"] = accentTitle ? Alpha(titleFg, 0.12) : p["border"],
            ["commandCenter.hoverBorder"] = p["strong"],
            ["commandCenter.hoverBackground"] = accentTitle ? Alpha(titleFg, 0.12) : p["hover"],
            ["windowControl.foreground"] = titleFg,
            ["windowControl.hoverBackground"] = Alpha(titleFg, 0.1),
            ["windowControl.closeHoverBackground"] = "#c42b1c",
            ["windowControl.closeHoverForeground"] = "#ffffff",
            ["menu.background"] = Glass(p["widget"], 0.96),
            ["menu.foreground"] = p["fg"],
            ["menu.selectionBackground"] = !dark ? p["hover"] : "#ffffff14",
            ["menu.border"] = p["border"],

            ["activityBar.background"] = sidebar,
            ["activityBar.foreground"] = p["fg"],
            ["activityBar.activeIconForeground"] = accent,
            ["activityBar.inactiveForeground"] = p["muted"],
            ["activityBar.activeBorder"] = accent,
            ["activityBar.activeBackground"] = selected,
            ["activityBar.hoverBackground"] = p["hover"],
            ["activityBar.border"] = "transparent",
            ["activityBarBadge.background"] = accent,
            ["activityBarBadge.foreground"] = On(accent),

            // Navigation pane and lists: subtle fill with the accent pill on the left of the selected row.
            ["sideBar.background"] = sidebar,
            ["sideBar.foreground"] = p["fg"],
            ["sideBar.border"] = "transparent",
            ["sideBarTitle.foreground"] = p["fg"],
            ["sideBarSectionHeader.foreground"] = p["muted"],
            ["list.hoverBackground"] = p["hover"],
            ["list.activeSelectionBackground"] = selected,
            ["list.activeSelectionForeground"] = p["fg"],
            ["list.activeFileBackground"] = selected,
            ["list.activeFileForeground"] = p["fg"],
            ["list.activeFileIndicator"] = accent,

            ["icon.folder"] = dark ? "#ffc83d" : "#e8a317",
            ["icon.file"] = p["muted"],
            ["icon.csharp"] = dark ? "#a179dc" : "#68217a",
            ["icon.project"] = accent,
            ["icon.solution"] = dark ? "#a179dc" : "#68217a",
            ["icon.json"] = dark ? "#fce100" : "#9d5d00",
            ["icon.markup"] = dark ? "#ff8c00" : "#ca5010",
            ["icon.markdown"] = p["teal"],
            ["icon.razor"] = dark ? "#ff8fd4" : "#bf0077",
            ["icon.style"] = p["blue"],
            ["icon.yaml"] = p["red"],
            ["icon.docker"] = "#2496ed",

            ["editorGroupHeader.background"] = Glass(p["window"], 0.7),
            ["editorGroupHeader.border"] = p["border"],
            ["tab.activeBackground"] = p["editor"],
            ["tab.activeForeground"] = p["fg"],
            ["tab.activeBorder"] = p["border"],
            ["tab.activeBorderTop"] = "transparent",
            ["tab.inactiveBackground"] = "transparent",
            ["tab.inactiveForeground"] = p["muted"],
            ["tab.hoverBackground"] = p["hover"],
            ["tab.border"] = "transparent",
            ["tab.modifiedIndicator"] = p["muted"],
            ["breadcrumb.background"] = p["editor"],
            ["breadcrumb.foreground"] = p["muted"],
            ["breadcrumb.activeForeground"] = p["fg"],
            ["breadcrumb.border"] = p["border"],

            ["editor.background"] = p["editor"],
            ["editor.foreground"] = vs["plain"],
            ["editor.lineHighlightBackground"] = p["line"],
            ["editor.lineHighlightBorder"] = dark ? "#46464680" : "#eeeeee",
            ["editor.selectionBackground"] = p["selection"],
            ["editor.inactiveSelectionBackground"] = Alpha(p["selection"], 0.5),
            ["editor.findMatchBackground"] = dark ? "#77381880" : "#f6b94d80",
            ["editor.findMatchBorder"] = dark ? "#f6b94d" : "#c08000",
            ["editor.wordHighlightBackground"] = dark ? "#36363680" : "#e0e0e0",
            ["editor.wordHighlightStrongBackground"] = Alpha(accent, 0.25),
            ["editorInlayHint.foreground"] = p["faint"],
            ["editorInlayHint.background"] = p["hover"],
            ["editorCursor.foreground"] = vs["plain"],
            ["editorCursor.background"] = p["editor"],
            ["editorLineNumber.foreground"] = !dark ? "#2b91af" : "#858585",
            ["editorLineNumber.activeForeground"] = p["fg"],
            ["editorGutter.background"] = p["editor"],
            ["editorIndentGuide.background"] = p["border"],
            ["editorWhitespace.foreground"] = Alpha(p["faint"], 0.5),
            ["editorRuler.foreground"] = p["border"],
            ["editorBracketMatch.background"] = !dark ? "#0064001a" : "#0e4583",
            ["editorBracketMatch.border"] = "transparent",
            ["editorBracketHighlight.foreground1"] = dark ? "#ffd700" : "#0431fa",
            ["editorBracketHighlight.foreground2"] = dark ? "#da70d6" : "#319331",
            ["editorBracketHighlight.foreground3"] = dark ? "#179fff" : "#7b3814",
            ["editorBracketHighlight.unexpectedBracket.foreground"] = p["red"],
            ["editorError.foreground"] = dark ? "#fc3e36" : "#e51400",
            ["editorWarning.foreground"] = dark ? "#95db7d" : "#008000",
            ["editorInfo.foreground"] = p["blue"],
            ["editorWidget.background"] = Glass(p["widget"], 0.97),
            ["editorWidget.border"] = p["border"],
            ["editorHoverWidget.background"] = Glass(p["widget"], 0.97),
            ["editorHoverWidget.foreground"] = p["fg"],
            ["editorHoverWidget.border"] = p["border"],

            ["editorGutter.addedBackground"] = p["green"],
            ["editorGutter.modifiedBackground"] = accent,
            ["editorGutter.deletedBackground"] = p["red"],
            ["diffEditor.insertedLineBackground"] = Alpha(p["green"], 0.15),
            ["diffEditor.removedLineBackground"] = Alpha(p["red"], 0.15),
            ["diffEditor.hunkHeaderBackground"] = Alpha(accent, 0.12),
            ["gitDecoration.addedResourceForeground"] = p["green"],
            ["gitDecoration.modifiedResourceForeground"] = p["yellow"],
            ["gitDecoration.stageModifiedResourceForeground"] = p["yellow"],
            ["gitDecoration.deletedResourceForeground"] = p["red"],
            ["gitDecoration.renamedResourceForeground"] = p["teal"],
            ["gitDecoration.untrackedResourceForeground"] = p["green"],
            ["gitDecoration.conflictingResourceForeground"] = p["red"],
            ["badge.background"] = accent,
            ["badge.foreground"] = On(accent),
            ["errorForeground"] = p["red"],

            ["panel.background"] = p["editor"],
            ["panel.border"] = p["border"],
            ["panelTitle.activeForeground"] = p["fg"],
            ["panelTitle.activeBackground"] = "transparent",
            ["panelTitle.inactiveForeground"] = p["muted"],
            ["panelTitle.activeBorder"] = accent,
            ["problemsErrorIcon.foreground"] = p["red"],
            ["problemsWarningIcon.foreground"] = p["yellow"],
            ["problemsInfoIcon.foreground"] = p["blue"],

            ["statusBar.background"] = Glass(p["window"], 0.7),
            ["statusBar.debuggingBackground"] = dark ? shade.Dark1 : shade.Base,
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
            ["button.hoverBackground"] = Mix(accent, p["window"], 0.1),
            ["button.secondaryBackground"] = p["field"],
            ["button.border"] = p["border"],
            ["scrollbarSlider.background"] = Alpha(p["fg"], 0.3),
            ["scrollbarSlider.hoverBackground"] = Alpha(p["fg"], 0.45),
            ["scrollbarSlider.activeBackground"] = Alpha(p["fg"], 0.55),
            ["quickInput.background"] = Glass(p["widget"], 0.97),
            ["quickInput.foreground"] = p["fg"],
            ["quickInput.border"] = p["border"],
            ["terminalCursor.foreground"] = p["fg"],
            ["terminal.selectionBackground"] = Alpha(p["selection"], 0.7),

            ["welcome.logo"] = Alpha(accent, 0.3),
            ["welcome.titleForeground"] = p["fg"],
            ["welcome.foreground"] = p["muted"],
        };

        var tokens = new JsonObject
        {
            ["keyword"] = vs["keyword"],
            ["keyword - control"] = vs["control"],
            ["preprocessor keyword"] = vs["preprocessor"],
            ["preprocessor text"] = vs["plain"],
            ["comment"] = vs["comment"],
            ["xml doc comment"] = vs["doc"],
            ["xml doc comment - name"] = vs["docTag"],
            ["xml doc comment - attribute name"] = vs["docTag"],
            ["excluded code"] = vs["excluded"],
            ["string"] = vs["string"],
            ["string - verbatim"] = vs["string"],
            ["string - escape character"] = vs["escape"],
            ["number"] = vs["number"],
            ["operator"] = vs["operator"],
            ["operator - overloaded"] = vs["method"],
            ["punctuation"] = vs["operator"],
            ["identifier"] = vs["plain"],
            ["namespace name"] = vs["plain"],
            ["class name"] = vs["type"],
            ["record class name"] = vs["type"],
            ["struct name"] = vs["struct"],
            ["record struct name"] = vs["struct"],
            ["interface name"] = vs["interface"],
            ["enum name"] = vs["interface"],
            ["delegate name"] = vs["type"],
            ["type parameter name"] = vs["interface"],
            ["module name"] = vs["type"],
            ["method name"] = vs["method"],
            ["extension method name"] = vs["method"],
            ["property name"] = vs["member"],
            ["field name"] = vs["member"],
            ["event name"] = vs["member"],
            ["local name"] = vs["local"],
            ["parameter name"] = vs["local"],
            ["label name"] = vs["plain"],
            ["constant name"] = vs["member"],
            ["enum member name"] = vs["member"],
            ["regex - character class"] = vs["escape"],
            ["regex - anchor"] = vs["control"],
            ["regex - quantifier"] = vs["number"],
            ["json - property name"] = vs["local"],
            ["json - string"] = vs["string"],
            ["html element name"] = vs["html"],
            ["html attribute name"] = vs["attribute"],
            ["html attribute value"] = !dark ? vs["string"] : "#c8c8c8",
            ["html tag delimiter"] = vs["operator"],
            ["razor transition"] = !dark ? "#ffff00" : "#f0e68c",
            ["razor directive"] = vs["keyword"],
            ["xml - msbuild reference"] = vs["method"],
        };

        var fonts = new JsonObject
        {
            ["ui.family"] = "Segoe UI Variable Text, Segoe UI Variable, Segoe UI, default",
            ["ui.size"] = 13,
            ["ui.smallSize"] = 11,
            ["tab.fontSize"] = 12,
            ["tab.activeWeight"] = "semibold",
            ["list.fontSize"] = 13,
            ["statusBar.fontSize"] = 12,
            ["sideBarTitle.fontSize"] = 13,
            ["sideBarTitle.weight"] = "semibold",
            ["sideBarTitle.textTransform"] = "none",
            ["sideBarSectionHeader.fontSize"] = 12,
            ["sideBarSectionHeader.weight"] = "semibold",
            ["sideBarSectionHeader.textTransform"] = "none",
            ["editor.family"] = "Cascadia Mono, Cascadia Code, Consolas, default",
            ["editor.size"] = 13,
            ["editor.lineHeight"] = 1.5,
            ["editor.lineNumberSize"] = 12,
        };

        var metrics = new JsonObject
        {
            ["commandCenter.radius"] = 4,
            ["windowControl.radius"] = 0,
            ["windowControl.width"] = 46,
            ["activityBar.itemRadius"] = 4,
            ["activityBar.indicatorLeft.border"] = 3,
            ["activityBar.indicatorRight.border"] = 0,
            ["list.radius"] = 4,
            ["list.itemMargin"] = "4,1",
            ["tab.radius"] = 6,
            ["tab.indicatorSize"] = 0,
            ["panelTab.radius"] = 4,
            ["statusBarItem.radius"] = 4,
            ["widget.radius"] = 8,
            ["palette.radius"] = 8,
            ["input.radius"] = 4,
            ["button.radius"] = 4,
            ["scrollbar.size"] = 10,
            ["scrollbar.radius"] = 5,
            ["scrollbar.thumbMargin"] = 3,
            ["titleBar.borderThickness"] = accentTitle ? 0 : 1,
        };

        var backdrop = translucent ? (look.Windows11 ? "mica" : "acrylic") : "none";
        var description = $"Windows {(dark ? "Dark" : "Light")} with the accent {shade.Base}"
            + (backdrop == "mica" ? ", Mica" : backdrop == "acrylic" ? ", acrylic" : "")
            + (accentTitle ? ", accent title bar" : "")
            + (look.Contrast ? ", high contrast" : "")
            + ". Generated by the Windows Theme module from Windows' settings; edits are overwritten.";

        return new JsonObject
        {
            ["name"] = ThemeName,
            ["type"] = dark ? "dark" : "light",
            ["inherits"] = dark ? "ByteForge Dark" : "ByteForge Light",
            ["description"] = description,
            ["colors"] = colors,
            ["tokenColors"] = tokens,
            ["fonts"] = fonts,
            ["metrics"] = metrics,
            ["editor"] = new JsonObject
            {
                ["selectionRadius"] = 2, ["lineHighlight"] = dark ? "border" : "fill", ["cursorStyle"] = "line", ["cursorWidth"] = 2,
            },
            ["effects"] = new JsonObject
            {
                ["window.backdrop"] = backdrop,
                ["palette.shadow"] = dark ? "0 16 32 0 #00000070" : "0 16 32 0 #00000030",
                ["widget.shadow"] = dark ? "0 8 16 0 #00000060" : "0 8 16 0 #00000024",
            },
        };
    }

    static readonly JsonSerializerOptions FileFormat = new()
    {
        WriteIndented = true,
        NewLine = "\n",
        // Like Python's json.dumps: an apostrophe stays an apostrophe.
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    };

    /// <summary>The theme file's text: indented JSON and a final newline.</summary>
    public static string Serialize(JsonObject theme) => theme.ToJsonString(FileFormat) + "\n";
}
