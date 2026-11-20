"""Fake Data module for ByteForge: realistic test data inserted at the cursor.

"Fake Data: Insert…" (Ctrl+Alt+F) inserts one value: a name, email, phone number, address, company, IBAN,
test credit card, date, price, sentence and so on. "Fake Data: Insert Records…" inserts 5 to 100 records
(people, customers, users, products, orders, companies) as JSON, C# objects, CSV, SQL INSERT or YAML, for seed
data, tests and .http request bodies.

Nothing generated reaches a real person or account: emails use the domains reserved for examples (RFC 2606:
example.com, .org, .net), German phone numbers the Bundesnetzagentur's block for films and TV (030 23125 xxx),
US ones 555-0100 to 555-0199, cards are the public test numbers payment providers document, IBANs use the bank
code of the well-known documentation example (with valid check digits).

"fakeData.locale" picks German or English data (auto: from the system language); "fakeData.seed" makes every
insertion repeatable (0: random).
"""

import datetime
import json
import locale as system_locale
import os
import random
import re
import string
import sys
import uuid

settings = {}

FIRST = {
    "de": {
        "f": ["Anna", "Lena", "Mia", "Emma", "Hannah", "Sophie", "Lea", "Laura", "Julia", "Sarah", "Lisa", "Marie",
              "Johanna", "Katharina", "Clara", "Paula", "Charlotte", "Greta", "Ida", "Nele", "Frieda", "Luise",
              "Sabine", "Petra", "Claudia", "Monika", "Ingrid", "Ursula", "Birgit", "Anja"],
        "m": ["Lukas", "Leon", "Finn", "Jonas", "Paul", "Felix", "Elias", "Noah", "Maximilian", "Ben", "Tim", "Jan",
              "Moritz", "Niklas", "Tobias", "Florian", "Sebastian", "Matthias", "Stefan", "Thomas", "Michael",
              "Andreas", "Jürgen", "Klaus", "Wolfgang", "Uwe", "Dieter", "Frank", "Markus", "Henrik"],
    },
    "en": {
        "f": ["Olivia", "Emma", "Ava", "Sophia", "Isabella", "Mia", "Amelia", "Harper", "Evelyn", "Abigail", "Emily",
              "Grace", "Chloe", "Zoe", "Lily", "Hannah", "Nora", "Ella", "Madison", "Aria", "Scarlett", "Victoria",
              "Hazel", "Ruby", "Alice", "Megan", "Rachel", "Laura", "Jessica", "Sarah"],
        "m": ["Liam", "Noah", "Oliver", "James", "Elijah", "William", "Henry", "Lucas", "Benjamin", "Theodore",
              "Jack", "Samuel", "Daniel", "Matthew", "Joseph", "David", "Michael", "Andrew", "Ryan", "Nathan",
              "Ethan", "Logan", "Owen", "Caleb", "Isaac", "Aaron", "Connor", "Dylan", "Robert", "Thomas"],
    },
}
LAST = {
    "de": ["Müller", "Schmidt", "Schneider", "Fischer", "Weber", "Meyer", "Wagner", "Becker", "Schulz", "Hoffmann",
           "Schäfer", "Koch", "Bauer", "Richter", "Klein", "Wolf", "Schröder", "Neumann", "Schwarz", "Zimmermann",
           "Braun", "Krüger", "Hofmann", "Hartmann", "Lange", "Schmitt", "Werner", "Schmitz", "Krause", "Meier",
           "Lehmann", "Köhler", "Herrmann", "König", "Walter", "Mayer", "Huber", "Kaiser", "Fuchs", "Peters"],
    "en": ["Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis", "Rodriguez", "Martinez",
           "Wilson", "Anderson", "Taylor", "Thomas", "Moore", "Jackson", "Martin", "Lee", "Thompson", "White",
           "Harris", "Clark", "Lewis", "Robinson", "Walker", "Young", "Allen", "King", "Wright", "Scott", "Green",
           "Baker", "Adams", "Nelson", "Hill", "Campbell", "Mitchell", "Roberts", "Carter", "Phillips"],
}
STREETS = {
    "de": ["Hauptstraße", "Bahnhofstraße", "Gartenstraße", "Schulstraße", "Dorfstraße", "Bergstraße", "Lindenstraße",
           "Kirchstraße", "Waldstraße", "Ringstraße", "Mühlenweg", "Am Markt", "Rosenweg", "Birkenweg", "Goethestraße",
           "Schillerstraße", "Friedrichstraße", "Wiesenweg", "Feldstraße", "Parkstraße", "Uhlandstraße", "Mozartstraße"],
    "en": ["Main Street", "Oak Avenue", "Maple Drive", "Cedar Lane", "Pine Street", "Elm Street", "Washington Avenue",
           "Lake Road", "Hill Street", "Park Avenue", "Sunset Boulevard", "River Road", "Church Street", "Highland Drive",
           "Spring Street", "Lincoln Avenue", "Walnut Street", "Chestnut Street", "Willow Way", "Meadow Lane"],
}
# Real cities with a postal code that belongs to them.
CITIES = {
    "de": [("Berlin", "10115"), ("Hamburg", "20095"), ("München", "80331"), ("Köln", "50667"),
           ("Frankfurt am Main", "60311"), ("Stuttgart", "70173"), ("Düsseldorf", "40213"), ("Leipzig", "04109"),
           ("Dortmund", "44135"), ("Essen", "45127"), ("Bremen", "28195"), ("Dresden", "01067"), ("Hannover", "30159"),
           ("Nürnberg", "90403"), ("Bonn", "53111"), ("Münster", "48143"), ("Freiburg im Breisgau", "79098"),
           ("Augsburg", "86150"), ("Heidelberg", "69117"), ("Regensburg", "93047"), ("Kiel", "24103"), ("Erfurt", "99084")],
    "en": [("New York", "NY", "10001"), ("Los Angeles", "CA", "90012"), ("Chicago", "IL", "60601"),
           ("Houston", "TX", "77002"), ("Phoenix", "AZ", "85003"), ("Philadelphia", "PA", "19103"),
           ("San Antonio", "TX", "78205"), ("San Diego", "CA", "92101"), ("Dallas", "TX", "75201"),
           ("Austin", "TX", "78701"), ("Seattle", "WA", "98101"), ("Denver", "CO", "80202"), ("Boston", "MA", "02108"),
           ("Portland", "OR", "97204"), ("Atlanta", "GA", "30303"), ("Miami", "FL", "33130"),
           ("Minneapolis", "MN", "55401"), ("Nashville", "TN", "37203"), ("San Francisco", "CA", "94103")],
}
COMPANY_SUFFIX = {"de": ["GmbH", "AG", "GmbH & Co. KG", "KG", "UG (haftungsbeschränkt)", "e.K."],
                  "en": ["Inc.", "LLC", "Corp.", "Ltd.", "Group", "& Co."]}
