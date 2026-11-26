using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using ByteForge.FakeData;
using ByteForge.Modules.Testing;

namespace ByteForge.FakeData.Tests;

static class Check
{
    public static bool Luhn(string number)
    {
        var total = 0;
        var digits = number.Reverse().Select(c => c - '0').ToList();
        for (var i = 0; i < digits.Count; i++)
        {
            var d = digits[i];
            total += i % 2 == 0 ? d : d * 2 > 9 ? d * 2 - 9 : d * 2;
        }
        return total % 10 == 0;
    }

    public static bool IbanValid(string iban)
    {
        var moved = iban[4..] + iban[..4];
        var digits = string.Concat(moved.Select(c => char.IsAsciiDigit(c) ? c.ToString() : (char.ToUpperInvariant(c) - 'A' + 10).ToString()));
        return System.Numerics.BigInteger.Parse(digits) % 97 == 1;
    }

    /// <summary>A CSV reader (quotes, doubled quotes, newlines in quotes) for the round trip.</summary>
    public static List<List<string>> ReadCsv(string text)
    {
        var rows = new List<List<string>>();
        var row = new List<string>();
        var cell = new StringBuilder();
        var quoted = false;
        for (var i = 0; i < text.Length; i++)
        {
            var c = text[i];
            if (quoted)
            {
                if (c == '"' && i + 1 < text.Length && text[i + 1] == '"')
                {
                    cell.Append('"');
                    i++;
                }
                else if (c == '"')
                {
                    quoted = false;
                }
                else
                {
                    cell.Append(c);
                }
            }
            else if (c == '"')
            {
                quoted = true;
            }
            else if (c == ',')
            {
                row.Add(cell.ToString());
                cell.Clear();
            }
            else if (c == '\n')
            {
                row.Add(cell.ToString());
                cell.Clear();
                rows.Add(row);
                row = [];
            }
            else
            {
                cell.Append(c);
            }
        }
        row.Add(cell.ToString());
        rows.Add(row);
        return rows;
    }
}

/// <summary>Nothing generated may reach a real person, phone, host or card.</summary>
public sealed class SafetyTests
{
    [Fact]
    public void Emails_only_on_reserved_domains()
    {
        foreach (var locale in new[] { "de", "en" })
        {
            var f = new Faker(locale, 1);
            for (var i = 0; i < 300; i++)
            {
                Assert.Matches(@"^[a-z0-9._]+@example\.(com|org|net)$", f.Email());
            }
        }
    }

    [Fact]
    public void Umlauts_are_spelled_out_in_emails() => Assert.Equal("JuergenMuellerWeiss", Faker.AsciiName("Jürgen Müller-Weiß"));

    [Fact]
    public void Phone_numbers_are_fictional()
    {
        for (var i = 0; i < 200; i++)
        {
            Assert.Matches(@"^\+49 30 23125 \d{3}$", new Faker("de").Phone());
            Assert.Matches(@"^\(\d{3}\) 555-01\d\d$", new Faker("en").Phone());
        }
    }

    [Fact]
    public void Cards_are_test_numbers_and_luhn_valid()
    {
        foreach (var (_, number) in Faker.TestCards)
        {
            Assert.True(Check.Luhn(number), number);
        }
    }

    [Fact]
    public void Ip_addresses_are_documentation_ranges()
    {
        var f = new Faker("en", 3);
        for (var i = 0; i < 100; i++)
        {
            Assert.Matches(@"^(192\.0\.2|198\.51\.100|203\.0\.113)\.\d+$", f.Ipv4());
            Assert.StartsWith("2001:db8:", f.Ipv6());
        }
    }
}

public sealed class ValueTests
{
    [Fact]
    public void Iban_check_digits()
    {
        var f = new Faker("de", 7);
        for (var i = 0; i < 200; i++)
        {
            var iban = f.Iban();
            Assert.Matches(@"^DE\d{20}$", iban);
            Assert.True(Check.IbanValid(iban), iban);
        }
        Assert.True(Check.IbanValid("DE89370400440532013000")); // the documentation example itself
    }

