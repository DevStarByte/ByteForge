"""Caelestia Theme module for ByteForge.

Turns the current Caelestia scheme (~/.local/state/caelestia/scheme.json, written by caelestia-shell whenever
the wallpaper or scheme changes) into a ByteForge theme called "Caelestia" in the user themes folder. ByteForge
hot-reloads theme files, so while the theme is active the IDE follows every scheme change live.

The scheme holds Material You roles (surface, primary, onSurface, …), terminal colors (term0-15) and a
Catppuccin-style palette (mauve, peach, green, …) derived from the wallpaper. UI chrome uses the Material roles,
syntax highlighting the Catppuccin-style palette, so the theme works for dark and light schemes alike.
"""

import json
import os
import sys
import threading
import time
from pathlib import Path

THEME_NAME = "Caelestia"
lock = threading.Lock()


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def scheme_path():
    state = os.environ.get("XDG_STATE_HOME") or os.path.join(Path.home(), ".local", "state")
    return Path(state) / "caelestia" / "scheme.json"


def themes_dir():
    # Modules live in <config>/byteforge/modules/<module>; themes in <config>/byteforge/themes.
    here = Path(__file__).resolve().parent
    if here.parent.name == "modules":
        return here.parent.parent / "themes"
    config = os.environ.get("XDG_CONFIG_HOME") or os.path.join(Path.home(), ".config")
    return Path(config) / "byteforge" / "themes"


THEME_FILE = themes_dir() / "caelestia.json"