COMPANY_WORDS = {"de": ["Software", "Logistik", "Bau", "Technik", "Handel", "Consulting", "Medien", "Systeme",
                        "Energie", "Digital", "Werkzeuge", "Elektro"],
                 "en": ["Software", "Logistics", "Systems", "Solutions", "Labs", "Consulting", "Media", "Industries",
                        "Dynamics", "Digital", "Analytics", "Networks"]}
JOBS = {"de": ["Softwareentwickler", "Projektleiterin", "Buchhalter", "Vertriebsleiterin", "Systemadministrator",
               "Produktmanagerin", "Lagerist", "Personalreferentin", "Data Scientist", "Kundenberater",
               "Marketingmanagerin", "Teamleiter IT"],
        "en": ["Software Engineer", "Project Manager", "Accountant", "Sales Director", "System Administrator",
               "Product Manager", "Warehouse Associate", "HR Specialist", "Data Scientist", "Customer Success Manager",
               "Marketing Manager", "IT Team Lead"]}
PRODUCT_ADJ = {"de": ["Ergonomische", "Kabellose", "Robuste", "Kompakte", "Elegante", "Smarte", "Leichte", "Praktische"],
               "en": ["Ergonomic", "Wireless", "Rugged", "Compact", "Sleek", "Smart", "Lightweight", "Handy"]}
PRODUCT_NOUN = {"de": ["Tastatur", "Maus", "Lampe", "Tasse", "Kamera", "Uhr", "Tasche", "Flasche", "Brille", "Jacke"],
                "en": ["Keyboard", "Mouse", "Lamp", "Mug", "Camera", "Watch", "Backpack", "Bottle", "Headphones", "Jacket"]}