    [Fact]
    public void Every_single_value_works_in_both_languages()
    {
        foreach (var locale in new[] { "de", "en" })
        {
            var f = new Faker(locale, 5);
            foreach (var kind in Faker.Values)
            {
                var value = f.Value(kind.Key);
                Assert.False(string.IsNullOrEmpty(value), kind.Key);
            }
        }
        Assert.Null(new Faker().Value("nope"));
    }

    [Fact]
    public void Single_values_have_their_formats()
    {
        var f = new Faker("en", 13);
        Assert.Matches(@"^\d{4}-\d\d-\d\d$", f.Value("birthDate"));
        Assert.Matches(@"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$", f.Value("pastDate"));
        Assert.Matches(@"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$", f.Value("futureDate"));
        Assert.Matches(@"^\d+\.\d\d$", f.Value("price"));
        Assert.Matches(@"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", f.Value("guid"));
        Assert.Matches(@"^02(:[0-9a-f]{2}){5}$", f.Value("mac"));
        Assert.Matches(@"^#[0-9a-f]{6}$", f.Value("color"));
        Assert.Matches(@"^[A-Z]{3}-\d{4}$", f.Value("sku"));
        Assert.Matches($@"^ORD-{DateTime.Today.Year}-\d{{5}}$", f.Value("orderNumber"));
        Assert.Matches(@"^-?\d+\.\d+, -?\d+\.\d+$", f.Value("latLon"));
        Assert.Matches(@"^[A-Z][a-z ]+\.$", f.Value("sentence"));
        Assert.Matches(@"^\d+ .+, .+, [A-Z]{2} \d{5}$", f.Value("address"));
        Assert.Matches(@"^.+ \d+[ab]?, \d{5} .+$", new Faker("de", 13).Value("address"));
    }

    [Fact]
    public void Password_has_all_kinds()
    {
        var p = new Faker("en", 9).Password();
        Assert.Equal(16, p.Length);
        Assert.True(p.Any(char.IsUpper) && p.Any(char.IsLower) && p.Any(char.IsDigit) && p.Any(c => !char.IsLetterOrDigit(c)));
    }

    [Fact]
    public void Seed_repeats()
    {
        var a = Enumerable.Range(0, 3).Select(_ => new Faker("de", "42:1").FullName()).ToHashSet();
        Assert.Single(a);
        Assert.NotEqual(new Faker("de", "42:1").Iban(), new Faker("de", "42:2").Iban());
        // Fixed across runs and platforms, and the Python module's values (random.Random("7:1")).
        Assert.Equal(new Faker("en", "7:1").Guid(), new Faker("en", "7:1").Guid());
    }

    [Fact]
    public void Random_is_pythons_mersenne_twister()
    {
        // What CPython's random.Random gives for these seeds.
        var r = new PyRandom(42);
        Assert.Equal(0.6394267984578837, r.Random());
        Assert.Equal(107420369, (long)r.GetRandBits(32));
        Assert.Equal(6, r.RandInt(1, 6));
        Assert.Equal(0.2720295377534757, new PyRandom("a").Random());
        Assert.Equal(0.8444218515250481, new PyRandom(0).Random());
        Assert.Equal(System.Numerics.BigInteger.Parse("1484549092569213973"), new PyRandom(System.Numerics.BigInteger.Pow(2, 100)).GetRandBits(64));
    }

    [Fact]
    public void Birth_dates_are_adults()
    {
        var f = new Faker("en", 2);
        var today = DateOnly.FromDateTime(DateTime.Today);
        for (var i = 0; i < 100; i++)
        {
            var age = (today.DayNumber - f.BirthDate().DayNumber) / 365.25;
            Assert.InRange(age, 18, 82);
        }
    }

