using System.Globalization;
using System.Numerics;
using System.Text;
using System.Text.RegularExpressions;

namespace ByteForge.FakeData;

/// <summary>A record: field names in order, with int, double, bool, string, DateOnly or DateTimeOffset (UTC) values.</summary>
public sealed class Record : OrderedDictionary<string, object?>;

/// <summary>
/// Realistic fake values. Nothing reaches a real person or account: emails use the domains reserved for examples
/// (RFC 2606), German phone numbers the Bundesnetzagentur's block for films and TV (030 23125 xxx), US ones
/// 555-0100 to 555-0199, cards are the payment providers' test numbers, IBANs use the documentation bank code.
/// </summary>
public sealed partial class Faker
{
    public static readonly IReadOnlyDictionary<string, IReadOnlyDictionary<string, string[]>> First = new Dictionary<string, IReadOnlyDictionary<string, string[]>>
    {
        ["de"] = new Dictionary<string, string[]>
        {
            ["f"] = ["Anna", "Lena", "Mia", "Emma", "Hannah", "Sophie", "Lea", "Laura", "Julia", "Sarah", "Lisa", "Marie",
                "Johanna", "Katharina", "Clara", "Paula", "Charlotte", "Greta", "Ida", "Nele", "Frieda", "Luise",
                "Sabine", "Petra", "Claudia", "Monika", "Ingrid", "Ursula", "Birgit", "Anja"],
            ["m"] = ["Lukas", "Leon", "Finn", "Jonas", "Paul", "Felix", "Elias", "Noah", "Maximilian", "Ben", "Tim", "Jan",
                "Moritz", "Niklas", "Tobias", "Florian", "Sebastian", "Matthias", "Stefan", "Thomas", "Michael",
                "Andreas", "Jürgen", "Klaus", "Wolfgang", "Uwe", "Dieter", "Frank", "Markus", "Henrik"],
        },
        ["en"] = new Dictionary<string, string[]>
        {
            ["f"] = ["Olivia", "Emma", "Ava", "Sophia", "Isabella", "Mia", "Amelia", "Harper", "Evelyn", "Abigail", "Emily",
                "Grace", "Chloe", "Zoe", "Lily", "Hannah", "Nora", "Ella", "Madison", "Aria", "Scarlett", "Victoria",
                "Hazel", "Ruby", "Alice", "Megan", "Rachel", "Laura", "Jessica", "Sarah"],
            ["m"] = ["Liam", "Noah", "Oliver", "James", "Elijah", "William", "Henry", "Lucas", "Benjamin", "Theodore",
                "Jack", "Samuel", "Daniel", "Matthew", "Joseph", "David", "Michael", "Andrew", "Ryan", "Nathan",
                "Ethan", "Logan", "Owen", "Caleb", "Isaac", "Aaron", "Connor", "Dylan", "Robert", "Thomas"],
        },
    };

    public static readonly IReadOnlyDictionary<string, string[]> Last = new Dictionary<string, string[]>
    {
        ["de"] = ["Müller", "Schmidt", "Schneider", "Fischer", "Weber", "Meyer", "Wagner", "Becker", "Schulz", "Hoffmann",
            "Schäfer", "Koch", "Bauer", "Richter", "Klein", "Wolf", "Schröder", "Neumann", "Schwarz", "Zimmermann",
            "Braun", "Krüger", "Hofmann", "Hartmann", "Lange", "Schmitt", "Werner", "Schmitz", "Krause", "Meier",
            "Lehmann", "Köhler", "Herrmann", "König", "Walter", "Mayer", "Huber", "Kaiser", "Fuchs", "Peters"],
        ["en"] = ["Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis", "Rodriguez", "Martinez",
            "Wilson", "Anderson", "Taylor", "Thomas", "Moore", "Jackson", "Martin", "Lee", "Thompson", "White",
            "Harris", "Clark", "Lewis", "Robinson", "Walker", "Young", "Allen", "King", "Wright", "Scott", "Green",
            "Baker", "Adams", "Nelson", "Hill", "Campbell", "Mitchell", "Roberts", "Carter", "Phillips"],
    };