CATEGORIES = {"de": ["Elektronik", "Büro", "Haushalt", "Sport", "Bekleidung", "Garten", "Spielwaren", "Bücher"],
              "en": ["Electronics", "Office", "Home", "Sports", "Clothing", "Garden", "Toys", "Books"]}
COUNTRY = {"de": "Deutschland", "en": "USA"}
DOMAINS = ["example.com", "example.org", "example.net"]
ORDER_STATUS = ["Pending", "Paid", "Shipped", "Delivered", "Cancelled", "Refunded"]
ROLES = ["Admin", "User", "Editor", "Viewer", "Support"]
LOREM = ("lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore "
         "magna aliqua enim ad minim veniam quis nostrud exercitation ullamco laboris nisi aliquip ex ea commodo "
         "consequat duis aute irure in reprehenderit voluptate velit esse cillum fugiat nulla pariatur excepteur sint "
         "occaecat cupidatat non proident sunt culpa qui officia deserunt mollit anim id est laborum").split()
# Test card numbers from the payment providers' documentation (Stripe and others); they never charge anyone.
TEST_CARDS = [("Visa", "4242424242424242"), ("Visa", "4000056655665556"), ("Mastercard", "5555555555554444"),
              ("Mastercard", "2223003122003222"), ("American Express", "378282246310005"),
              ("Discover", "6011111111111117"), ("Diners Club", "3056930009020004"), ("JCB", "3566002020360505")]
USER_AGENTS = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
    "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
]


def send(message):
    print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


def detect_locale():
    for name in ("LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"):
        value = os.environ.get(name)
        if value:
            return "de" if value.lower().startswith("de") else "en"
    try:
        value = system_locale.getlocale()[0] or ""
    except ValueError:
        value = ""
    return "de" if value.lower().startswith(("de", "german")) else "en"


def ascii_name(text):
    """For emails and user names: ä → ae, ß → ss, spaces and the rest dropped."""
    for a, b in (("ä", "ae"), ("ö", "oe"), ("ü", "ue"), ("Ä", "Ae"), ("Ö", "Oe"), ("Ü", "Ue"), ("ß", "ss")):
        text = text.replace(a, b)
    return re.sub(r"[^A-Za-z0-9]", "", text)