    [Fact]
    public void Records_are_consistent()
    {
        var f = new Faker("de", 11);
        var person = f.MakeRecord("person", 1);
        var first = Faker.AsciiName((string)person["firstName"]!).ToLowerInvariant();
        Assert.Contains(first[0], ((string)person["email"]!).Split('@')[0]);
        Assert.Equal("EUR", f.MakeRecord("order", 1)["currency"]);
        Assert.Equal("USD", new Faker("en", 1).MakeRecord("order", 1)["currency"]);
        foreach (var kind in Render.RecordKinds)
        {
            Assert.NotEmpty(f.MakeRecord(kind.Key, 1));
        }
        Assert.Throws<ArgumentException>(() => f.MakeRecord("nope", 1));
    }
}

public sealed class FormatTests
{
    static List<Record> Records(string kind = "customer", int n = 5, string locale = "de")
    {
        var f = new Faker(locale, 21);
        return Enumerable.Range(1, n).Select(i => f.MakeRecord(kind, i)).ToList();
    }

    [Fact]
    public void Json()
    {
        var data = JsonNode.Parse(Render.ToText(Records("person"), "person", "json"))!.AsArray();
        Assert.Equal(5, data.Count);
        Assert.Matches(@"^\d{4}-\d\d-\d\d$", data[0]!["birthDate"]!.GetValue<string>());
    }

    [Fact]
    public void Csv_round_trips()
    {
        var records = Records("customer");
        records[0]["company"] = "Weber, \"Schmidt\" & Co. KG";
        var rows = Check.ReadCsv(Render.ToText(records, "customer", "csv"));
        Assert.Equal(["id", "name", "email"], rows[0][..3]);
        Assert.Equal("Weber, \"Schmidt\" & Co. KG", rows[1][4]);
        Assert.Equal(6, rows.Count);
    }

    [Fact]
    public void Sql()
    {
        var records = Records("product", 3);
        records[0]["name"] = "Kinder's Mug";
        var sql = Render.ToText(records, "product", "sql");
        Assert.StartsWith("INSERT INTO products (id, sku, name, category, price, stock, is_available)\nVALUES\n", sql);
        Assert.Contains("'Kinder''s Mug'", sql);
        Assert.EndsWith(";", sql);
        Assert.Equal(3, Regex.Matches(sql, @"\n    \(").Count);
    }

    [Fact]
    public void CSharp()
    {
        var code = Render.ToText(Records("user", 2, "en"), "user", "csharp");
        Assert.StartsWith("var users = new List<User>\n{\n    new User { Id = Guid.Parse(\"", code);
        Assert.Contains("IsActive = ", code);
        Assert.Contains("LastLogin = new DateTimeOffset(", code);
        code = Render.ToText(Records("person", 1), "person", "csharp");
        Assert.Contains("var people = new List<Person>", code);
        Assert.Contains("BirthDate = new DateOnly(", code);
        Assert.Matches(@"Total = \d+\.\d\dm", Render.ToText(Records("order", 1), "order", "csharp"));
    }

    [Fact]
    public void Yaml()
    {
        var lines = Render.ToText(Records("company", 2), "company", "yaml").Split('\n');
        Assert.StartsWith("- id: 1", lines[0]);
        Assert.StartsWith("  name: \"", lines[1]);
        Assert.Equal(2, lines.Count(l => l.StartsWith("- ", StringComparison.Ordinal)));
    }

    /// <summary>The records the Python module rendered into python-render.json.</summary>
    static List<Record> Fixed() =>
    [
        new()
        {
            ["id"] = 1, ["name"] = "Weber, \"Schmidt\" & Co.\nKG", ["note"] = "Kinder's \\ Müller\t\u0001", ["price"] = 12.0, ["total"] = 1234.5,
            ["ratio"] = 0.1, ["isActive"] = true, ["missing"] = null, ["birthDate"] = new DateOnly(1990, 3, 7),
            ["createdAt"] = new DateTimeOffset(2025, 1, 2, 3, 4, 5, TimeSpan.Zero),
            ["uid"] = "0f8fad5b-d9cb-469f-a165-70867728950e", ["lat"] = 52.520008,
        },
        new()
        {
            ["id"] = 2, ["name"] = "Plain", ["note"] = "", ["price"] = 499.99, ["total"] = 5.0, ["ratio"] = 1e-05, ["isActive"] = false, ["missing"] = null,
            ["birthDate"] = new DateOnly(2001, 12, 31), ["createdAt"] = new DateTimeOffset(2024, 12, 31, 23, 59, 59, TimeSpan.Zero),
            ["uid"] = "ABC", ["lat"] = -124.0,
        },
    ];