    public static readonly IReadOnlyDictionary<string, string[]> Streets = new Dictionary<string, string[]>
    {
        ["de"] = ["Hauptstraße", "Bahnhofstraße", "Gartenstraße", "Schulstraße", "Dorfstraße", "Bergstraße", "Lindenstraße",
            "Kirchstraße", "Waldstraße", "Ringstraße", "Mühlenweg", "Am Markt", "Rosenweg", "Birkenweg", "Goethestraße",
            "Schillerstraße", "Friedrichstraße", "Wiesenweg", "Feldstraße", "Parkstraße", "Uhlandstraße", "Mozartstraße"],
        ["en"] = ["Main Street", "Oak Avenue", "Maple Drive", "Cedar Lane", "Pine Street", "Elm Street", "Washington Avenue",
            "Lake Road", "Hill Street", "Park Avenue", "Sunset Boulevard", "River Road", "Church Street", "Highland Drive",
            "Spring Street", "Lincoln Avenue", "Walnut Street", "Chestnut Street", "Willow Way", "Meadow Lane"],
    };

    /// <summary>A real city with a postal code that belongs to it (and its state in the US).</summary>
    public sealed record City(string Name, string? State, string Zip);

    public static readonly IReadOnlyDictionary<string, City[]> Cities = new Dictionary<string, City[]>
    {
        ["de"] = [new("Berlin", null, "10115"), new("Hamburg", null, "20095"), new("München", null, "80331"), new("Köln", null, "50667"),
            new("Frankfurt am Main", null, "60311"), new("Stuttgart", null, "70173"), new("Düsseldorf", null, "40213"), new("Leipzig", null, "04109"),
            new("Dortmund", null, "44135"), new("Essen", null, "45127"), new("Bremen", null, "28195"), new("Dresden", null, "01067"), new("Hannover", null, "30159"),
            new("Nürnberg", null, "90403"), new("Bonn", null, "53111"), new("Münster", null, "48143"), new("Freiburg im Breisgau", null, "79098"),
            new("Augsburg", null, "86150"), new("Heidelberg", null, "69117"), new("Regensburg", null, "93047"), new("Kiel", null, "24103"), new("Erfurt", null, "99084")],
        ["en"] = [new("New York", "NY", "10001"), new("Los Angeles", "CA", "90012"), new("Chicago", "IL", "60601"),
            new("Houston", "TX", "77002"), new("Phoenix", "AZ", "85003"), new("Philadelphia", "PA", "19103"),
            new("San Antonio", "TX", "78205"), new("San Diego", "CA", "92101"), new("Dallas", "TX", "75201"),
            new("Austin", "TX", "78701"), new("Seattle", "WA", "98101"), new("Denver", "CO", "80202"), new("Boston", "MA", "02108"),
            new("Portland", "OR", "97204"), new("Atlanta", "GA", "30303"), new("Miami", "FL", "33130"),
            new("Minneapolis", "MN", "55401"), new("Nashville", "TN", "37203"), new("San Francisco", "CA", "94103")],
    };

    static readonly IReadOnlyDictionary<string, string[]> CompanySuffix = new Dictionary<string, string[]>
    {
        ["de"] = ["GmbH", "AG", "GmbH & Co. KG", "KG", "UG (haftungsbeschränkt)", "e.K."],
        ["en"] = ["Inc.", "LLC", "Corp.", "Ltd.", "Group", "& Co."],
    };

    static readonly IReadOnlyDictionary<string, string[]> CompanyWords = new Dictionary<string, string[]>
    {
        ["de"] = ["Software", "Logistik", "Bau", "Technik", "Handel", "Consulting", "Medien", "Systeme", "Energie", "Digital", "Werkzeuge", "Elektro"],
        ["en"] = ["Software", "Logistics", "Systems", "Solutions", "Labs", "Consulting", "Media", "Industries", "Dynamics", "Digital", "Analytics", "Networks"],
    };