class Faker:
    def __init__(self, loc="en", seed=None):
        self.loc = loc
        self.r = random.Random(seed)

    def pick(self, items):
        return self.r.choice(items)

    # people
    def gender(self):
        return self.pick(["f", "m"])

    def first_name(self, gender=None):
        return self.pick(FIRST[self.loc][gender or self.gender()])

    def last_name(self):
        return self.pick(LAST[self.loc])

    def full_name(self):
        return f"{self.first_name()} {self.last_name()}"

    def email(self, first=None, last=None):
        first, last = ascii_name(first or self.first_name()).lower(), ascii_name(last or self.last_name()).lower()
        local = self.pick([f"{first}.{last}", f"{first[0]}.{last}", f"{first}{last}", f"{first}.{last}{self.r.randint(1, 99)}",
                           f"{first}_{last}"])
        return f"{local}@{self.pick(DOMAINS)}"

    def user_name(self, first=None, last=None):
        first, last = ascii_name(first or self.first_name()).lower(), ascii_name(last or self.last_name()).lower()
        return self.pick([f"{first}{last[0]}", f"{first}.{last}", f"{first[0]}{last}", f"{first}{self.r.randint(1, 999)}"])

    def phone(self):
        if self.loc == "de":  # the Bundesnetzagentur's numbers for films and TV: nobody is called
            return f"+49 30 23125 {self.r.randint(0, 999):03d}"
        city = self.pick(CITIES["en"])
        area = {"NY": 212, "CA": 213, "IL": 312, "TX": 512, "AZ": 602, "PA": 215, "WA": 206, "CO": 303, "MA": 617,
                "OR": 503, "GA": 404, "FL": 305, "MN": 612, "TN": 615}.get(city[1], 555)
        return f"({area}) 555-01{self.r.randint(0, 99):02d}"  # 555-0100..0199 are reserved for fiction

    def birth_date(self):
        return datetime.date(2006, 1, 1) - datetime.timedelta(days=self.r.randint(0, 365 * 60))

    def job(self):
        return self.pick(JOBS[self.loc])

    # places
    def street(self):
        if self.loc == "de":
            return f"{self.pick(STREETS['de'])} {self.r.randint(1, 120)}{self.pick(['', '', '', 'a', 'b'])}"
        return f"{self.r.randint(100, 9999)} {self.pick(STREETS['en'])}"

    def city(self):
        c = self.pick(CITIES[self.loc])
        return {"city": c[0], "zip": c[-1], "state": c[1] if self.loc == "en" else None}

    def address(self):
        c = self.city()
        if self.loc == "de":
            return f"{self.street()}, {c['zip']} {c['city']}"
        return f"{self.street()}, {c['city']}, {c['state']} {c['zip']}"

    # business
    def company(self, last=None):
        word = self.pick(COMPANY_WORDS[self.loc])
        style = self.r.randint(0, 2)
        name = (f"{last or self.last_name()} {word}" if style == 0 else
                f"{last or self.last_name()} & {self.last_name()}" if style == 1 else
                f"{self.pick(['Nord', 'Süd', 'Alpha', 'Nova', 'Blau', 'Stern']) if self.loc == 'de' else self.pick(['North', 'Blue', 'Alpha', 'Nova', 'Summit', 'Bright'])} {word}")
        return f"{name} {self.pick(COMPANY_SUFFIX[self.loc])}"

    def iban(self):
        """A German IBAN with valid check digits, on the bank code of the well-known example (Commerzbank Köln)."""
        bban = "37040044" + f"{self.r.randint(0, 10**10 - 1):010d}"
        check = 98 - int(bban + "131400") % 97  # D=13 E=14, then 00
        return f"DE{check:02d}{bban}"

    def vat_id(self):
        return ("DE" if self.loc == "de" else "US") + "".join(str(self.r.randint(0, 9)) for _ in range(9))

    def card(self):
        return self.pick(TEST_CARDS)

    def product(self):
        adjectives, nouns = PRODUCT_ADJ[self.loc], PRODUCT_NOUN[self.loc]
        return f"{self.pick(adjectives)} {self.pick(nouns)}"

    def price(self, low=1, high=500):
        return round(self.r.uniform(low, high), 2)

    def sku(self):
        return f"{''.join(self.r.choice(string.ascii_uppercase) for _ in range(3))}-{self.r.randint(1000, 9999)}"

    def order_number(self):
        return f"ORD-{datetime.date.today().year}-{self.r.randint(1, 99999):05d}"

    # internet
    def url(self):
        return f"https://www.{self.pick(DOMAINS)}/{self.pick(['shop', 'blog', 'docs', 'api', 'about'])}/{self.r.randint(1, 999)}"

    def ipv4(self):
        # Documentation ranges (RFC 5737): never a real host.
        return f"{self.pick(['192.0.2', '198.51.100', '203.0.113'])}.{self.r.randint(1, 254)}"

    def ipv6(self):
        return "2001:db8:" + ":".join(f"{self.r.randint(0, 0xffff):x}" for _ in range(6))  # RFC 3849

    def mac(self):
        return ":".join(f"{b:02x}" for b in [0x02] + [self.r.randint(0, 255) for _ in range(5)])  # locally administered

    def password(self, length=16):
        alphabet = string.ascii_letters + string.digits + "!@#$%^&*-_=+"
        while True:
            p = "".join(self.r.choice(alphabet) for _ in range(length))
            if any(c.islower() for c in p) and any(c.isupper() for c in p) and any(c.isdigit() for c in p) \
                    and any(not c.isalnum() for c in p):
                return p

    def color(self):
        return "#" + "".join(f"{self.r.randint(0, 255):02x}" for _ in range(3))

    def guid(self):
        return str(uuid.UUID(int=self.r.getrandbits(128), version=4))

    # time
    def past(self, days=365):
        moment = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(seconds=self.r.randint(0, days * 86400))
        return moment.replace(microsecond=0)

    def future(self, days=365):
        moment = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=self.r.randint(3600, days * 86400))
        return moment.replace(microsecond=0)

    # text
    def words(self, n):
        return " ".join(self.pick(LOREM) for _ in range(n))

    def sentence(self):
        text = self.words(self.r.randint(6, 14))
        return text[0].upper() + text[1:] + "."

    def paragraph(self):
        return " ".join(self.sentence() for _ in range(self.r.randint(3, 6)))

    def latitude_longitude(self):
        if self.loc == "de":
            return round(self.r.uniform(47.3, 55.0), 6), round(self.r.uniform(5.9, 15.0), 6)
        return round(self.r.uniform(25.0, 48.0), 6), round(self.r.uniform(-124.0, -71.0), 6)

    # records
    def record(self, kind, index):
        if kind == "person":
            g = self.gender()
            first, last = self.first_name(g), self.last_name()
            return {"id": index, "firstName": first, "lastName": last, "email": self.email(first, last),
                    "phone": self.phone(), "birthDate": self.birth_date()}
        if kind == "customer":
            first, last = self.first_name(), self.last_name()
            c = self.city()
            return {"id": index, "name": f"{first} {last}", "email": self.email(first, last), "phone": self.phone(),
                    "company": self.company(), "street": self.street(), "zip": c["zip"], "city": c["city"],
                    "country": COUNTRY[self.loc], "createdAt": self.past(730)}
        if kind == "user":
            first, last = self.first_name(), self.last_name()
            return {"id": self.guid(), "userName": self.user_name(first, last), "email": self.email(first, last),
                    "displayName": f"{first} {last}", "role": self.pick(ROLES), "isActive": self.r.random() > 0.15,
                    "lastLogin": self.past(60)}
        if kind == "product":
            return {"id": index, "sku": self.sku(), "name": self.product(), "category": self.pick(CATEGORIES[self.loc]),
                    "price": self.price(), "stock": self.r.randint(0, 250), "isAvailable": self.r.random() > 0.1}
        if kind == "order":
            first, last = self.first_name(), self.last_name()
            return {"id": index, "orderNumber": self.order_number(), "customer": f"{first} {last}",
                    "email": self.email(first, last), "total": self.price(5, 2000),
                    "currency": "EUR" if self.loc == "de" else "USD", "status": self.pick(ORDER_STATUS),
                    "createdAt": self.past(180)}
        if kind == "company":
            c = self.city()
            name = self.company()
            domain = ascii_name(name.split()[0]).lower() + "." + self.pick(DOMAINS)
            return {"id": index, "name": name, "vatId": self.vat_id(), "email": f"info@{domain}", "phone": self.phone(),
                    "website": f"https://{domain}", "city": c["city"], "iban": self.iban()}
        raise ValueError(kind)


