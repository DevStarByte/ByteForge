using System.Globalization;
using System.Numerics;
using System.Text.Json;
using System.Text.Json.Nodes;
using ByteForge.Modules;

namespace ByteForge.FakeData;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<FakeDataModule>(input, output, moduleDir, cancel);
}

/// <summary>
/// Fake Data: realistic test data inserted at the cursor. "Fake Data: Insert…" (Ctrl+Alt+F) inserts one value,
/// "Fake Data: Insert Records…" 3 to 100 records as JSON, C#, CSV, SQL INSERT or YAML. "fakeData.locale" picks
/// German or English data (auto: from the system language); "fakeData.seed" makes every insertion repeatable.
/// </summary>
public sealed class FakeDataModule : Module
{
    int insertions;
    volatile string? last; // the last single value kind, for "Insert Again"

    protected override Task OnStartAsync()
    {
        RegisterCommand("fakeData.insert", "Insert…", "Fake Data", "ctrl-alt-f");
        RegisterCommand("fakeData.records", "Insert Records…", "Fake Data");
        RegisterCommand("fakeData.again", "Insert Again (same kind, new value)", "Fake Data");
        return Task.CompletedTask;
    }

    protected override async Task OnCommandAsync(string id)
    {
        try
        {
            switch (id)
            {
                case "fakeData.insert":
                    await PickValueAsync();
                    break;
                case "fakeData.records":
                    await PickRecordsAsync();
                    break;
                case "fakeData.again":
                    if (last is { } kind)
                    {
                        InsertValue(kind);
                    }
                    else
                    {
                        await PickValueAsync();
                    }
                    break;
            }
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            Log($"fake-data: {e.GetType().Name}: {e.Message}");
        }
    }

    /// <summary>"de" or "en" from LC_ALL, LC_MESSAGES, LANG or LANGUAGE, else the system's language.</summary>
    public static string DetectLocale()
    {
        foreach (var name in new[] { "LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE" })
        {
            var value = Environment.GetEnvironmentVariable(name);
            if (!string.IsNullOrEmpty(value))
            {
                return value.StartsWith("de", StringComparison.OrdinalIgnoreCase) ? "de" : "en";
            }
        }
        return CultureInfo.CurrentCulture.TwoLetterISOLanguageName == "de" ? "de" : "en";
    }

    /// <summary>A generator for one insertion. With a seed, the n-th insertion of a session is always the same.</summary>
    Faker NewFaker()
    {
        var setting = Settings["fakeData.locale"];
        var locale = !Truthy(setting) ? "auto" : setting is JsonValue v && v.TryGetValue<string>(out var s) ? s : "";
        locale = locale == "auto" ? DetectLocale() : locale is "de" or "en" ? locale : "en";
        var seed = Settings["fakeData.seed"];
        var n = Interlocked.Increment(ref insertions);
        return new Faker(locale, Truthy(seed) ? $"{SeedText(seed!)}:{n}" : null);
    }

    /// <summary>Python's truth value of a JSON setting (null, false, 0, "", [] and {} are false).</summary>
    static bool Truthy(JsonNode? node) => node switch
    {
        null => false,
        JsonArray array => array.Count > 0,
        JsonObject obj => obj.Count > 0,
        _ => node.GetValueKind() switch
        {
            JsonValueKind.False or JsonValueKind.Null => false,
            JsonValueKind.String => node.GetValue<string>().Length > 0,
            JsonValueKind.Number => node.GetValue<double>() != 0,
            _ => true,
        },
    };

    /// <summary>The seed as Python's str() of it: 42, 42.5, or the text.</summary>
    static string SeedText(JsonNode seed)
    {
        if (seed.GetValueKind() == JsonValueKind.String)
        {
            return seed.GetValue<string>();
        }
        if (seed.GetValueKind() == JsonValueKind.Number)
        {
            var raw = seed.ToJsonString();
            return raw.AsSpan().IndexOfAny(".eE") < 0
                ? BigInteger.Parse(raw, CultureInfo.InvariantCulture).ToString(CultureInfo.InvariantCulture)
                : Py.Repr(seed.GetValue<double>());
        }
        if (seed.GetValueKind() == JsonValueKind.True)
        {
            return "True";
        }
        return seed.ToJsonString();
    }

    async Task PickValueAsync()
    {
        var key = await QuickPickAsync("Insert fake…", Faker.Values.Select(v => new PickItem(v.Label, v.Key, v.Detail)));
        if (!string.IsNullOrEmpty(key))
        {
            InsertValue(key);
        }
    }

    void InsertValue(string key)
    {
        if (NewFaker().Value(key) is { } value)
        {
            last = key;
            InsertText(value);
        }
    }

    async Task PickRecordsAsync()
    {
        var kind = await QuickPickAsync("Insert records of…", Render.RecordKinds.Select(k => new PickItem(k.Label, k.Key, k.Detail)));
        if (string.IsNullOrEmpty(kind))
        {
            return;
        }
        var label = Render.RecordKinds.FirstOrDefault(k => k.Key == kind)?.Label ?? throw new KeyNotFoundException(kind);
        var format = await QuickPickAsync($"{label} as…", Render.Formats.Select(f => new PickItem(f.Label, f.Key, f.Detail)));
        if (string.IsNullOrEmpty(format))
        {
            return;
        }
        var count = await QuickPickAsync("How many?", new[] { 3, 5, 10, 25, 50, 100 }.Select(n => new PickItem(n.ToString(CultureInfo.InvariantCulture))));
        if (string.IsNullOrEmpty(count))
        {
            return;
        }
        var faker = NewFaker();
        var records = Enumerable.Range(1, int.Parse(count, CultureInfo.InvariantCulture)).Select(i => faker.MakeRecord(kind, i)).ToList();
        InsertText(Render.ToText(records, kind, format));
    }
}