    static readonly IReadOnlyDictionary<string, string[]> Jobs = new Dictionary<string, string[]>
    {
        ["de"] = ["Softwareentwickler", "Projektleiterin", "Buchhalter", "Vertriebsleiterin", "Systemadministrator",
            "Produktmanagerin", "Lagerist", "Personalreferentin", "Data Scientist", "Kundenberater", "Marketingmanagerin", "Teamleiter IT"],
        ["en"] = ["Software Engineer", "Project Manager", "Accountant", "Sales Director", "System Administrator",
            "Product Manager", "Warehouse Associate", "HR Specialist", "Data Scientist", "Customer Success Manager", "Marketing Manager", "IT Team Lead"],
    };

    static readonly IReadOnlyDictionary<string, string[]> ProductAdjectives = new Dictionary<string, string[]>
    {
        ["de"] = ["Ergonomische", "Kabellose", "Robuste", "Kompakte", "Elegante", "Smarte", "Leichte", "Praktische"],
        ["en"] = ["Ergonomic", "Wireless", "Rugged", "Compact", "Sleek", "Smart", "Lightweight", "Handy"],
    };

    static readonly IReadOnlyDictionary<string, string[]> ProductNouns = new Dictionary<string, string[]>
    {
        ["de"] = ["Tastatur", "Maus", "Lampe", "Tasse", "Kamera", "Uhr", "Tasche", "Flasche", "Brille", "Jacke"],
        ["en"] = ["Keyboard", "Mouse", "Lamp", "Mug", "Camera", "Watch", "Backpack", "Bottle", "Headphones", "Jacket"],
    };

    static readonly IReadOnlyDictionary<string, string[]> Categories = new Dictionary<string, string[]>
    {
        ["de"] = ["Elektronik", "Büro", "Haushalt", "Sport", "Bekleidung", "Garten", "Spielwaren", "Bücher"],
        ["en"] = ["Electronics", "Office", "Home", "Sports", "Clothing", "Garden", "Toys", "Books"],
    };

    static readonly IReadOnlyDictionary<string, string> Country = new Dictionary<string, string> { ["de"] = "Deutschland", ["en"] = "USA" };
    public static readonly string[] Domains = ["example.com", "example.org", "example.net"];
    static readonly string[] OrderStatus = ["Pending", "Paid", "Shipped", "Delivered", "Cancelled", "Refunded"];
    static readonly string[] Roles = ["Admin", "User", "Editor", "Viewer", "Support"];

    static readonly string[] Lorem = ("lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore " +
        "magna aliqua enim ad minim veniam quis nostrud exercitation ullamco laboris nisi aliquip ex ea commodo " +
        "consequat duis aute irure in reprehenderit voluptate velit esse cillum fugiat nulla pariatur excepteur sint " +
        "occaecat cupidatat non proident sunt culpa qui officia deserunt mollit anim id est laborum").Split(' ');

    /// <summary>Test card numbers from the payment providers' documentation (Stripe and others); they never charge anyone.</summary>
    public static readonly (string Brand, string Number)[] TestCards =
    [
        ("Visa", "4242424242424242"), ("Visa", "4000056655665556"), ("Mastercard", "5555555555554444"),
        ("Mastercard", "2223003122003222"), ("American Express", "378282246310005"),
        ("Discover", "6011111111111117"), ("Diners Club", "3056930009020004"), ("JCB", "3566002020360505"),
    ];

    static readonly string[] UserAgents =
    [
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
        "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
    ];

    static readonly Dictionary<string, int> AreaCodes = new()
    {
        ["NY"] = 212, ["CA"] = 213, ["IL"] = 312, ["TX"] = 512, ["AZ"] = 602, ["PA"] = 215, ["WA"] = 206, ["CO"] = 303,
        ["MA"] = 617, ["OR"] = 503, ["GA"] = 404, ["FL"] = 305, ["MN"] = 612, ["TN"] = 615,
    };

    readonly PyRandom r;

