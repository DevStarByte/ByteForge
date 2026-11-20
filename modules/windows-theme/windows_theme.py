"""Windows Theme module for ByteForge: a theme that looks like Windows 11 and follows its settings live.

Writes a theme called "Windows" into the user themes folder from what Windows is set to right now: the app mode
(Light or Dark), the accent color (with the light and dark shades Windows derives from it), transparency effects,
"Show accent color on title bars and window borders" and high contrast. ByteForge hot-reloads theme files, so
while the theme is active the IDE follows every change in Settings > Personalization.

The look: Fluent colors on Mica (Windows 11; acrylic on Windows 10), Visual Studio 2022's Dark / Light syntax
colors, Segoe UI Variable and Cascadia Mono, rounded controls and wide caption buttons.

On another system the theme is still written (dark, the default blue, or what the settings say), so the look
can be used anywhere.
"""

import json
import os
import sys
import threading
import time
from pathlib import Path

THEME_NAME = "Windows"
POLL_SECONDS = 3
lock = threading.Lock()

# Windows' own accent presets (Settings > Personalization > Colors), for the "accent" setting.
ACCENTS = {
    "blue": "#0078d4", "purple": "#8764b8", "pink": "#e3008c", "red": "#e81123", "orange": "#ca5010",
    "yellow": "#ffb900", "green": "#107c10", "teal": "#038387", "gray": "#69797e",
}

settings = {}


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def themes_dir():
    # Modules live in <config>/byteforge/modules/<module>; themes in <config>/byteforge/themes.
    here = Path(__file__).resolve().parent
    if here.parent.name == "modules":
        return here.parent.parent / "themes"
    if os.name == "nt":
        return Path(os.environ.get("APPDATA") or Path.home()) / "byteforge" / "themes"
    config = os.environ.get("XDG_CONFIG_HOME") or os.path.join(Path.home(), ".config")
    return Path(config) / "byteforge" / "themes"


THEME_FILE = themes_dir() / "windows.json"


# --- reading Windows ----------------------------------------------------------------------------------------

def registry(path, name):
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, path) as key:
            return winreg.QueryValueEx(key, name)[0]
    except (ImportError, OSError):
        return None


def palette_from_bytes(data):
    """AccentPalette: eight RGBA colors, from the lightest shade to the darkest (the base color is the 4th)."""
    if not isinstance(data, (bytes, bytearray)) or len(data) < 28:
        return None
    shades = ["#%02x%02x%02x" % tuple(data[i:i + 3]) for i in range(0, 28, 4)]
    return {"light3": shades[0], "light2": shades[1], "light1": shades[2], "base": shades[3],
            "dark1": shades[4], "dark2": shades[5], "dark3": shades[6]}


def abgr(value):
    """A DWORD color as Windows stores AccentColor (0xAABBGGRR)."""
    return "#%02x%02x%02x" % (value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff)


