"""Tests for the Fake Data module: python3 -m unittest discover -s modules/fake-data -p 'test_*.py' -v"""

import csv
import datetime
import io
import json
import os
import re
import sys
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import fake_data as fd  # noqa: E402


def luhn(number):
    digits = [int(d) for d in number][::-1]
    total = sum(d if i % 2 == 0 else (d * 2 - 9 if d * 2 > 9 else d * 2) for i, d in enumerate(digits))
    return total % 10 == 0


def iban_valid(iban):
    moved = iban[4:] + iban[:4]
    return int("".join(str(int(c, 36)) for c in moved)) % 97 == 1


class SafetyTests(unittest.TestCase):
    """Nothing generated may reach a real person, phone, host or card."""

    def test_emails_only_on_reserved_domains(self):
        for loc in ("de", "en"):
            f = fd.Faker(loc, 1)
            for _ in range(300):
                self.assertRegex(f.email(), r"^[a-z0-9._]+@example\.(com|org|net)$")

    def test_umlauts_are_spelled_out_in_emails(self):
        self.assertEqual(fd.ascii_name("Jürgen Müller-Weiß"), "JuergenMuellerWeiss")

    def test_phone_numbers_are_fictional(self):
        for _ in range(200):
            self.assertRegex(fd.Faker("de").phone(), r"^\+49 30 23125 \d{3}$")
            self.assertRegex(fd.Faker("en").phone(), r"^\(\d{3}\) 555-01\d\d$")

    def test_cards_are_test_numbers_and_luhn_valid(self):
        for brand, number in fd.TEST_CARDS:
            self.assertTrue(luhn(number), number)

    def test_ip_addresses_are_documentation_ranges(self):
        f = fd.Faker("en", 3)
        for _ in range(100):
            self.assertRegex(f.ipv4(), r"^(192\.0\.2|198\.51\.100|203\.0\.113)\.\d+$")
            self.assertTrue(f.ipv6().startswith("2001:db8:"))


class ValueTests(unittest.TestCase):
    def test_iban_check_digits(self):
        f = fd.Faker("de", 7)
        for _ in range(200):
            iban = f.iban()
            self.assertRegex(iban, r"^DE\d{20}$")
            self.assertTrue(iban_valid(iban), iban)
        self.assertTrue(iban_valid("DE89370400440532013000"))  # the documentation example itself

    def test_every_single_value_works_in_both_languages(self):
        for loc in ("de", "en"):
            for key, (label, detail, make) in fd.single_values(fd.Faker(loc, 5)).items():
                value = make()
                self.assertIsInstance(value, str, key)
                self.assertTrue(value, key)

    def test_password_has_all_kinds(self):
        p = fd.Faker("en", 9).password()
        self.assertEqual(len(p), 16)
        self.assertTrue(any(c.isupper() for c in p) and any(c.islower() for c in p) and any(c.isdigit() for c in p))

    def test_seed_repeats(self):
        a = [fd.Faker("de", "42:1").full_name() for _ in range(3)]
        self.assertEqual(len(set(a)), 1)
        self.assertNotEqual(fd.Faker("de", "42:1").iban(), fd.Faker("de", "42:2").iban())

    def test_birth_dates_are_adults(self):
        f = fd.Faker("en", 2)
        for _ in range(100):
            age = (datetime.date.today() - f.birth_date()).days / 365.25
            self.assertTrue(18 <= age <= 82, age)

    def test_records_are_consistent(self):
        f = fd.Faker("de", 11)
        person = f.record("person", 1)
        first = fd.ascii_name(person["firstName"]).lower()
        self.assertIn(first[0], person["email"].split("@")[0])
        self.assertEqual(f.record("order", 1)["currency"], "EUR")
        self.assertEqual(fd.Faker("en", 1).record("order", 1)["currency"], "USD")
        for kind in fd.RECORD_KINDS:
            self.assertTrue(f.record(kind, 1))


