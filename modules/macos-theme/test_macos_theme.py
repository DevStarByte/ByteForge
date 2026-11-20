"""Tests for the macOS Theme module: python3 -m unittest discover -s modules/macos-theme -p 'test_*.py' -v"""

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

import macos_theme as mt  # noqa: E402

COLOR = re.compile(r"^(#[0-9a-f]{6}([0-9a-f]{2})?|transparent)$")
SYSTEM = {"dark": False, "accent": "purple", "reduceTransparency": False, "increaseContrast": False}


def defaults_stub(values):
    return lambda domain, key: values.get(key)


class ReadTests(unittest.TestCase):
    def test_reads_appearance_accent_and_accessibility(self):
        values = {"AppleInterfaceStyle": "Dark", "AppleAccentColor": "5", "reduceTransparency": "1"}
        with mock.patch.object(mt.sys, "platform", "darwin"), mock.patch.object(mt, "defaults", defaults_stub(values)):
            self.assertEqual(mt.read_system(), {"dark": True, "accent": "purple", "reduceTransparency": True,
                                                "increaseContrast": False})

    def test_missing_keys_mean_light_and_blue(self):
        with mock.patch.object(mt.sys, "platform", "darwin"), mock.patch.object(mt, "defaults", defaults_stub({})):
            system = mt.read_system()
        self.assertFalse(system["dark"])
        self.assertEqual(system["accent"], "blue")

    def test_off_macos(self):
        with mock.patch.object(mt.sys, "platform", "linux"):
            self.assertIsNone(mt.read_system())


class LookTests(unittest.TestCase):
    def test_system_wins_unless_settings_say_otherwise(self):
        look = mt.effective(SYSTEM, {})
        self.assertEqual((look["dark"], look["accent"], look["translucent"]), (False, "purple", True))
        look = mt.effective(SYSTEM, {"macosTheme.appearance": "dark", "macosTheme.accent": "green",
                                     "macosTheme.translucent": False})
        self.assertEqual((look["dark"], look["accent"], look["translucent"]), (True, "green", False))

    def test_reduce_transparency_turns_translucency_off(self):
        self.assertFalse(mt.effective(dict(SYSTEM, reduceTransparency=True), {})["translucent"])

    def test_off_macos_defaults_to_dark_blue(self):
        look = mt.effective(None, {})
        self.assertEqual((look["dark"], look["accent"]), (True, "blue"))


class ThemeTests(unittest.TestCase):
    def check(self, theme):
        for key, value in theme["colors"].items():
            self.assertRegex(value, COLOR, key)
        for key, value in theme["tokenColors"].items():
            value = value["foreground"] if isinstance(value, dict) else value
            self.assertRegex(value, COLOR, key)

    def test_every_variant_is_valid(self):
        for dark in (True, False):
            for accent in mt.ACCENTS:
                for translucent in (True, False):
                    for contrast in (True, False):
                        self.check(mt.build_theme({"dark": dark, "accent": accent, "translucent": translucent,
                                                   "contrast": contrast}))

    def test_light_purple_translucent(self):
        theme = mt.build_theme({"dark": False, "accent": "purple", "translucent": True, "contrast": False})
        self.assertEqual(theme["type"], "light")
        self.assertEqual(theme["inherits"], "ByteForge Light")
        self.assertEqual(theme["colors"]["accent"], "#af52de")
        self.assertEqual(theme["colors"]["button.foreground"], "#ffffff")
        self.assertEqual(len(theme["colors"]["sideBar.background"]), 9)  # translucent
        self.assertEqual(len(theme["colors"]["editor.background"]), 7)   # the editor stays opaque
        self.assertEqual(theme["effects"]["window.backdrop"], "blur")
        self.assertEqual(theme["tokenColors"]["keyword"], {"foreground": "#9b2393", "fontStyle": "bold"})

    def test_yellow_accent_gets_dark_button_text(self):
        theme = mt.build_theme({"dark": True, "accent": "yellow", "translucent": False, "contrast": False})
        self.assertEqual(theme["colors"]["button.foreground"], "#000000")
        self.assertEqual(theme["effects"]["window.backdrop"], "none")
        self.assertEqual(len(theme["colors"]["sideBar.background"]), 7)

    def test_theme_file_is_rewritten_only_when_it_changes(self):
        with tempfile.TemporaryDirectory() as folder, mock.patch.object(mt, "THEME_FILE", Path(folder) / "macos.json"):
            theme = mt.build_theme(mt.effective(SYSTEM, {}))
            self.assertTrue(mt.write_theme(theme))
            self.assertFalse(mt.write_theme(theme))
            self.assertEqual(json.loads((Path(folder) / "macos.json").read_text())["name"], "macOS")

    def test_manifest_settings_match(self):
        with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
            manifest = json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))
        accents = next(s for s in manifest["settings"] if s["key"] == "macosTheme.accent")["choices"]
        self.assertEqual(set(accents), {"system"} | set(mt.ACCENTS))


if __name__ == "__main__":
    unittest.main()
