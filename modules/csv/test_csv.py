"""Tests for the CSV module: python3 -m unittest discover -s modules/csv -p 'test_*.py' -v

The rainbow rules in module.json are generated here: python3 modules/csv/test_csv.py --write
"""

import json
import os
import re
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import csv_tools as ct  # noqa: E402

# Theme tokens with clearly different colors in most themes, one per column, repeating.
TOKENS = ["keyword", "class name", "string", "method name", "property name", "number", "keyword - control",
          "regex - character class"]
COLUMNS = 20
# Tab first, then semicolon, then comma, then pipe: the earlier rule wins, and the semicolon rules only match
# lines with semicolons, so German Excel files (1,5;Berlin) are split at ; and US ones at ,.
SEPARATORS = [("\\t", "\t"), (";", ";"), (",", ","), ("\\|", "|")]


def rules():
    result = []
    for regex_sep, raw in SEPARATORS:
        cls = raw if raw != "\t" else "\\t"
        field = f'(?:"(?:[^"]|"")*"|[^{cls}"\\n]*)'
        for k in range(COLUMNS):
            if k == 0:
                pattern = f"^({field}){regex_sep}"
            else:
                pattern = f"^(?:{field}{regex_sep}){{{k}}}({field})"
            result.append({"match": pattern, "group": 1, "token": TOKENS[k % len(TOKENS)]})
    return result


def manifest():
    with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
        return json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))


def token_of(line, text):
    """The token the IDE gives the first character of `text` in `line` (the earlier rule wins)."""
    start = line.index(text)
    for rule in manifest()["languages"][0]["rules"]:
        for m in re.finditer(rule["match"], line):
            g = rule.get("group", 0)
            if m.start(g) <= start < m.end(g):
                return rule["token"]
    return None


class HighlightTests(unittest.TestCase):
    def test_manifest_rules_are_the_generated_ones(self):
        self.assertEqual(manifest()["languages"][0]["rules"], rules(), "run: python3 modules/csv/test_csv.py --write")

    def test_comma_columns(self):
        line = 'id,name,"Smith, John",42'
        self.assertEqual(token_of(line, "id"), TOKENS[0])
        self.assertEqual(token_of(line, "name"), TOKENS[1])
        self.assertEqual(token_of(line, '"Smith, John"'), TOKENS[2])  # the comma in quotes doesn't split
        self.assertEqual(token_of(line, "42"), TOKENS[3])

    def test_semicolon_lines_split_at_semicolons(self):
        line = "1,5;Berlin;2,75"
        self.assertEqual(token_of(line, "1,5"), TOKENS[0])
        self.assertEqual(token_of(line, "Berlin"), TOKENS[1])
        self.assertEqual(token_of(line, "2,75"), TOKENS[2])

    def test_tabs_and_wrapping_colors(self):
        line = "\t".join(f"c{i}" for i in range(10))
        self.assertEqual(token_of(line, "c9"), TOKENS[9 % len(TOKENS)])
        self.assertEqual(token_of(line, "c1"), TOKENS[1])


class SniffTests(unittest.TestCase):
    def test_separators(self):
        self.assertEqual(ct.sniff("a,b,c\n1,2,3\n"), ",")
        self.assertEqual(ct.sniff("Name;Preis;Ort\nApfel;1,50;Berlin\nBirne;2,75;Köln\n"), ";")
        self.assertEqual(ct.sniff("a\tb\n1\t2\n"), "\t")
        self.assertEqual(ct.sniff("a|b|c\n1|2|3\n"), "|")
        self.assertEqual(ct.sniff('a,b\n"x;y;z",2\n'), ",")
        self.assertEqual(ct.sniff("whatever", "data.tsv"), "\t")

    def test_header(self):
        self.assertTrue(ct.has_header([["id", "name"], ["1", "Ada"]]))
        self.assertFalse(ct.has_header([["1", "Ada"], ["2", "Linus"]]))
        self.assertFalse(ct.has_header([["2024-01-01", "x"], ["2024-01-02", "y"]]))