    [Fact]
    public void Renders_exactly_like_the_python_module()
    {
        var expected = JsonNode.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "python-render.json")))!.AsObject();
        Assert.Equal(30, expected.Count);
        foreach (var (key, text) in expected)
        {
            var parts = key.Split('/');
            Assert.Equal(text!.GetValue<string>(), Render.ToText(Fixed(), parts[0], parts[1]));
        }
    }

    [Fact]
    public void Floats_are_written_as_python_writes_them()
    {
        var cases = new (double Value, string Repr)[]
        {
            (12.0, "12.0"), (0.1, "0.1"), (1e-05, "1e-05"), (0.0001, "0.0001"), (1e16, "1e+16"), (1e15, "1000000000000000.0"),
            (-0.0, "-0.0"), (123.45, "123.45"), (1.5e300, "1.5e+300"), (5e-324, "5e-324"), (double.PositiveInfinity, "inf"),
            (0.30000000000000004, "0.30000000000000004"), (-124.0, "-124.0"), (52.520008, "52.520008"),
        };
        foreach (var (value, repr) in cases)
        {
            Assert.Equal(repr, Py.Repr(value));
        }
    }
}

[CollectionDefinition("process environment", DisableParallelization = true)]
public sealed class ProcessEnvironment;

[Collection("process environment")]
public sealed class ModuleTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-fake-data-").FullName;

    public void Dispose()
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    string ModuleDir => Path.Combine(dir, "modules", "fake-data");

    static JsonObject Run(string id) => new() { ["type"] = "executeCommand", ["id"] = id };

    static JsonObject Pick(JsonObject request, string value) =>
        new() { ["type"] = "quickPickResult", ["requestId"] = request["requestId"]!.GetValue<long>(), ["value"] = value };

    [Fact]
    public async Task Registers_the_commands()
    {
        await using var module = ModuleHarness.Start(new FakeDataModule(), ModuleDir);
        var insert = await module.Next("registerCommand");
        Assert.Equal(("fakeData.insert", "Insert…", "Fake Data", "ctrl-alt-f"),
            (insert["id"]!.GetValue<string>(), insert["title"]!.GetValue<string>(), insert["category"]!.GetValue<string>(), insert["keybinding"]!.GetValue<string>()));
        var records = await module.Next("registerCommand");
        Assert.Equal(("fakeData.records", "Insert Records…", null), (records["id"]!.GetValue<string>(), records["title"]!.GetValue<string>(), records["keybinding"]));
        var again = await module.Next("registerCommand");
        Assert.Equal("Insert Again (same kind, new value)", again["title"]!.GetValue<string>());
    }

    [Fact]
    public async Task Insert_records_flow()
    {
        await using var module = ModuleHarness.Start(new FakeDataModule(), ModuleDir);
        module.Activate(dir, new() { ["fakeData.locale"] = "de", ["fakeData.seed"] = 5 });
        module.Send(Run("fakeData.records"));
        var kinds = await module.Next("quickPick");
        Assert.Equal("Insert records of…", kinds["placeholder"]!.GetValue<string>());
        Assert.Equal(["person", "customer", "user", "product", "order", "company"], kinds["items"]!.AsArray().Select(i => i!["value"]!.GetValue<string>()));
        module.Send(Pick(kinds, "order"));
        var formats = await module.Next("quickPick");
        Assert.Equal("Orders as…", formats["placeholder"]!.GetValue<string>());
        module.Send(Pick(formats, "json"));
        var counts = await module.Next("quickPick");
        Assert.Equal(["3", "5", "10", "25", "50", "100"], counts["items"]!.AsArray().Select(i => i!["label"]!.GetValue<string>()));
        module.Send(Pick(counts, "10"));
        var data = JsonNode.Parse((await module.Next("insertText"))["text"]!.GetValue<string>())!.AsArray();
        Assert.Equal(10, data.Count);
        Assert.Equal("EUR", data[0]!["currency"]!.GetValue<string>());
    }

    [Fact]
    public async Task Insert_again()
    {
        await using var module = ModuleHarness.Start(new FakeDataModule(), ModuleDir);
        module.Activate(dir, new() { ["fakeData.locale"] = "en" });
        module.Send(Run("fakeData.insert"));
        var pick = await module.Next("quickPick");
        Assert.Equal("Insert fake…", pick["placeholder"]!.GetValue<string>());
        Assert.Equal(Faker.Values.Length, pick["items"]!.AsArray().Count);
        module.Send(Pick(pick, "iban"));
        var first = await module.Next("insertText");
        module.Send(Run("fakeData.again"));
        var second = await module.Next("insertText");
        Assert.True(Check.IbanValid(first["text"]!.GetValue<string>()));
        Assert.True(Check.IbanValid(second["text"]!.GetValue<string>()));
    }

    [Fact]
    public async Task Insert_again_without_a_value_asks()
    {
        await using var module = ModuleHarness.Start(new FakeDataModule(), ModuleDir);
        module.Activate(dir);
        module.Send(Run("fakeData.again"));
        Assert.Equal("Insert fake…", (await module.Next("quickPick"))["placeholder"]!.GetValue<string>());
    }

    [Fact]
    public async Task A_seed_repeats_the_nth_insertion()
    {
        async Task<List<string>> Insertions()
        {
            await using var module = ModuleHarness.Start(new FakeDataModule(), ModuleDir);
            module.Activate(dir, new() { ["fakeData.locale"] = "en", ["fakeData.seed"] = 42 });
            var texts = new List<string>();
            module.Send(Run("fakeData.insert"));
            module.Send(Pick(await module.Next("quickPick"), "guid"));
            texts.Add((await module.Next("insertText"))["text"]!.GetValue<string>());
            module.Send(Run("fakeData.again"));
            texts.Add((await module.Next("insertText"))["text"]!.GetValue<string>());
            return texts;
        }
        var a = await Insertions();
        var b = await Insertions();
        Assert.Equal(a, b);
        Assert.NotEqual(a[0], a[1]);
    }

    [Fact]
    public void Locale_from_environment()
    {
        var names = new[] { "LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE" };
        var old = names.ToDictionary(n => n, Environment.GetEnvironmentVariable);
        try
        {
            foreach (var name in names)
            {
                Environment.SetEnvironmentVariable(name, null);
            }
            Environment.SetEnvironmentVariable("LANG", "de_DE.UTF-8");
            Assert.Equal("de", FakeDataModule.DetectLocale());
            Environment.SetEnvironmentVariable("LANG", "en_US.UTF-8");
            Assert.Equal("en", FakeDataModule.DetectLocale());
        }
        finally
        {
            foreach (var (name, value) in old)
            {
                Environment.SetEnvironmentVariable(name, value);
            }
        }
    }

    [Fact]
    public void Manifest()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "..", "..", "module.json");
        var manifest = JsonNode.Parse(File.ReadAllText(path), documentOptions: new JsonDocumentOptions { CommentHandling = JsonCommentHandling.Skip })!;
        Assert.Equal(["fakeData.locale", "fakeData.seed"], manifest["settings"]!.AsArray().Select(s => s!["key"]!.GetValue<string>()).Order());
        Assert.Equal("dotnet", manifest["runtime"]!.GetValue<string>());
        Assert.Equal("bin/FakeData.dll", manifest["assembly"]!.GetValue<string>());
    }
}