def build_theme(scheme):
    c = {k: v.lower() for k, v in scheme.get("colours", {}).items() if isinstance(v, str)}
    light = scheme.get("mode") == "light"

    def col(key, fallback="ff00ff"):
        return "#" + c.get(key, c.get(fallback, fallback))

    def alpha(key, a):
        return col(key) + a

    bg = col("surface")
    chrome = col("surfaceContainerLowest" if not light else "surfaceContainerLow")
    editor = col("surfaceContainerLow" if not light else "surface")
    widget = col("surfaceContainerHigh")
    field = col("surfaceContainerHighest")
    border = col("outlineVariant")
    fg = col("onSurface")
    muted = col("onSurfaceVariant")
    accent = col("primary")
    warning = col("yellow" if light else "peach")

    colors = {
        "accent": accent,
        "focusBorder": accent,
        "window.background": bg,
        "window.foreground": fg,
        "foreground.muted": muted,
        "icon.foreground": muted,
        "toolbar.hoverBackground": alpha("onSurface", "1a"),
        "sash.hoverBorder": accent,

        "titleBar.background": chrome,
        "titleBar.foreground": fg,
        "titleBar.inactiveForeground": muted,
        "titleBar.border": border,
        "titleBar.logo": accent,
        "titleIcon.activeForeground": accent,
        "commandCenter.background": widget,
        "commandCenter.foreground": muted,
        "commandCenter.activeForeground": fg,
        "commandCenter.border": border,
        "commandCenter.hoverBorder": col("outline"),
        "commandCenter.hoverBackground": field,
        "windowControl.foreground": fg,
        "windowControl.hoverBackground": alpha("onSurface", "1a"),
        "windowControl.closeHoverBackground": col("error"),
        "windowControl.closeHoverForeground": col("onError"),
        "menu.background": widget,
        "menu.foreground": fg,
        "menu.selectionBackground": col("secondaryContainer"),
        "menu.border": border,

        "activityBar.background": chrome,
        "activityBar.foreground": accent,
        "activityBar.inactiveForeground": muted,
        "activityBar.activeBorder": accent,
        "activityBar.border": border,
        "activityBarBadge.background": accent,
        "activityBarBadge.foreground": col("onPrimary"),

        "sideBar.background": bg,
        "sideBar.foreground": fg,
        "sideBar.border": border,
        "sideBarTitle.foreground": accent,
        "sideBarSectionHeader.foreground": col("secondary"),
        "list.hoverBackground": alpha("onSurface", "12"),
        "list.activeSelectionBackground": col("secondaryContainer"),
        "list.activeSelectionForeground": col("onSecondaryContainer"),
        "list.activeFileBackground": col("secondaryContainer"),
        "list.activeFileForeground": col("onSecondaryContainer"),
        "list.activeFileIndicator": accent,
        "terminalCursor.foreground": accent,
        "terminal.selectionBackground": alpha("primary", "40"),

        "icon.folder": col("tertiary"),
        "icon.file": muted,
        "icon.csharp": col("primary"),
        "icon.project": col("secondary"),
        "icon.json": col("yellow"),
        "icon.markup": col("peach"),
        "icon.markdown": col("sapphire"),
        "icon.docker": col("sky"),

        "editorGroupHeader.background": bg,
        "editorGroupHeader.border": border,
        "tab.activeBackground": editor,
        "tab.activeForeground": fg,
        "tab.activeBorder": border,
        "tab.inactiveBackground": bg,
        "tab.inactiveForeground": muted,
        "tab.hoverBackground": editor,
        "tab.border": border,
        "tab.activeBorderTop": accent,
        "tab.modifiedIndicator": accent,
        "breadcrumb.background": editor,
        "breadcrumb.foreground": muted,
        "breadcrumb.activeForeground": fg,

        "editor.background": editor,
        "editor.foreground": fg,
        "editor.lineHighlightBackground": alpha("onSurface", "0d"),
        "editor.lineHighlightBorder": "transparent",
        "editor.selectionBackground": alpha("primary", "40"),
        "editor.inactiveSelectionBackground": alpha("primary", "20"),
        "editor.findMatchBackground": alpha("tertiary", "50"),
        "editor.findMatchBorder": col("tertiary"),
        "editor.wordHighlightBackground": alpha("onSurface", "1f"),
        "editor.wordHighlightStrongBackground": alpha("primary", "30"),
        "editorInlayHint.foreground": col("outline"),
        "editorInlayHint.background": alpha("onSurface", "0f"),
        "editorCursor.foreground": accent,
        "editorCursor.background": editor,
        "editorLineNumber.foreground": col("outline"),
        "editorLineNumber.activeForeground": accent,
        "editorGutter.background": editor,
        "editorIndentGuide.background": border,
        "editorWhitespace.foreground": alpha("outline", "60"),
        "editorRuler.foreground": border,
        "editorBracketMatch.background": alpha("primary", "20"),
        "editorBracketMatch.border": accent,
        "editorBracketHighlight.foreground1": col("primary"),
        "editorBracketHighlight.foreground2": col("tertiary"),
        "editorBracketHighlight.foreground3": col("secondary"),
        "editorBracketHighlight.unexpectedBracket.foreground": col("error"),
        "editorError.foreground": col("error"),
        "editorWarning.foreground": warning,
        "editorInfo.foreground": col("sapphire"),
        "editorWidget.background": widget,
        "editorWidget.border": border,
        "editorHoverWidget.background": widget,
        "editorHoverWidget.foreground": fg,
        "editorHoverWidget.border": border,

        "editorGutter.addedBackground": col("success"),
        "editorGutter.modifiedBackground": col("primary"),
        "editorGutter.deletedBackground": col("error"),
        "diffEditor.insertedLineBackground": alpha("success", "26"),
        "diffEditor.removedLineBackground": alpha("error", "26"),
        "diffEditor.hunkHeaderBackground": alpha("primary", "1f"),
        "gitDecoration.addedResourceForeground": col("success"),
        "gitDecoration.modifiedResourceForeground": warning,
        "gitDecoration.stageModifiedResourceForeground": warning,
        "gitDecoration.deletedResourceForeground": col("error"),
        "gitDecoration.renamedResourceForeground": col("teal"),
        "gitDecoration.untrackedResourceForeground": col("success"),
        "gitDecoration.conflictingResourceForeground": col("error"),
        "badge.background": col("secondaryContainer"),
        "badge.foreground": col("onSecondaryContainer"),
        "errorForeground": col("error"),

        "panel.background": bg,
        "panel.border": border,
        "panelTitle.activeForeground": fg,
        "panelTitle.inactiveForeground": muted,
        "panelTitle.activeBorder": accent,
        "problemsErrorIcon.foreground": col("error"),
        "problemsWarningIcon.foreground": warning,
        "problemsInfoIcon.foreground": col("sapphire"),

        "statusBar.background": chrome,
        "statusBar.debuggingBackground": col("tertiaryContainer"),
        "statusBar.foreground": muted,
        "statusBar.border": border,
        "statusBarItem.hoverBackground": alpha("onSurface", "1a"),

        "input.background": field,
        "input.foreground": fg,
        "input.border": border,
        "input.placeholderForeground": col("outline"),
        "button.background": accent,
        "button.foreground": col("onPrimary"),
        "button.hoverBackground": col("primaryDim", "primary"),
        "button.secondaryBackground": field,
        "button.border": "transparent",
        "scrollbarSlider.background": alpha("outline", "50"),
        "scrollbarSlider.hoverBackground": alpha("outline", "80"),
        "scrollbarSlider.activeBackground": alpha("primary", "80"),
        "quickInput.background": widget,
        "quickInput.foreground": fg,
        "quickInput.border": border,

        "welcome.logo": alpha("primary", "40"),
        "welcome.titleForeground": accent,
        "welcome.foreground": muted,
    }

    keyword, control = col("mauve"), col("tertiary")
    types, method = col("yellow"), col("blue")
    string, number = col("green"), col("peach")
    member, param = col("lavender"), col("maroon")
    comment = col("outline")
    tokens = {
        "keyword": keyword,
        "keyword - control": control,
        "preprocessor keyword": muted,
        "preprocessor text": muted,
        "comment": {"foreground": comment, "fontStyle": "italic"},
        "xml doc comment": comment,
        "xml doc comment - name": types,
        "xml doc comment - attribute name": member,
        "excluded code": col("outlineVariant"),
        "string": string,
        "string - verbatim": string,
        "string - escape character": col("pink"),
        "number": number,
        "operator": col("sky"),
        "operator - overloaded": method,
        "punctuation": muted,
        "identifier": fg,
        "namespace name": col("rosewater"),
        "class name": types,
        "record class name": types,
        "struct name": col("teal"),
        "record struct name": col("teal"),
        "interface name": col("sapphire"),
        "enum name": col("sapphire"),
        "delegate name": types,
        "type parameter name": col("teal"),
        "module name": types,
        "method name": method,
        "extension method name": method,
        "property name": member,
        "field name": member,
        "event name": member,
        "local name": fg,
        "parameter name": param,
        "label name": muted,
        "constant name": number,
        "enum member name": number,
        "regex - character class": col("pink"),
        "regex - anchor": control,
        "regex - quantifier": number,
        "json - property name": accent,
        "json - string": string,
        "html element name": keyword,
        "html attribute name": member,
        "html attribute value": string,
        "html tag delimiter": muted,
        "razor transition": control,
        "razor directive": control,
        "xml - msbuild reference": method,
    }

    flavour = " · ".join(str(scheme[k]) for k in ("name", "flavour", "variant") if scheme.get(k))
    return {
        "name": THEME_NAME,
        "type": "light" if light else "dark",
        "description": f"Follows your Caelestia scheme ({flavour}). Generated by the Caelestia Theme module; edits are overwritten.",
        "colors": colors,
        "tokenColors": tokens,
    }


