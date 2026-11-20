"""Tests for the Windows Theme module: python3 -m unittest discover -s modules/windows-theme -p 'test_*.py' -v"""

import json
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import windows_theme as wt  # noqa: E402

COLOR = re.compile(r"^(#[0-9a-f]{6}([0-9a-f]{2})?|transparent)$")
# An AccentPalette as Windows stores it (REG_BINARY: eight RGBA shades, base blue 4th).
PALETTE = bytes.fromhex("99ebff0049d8ff00009cff000078d4000059a100004280000026440000b7c300")


class ReadTests(unittest.TestCase):
    def test_accent_palette(self):
        palette = wt.palette_from_bytes(PALETTE)
        self.assertEqual(palette["base"], "#0078d4")
        self.assertEqual(palette["light2"], "#49d8ff")
        self.assertEqual(palette["dark1"], "#0059a1")
        self.assertIsNone(wt.palette_from_bytes(b"\x00\x01"))

    def test_accent_color_dword_is_abgr(self):
        self.assertEqual(wt.abgr(0xffd47800), "#0078d4")

    def test_reads_the_registry(self):
        values = {
            ("Personalize", "AppsUseLightTheme"): 1, ("Personalize", "EnableTransparency"): 0,
            ("Accent", "AccentPalette"): PALETTE, ("DWM", "ColorPrevalence"): 1, ("HighContrast", "Flags"): "126",
        }
        def registry(path, name):
            return values.get((path.rsplit("\\", 1)[-1], name))
        with mock.patch.object(wt.os, "name", "nt"), mock.patch.object(wt, "registry", registry), \
                mock.patch.object(wt.sys, "getwindowsversion", lambda: mock.Mock(build=22631), create=True):
            system = wt.read_system()
        self.assertEqual(system, {"dark": False, "palette": wt.palette_from_bytes(PALETTE), "transparency": False,
                                  "accentTitleBar": True, "highContrast": False, "windows11": True})

    def test_off_windows(self):
        with mock.patch.object(wt.os, "name", "posix"):
            self.assertIsNone(wt.read_system())


class ThemeTests(unittest.TestCase):
    def check(self, theme):
        for key, value in theme["colors"].items():
            self.assertRegex(value, COLOR, key)
        for key, value in theme["tokenColors"].items():
            self.assertRegex(value, COLOR, key)

    def test_every_variant_is_valid(self):
        for dark in (True, False):
            for accent in wt.ACCENTS.values():
                for flags in range(8):
                    self.check(wt.build_theme({"dark": dark, "palette": wt.shades(accent),
                                               "translucent": bool(flags & 1), "accentTitleBar": bool(flags & 2),
                                               "contrast": bool(flags & 4), "windows11": True}))

    def test_dark_uses_the_light_accent_shade_and_mica(self):
        look = wt.effective({"dark": True, "palette": wt.palette_from_bytes(PALETTE), "transparency": True,
                             "accentTitleBar": False, "highContrast": False, "windows11": True}, {})
        theme = wt.build_theme(look)
        self.assertEqual(theme["colors"]["accent"], "#49d8ff")
        self.assertEqual(theme["colors"]["list.activeFileIndicator"], "#49d8ff")
        self.assertEqual(theme["effects"]["window.backdrop"], "mica")
        self.assertEqual(theme["tokenColors"]["keyword"], "#569cd6")
        self.assertEqual(theme["inherits"], "ByteForge Dark")

    def test_light_accent_title_bar_on_windows_10(self):
        look = wt.effective({"dark": False, "palette": wt.palette_from_bytes(PALETTE), "transparency": True,
                             "accentTitleBar": True, "highContrast": False, "windows11": False}, {})
        theme = wt.build_theme(look)
        self.assertEqual(theme["colors"]["titleBar.background"], "#0078d4")
        self.assertEqual(theme["colors"]["titleBar.foreground"], "#ffffff")
        self.assertEqual(theme["effects"]["window.backdrop"], "acrylic")
        self.assertEqual(theme["tokenColors"]["keyword"], "#0000ff")

    def test_settings_override_windows(self):
        system = {"dark": True, "palette": wt.shades("#0078d4"), "transparency": False, "accentTitleBar": True,
                  "highContrast": False, "windows11": True}
        look = wt.effective(system, {"windowsTheme.appearance": "light", "windowsTheme.accent": "green",
                                     "windowsTheme.accentTitleBar": "off"})
        self.assertFalse(look["dark"])
        self.assertEqual(look["palette"]["base"], "#107c10")
        self.assertFalse(look["accentTitleBar"])
        self.assertFalse(look["translucent"])  # Windows' transparency effects are off

    def test_theme_file(self):
        with tempfile.TemporaryDirectory() as folder, mock.patch.object(wt, "THEME_FILE", Path(folder) / "windows.json"):
            theme = wt.build_theme(wt.effective(None, {}))
            self.assertTrue(wt.write_theme(theme))
            self.assertFalse(wt.write_theme(theme))

    def test_manifest_settings_match(self):
        with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
            manifest = json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))
        accents = next(s for s in manifest["settings"] if s["key"] == "windowsTheme.accent")["choices"]
        self.assertEqual(set(accents), {"system"} | set(wt.ACCENTS))


if __name__ == "__main__":
    unittest.main()