class TypeTests(unittest.TestCase):
    def test_kinds(self):
        cases = {"42": "integer", "-7": "integer", "1,234,567": "integer", "3.14": "decimal", "1e5": "decimal",
                 "2024-05-01": "date", "01.05.2024": "date", "2024-05-01T10:00:00Z": "date", "true": "boolean",
                 "Ja": "boolean", "007": "text", "hello": "text", "": "empty"}
        for value, expected in cases.items():
            self.assertEqual(ct.kind(value), expected, value)

    def test_decimal_comma_in_semicolon_files(self):
        self.assertEqual(ct.kind("1,5", decimal_comma=True), "decimal")
        self.assertEqual(ct.kind("1.234,50", decimal_comma=True), "decimal")
        self.assertEqual(ct.kind("1.234", decimal_comma=True), "integer")
        self.assertEqual(ct.to_number("1.234,50", decimal_comma=True), 1234.5)
        self.assertEqual(ct.to_number("1,234"), 1234.0)  # a comma file: thousands
        self.assertEqual(ct.to_number("1,5"), 1.5)

    def test_column_stats(self):
        stats = ct.column_stats(["3", "1", "", "2"])
        self.assertEqual((stats["type"], stats["min"], stats["max"], stats["empty"]), ("integer", 1.0, 3.0, 1))
        self.assertEqual(ct.describe(stats), "integer · 1 … 3 · 1 empty · unique")
        self.assertEqual(ct.column_stats(["a", "1", "b"])["type"], "text")
        self.assertEqual(ct.column_stats(["2024-01-02", "2023-12-31"])["min"], "2023-12-31 00:00:00")
        self.assertEqual(ct.column_stats(["1", "true"])["type"], "mixed")


class OutputTests(unittest.TestCase):
    ROWS = [["name", "price", "city"], ["Apfel", "1,50", "Berlin"], ["Birne", "12,75", "Köln"], ["Kiwi", "", "Bonn"]]

    def test_table_aligns_and_right_aligns_numbers(self):
        text = ct.table(self.ROWS, header=True)
        lines = text.splitlines()
        self.assertEqual(lines[0], "name  │ price │ city")
        self.assertTrue(lines[1].startswith("──────┼"))
        self.assertEqual(lines[2], "Apfel │  1,50 │ Berlin")

    def test_json_with_decimal_commas(self):
        data = json.loads(ct.to_json(self.ROWS, header=True, decimal_comma=True))
        self.assertEqual(data[0], {"name": "Apfel", "price": 1.5, "city": "Berlin"})
        self.assertIsNone(data[2]["price"])
        data = json.loads(ct.to_json([["1", "x"], ["2", "y"]], header=False))
        self.assertEqual(data[1], {"column1": 2, "column2": "y"})

    def test_view_for_a_german_excel_file(self):
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "preise.csv")
            with open(path, "w", encoding="cp1252") as f:  # Excel's ANSI
                f.write("Name;Preis;Ort\r\nApfel;1,50;Berlin\r\nBirne;2,75;Köln\r\n")
            module = ct.CsvModule()
            module.active = path
            tree = module.tree()
            item = tree["items"][0]
            self.assertIn("semicolon separated", item["description"])
            self.assertEqual([c["label"] for c in item["children"]], ["Name", "Preis", "Ort"])
            self.assertTrue(item["children"][1]["description"].startswith("decimal · 1.5 … 2.75"))
            self.assertEqual(item["children"][2]["description"], "text · unique")


if __name__ == "__main__":
    if "--write" in sys.argv:
        path = os.path.join(HERE, "module.json")
        with open(path, encoding="utf-8") as f:
            text = f.read()
        start = text.index('"rules": [')
        end = text.index("\n      ]", start)
        body = ",\n".join("        " + json.dumps(r, ensure_ascii=False) for r in rules())
        with open(path, "w", encoding="utf-8") as f:
            f.write(text[:start] + '"rules": [\n' + body + text[end:])
        print(f"wrote {len(rules())} rules")
    else:
        unittest.main()