def generate():
    """Writes the theme from the current scheme. Returns True when the file changed."""
    path = scheme_path()
    try:
        scheme = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        log(f"Caelestia Theme: {path} not found. Is caelestia-shell installed and running?")
        return False
    except (OSError, ValueError) as e:
        log(f"Caelestia Theme: could not read {path}: {e}")
        return False
    text = json.dumps(build_theme(scheme), indent=2) + "\n"
    try:
        if THEME_FILE.exists() and THEME_FILE.read_text(encoding="utf-8") == text:
            return False
        THEME_FILE.parent.mkdir(parents=True, exist_ok=True)
        tmp = THEME_FILE.with_suffix(".json.tmp")
        tmp.write_text(text, encoding="utf-8")
        os.replace(tmp, THEME_FILE)  # atomic, so ByteForge never reads a half-written theme
    except OSError as e:
        log(f"Caelestia Theme: could not write {THEME_FILE}: {e}")
        return False
    return True


def use_theme():
    generate()
    if THEME_FILE.exists():
        send({"type": "setTheme", "name": THEME_NAME})


def regenerate():
    log("Caelestia Theme: theme updated from the current scheme." if generate() else "Caelestia Theme: theme is up to date.")


def watch():
    """Polls the scheme file; caelestia-shell rewrites it on every wallpaper or scheme change."""
    path = scheme_path()
    last = None
    while True:
        try:
            stamp = path.stat().st_mtime_ns
        except OSError:
            stamp = None
        if stamp is not None and stamp != last:
            if last is not None:
                generate()
            last = stamp
        time.sleep(1)


COMMANDS = {
    "caelestia.use": ("Use Caelestia Theme", use_theme),
    "caelestia.regenerate": ("Regenerate Theme from Scheme", regenerate),
}

if __name__ == "__main__":
    first_run = not THEME_FILE.exists()
    for command, (title, _) in COMMANDS.items():
        send({"type": "registerCommand", "id": command, "title": title, "category": "Caelestia"})
    generate()
    if first_run and THEME_FILE.exists():
        # Just installed: switch to the theme once. Afterwards the user's theme choice is respected.
        send({"type": "setTheme", "name": THEME_NAME})
    threading.Thread(target=watch, daemon=True).start()

    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if message.get("type") == "executeCommand" and message.get("id") in COMMANDS:
            COMMANDS[message["id"]][1]()
