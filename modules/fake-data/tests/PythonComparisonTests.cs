using System.Globalization;
using System.Numerics;
using System.Text.Json.Nodes;
using ByteForge.FakeData;
using ByteForge.Modules.Testing;

namespace ByteForge.FakeData.Tests;

/// <summary>
/// The C# against the Python module it replaces: python-faker.json holds what fake_data.py (ByteForge-packages
/// commit 6f46c2c, Python 3.14) generated for seeded generators with the clock fixed — every single value twice,
/// three records of each kind in every format, round() results, and a seeded session in the module.
/// </summary>
public sealed class PythonComparisonTests
{
    static readonly JsonObject Expected =
        JsonNode.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "python-faker.json")))!.AsObject();

    static readonly DateTimeOffset Now = DateTimeOffset.Parse(Expected["now"]!.GetValue<string>(), CultureInfo.InvariantCulture);

    internal static Faker Seeded(string locale, JsonNode seed, DateTimeOffset now, int year) =>
        seed["text"] is { } text
            ? new Faker(locale, text.GetValue<string>()) { UtcNow = () => now, Year = () => year }
            : new Faker(locale, BigInteger.Parse(seed["int"]!.GetValue<string>(), CultureInfo.InvariantCulture)) { UtcNow = () => now, Year = () => year };

    [Fact]
    public void Single_values_are_the_pythons()
    {
        var year = Expected["year"]!.GetValue<int>();
        foreach (var set in Expected["values"]!.AsArray())
        {
            var faker = Seeded(set!["locale"]!.GetValue<string>(), set["seed"]!, Now, year);
            foreach (var pair in set["values"]!.AsArray())
            {
                var key = pair![0]!.GetValue<string>();
                Assert.True(pair[1]!.GetValue<string>() == faker.Value(key), $"{set["locale"]} {set["seed"]!.ToJsonString()} {key}");
            }
        }
    }

    [Fact]
    public void Records_in_every_format_are_the_pythons()
    {
        var year = Expected["year"]!.GetValue<int>();
        foreach (var set in Expected["records"]!.AsArray())
        {
            var kind = set!["kind"]!.GetValue<string>();
            var faker = Seeded(set["locale"]!.GetValue<string>(), set["seed"]!, Now, year);
            var records = Enumerable.Range(1, 3).Select(i => faker.MakeRecord(kind, i)).ToList();
            foreach (var (format, text) in set["texts"]!.AsObject())
            {
                Assert.Equal(text!.GetValue<string>(), Render.ToText(records, kind, format));
            }
        }
    }

    [Fact]
    public void Round_is_pythons()
    {
        foreach (var round in Expected["rounds"]!.AsArray())
        {
            var value = double.Parse(round![0]!.GetValue<string>(), CultureInfo.InvariantCulture);
            Assert.Equal(round[2]!.GetValue<string>(), Py.Repr(Py.Round(value, round[1]!.GetValue<int>())));
        }
        Assert.Equal(0.12, Py.Round(0.125, 2)); // half to even on the exact value
        Assert.Equal(2.67, Py.Round(2.675, 2)); // 2.675 is 2.67499999… as a double
    }

    [Fact]
    public async Task A_seeded_session_inserts_what_the_python_inserted()
    {
        var dir = Directory.CreateTempSubdirectory("bf-fake-data-").FullName;
        try
        {
            await using var module = ModuleHarness.Start(new FakeDataModule(), Path.Combine(dir, "modules", "fake-data"));
            module.Activate(dir, Expected["module"]!["settings"]!.DeepClone().AsObject());
            static JsonObject Run(string id) => new() { ["type"] = "executeCommand", ["id"] = id };
            static JsonObject Pick(JsonObject request, string value) =>
                new() { ["type"] = "quickPickResult", ["requestId"] = request["requestId"]!.GetValue<long>(), ["value"] = value };
            var inserted = new List<string>();
            module.Send(Run("fakeData.insert"));
            module.Send(Pick(await module.Next("quickPick"), "iban"));
            inserted.Add((await module.Next("insertText"))["text"]!.GetValue<string>());
            module.Send(Run("fakeData.again"));
            inserted.Add((await module.Next("insertText"))["text"]!.GetValue<string>());
            module.Send(Run("fakeData.records"));
            module.Send(Pick(await module.Next("quickPick"), "product"));
            module.Send(Pick(await module.Next("quickPick"), "csv"));
            module.Send(Pick(await module.Next("quickPick"), "5"));
            inserted.Add((await module.Next("insertText"))["text"]!.GetValue<string>());
            Assert.Equal(Expected["module"]!["inserted"]!.AsArray().Select(t => t!.GetValue<string>()), inserted);
        }
        finally
        {
            Directory.Delete(dir, recursive: true);
        }
    }
}