# One value: (label, detail, function)
def single_values(f):
    def card():
        brand, number = f.card()
        return number
    lat_lon = lambda: "{}, {}".format(*f.latitude_longitude())
    return {
        "firstName": ("First name", "Anna, Liam", f.first_name),
        "lastName": ("Last name", "Müller, Smith", f.last_name),
        "fullName": ("Full name", "Anna Müller", f.full_name),
        "email": ("Email", "on example.com / .org / .net, never a real mailbox", f.email),
        "userName": ("User name", "anna.mueller", f.user_name),
        "phone": ("Phone number", "fictional ranges: 030 23125 xxx, 555-01xx", f.phone),
        "birthDate": ("Date of birth", "ISO date, 20 to 80 years ago", lambda: f.birth_date().isoformat()),
        "job": ("Job title", "", f.job),
        "street": ("Street and number", "", f.street),
        "city": ("City", "", lambda: f.city()["city"]),
        "zip": ("Postal code", "", lambda: f.city()["zip"]),
        "address": ("Full address", "street, postal code and city", f.address),
        "company": ("Company", "Müller Software GmbH", f.company),
        "iban": ("IBAN", "valid check digits, documentation bank code", f.iban),
        "card": ("Credit card (test number)", "payment providers' test numbers", card),
        "vatId": ("VAT ID", "", f.vat_id),
        "product": ("Product name", "", f.product),
        "sku": ("SKU", "ABC-1234", f.sku),
        "price": ("Price", "decimal with a dot", lambda: f"{f.price():.2f}"),
        "orderNumber": ("Order number", "ORD-2026-00042", f.order_number),
        "guid": ("GUID", "", f.guid),
        "pastDate": ("Date and time (past)", "ISO 8601 UTC, within a year", lambda: f.past().isoformat().replace("+00:00", "Z")),
        "futureDate": ("Date and time (future)", "ISO 8601 UTC, within a year", lambda: f.future().isoformat().replace("+00:00", "Z")),
        "url": ("URL", "on example.com", f.url),
        "ipv4": ("IPv4 address", "documentation ranges", f.ipv4),
        "ipv6": ("IPv6 address", "2001:db8::/32", f.ipv6),
        "mac": ("MAC address", "", f.mac),
        "password": ("Password", "16 characters, all four kinds", f.password),
        "color": ("Hex color", "", f.color),
        "latLon": ("Latitude, longitude", "in Germany / the US", lat_lon),
        "userAgent": ("User agent", "", lambda: f.pick(USER_AGENTS)),
        "sentence": ("Sentence", "lorem ipsum", f.sentence),
        "paragraph": ("Paragraph", "lorem ipsum", f.paragraph),
    }