def read_system():
    """What Windows is set to: {dark, palette, transparency, accentTitleBar, highContrast, windows11}, or None
    off Windows."""
    if os.name != "nt":
        return None
    personalize = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
    palette = palette_from_bytes(registry(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentPalette"))
    if palette is None:
        color = registry(r"Software\Microsoft\Windows\DWM", "AccentColor")
        palette = shades(abgr(color) if isinstance(color, int) else ACCENTS["blue"])
    flags = registry(r"Control Panel\Accessibility\HighContrast", "Flags")
    try:
        build = sys.getwindowsversion().build
    except AttributeError:
        build = 0
    return {
        "dark": registry(personalize, "AppsUseLightTheme") == 0,
        "palette": palette,
        "transparency": registry(personalize, "EnableTransparency") != 0,
        "accentTitleBar": registry(r"Software\Microsoft\Windows\DWM", "ColorPrevalence") == 1,
        "highContrast": bool(int(flags or 0) & 1),
        "windows11": build >= 22000,
    }


def effective(system, settings):
    """The look to build, from Windows and the module's settings (which win when they aren't "system")."""
    system = system or {"dark": True, "palette": shades(ACCENTS["blue"]), "transparency": True,
                        "accentTitleBar": False, "highContrast": False, "windows11": True}
    appearance = settings.get("windowsTheme.appearance") or "system"
    accent = settings.get("windowsTheme.accent") or "system"
    title = settings.get("windowsTheme.accentTitleBar") or "system"
    return {
        "dark": system["dark"] if appearance == "system" else appearance == "dark",
        "palette": system["palette"] if accent == "system" else shades(ACCENTS.get(accent, ACCENTS["blue"])),
        "translucent": settings.get("windowsTheme.translucent", True) is not False and system["transparency"],
        "accentTitleBar": system["accentTitleBar"] if title == "system" else title == "on",
        "contrast": system["highContrast"],
        "windows11": system["windows11"],
    }


# --- colors -------------------------------------------------------------------------------------------------

def rgb(color):
    c = color.lstrip("#")
    return tuple(int(c[i:i + 2], 16) for i in (0, 2, 4))


def luminance(color):
    def channel(v):
        v /= 255
        return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4
    r, g, b = (channel(v) for v in rgb(color))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def on(color):
    """Black or white text, whichever reads better on the color."""
    return "#000000" if luminance(color) > 0.4 else "#ffffff"


def mix(a, b, t):
    """a blended toward b by t (0..1)."""
    return "#" + "".join(f"{round(x + (y - x) * t):02x}" for x, y in zip(rgb(a), rgb(b)))


def alpha(color, a):
    return color[:7] + f"{round(a * 255):02x}"


def shades(base):
    """The shades Windows derives from an accent color (close to its AccentPalette)."""
    return {"light3": mix(base, "#ffffff", 0.75), "light2": mix(base, "#ffffff", 0.5), "light1": mix(base, "#ffffff", 0.25),
            "base": base, "dark1": mix(base, "#000000", 0.2), "dark2": mix(base, "#000000", 0.4),
            "dark3": mix(base, "#000000", 0.6)}


DARK = {
    "window": "#202020", "layer": "#2b2b2b", "editor": "#1f1f1f", "widget": "#2c2c2c", "field": "#ffffff0f",
    "border": "#ffffff14", "strong": "#ffffff33", "fg": "#ffffff", "muted": "#c5c5c5", "faint": "#9d9d9d",
    "line": "#282828", "selection": "#264f78", "hover": "#ffffff0f",
    "red": "#ff99a4", "orange": "#fce100", "yellow": "#fce100", "green": "#6ccb5f", "teal": "#4cc2c4",
    "blue": "#60cdff", "purple": "#b4a0ff", "pink": "#ff8fd4",
}
LIGHT = {
    "window": "#f3f3f3", "layer": "#fbfbfb", "editor": "#ffffff", "widget": "#f9f9f9", "field": "#ffffffb3",
    "border": "#0000000f", "strong": "#00000029", "fg": "#1b1b1b", "muted": "#5f5f5f", "faint": "#8a8a8a",
    "line": "#f5f5f5", "selection": "#add6ff", "hover": "#0000000a",
    "red": "#c42b1c", "orange": "#9d5d00", "yellow": "#9d5d00", "green": "#0f7b0f", "teal": "#007f7f",
    "blue": "#005fb8", "purple": "#6b4fbb", "pink": "#bf0077",
}

# Visual Studio 2022's Dark and Light themes.
VS_DARK = {
    "plain": "#dcdcdc", "keyword": "#569cd6", "control": "#d8a0df", "preprocessor": "#9b9b9b", "comment": "#57a64a",
    "doc": "#608b4e", "docTag": "#808080", "string": "#d69d85", "escape": "#ffd68f", "number": "#b5cea8",
    "operator": "#b4b4b4", "type": "#4ec9b0", "struct": "#86c691", "interface": "#b8d7a3", "method": "#dcdcaa",
    "local": "#9cdcfe", "member": "#dcdcdc", "html": "#569cd6", "attribute": "#9cdcfe", "excluded": "#9b9b9b",
}
VS_LIGHT = {
    "plain": "#000000", "keyword": "#0000ff", "control": "#8f08c4", "preprocessor": "#808080", "comment": "#008000",
    "doc": "#008000", "docTag": "#808080", "string": "#a31515", "escape": "#b776fb", "number": "#000000",
    "operator": "#000000", "type": "#2b91af", "struct": "#2b91af", "interface": "#2b91af", "method": "#74531f",
    "local": "#1f377f", "member": "#000000", "html": "#800000", "attribute": "#ff0000", "excluded": "#808080",
}


def build_theme(look):
    dark = look["dark"]
    p = dict(DARK if dark else LIGHT)
    shade = look["palette"]
    # Fluent's AccentFillColorDefault: Light 2 on dark, Dark 1 on light; AccentTextFillColor alike.
    accent = shade["light2"] if dark else shade["dark1"]
    if look["contrast"]:
        p["border"], p["muted"] = p["strong"], mix(p["muted"], p["fg"], 0.5)
    translucent = look["translucent"]
    glass = (lambda c, a: alpha(c, a)) if translucent else (lambda c, a: c[:7])
    window = glass(p["window"], 0.7)
    chrome = glass(p["window"], 0.7)
    sidebar = glass(p["window"], 0.55)
    if look["accentTitleBar"]:
        chrome = shade["dark2"] if dark else shade["base"]
    title_fg = on(chrome[:7]) if look["accentTitleBar"] else p["fg"]
    vs = VS_DARK if dark else VS_LIGHT
    selected = alpha("#ffffff" if dark else "#000000", 0.06)

    colors = {
        "accent": accent,
        "focusBorder": accent,
        "window.background": window,
        "window.foreground": p["fg"],
        "foreground.muted": p["muted"],
        "icon.foreground": p["muted"],
        "toolbar.hoverBackground": p["hover"],
        "sash.hoverBorder": accent,
        "overlay.background": "#0000004d" if dark else "#0000001f",

        "titleBar.background": chrome,
        "titleBar.foreground": title_fg,
        "titleBar.inactiveForeground": alpha(title_fg, 0.55),
        "titleBar.border": "transparent" if look["accentTitleBar"] else p["border"],
        "titleBar.logo": title_fg if look["accentTitleBar"] else accent,
        "titleIcon.activeForeground": title_fg if look["accentTitleBar"] else accent,
        "commandCenter.background": alpha(title_fg, 0.08) if look["accentTitleBar"] else p["field"],
        "commandCenter.foreground": alpha(title_fg, 0.75) if look["accentTitleBar"] else p["muted"],
        "commandCenter.activeForeground": title_fg,
        "commandCenter.border": alpha(title_fg, 0.12) if look["accentTitleBar"] else p["border"],
        "commandCenter.hoverBorder": p["strong"],
        "commandCenter.hoverBackground": alpha(title_fg, 0.12) if look["accentTitleBar"] else p["hover"],
        "windowControl.foreground": title_fg,
        "windowControl.hoverBackground": alpha(title_fg, 0.1),
        "windowControl.closeHoverBackground": "#c42b1c",
        "windowControl.closeHoverForeground": "#ffffff",
        "menu.background": glass(p["widget"], 0.96),
        "menu.foreground": p["fg"],
        "menu.selectionBackground": p["hover"] if not dark else "#ffffff14",
        "menu.border": p["border"],

        "activityBar.background": sidebar,
        "activityBar.foreground": p["fg"],
        "activityBar.activeIconForeground": accent,
        "activityBar.inactiveForeground": p["muted"],
        "activityBar.activeBorder": accent,
        "activityBar.activeBackground": selected,
        "activityBar.hoverBackground": p["hover"],
        "activityBar.border": "transparent",
        "activityBarBadge.background": accent,
        "activityBarBadge.foreground": on(accent),

        # Navigation pane and lists: subtle fill with the accent pill on the left of the selected row.
        "sideBar.background": sidebar,
        "sideBar.foreground": p["fg"],
        "sideBar.border": "transparent",
        "sideBarTitle.foreground": p["fg"],
        "sideBarSectionHeader.foreground": p["muted"],
        "list.hoverBackground": p["hover"],
        "list.activeSelectionBackground": selected,
        "list.activeSelectionForeground": p["fg"],
        "list.activeFileBackground": selected,
        "list.activeFileForeground": p["fg"],
        "list.activeFileIndicator": accent,

        "icon.folder": "#ffc83d" if dark else "#e8a317",
        "icon.file": p["muted"],
        "icon.csharp": "#a179dc" if dark else "#68217a",
        "icon.project": accent,
        "icon.solution": "#a179dc" if dark else "#68217a",
        "icon.json": "#fce100" if dark else "#9d5d00",
        "icon.markup": "#ff8c00" if dark else "#ca5010",
        "icon.markdown": p["teal"],
        "icon.razor": "#ff8fd4" if dark else "#bf0077",
        "icon.style": p["blue"],
        "icon.yaml": p["red"],
        "icon.docker": "#2496ed",

        "editorGroupHeader.background": glass(p["window"], 0.7),
        "editorGroupHeader.border": p["border"],
        "tab.activeBackground": p["editor"],
        "tab.activeForeground": p["fg"],
        "tab.activeBorder": p["border"],
        "tab.activeBorderTop": "transparent",
        "tab.inactiveBackground": "transparent",
        "tab.inactiveForeground": p["muted"],
        "tab.hoverBackground": p["hover"],
        "tab.border": "transparent",
        "tab.modifiedIndicator": p["muted"],
        "breadcrumb.background": p["editor"],
        "breadcrumb.foreground": p["muted"],
        "breadcrumb.activeForeground": p["fg"],
        "breadcrumb.border": p["border"],

        "editor.background": p["editor"],
        "editor.foreground": vs["plain"],
        "editor.lineHighlightBackground": p["line"],
        "editor.lineHighlightBorder": "#46464680" if dark else "#eeeeee",
        "editor.selectionBackground": p["selection"],
        "editor.inactiveSelectionBackground": alpha(p["selection"], 0.5),
        "editor.findMatchBackground": "#77381880" if dark else "#f6b94d80",
        "editor.findMatchBorder": "#f6b94d" if dark else "#c08000",
        "editor.wordHighlightBackground": "#36363680" if dark else "#e0e0e0",
        "editor.wordHighlightStrongBackground": alpha(accent, 0.25),
        "editorInlayHint.foreground": p["faint"],
        "editorInlayHint.background": p["hover"],
        "editorCursor.foreground": vs["plain"],
        "editorCursor.background": p["editor"],
        "editorLineNumber.foreground": "#2b91af" if not dark else "#858585",
        "editorLineNumber.activeForeground": p["fg"],
        "editorGutter.background": p["editor"],
        "editorIndentGuide.background": p["border"],
        "editorWhitespace.foreground": alpha(p["faint"], 0.5),
        "editorRuler.foreground": p["border"],
        "editorBracketMatch.background": "#0064001a" if not dark else "#0e4583",
        "editorBracketMatch.border": "transparent",
        "editorBracketHighlight.foreground1": "#ffd700" if dark else "#0431fa",
        "editorBracketHighlight.foreground2": "#da70d6" if dark else "#319331",
        "editorBracketHighlight.foreground3": "#179fff" if dark else "#7b3814",
        "editorBracketHighlight.unexpectedBracket.foreground": p["red"],
        "editorError.foreground": "#fc3e36" if dark else "#e51400",
        "editorWarning.foreground": "#95db7d" if dark else "#008000",
        "editorInfo.foreground": p["blue"],
        "editorWidget.background": glass(p["widget"], 0.97),
        "editorWidget.border": p["border"],
        "editorHoverWidget.background": glass(p["widget"], 0.97),
        "editorHoverWidget.foreground": p["fg"],
        "editorHoverWidget.border": p["border"],

        "editorGutter.addedBackground": p["green"],
        "editorGutter.modifiedBackground": accent,
        "editorGutter.deletedBackground": p["red"],
        "diffEditor.insertedLineBackground": alpha(p["green"], 0.15),
        "diffEditor.removedLineBackground": alpha(p["red"], 0.15),
        "diffEditor.hunkHeaderBackground": alpha(accent, 0.12),
        "gitDecoration.addedResourceForeground": p["green"],
        "gitDecoration.modifiedResourceForeground": p["yellow"],
        "gitDecoration.stageModifiedResourceForeground": p["yellow"],
        "gitDecoration.deletedResourceForeground": p["red"],
        "gitDecoration.renamedResourceForeground": p["teal"],
        "gitDecoration.untrackedResourceForeground": p["green"],
        "gitDecoration.conflictingResourceForeground": p["red"],
        "badge.background": accent,
        "badge.foreground": on(accent),
        "errorForeground": p["red"],

        "panel.background": p["editor"],
        "panel.border": p["border"],
        "panelTitle.activeForeground": p["fg"],
        "panelTitle.activeBackground": "transparent",
        "panelTitle.inactiveForeground": p["muted"],
        "panelTitle.activeBorder": accent,
        "problemsErrorIcon.foreground": p["red"],
        "problemsWarningIcon.foreground": p["yellow"],
        "problemsInfoIcon.foreground": p["blue"],

        "statusBar.background": glass(p["window"], 0.7),
        "statusBar.debuggingBackground": shade["dark1"] if dark else shade["base"],
        "statusBar.foreground": p["muted"],
        "statusBar.border": p["border"],
        "statusBarItem.hoverBackground": p["hover"],
        "statusBarItem.branch.background": "transparent",
        "statusBarItem.branch.foreground": p["fg"],

        "input.background": p["field"],
        "input.foreground": p["fg"],
        "input.border": p["border"],
        "input.placeholderForeground": p["faint"],
        "button.background": accent,
        "button.foreground": on(accent),
        "button.hoverBackground": mix(accent, p["window"], 0.1),
        "button.secondaryBackground": p["field"],
        "button.border": p["border"],
        "scrollbarSlider.background": alpha(p["fg"], 0.3),
        "scrollbarSlider.hoverBackground": alpha(p["fg"], 0.45),
        "scrollbarSlider.activeBackground": alpha(p["fg"], 0.55),
        "quickInput.background": glass(p["widget"], 0.97),
        "quickInput.foreground": p["fg"],
        "quickInput.border": p["border"],
        "terminalCursor.foreground": p["fg"],
        "terminal.selectionBackground": alpha(p["selection"], 0.7),

        "welcome.logo": alpha(accent, 0.3),
        "welcome.titleForeground": p["fg"],
        "welcome.foreground": p["muted"],
    }

    tokens = {
        "keyword": vs["keyword"],
        "keyword - control": vs["control"],
        "preprocessor keyword": vs["preprocessor"],
        "preprocessor text": vs["plain"],
        "comment": vs["comment"],
        "xml doc comment": vs["doc"],
        "xml doc comment - name": vs["docTag"],
        "xml doc comment - attribute name": vs["docTag"],
        "excluded code": vs["excluded"],
        "string": vs["string"],
        "string - verbatim": vs["string"],
        "string - escape character": vs["escape"],
        "number": vs["number"],
        "operator": vs["operator"],
        "operator - overloaded": vs["method"],
        "punctuation": vs["operator"],
        "identifier": vs["plain"],
        "namespace name": vs["plain"],
        "class name": vs["type"],
        "record class name": vs["type"],
        "struct name": vs["struct"],
        "record struct name": vs["struct"],
        "interface name": vs["interface"],
        "enum name": vs["interface"],
        "delegate name": vs["type"],
        "type parameter name": vs["interface"],
        "module name": vs["type"],
        "method name": vs["method"],
        "extension method name": vs["method"],
        "property name": vs["member"],
        "field name": vs["member"],
        "event name": vs["member"],
        "local name": vs["local"],
        "parameter name": vs["local"],
        "label name": vs["plain"],
        "constant name": vs["member"],
        "enum member name": vs["member"],
        "regex - character class": vs["escape"],
        "regex - anchor": vs["control"],
        "regex - quantifier": vs["number"],
        "json - property name": vs["local"],
        "json - string": vs["string"],
        "html element name": vs["html"],
        "html attribute name": vs["attribute"],
        "html attribute value": vs["string"] if not dark else "#c8c8c8",
        "html tag delimiter": vs["operator"],
        "razor transition": "#ffff00" if not dark else "#f0e68c",
        "razor directive": vs["keyword"],
        "xml - msbuild reference": vs["method"],
    }

    fonts = {
        "ui.family": "Segoe UI Variable Text, Segoe UI Variable, Segoe UI, default",
        "ui.size": 13,
        "ui.smallSize": 11,
        "tab.fontSize": 12,
        "tab.activeWeight": "semibold",
        "list.fontSize": 13,
        "statusBar.fontSize": 12,
        "sideBarTitle.fontSize": 13,
        "sideBarTitle.weight": "semibold",
        "sideBarTitle.textTransform": "none",
        "sideBarSectionHeader.fontSize": 12,
        "sideBarSectionHeader.weight": "semibold",
        "sideBarSectionHeader.textTransform": "none",
        "editor.family": "Cascadia Mono, Cascadia Code, Consolas, default",
        "editor.size": 13,
        "editor.lineHeight": 1.5,
        "editor.lineNumberSize": 12,
    }
    metrics = {
        "commandCenter.radius": 4,
        "windowControl.radius": 0,
        "windowControl.width": 46,
        "activityBar.itemRadius": 4,
        "activityBar.indicatorLeft.border": 3,
        "activityBar.indicatorRight.border": 0,
        "list.radius": 4,
        "list.itemMargin": "4,1",
        "tab.radius": 6,
        "tab.indicatorSize": 0,
        "panelTab.radius": 4,
        "statusBarItem.radius": 4,
        "widget.radius": 8,
        "palette.radius": 8,
        "input.radius": 4,
        "button.radius": 4,
        "scrollbar.size": 10,
        "scrollbar.radius": 5,
        "scrollbar.thumbMargin": 3,
        "titleBar.borderThickness": 0 if look["accentTitleBar"] else 1,
    }
    backdrop = ("mica" if look["windows11"] else "acrylic") if translucent else "none"
    return {
        "name": THEME_NAME,
        "type": "dark" if dark else "light",
        "inherits": "ByteForge Dark" if dark else "ByteForge Light",
        "description": f"Windows {'Dark' if dark else 'Light'} with the accent {shade['base']}"
                       f"{', Mica' if backdrop == 'mica' else ', acrylic' if backdrop == 'acrylic' else ''}"
                       f"{', accent title bar' if look['accentTitleBar'] else ''}"
                       f"{', high contrast' if look['contrast'] else ''}. "
                       "Generated by the Windows Theme module from Windows' settings; edits are overwritten.",
        "colors": colors,
        "tokenColors": tokens,
        "fonts": fonts,
        "metrics": metrics,
        "editor": {"selectionRadius": 2, "lineHighlight": "border" if dark else "fill", "cursorStyle": "line",
                   "cursorWidth": 2},
        "effects": {
            "window.backdrop": backdrop,
            "palette.shadow": "0 16 32 0 #00000070" if dark else "0 16 32 0 #00000030",
            "widget.shadow": "0 8 16 0 #00000060" if dark else "0 8 16 0 #00000024",
        },
    }


# --- module -------------------------------------------------------------------------------------------------

def write_theme(theme):
    """Writes the theme file atomically. Returns True when it changed."""
    text = json.dumps(theme, indent=2) + "\n"
    try:
        if THEME_FILE.exists() and THEME_FILE.read_text(encoding="utf-8") == text:
            return False
        THEME_FILE.parent.mkdir(parents=True, exist_ok=True)
        tmp = THEME_FILE.with_suffix(".json.tmp")
        tmp.write_text(text, encoding="utf-8")
        os.replace(tmp, THEME_FILE)  # ByteForge never reads a half-written theme
    except OSError as e:
        log(f"Windows Theme: could not write {THEME_FILE}: {e}")
        return False
    return True


def generate(system=None):
    return write_theme(build_theme(effective(system if system is not None else read_system(), settings)))


def watch():
    """Polls Windows' settings: the mode and accent change without telling us (and with Auto accent, by the
    wallpaper)."""
    last = read_system()
    while True:
        time.sleep(POLL_SECONDS)
        now = read_system()
        if now != last:
            last = now
            generate(now)


def use_theme():
    generate()
    if THEME_FILE.exists():
        send({"type": "setTheme", "name": THEME_NAME})


def main():
    global settings
    first_run = not THEME_FILE.exists()
    for command, title in (("windowsTheme.use", "Use Windows Theme"), ("windowsTheme.regenerate", "Regenerate from Windows Settings")):
        send({"type": "registerCommand", "id": command, "title": title, "category": "Windows Theme"})
    if os.name != "nt":
        log("Windows Theme: not on Windows, so the theme uses the settings (or Dark with the default blue).")
    generate()
    if first_run and THEME_FILE.exists():
        # Just installed: switch to the theme once. Afterwards the user's theme choice is respected.
        send({"type": "setTheme", "name": THEME_NAME})
    if os.name == "nt":
        threading.Thread(target=watch, daemon=True).start()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        kind = message.get("type")
        if kind == "activate" or (kind == "event" and message.get("name") == "settingsChanged"):
            settings = message.get("settings") or {}
            generate()
        elif kind == "executeCommand":
            if message.get("id") == "windowsTheme.use":
                use_theme()
            elif message.get("id") == "windowsTheme.regenerate":
                log("Windows Theme: updated from Windows' settings." if generate() else "Windows Theme: already up to date.")


if __name__ == "__main__":
    main()