    /// <summary>
    /// "de" or "en"; without a seed the values are random, with one they are exactly those of the Python module's
    /// random.Random(seed) (the module seeds each insertion with "&lt;seed&gt;:&lt;n&gt;").
    /// </summary>
    public Faker(string locale = "en", string? seed = null)
    {
        Locale = locale;
        r = seed == null ? new PyRandom() : new PyRandom(seed);
    }

    /// <summary>Seeded with an int, as Python's Random(42) (not the text "42").</summary>
    public Faker(string locale, BigInteger seed)
    {
        Locale = locale;
        r = new PyRandom(seed);
    }

    public string Locale { get; }

    /// <summary>The current time (UTC) for past and future dates; fixed in tests.</summary>
    public Func<DateTimeOffset> UtcNow { get; init; } = () => DateTimeOffset.UtcNow;

    /// <summary>The current year (local) for order numbers; fixed in tests.</summary>
    public Func<int> Year { get; init; } = () => DateTime.Today.Year;

    bool German => Locale == "de";

    T Pick<T>(IReadOnlyList<T> items) => r.Choice(items);

    int RandInt(int low, int high) => (int)r.RandInt(low, high);

    double Uniform(double low, double high) => r.Uniform(low, high);

    /// <summary>For emails and user names: ä → ae, ß → ss, spaces and the rest dropped.</summary>
    public static string AsciiName(string text)
    {
        foreach (var (a, b) in new[] { ("ä", "ae"), ("ö", "oe"), ("ü", "ue"), ("Ä", "Ae"), ("Ö", "Oe"), ("Ü", "Ue"), ("ß", "ss") })
        {
            text = text.Replace(a, b, StringComparison.Ordinal);
        }
        return NotAsciiAlphanumeric().Replace(text, "");
    }

    [GeneratedRegex("[^A-Za-z0-9]")]
    private static partial Regex NotAsciiAlphanumeric();

    // people
    public string Gender() => Pick(["f", "m"]);

    public string FirstName(string? gender = null) => Pick(First[Locale][gender ?? Gender()]);

    public string LastName() => Pick(Last[Locale]);

    public string FullName() => $"{FirstName()} {LastName()}";

    public string Email(string? first = null, string? last = null)
    {
        var f = AsciiName(Or(first, () => FirstName())).ToLowerInvariant();
        var l = AsciiName(Or(last, LastName)).ToLowerInvariant();
        var local = Pick([$"{f}.{l}", $"{f[0]}.{l}", $"{f}{l}", $"{f}.{l}{RandInt(1, 99)}", $"{f}_{l}"]);
        return $"{local}@{Pick(Domains)}";
    }

    public string UserName(string? first = null, string? last = null)
    {
        var f = AsciiName(Or(first, () => FirstName())).ToLowerInvariant();
        var l = AsciiName(Or(last, LastName)).ToLowerInvariant();
        return Pick([$"{f}{l[0]}", $"{f}.{l}", $"{f[0]}{l}", $"{f}{RandInt(1, 999)}"]);
    }

    /// <summary>Python's <c>value or make()</c>.</summary>
    static string Or(string? value, Func<string> make) => string.IsNullOrEmpty(value) ? make() : value;

    public string Phone()
    {
        if (German)
        {
            // The Bundesnetzagentur's numbers for films and TV: nobody is called.
            return $"+49 30 23125 {RandInt(0, 999):000}";
        }
        var city = Pick(Cities["en"]);
        var area = AreaCodes.GetValueOrDefault(city.State ?? "", 555);
        return $"({area}) 555-01{RandInt(0, 99):00}"; // 555-0100..0199 are reserved for fiction
    }

    public DateOnly BirthDate() => new DateOnly(2006, 1, 1).AddDays(-RandInt(0, 365 * 60));

    public string Job() => Pick(Jobs[Locale]);

    // places
    public string Street() => German
        ? $"{Pick(Streets["de"])} {RandInt(1, 120)}{Pick(["", "", "", "a", "b"])}"
        : $"{RandInt(100, 9999)} {Pick(Streets["en"])}";