RECORD_KINDS = {
    "person": ("People", "id, firstName, lastName, email, phone, birthDate"),
    "customer": ("Customers", "name, email, phone, company, street, zip, city, country, createdAt"),
    "user": ("User accounts", "id (GUID), userName, email, displayName, role, isActive, lastLogin"),
    "product": ("Products", "sku, name, category, price, stock, isAvailable"),
    "order": ("Orders", "orderNumber, customer, email, total, currency, status, createdAt"),
    "company": ("Companies", "name, vatId, email, phone, website, city, iban"),
}
FORMATS = {
    "json": ("JSON", "an array of objects (also for .http bodies)"),
    "csharp": ("C#", "new List<T> { new T { … } } with typed values"),
    "csv": ("CSV", "a header row, comma separated"),
    "sql": ("SQL INSERT", "one INSERT with a row per record, snake_case columns"),
    "yaml": ("YAML", "a list of mappings"),
}
CLASS_NAMES = {"person": "Person", "customer": "Customer", "user": "User", "product": "Product", "order": "Order",
               "company": "Company"}


# --- output formats -----------------------------------------------------------------------------------------

def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def pascal(name):
    return name[0].upper() + name[1:]


def json_value(v):
    if isinstance(v, datetime.datetime):
        return v.isoformat().replace("+00:00", "Z")
    if isinstance(v, datetime.date):
        return v.isoformat()
    return v


def to_json(records):
    return json.dumps([{k: json_value(v) for k, v in r.items()} for r in records], indent=2, ensure_ascii=False)


