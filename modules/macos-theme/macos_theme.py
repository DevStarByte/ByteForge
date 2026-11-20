"""macOS Theme module for ByteForge: a theme that looks like macOS and follows its settings live.

Writes a theme called "macOS" into the user themes folder from what macOS is set to right now: Light or Dark
appearance (also when it switches automatically), the accent color, Reduce Transparency and Increase Contrast.
ByteForge hot-reloads theme files, so while the theme is active the IDE follows every change in System Settings.

The look: Xcode's Default (Dark) / Default (Light) syntax colors, SF Pro and SF Mono, a translucent sidebar
and title bar (window vibrancy), the accent in selections, buttons and menus, rounded controls.

On another system the theme is still written (dark, blue accent, or what the settings say), so the look can be
used anywhere.
"""

import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

THEME_NAME = "macOS"
POLL_SECONDS = 3
lock = threading.Lock()

# NSColor system accent colors, (dark appearance, light appearance). AppleAccentColor: -1 graphite, 0 red,
# 1 orange, 2 yellow, 3 green, (absent) blue or multicolor, 5 purple, 6 pink.
ACCENTS = {
    "blue": ("#0a84ff", "#007aff"),
    "purple": ("#bf5af2", "#af52de"),
    "pink": ("#ff375f", "#ff2d55"),
    "red": ("#ff453a", "#ff3b30"),
    "orange": ("#ff9f0a", "#ff9500"),
    "yellow": ("#ffd60a", "#ffcc00"),
    "green": ("#30d158", "#34c759"),
    "graphite": ("#8e8e93", "#8e8e93"),
}
ACCENT_CODES = {"-1": "graphite", "0": "red", "1": "orange", "2": "yellow", "3": "green", "4": "blue", "5": "purple",
                "6": "pink"}

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
    config = os.environ.get("XDG_CONFIG_HOME") or os.path.join(Path.home(), ".config")
    return Path(config) / "byteforge" / "themes"


THEME_FILE = themes_dir() / "macos.json"


# --- reading macOS ------------------------------------------------------------------------------------------