    public City CityOf() => Pick(Cities[Locale]);

    public string Address()
    {
        var c = CityOf();
        return German ? $"{Street()}, {c.Zip} {c.Name}" : $"{Street()}, {c.Name}, {c.State} {c.Zip}";
    }

    // business
    public string Company(string? last = null)
    {
        var word = Pick(CompanyWords[Locale]);
        var style = RandInt(0, 2);
        var name = style switch
        {
            0 => $"{Or(last, LastName)} {word}",
            1 => $"{Or(last, LastName)} & {LastName()}",
            _ => $"{(German ? Pick(["Nord", "Süd", "Alpha", "Nova", "Blau", "Stern"]) : Pick(["North", "Blue", "Alpha", "Nova", "Summit", "Bright"]))} {word}",
        };
        return $"{name} {Pick(CompanySuffix[Locale])}";
    }

    /// <summary>A German IBAN with valid check digits, on the bank code of the well-known example (Commerzbank Köln).</summary>
    public string Iban()
    {
        var bban = "37040044" + r.RandInt(0, 9_999_999_999L).ToString("0000000000", CultureInfo.InvariantCulture);
        var check = 98 - (int)(BigInteger.Parse(bban + "131400", CultureInfo.InvariantCulture) % 97); // D=13 E=14, then 00
        return $"DE{check:00}{bban}";
    }

    public string VatId()
    {
        var id = new StringBuilder(German ? "DE" : "US");
        for (var i = 0; i < 9; i++)
        {
            id.Append(RandInt(0, 9));
        }
        return id.ToString();
    }

    public (string Brand, string Number) Card() => Pick(TestCards);

    public string Product() => $"{Pick(ProductAdjectives[Locale])} {Pick(ProductNouns[Locale])}";

    public double Price(double low = 1, double high = 500) => Py.Round(Uniform(low, high), 2);

    public string Sku()
    {
        var letters = new char[3];
        for (var i = 0; i < 3; i++)
        {
            letters[i] = Pick(Uppercase);
        }
        return $"{new string(letters)}-{RandInt(1000, 9999)}";
    }

    public string OrderNumber() => $"ORD-{Year()}-{RandInt(1, 99999):00000}";

    // internet
    public string Url() => $"https://www.{Pick(Domains)}/{Pick(["shop", "blog", "docs", "api", "about"])}/{RandInt(1, 999)}";

    /// <summary>Documentation ranges (RFC 5737): never a real host.</summary>
    public string Ipv4() => $"{Pick(["192.0.2", "198.51.100", "203.0.113"])}.{RandInt(1, 254)}";

    /// <summary>The documentation prefix (RFC 3849).</summary>
    public string Ipv6() => "2001:db8:" + string.Join(":", Enumerable.Range(0, 6).Select(_ => RandInt(0, 0xffff).ToString("x", CultureInfo.InvariantCulture)));

    /// <summary>Locally administered.</summary>
    public string Mac() => "02:" + string.Join(":", Enumerable.Range(0, 5).Select(_ => RandInt(0, 255).ToString("x2", CultureInfo.InvariantCulture)));

    static readonly char[] Uppercase = [.. "ABCDEFGHIJKLMNOPQRSTUVWXYZ"];

    const string PasswordAlphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*-_=+";

    public string Password(int length = 16)
    {
        while (true)
        {
            var p = new string(Enumerable.Range(0, length).Select(_ => PasswordAlphabet[(int)r.RandBelow(PasswordAlphabet.Length)]).ToArray());
            if (p.Any(char.IsAsciiLetterLower) && p.Any(char.IsAsciiLetterUpper) && p.Any(char.IsAsciiDigit) && p.Any(c => !char.IsAsciiLetterOrDigit(c)))
            {
                return p;
            }
        }
    }

    public string Color() => "#" + string.Concat(Enumerable.Range(0, 3).Select(_ => RandInt(0, 255).ToString("x2", CultureInfo.InvariantCulture)));