class FormatTests(unittest.TestCase):
    def records(self, kind="customer", n=5, loc="de"):
        f = fd.Faker(loc, 21)
        return [f.record(kind, i + 1) for i in range(n)]

    def test_json(self):
        data = json.loads(fd.render(self.records("person"), "person", "json"))
        self.assertEqual(len(data), 5)
        self.assertRegex(data[0]["birthDate"], r"^\d{4}-\d\d-\d\d$")

    def test_csv_round_trips(self):
        records = self.records("customer")
        records[0]["company"] = 'Weber, "Schmidt" & Co. KG'
        rows = list(csv.reader(io.StringIO(fd.render(records, "customer", "csv"))))
        self.assertEqual(rows[0][:3], ["id", "name", "email"])
        self.assertEqual(rows[1][4], 'Weber, "Schmidt" & Co. KG')
        self.assertEqual(len(rows), 6)

    def test_sql(self):
        records = self.records("product", 3)
        records[0]["name"] = "Kinder's Mug"
        sql = fd.render(records, "product", "sql")
        self.assertTrue(sql.startswith("INSERT INTO products (id, sku, name, category, price, stock, is_available)\nVALUES\n"))
        self.assertIn("'Kinder''s Mug'", sql)
        self.assertTrue(sql.endswith(";"))
        self.assertEqual(sql.count("\n    ("), 3)

    def test_csharp(self):
        code = fd.render(self.records("user", 2, "en"), "user", "csharp")
        self.assertTrue(code.startswith("var users = new List<User>\n{\n    new User { Id = Guid.Parse(\""))
        self.assertIn("IsActive = ", code)
        self.assertIn("LastLogin = new DateTimeOffset(", code)
        code = fd.render(self.records("person", 1), "person", "csharp")
        self.assertIn("var people = new List<Person>", code)
        self.assertIn("BirthDate = new DateOnly(", code)
        self.assertRegex(fd.render(self.records("order", 1), "order", "csharp"), r"Total = \d+\.\d\dm")

    def test_yaml(self):
        text = fd.render(self.records("company", 2), "company", "yaml")
        lines = text.splitlines()
        self.assertTrue(lines[0].startswith("- id: 1"))
        self.assertTrue(lines[1].startswith("  name: \""))
        self.assertEqual(sum(1 for l in lines if l.startswith("- ")), 2)


class ModuleTests(unittest.TestCase):
    def run_messages(self, module, *messages):
        out = io.StringIO()
        with redirect_stdout(out):
            for m in messages:
                module.handle(m)
        return [json.loads(l) for l in out.getvalue().splitlines()]

    def test_insert_records_flow(self):
        fd.settings = {"fakeData.locale": "de", "fakeData.seed": 5}
        module = fd.FakeData()
        sent = self.run_messages(module, {"type": "executeCommand", "id": "fakeData.records"})
        sent += self.run_messages(module, {"type": "quickPickResult", "requestId": sent[-1]["requestId"], "value": "order"})
        sent += self.run_messages(module, {"type": "quickPickResult", "requestId": sent[-1]["requestId"], "value": "json"})
        sent += self.run_messages(module, {"type": "quickPickResult", "requestId": sent[-1]["requestId"], "value": "10"})
        data = json.loads(sent[-1]["text"])
        self.assertEqual(len(data), 10)
        self.assertEqual(data[0]["currency"], "EUR")

    def test_insert_again(self):
        fd.settings = {"fakeData.locale": "en"}
        module = fd.FakeData()
        sent = self.run_messages(module, {"type": "executeCommand", "id": "fakeData.insert"})
        sent = self.run_messages(module, {"type": "quickPickResult", "requestId": sent[-1]["requestId"], "value": "iban"},
                                 {"type": "executeCommand", "id": "fakeData.again"})
        self.assertEqual([m["type"] for m in sent], ["insertText", "insertText"])
        self.assertTrue(all(iban_valid(m["text"]) for m in sent))

    def test_locale_from_environment(self):
        old = {k: os.environ.pop(k, None) for k in ("LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE")}
        try:
            os.environ["LANG"] = "de_DE.UTF-8"
            self.assertEqual(fd.detect_locale(), "de")
            os.environ["LANG"] = "en_US.UTF-8"
            self.assertEqual(fd.detect_locale(), "en")
        finally:
            for k, v in old.items():
                os.environ.pop(k, None)
                if v is not None:
                    os.environ[k] = v

    def test_manifest(self):
        with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
            manifest = json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))
        self.assertEqual({s["key"] for s in manifest["settings"]}, {"fakeData.locale", "fakeData.seed"})


if __name__ == "__main__":
    unittest.main()