def defaults(domain, key):
    try:
        result = subprocess.run(["defaults", "read", domain, key], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def read_system():
    """What macOS is set to: {dark, accent, reduceTransparency, increaseContrast}, or None off macOS."""
    if sys.platform != "darwin":
        return None
    return {
        # Only set when the appearance is Dark at the moment (with Auto too).
        "dark": defaults("-g", "AppleInterfaceStyle") == "Dark",
        # Absent for blue and for multicolor (which shows blue in apps without their own accent).
        "accent": ACCENT_CODES.get(defaults("-g", "AppleAccentColor") or "4", "blue"),
        "reduceTransparency": defaults("com.apple.universalaccess", "reduceTransparency") == "1",
        "increaseContrast": defaults("com.apple.universalaccess", "increaseContrast") == "1",
    }


def effective(system, settings):
    """The look to build, from macOS and the module's settings (which win when they aren't "system")."""
    system = system or {"dark": True, "accent": "blue", "reduceTransparency": False, "increaseContrast": False}
    appearance = settings.get("macosTheme.appearance") or "system"
    accent = settings.get("macosTheme.accent") or "system"
    return {
        "dark": system["dark"] if appearance == "system" else appearance == "dark",
        "accent": system["accent"] if accent == "system" else accent if accent in ACCENTS else "blue",
        "translucent": settings.get("macosTheme.translucent", True) is not False and not system["reduceTransparency"],
        "contrast": system["increaseContrast"],
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


DARK = {
    "window": "#1e1e1e", "chrome": "#2a2a2d", "sidebar": "#232325", "editor": "#1f1f24", "widget": "#2c2c2e",
    "field": "#ffffff14", "border": "#ffffff1a", "strong": "#ffffff38", "fg": "#dfdfe0", "muted": "#98989d",
    "faint": "#6e6e73", "line": "#23252b", "selection": "#646f83", "hover": "#ffffff12",
    "red": "#ff453a", "orange": "#ff9f0a", "yellow": "#ffd60a", "green": "#30d158", "teal": "#40c8e0",
    "blue": "#0a84ff", "purple": "#bf5af2", "pink": "#ff375f",
}
LIGHT = {
    "window": "#ececec", "chrome": "#f6f6f6", "sidebar": "#e8e8e8", "editor": "#ffffff", "widget": "#f6f6f6",
    "field": "#ffffff", "border": "#0000001a", "strong": "#00000040", "fg": "#262626", "muted": "#6e6e73",
    "faint": "#a1a1a6", "line": "#ecf5ff", "selection": "#b2d7ff", "hover": "#0000000d",
    "red": "#ff3b30", "orange": "#ff9500", "yellow": "#ffcc00", "green": "#34c759", "teal": "#30b0c7",
    "blue": "#007aff", "purple": "#af52de", "pink": "#ff2d55",
}

# Xcode's Default (Dark) and Default (Light) themes.
XCODE_DARK = {
    "plain": "#dfdfe0", "comment": "#6c7986", "doc": "#92a1b1", "string": "#fc6a5d", "number": "#d0bf69",
    "keyword": "#fc5fa3", "preprocessor": "#fd8f3f", "attribute": "#bf8555", "typeDecl": "#5dd8ff",
    "otherDecl": "#41a1c0", "type": "#9ef1dd", "method": "#67b7a4", "member": "#67b7a4", "otherType": "#d0a8ff",
    "otherMethod": "#a167e6", "url": "#5482ff",
}
XCODE_LIGHT = {
    "plain": "#262626", "comment": "#5d6c79", "doc": "#4a5560", "string": "#c41a16", "number": "#1c00cf",
    "keyword": "#9b2393", "preprocessor": "#643820", "attribute": "#815f03", "typeDecl": "#0b4f79",
    "otherDecl": "#0f68a0", "type": "#1c464a", "method": "#326d74", "member": "#326d74", "otherType": "#3900a0",
    "otherMethod": "#6c36a9", "url": "#0e0eff",
}


def build_theme(look):
    dark = look["dark"]
    p = dict(DARK if dark else LIGHT)
    accent = ACCENTS[look["accent"]][0 if dark else 1]
    if look["contrast"]:
        p["border"], p["muted"] = p["strong"], mix(p["muted"], p["fg"], 0.4)
    translucent = look["translucent"]
    # With vibrancy the window shows the desktop through the chrome; the editor stays opaque.
    glass = (lambda c, a: alpha(c, a)) if translucent else (lambda c, a: c[:7])
    window, chrome, sidebar = glass(p["window"], 0.72), glass(p["chrome"], 0.78), glass(p["sidebar"], 0.62)
    selection_list = alpha(accent, 0.35 if dark else 0.22)
    xc = XCODE_DARK if dark else XCODE_LIGHT

    colors = {
        "accent": accent,
        "focusBorder": alpha(accent, 0.7),
        "window.background": window,
        "window.foreground": p["fg"],
        "foreground.muted": p["muted"],
        "icon.foreground": p["muted"],
        "toolbar.hoverBackground": p["hover"],
        "sash.hoverBorder": accent,
        "overlay.background": "#00000059" if dark else "#00000026",

        "titleBar.background": chrome,
        "titleBar.foreground": p["fg"],
        "titleBar.inactiveForeground": p["faint"],
        "titleBar.border": p["border"],
        "titleBar.logo": accent,
        "titleIcon.activeForeground": accent,
        "commandCenter.background": p["field"] if dark else "#00000008",
        "commandCenter.foreground": p["muted"],
        "commandCenter.activeForeground": p["fg"],
        "commandCenter.border": p["border"],
        "commandCenter.hoverBorder": p["strong"],
        "commandCenter.hoverBackground": p["hover"],
        "windowControl.foreground": p["fg"],
        "windowControl.hoverBackground": p["hover"],
        "windowControl.closeHoverBackground": p["red"],
        "windowControl.closeHoverForeground": "#ffffff",
        # macOS menus highlight the hovered item with the accent color.
        "menu.background": alpha(p["widget"], 0.96),
        "menu.foreground": p["fg"],
        "menu.selectionBackground": accent,
        "menu.border": p["border"],

        "activityBar.background": sidebar,
        "activityBar.foreground": accent,
        "activityBar.activeIconForeground": accent,
        "activityBar.inactiveForeground": p["muted"],
        "activityBar.activeBorder": "transparent",
        "activityBar.activeBackground": p["hover"],
        "activityBar.hoverBackground": p["hover"],
        "activityBar.border": p["border"],
        "activityBarBadge.background": p["red"],
        "activityBarBadge.foreground": "#ffffff",

        "sideBar.background": sidebar,
        "sideBar.foreground": p["fg"],
        "sideBar.border": p["border"],
        "sideBarTitle.foreground": p["muted"],
        "sideBarSectionHeader.foreground": p["faint"] if not look["contrast"] else p["muted"],
        "list.hoverBackground": p["hover"],
        "list.activeSelectionBackground": selection_list,
        "list.activeSelectionForeground": p["fg"],
        "list.activeFileBackground": selection_list,
        "list.activeFileForeground": p["fg"],
        "list.activeFileIndicator": "transparent",

        "icon.folder": ACCENTS["blue"][0 if dark else 1],
        "icon.file": p["muted"],
        "icon.csharp": p["purple"],
        "icon.project": p["blue"],
        "icon.solution": p["purple"],
        "icon.json": p["yellow"],
        "icon.markup": p["orange"],
        "icon.markdown": p["teal"],
        "icon.razor": p["pink"],
        "icon.style": p["blue"],
        "icon.yaml": p["red"],
        "icon.docker": p["teal"],

        "editorGroupHeader.background": chrome,
        "editorGroupHeader.border": p["border"],
        "tab.activeBackground": p["editor"],
        "tab.activeForeground": p["fg"],
        "tab.activeBorder": p["border"],
        "tab.activeBorderTop": "transparent",
        "tab.inactiveBackground": "transparent",
        "tab.inactiveForeground": p["muted"],
        "tab.hoverBackground": p["hover"],
        "tab.border": p["border"],
        "tab.modifiedIndicator": p["muted"],
        "breadcrumb.background": p["editor"],
        "breadcrumb.foreground": p["muted"],
        "breadcrumb.activeForeground": p["fg"],
        "breadcrumb.border": p["border"],

        "editor.background": p["editor"],
        "editor.foreground": xc["plain"],
        "editor.lineHighlightBackground": p["line"],
        "editor.lineHighlightBorder": "transparent",
        "editor.selectionBackground": p["selection"] if not dark else alpha(p["selection"], 0.8),
        "editor.inactiveSelectionBackground": alpha(p["selection"], 0.45),
        "editor.findMatchBackground": "#ffd60a80" if dark else "#ffe50080",
        "editor.findMatchBorder": p["yellow"],
        "editor.wordHighlightBackground": alpha(p["selection"], 0.35),
        "editor.wordHighlightStrongBackground": alpha(accent, 0.3),
        "editorInlayHint.foreground": p["faint"],
        "editorInlayHint.background": p["hover"],
        "editorCursor.foreground": xc["plain"],
        "editorCursor.background": p["editor"],
        "editorLineNumber.foreground": "#747478" if dark else "#a6a6a6",
        "editorLineNumber.activeForeground": p["fg"],
        "editorGutter.background": p["editor"],
        "editorIndentGuide.background": p["border"],
        "editorWhitespace.foreground": alpha(p["faint"], 0.5),
        "editorRuler.foreground": p["border"],
        "editorBracketMatch.background": alpha(accent, 0.2),
        "editorBracketMatch.border": alpha(accent, 0.6),
        "editorBracketHighlight.foreground1": xc["keyword"],
        "editorBracketHighlight.foreground2": xc["typeDecl"],
        "editorBracketHighlight.foreground3": xc["number"],
        "editorBracketHighlight.unexpectedBracket.foreground": p["red"],
        "editorError.foreground": p["red"],
        "editorWarning.foreground": p["yellow"] if dark else p["orange"],
        "editorInfo.foreground": p["blue"],
        "editorWidget.background": alpha(p["widget"], 0.97),
        "editorWidget.border": p["border"],
        "editorHoverWidget.background": alpha(p["widget"], 0.97),
        "editorHoverWidget.foreground": p["fg"],
        "editorHoverWidget.border": p["border"],

        "editorGutter.addedBackground": p["green"],
        "editorGutter.modifiedBackground": p["blue"],
        "editorGutter.deletedBackground": p["red"],
        "diffEditor.insertedLineBackground": alpha(p["green"], 0.15),
        "diffEditor.removedLineBackground": alpha(p["red"], 0.15),
        "diffEditor.hunkHeaderBackground": alpha(p["blue"], 0.12),
        "gitDecoration.addedResourceForeground": p["green"],
        "gitDecoration.modifiedResourceForeground": p["orange"],
        "gitDecoration.stageModifiedResourceForeground": p["orange"],
        "gitDecoration.deletedResourceForeground": p["red"],
        "gitDecoration.renamedResourceForeground": p["teal"],
        "gitDecoration.untrackedResourceForeground": p["green"],
        "gitDecoration.conflictingResourceForeground": p["red"],
        "badge.background": alpha(p["fg"], 0.15),
        "badge.foreground": p["fg"],
        "errorForeground": p["red"],

        "panel.background": p["editor"],
        "panel.border": p["border"],
        "panelTitle.activeForeground": p["fg"],
        "panelTitle.activeBackground": p["hover"],
        "panelTitle.inactiveForeground": p["muted"],
        "panelTitle.activeBorder": "transparent",
        "problemsErrorIcon.foreground": p["red"],
        "problemsWarningIcon.foreground": p["yellow"] if dark else p["orange"],
        "problemsInfoIcon.foreground": p["blue"],

        "statusBar.background": chrome,
        "statusBar.debuggingBackground": p["orange"],
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
        "button.hoverBackground": mix(accent, "#ffffff" if dark else "#000000", 0.12),
        "button.secondaryBackground": p["field"] if dark else "#0000000d",
        "button.border": "transparent",
        "scrollbarSlider.background": alpha(p["fg"], 0.25),
        "scrollbarSlider.hoverBackground": alpha(p["fg"], 0.4),
        "scrollbarSlider.activeBackground": alpha(p["fg"], 0.5),
        "quickInput.background": alpha(p["widget"], 0.97),
        "quickInput.foreground": p["fg"],
        "quickInput.border": p["border"],
        "terminalCursor.foreground": p["fg"],
        "terminal.selectionBackground": alpha(p["selection"], 0.6),

        "welcome.logo": alpha(accent, 0.3),
        "welcome.titleForeground": p["fg"],
        "welcome.foreground": p["muted"],
    }

    bold = lambda c: {"foreground": c, "fontStyle": "bold"}
    tokens = {
        "keyword": bold(xc["keyword"]),
        "keyword - control": bold(xc["keyword"]),
        "preprocessor keyword": xc["preprocessor"],
        "preprocessor text": xc["preprocessor"],
        "comment": xc["comment"],
        "xml doc comment": xc["doc"],
        "xml doc comment - name": bold(xc["doc"]),
        "xml doc comment - attribute name": xc["doc"],
        "excluded code": xc["comment"],
        "string": xc["string"],
        "string - verbatim": xc["string"],
        "string - escape character": xc["number"],
        "number": xc["number"],
        "operator": xc["plain"],
        "operator - overloaded": xc["method"],
        "punctuation": xc["plain"],
        "identifier": xc["plain"],
        "namespace name": xc["plain"],
        "class name": xc["type"],
        "record class name": xc["type"],
        "struct name": xc["type"],
        "record struct name": xc["type"],
        "interface name": xc["type"],
        "enum name": xc["type"],
        "delegate name": xc["type"],
        "type parameter name": xc["otherType"],
        "module name": xc["type"],
        "method name": xc["method"],
        "extension method name": xc["method"],
        "property name": xc["member"],
        "field name": xc["member"],
        "event name": xc["member"],
        "local name": xc["plain"],
        "parameter name": xc["plain"],
        "label name": xc["plain"],
        "constant name": xc["member"],
        "enum member name": xc["member"],
        "regex - character class": xc["number"],
        "regex - anchor": xc["keyword"],
        "regex - quantifier": xc["number"],
        "json - property name": xc["typeDecl"],
        "json - string": xc["string"],
        "html element name": xc["keyword"],
        "html attribute name": xc["attribute"],
        "html attribute value": xc["string"],
        "html tag delimiter": xc["plain"],
        "razor transition": xc["preprocessor"],
        "razor directive": xc["preprocessor"],
        "xml - msbuild reference": xc["otherDecl"],
    }

    fonts = {
        "ui.family": "SF Pro Text, SF Pro, .AppleSystemUIFont, Helvetica Neue, default",
        "ui.size": 13,
        "ui.smallSize": 11,
        "tab.fontSize": 12,
        "tab.activeWeight": "medium",
        "list.fontSize": 13,
        "statusBar.fontSize": 11,
        "sideBarTitle.fontSize": 11,
        "sideBarTitle.weight": "semibold",
        "sideBarTitle.textTransform": "none",
        "sideBarSectionHeader.fontSize": 11,
        "sideBarSectionHeader.weight": "bold",
        "sideBarSectionHeader.textTransform": "none",
        "editor.family": "SF Mono, Menlo, Monaco, JetBrains Mono, default",
        "editor.size": 13,
        "editor.lineHeight": 1.45,
        "editor.lineNumberSize": 12,
    }
    metrics = {
        "commandCenter.radius": 6,
        "windowControl.radius": 6,
        "activityBar.itemRadius": 6,
        "activityBar.indicatorLeft.border": 0,
        "activityBar.indicatorRight.border": 0,
        "list.radius": 5,
        "list.itemMargin": "8,1",
        "tab.radius": 6,
        "tab.indicatorSize": 0,
        "panelTab.radius": 5,
        "statusBarItem.radius": 4,
        "widget.radius": 10,
        "palette.radius": 12,
        "input.radius": 6,
        "button.radius": 6,
        "scrollbar.size": 10,
        "scrollbar.radius": 5,
        "scrollbar.thumbMargin": 2,
        "titleBar.borderThickness": 1,
    }
    return {
        "name": THEME_NAME,
        "type": "dark" if dark else "light",
        "inherits": "ByteForge Dark" if dark else "ByteForge Light",
        "description": f"macOS {'Dark' if dark else 'Light'} with the {look['accent']} accent"
                       f"{', translucent' if translucent else ''}{', increased contrast' if look['contrast'] else ''}. "
                       "Generated by the macOS Theme module from System Settings; edits are overwritten.",
        "colors": colors,
        "tokenColors": tokens,
        "fonts": fonts,
        "metrics": metrics,
        "editor": {"selectionRadius": 3, "lineHighlight": "fill", "cursorStyle": "line", "cursorWidth": 2},
        "effects": {
            "window.backdrop": "blur" if translucent else "none",
            "palette.shadow": "0 20 50 0 #00000080" if dark else "0 20 50 0 #00000038",
            "widget.shadow": "0 8 24 0 #00000060" if dark else "0 8 24 0 #00000026",
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
        log(f"macOS Theme: could not write {THEME_FILE}: {e}")
        return False
    return True


def generate(system=None):
    return write_theme(build_theme(effective(system if system is not None else read_system(), settings)))


def watch():
    """Polls macOS's settings: the appearance switches by itself with Auto, and nothing tells us."""
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
    for command, title in (("macosTheme.use", "Use macOS Theme"), ("macosTheme.regenerate", "Regenerate from System Settings")):
        send({"type": "registerCommand", "id": command, "title": title, "category": "macOS Theme"})
    if sys.platform != "darwin":
        log("macOS Theme: not on macOS, so the theme uses the settings (or Dark with the blue accent).")
    generate()
    if first_run and THEME_FILE.exists():
        # Just installed: switch to the theme once. Afterwards the user's theme choice is respected.
        send({"type": "setTheme", "name": THEME_NAME})
    if sys.platform == "darwin":
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
            if message.get("id") == "macosTheme.use":
                use_theme()
            elif message.get("id") == "macosTheme.regenerate":
                log("macOS Theme: updated from System Settings." if generate() else "macOS Theme: already up to date.")


if __name__ == "__main__":
    main()