    /// <summary>A random version 4 GUID, lower case: str(uuid.UUID(int=getrandbits(128), version=4)).</summary>
    public string Guid()
    {
        var n = (UInt128)r.GetRandBits(128);
        n &= ~((UInt128)0xc000 << 48);
        n |= (UInt128)0x8000 << 48;
        n &= ~((UInt128)0xf000 << 64);
        n |= (UInt128)4 << 76;
        var hex = n.ToString("x32", CultureInfo.InvariantCulture);
        return $"{hex[..8]}-{hex[8..12]}-{hex[12..16]}-{hex[16..20]}-{hex[20..]}";
    }

    // time
    DateTimeOffset NowToTheSecond()
    {
        var now = UtcNow().ToUniversalTime();
        return now.AddTicks(-(now.Ticks % TimeSpan.TicksPerSecond));
    }

    public DateTimeOffset Past(int days = 365) => NowToTheSecond().AddSeconds(-RandInt(0, days * 86400));

    public DateTimeOffset Future(int days = 365) => NowToTheSecond().AddSeconds(RandInt(3600, days * 86400));

    // text
    public string Words(int n) => string.Join(" ", Enumerable.Range(0, n).Select(_ => Pick(Lorem)));

    public string Sentence()
    {
        var text = Words(RandInt(6, 14));
        return char.ToUpperInvariant(text[0]) + text[1..] + ".";
    }

    public string Paragraph() => string.Join(" ", Enumerable.Range(0, RandInt(3, 6)).Select(_ => Sentence()));

    public (double Latitude, double Longitude) LatitudeLongitude() => German
        ? (Py.Round(Uniform(47.3, 55.0), 6), Py.Round(Uniform(5.9, 15.0), 6))
        : (Py.Round(Uniform(25.0, 48.0), 6), Py.Round(Uniform(-124.0, -71.0), 6));

    // records
    public Record MakeRecord(string kind, int index)
    {
        switch (kind)
        {
            case "person":
            {
                var g = Gender();
                string first = FirstName(g), last = LastName();
                return new() { ["id"] = index, ["firstName"] = first, ["lastName"] = last, ["email"] = Email(first, last), ["phone"] = Phone(), ["birthDate"] = BirthDate() };
            }
            case "customer":
            {
                string first = FirstName(), last = LastName();
                var c = CityOf();
                return new()
                {
                    ["id"] = index, ["name"] = $"{first} {last}", ["email"] = Email(first, last), ["phone"] = Phone(),
                    ["company"] = Company(), ["street"] = Street(), ["zip"] = c.Zip, ["city"] = c.Name,
                    ["country"] = Country[Locale], ["createdAt"] = Past(730),
                };
            }
            case "user":
            {
                string first = FirstName(), last = LastName();
                return new()
                {
                    ["id"] = Guid(), ["userName"] = UserName(first, last), ["email"] = Email(first, last),
                    ["displayName"] = $"{first} {last}", ["role"] = Pick(Roles), ["isActive"] = r.Random() > 0.15,
                    ["lastLogin"] = Past(60),
                };
            }
            case "product":
                return new()
                {
                    ["id"] = index, ["sku"] = Sku(), ["name"] = Product(), ["category"] = Pick(Categories[Locale]),
                    ["price"] = Price(), ["stock"] = RandInt(0, 250), ["isAvailable"] = r.Random() > 0.1,
                };
            case "order":
            {
                string first = FirstName(), last = LastName();
                return new()
                {
                    ["id"] = index, ["orderNumber"] = OrderNumber(), ["customer"] = $"{first} {last}",
                    ["email"] = Email(first, last), ["total"] = Price(5, 2000),
                    ["currency"] = German ? "EUR" : "USD", ["status"] = Pick(OrderStatus),
                    ["createdAt"] = Past(180),
                };
            }
            case "company":
            {
                var c = CityOf();
                var name = Company();
                var domain = AsciiName(name.Split(' ', StringSplitOptions.RemoveEmptyEntries)[0]).ToLowerInvariant() + "." + Pick(Domains);
                return new()
                {
                    ["id"] = index, ["name"] = name, ["vatId"] = VatId(), ["email"] = $"info@{domain}", ["phone"] = Phone(),
                    ["website"] = $"https://{domain}", ["city"] = c.Name, ["iban"] = Iban(),
                };
            }
            default:
                throw new ArgumentException(kind, nameof(kind));
        }
    }