def cs_value(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return f"{v:.2f}m"
    if isinstance(v, datetime.datetime):
        return f"new DateTimeOffset({v.year}, {v.month}, {v.day}, {v.hour}, {v.minute}, {v.second}, TimeSpan.Zero)"
    if isinstance(v, datetime.date):
        return f"new DateOnly({v.year}, {v.month}, {v.day})"
    if isinstance(v, str) and re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", v):
        return f'Guid.Parse("{v}")'
    return json.dumps(v, ensure_ascii=False)  # a C# string literal for everything we generate


def to_csharp(records, kind):
    name = CLASS_NAMES[kind]
    plural = {"person": "people", "company": "companies"}.get(kind, kind + "s")
    lines = [f"var {plural} = new List<{name}>", "{"]
    for r in records:
        props = ", ".join(f"{pascal(k)} = {cs_value(v)}" for k, v in r.items())
        lines.append(f"    new {name} {{ {props} }},")
    lines.append("};")
    return "\n".join(lines)


def csv_cell(v):
    v = json_value(v)
    text = "true" if v is True else "false" if v is False else "" if v is None else str(v)
    return '"' + text.replace('"', '""') + '"' if re.search(r'[",\n]', text) else text


def to_csv(records):
    keys = list(records[0])
    return "\n".join([",".join(keys)] + [",".join(csv_cell(r[k]) for k in keys) for r in records])


def sql_value(v):
    v = json_value(v)
    if isinstance(v, bool):
        return "TRUE" if v else "FALSE"
    if isinstance(v, (int, float)):
        return str(v)
    if v is None:
        return "NULL"
    return "'" + str(v).replace("'", "''") + "'"


def to_sql(records, kind):
    table = {"person": "people", "company": "companies"}.get(kind, kind + "s")
    keys = list(records[0])
    rows = ",\n".join("    (" + ", ".join(sql_value(r[k]) for k in keys) + ")" for r in records)
    return f"INSERT INTO {table} ({', '.join(snake(k) for k in keys)})\nVALUES\n{rows};"


def yaml_value(v):
    v = json_value(v)
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return str(v)
    return json.dumps(v, ensure_ascii=False)  # a double-quoted YAML string


def to_yaml(records):
    lines = []
    for r in records:
        for i, (k, v) in enumerate(r.items()):
            lines.append(f"{'- ' if i == 0 else '  '}{k}: {yaml_value(v)}")
    return "\n".join(lines)


def render(records, kind, fmt):
    if fmt == "json":
        return to_json(records)
    if fmt == "csharp":
        return to_csharp(records, kind)
    if fmt == "csv":
        return to_csv(records)
    if fmt == "sql":
        return to_sql(records, kind)
    return to_yaml(records)


# --- module -------------------------------------------------------------------------------------------------

class FakeData:
    def __init__(self):
        self.pending = {}
        self.next_request = 1
        self.insertions = 0
        self.last = None  # the last single value kind, for "Insert Again"

    def faker(self):
        loc = settings.get("fakeData.locale") or "auto"
        loc = detect_locale() if loc == "auto" else loc if loc in ("de", "en") else "en"
        seed = settings.get("fakeData.seed") or 0
        self.insertions += 1
        # With a seed, the n-th insertion of a session is always the same.
        return Faker(loc, None if not seed else f"{seed}:{self.insertions}")

    def ask(self, kind, items, placeholder, data=None):
        request = self.next_request
        self.next_request += 1
        self.pending[request] = (kind, data)
        send({"type": "quickPick", "requestId": request, "placeholder": placeholder, "items": items})

    def pick_value(self):
        values = single_values(Faker("en"))
        self.ask("value", [{"label": label, "detail": detail, "value": key} for key, (label, detail, _) in values.items()],
                 "Insert fake…")

    def insert_value(self, key):
        values = single_values(self.faker())
        if key in values:
            self.last = key
            send({"type": "insertText", "text": str(values[key][2]())})

    def pick_records(self):
        self.ask("records", [{"label": label, "detail": detail, "value": key} for key, (label, detail) in RECORD_KINDS.items()],
                 "Insert records of…")

    def handle(self, message):
        global settings
        kind = message.get("type")
        if kind == "activate" or (kind == "event" and message.get("name") == "settingsChanged"):
            settings = message.get("settings") or {}
        elif kind == "executeCommand":
            command = message.get("id")
            if command == "fakeData.insert":
                self.pick_value()
            elif command == "fakeData.records":
                self.pick_records()
            elif command == "fakeData.again":
                if self.last:
                    self.insert_value(self.last)
                else:
                    self.pick_value()
        elif kind == "quickPickResult":
            what, data = self.pending.pop(message.get("requestId"), (None, None))
            value = message.get("value")
            if not value:
                return
            if what == "value":
                self.insert_value(value)
            elif what == "records":
                self.ask("format", [{"label": label, "detail": detail, "value": key} for key, (label, detail) in FORMATS.items()],
                         f"{RECORD_KINDS[value][0]} as…", value)
            elif what == "format":
                self.ask("count", [{"label": str(n), "value": str(n)} for n in (3, 5, 10, 25, 50, 100)],
                         "How many?", (data, value))
            elif what == "count":
                record_kind, fmt = data
                f = self.faker()
                records = [f.record(record_kind, i + 1) for i in range(int(value))]
                send({"type": "insertText", "text": render(records, record_kind, fmt)})


def main():
    for command, title, key in (("fakeData.insert", "Insert…", "ctrl-alt-f"), ("fakeData.records", "Insert Records…", None),
                                ("fakeData.again", "Insert Again (same kind, new value)", None)):
        registration = {"type": "registerCommand", "id": command, "title": title, "category": "Fake Data"}
        if key:
            registration["keybinding"] = key
        send(registration)
    module = FakeData()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        try:
            module.handle(message)
        except Exception as e:  # keep the module alive; say what went wrong
            log(f"fake-data: {type(e).__name__}: {e}")


if __name__ == "__main__":
    main()