    // single values

    /// <summary>One value to insert: its key, label and detail in the pick list.</summary>
    public sealed record ValueKind(string Key, string Label, string Detail);

    public static readonly ValueKind[] Values =
    [
        new("firstName", "First name", "Anna, Liam"),
        new("lastName", "Last name", "Müller, Smith"),
        new("fullName", "Full name", "Anna Müller"),
        new("email", "Email", "on example.com / .org / .net, never a real mailbox"),
        new("userName", "User name", "anna.mueller"),
        new("phone", "Phone number", "fictional ranges: 030 23125 xxx, 555-01xx"),
        new("birthDate", "Date of birth", "ISO date, 20 to 80 years ago"),
        new("job", "Job title", ""),
        new("street", "Street and number", ""),
        new("city", "City", ""),
        new("zip", "Postal code", ""),
        new("address", "Full address", "street, postal code and city"),
        new("company", "Company", "Müller Software GmbH"),
        new("iban", "IBAN", "valid check digits, documentation bank code"),
        new("card", "Credit card (test number)", "payment providers' test numbers"),
        new("vatId", "VAT ID", ""),
        new("product", "Product name", ""),
        new("sku", "SKU", "ABC-1234"),
        new("price", "Price", "decimal with a dot"),
        new("orderNumber", "Order number", "ORD-2026-00042"),
        new("guid", "GUID", ""),
        new("pastDate", "Date and time (past)", "ISO 8601 UTC, within a year"),
        new("futureDate", "Date and time (future)", "ISO 8601 UTC, within a year"),
        new("url", "URL", "on example.com"),
        new("ipv4", "IPv4 address", "documentation ranges"),
        new("ipv6", "IPv6 address", "2001:db8::/32"),
        new("mac", "MAC address", ""),
        new("password", "Password", "16 characters, all four kinds"),
        new("color", "Hex color", ""),
        new("latLon", "Latitude, longitude", "in Germany / the US"),
        new("userAgent", "User agent", ""),
        new("sentence", "Sentence", "lorem ipsum"),
        new("paragraph", "Paragraph", "lorem ipsum"),
    ];

    /// <summary>A value of one of <see cref="Values"/>' keys, or null for an unknown key.</summary>
    public string? Value(string key) => key switch
    {
        "firstName" => FirstName(),
        "lastName" => LastName(),
        "fullName" => FullName(),
        "email" => Email(),
        "userName" => UserName(),
        "phone" => Phone(),
        "birthDate" => Render.IsoDate(BirthDate()),
        "job" => Job(),
        "street" => Street(),
        "city" => CityOf().Name,
        "zip" => CityOf().Zip,
        "address" => Address(),
        "company" => Company(),
        "iban" => Iban(),
        "card" => Card().Number,
        "vatId" => VatId(),
        "product" => Product(),
        "sku" => Sku(),
        "price" => Price().ToString("F2", CultureInfo.InvariantCulture),
        "orderNumber" => OrderNumber(),
        "guid" => Guid(),
        "pastDate" => Render.IsoMoment(Past()),
        "futureDate" => Render.IsoMoment(Future()),
        "url" => Url(),
        "ipv4" => Ipv4(),
        "ipv6" => Ipv6(),
        "mac" => Mac(),
        "password" => Password(),
        "color" => Color(),
        "latLon" => LatLon(),
        "userAgent" => Pick(UserAgents),
        "sentence" => Sentence(),
        "paragraph" => Paragraph(),
        _ => null,
    };

    string LatLon()
    {
        var (lat, lon) = LatitudeLongitude();
        return $"{Py.Repr(lat)}, {Py.Repr(lon)}";
    }
}
