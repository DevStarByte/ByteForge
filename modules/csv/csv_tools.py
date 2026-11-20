"""CSV module for ByteForge: rainbow columns (declared in module.json), a CSV view with the active file's
columns and what's in them, "Show as Table" and "Convert to JSON".

The separator is found per file (comma, semicolon as Excel writes it with a German or French locale, tab or
pipe), and so are decimal commas, dates and quoted fields. Files are read as saved.
"""

import csv
import datetime
import io
import json
import os
import re
import sys
import threading

VIEW = "csv"
EXTENSIONS = (".csv", ".tsv", ".tab", ".psv")
MAX_BYTES = 50_000_000
TABLE_ROWS = 2000
CELL_WIDTH = 40
lock = threading.Lock()


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def log(text):
    send({"type": "log", "text": text})


# --- reading ------------------------------------------------------------------------------------------------

def read_text(path):
    with open(path, "rb") as f:
        data = f.read(MAX_BYTES + 1)
    truncated = len(data) > MAX_BYTES
    data = data[:MAX_BYTES]
    for encoding in ("utf-8-sig", "cp1252"):  # Excel on Windows still writes ANSI
        try:
            return data.decode(encoding), truncated
        except UnicodeDecodeError:
            continue
    return data.decode("latin-1"), truncated


def sniff(text, path=""):
    """The separator: tab for .tsv/.tab, pipe for .psv, else whichever of , ; \\t | splits the first lines into
    the same number of fields most consistently (quotes respected)."""
    ext = os.path.splitext(path)[1].lower()
    if ext in (".tsv", ".tab"):
        return "\t"
    if ext == ".psv":
        return "|"
    sample = "\n".join(text.splitlines()[:50])
    best, best_score = ",", -1
    for sep in (",", ";", "\t", "|"):
        try:
            rows = [r for r in csv.reader(io.StringIO(sample), delimiter=sep) if r]
        except csv.Error:
            continue
        if not rows:
            continue
        counts = [len(r) for r in rows]
        width = max(set(counts), key=counts.count)
        if width < 2:
            continue
        score = counts.count(width) * 10 + width  # consistency first, then more columns
        if score > best_score:
            best, best_score = sep, score
    return best


def parse(text, sep):
    return [row for row in csv.reader(io.StringIO(text), delimiter=sep) if any(c.strip() for c in row)]


def has_header(rows):
    """A first row of distinct non-numeric names over data with numbers or dates in it."""
    if len(rows) < 2:
        return bool(rows)
    first = rows[0]
    if len(set(first)) != len(first) or any(not c.strip() for c in first):
        return False
    if any(kind(c) in ("integer", "decimal", "date") for c in first):
        return False
    return True


# --- column types -------------------------------------------------------------------------------------------

INTEGER = re.compile(r"^[+-]?\d{1,3}(?:([.,' ])\d{3})*$|^[+-]?\d+$")
DECIMAL_DOT = re.compile(r"^[+-]?(?:\d{1,3}(?:,\d{3})+|\d+)?\.\d+(?:[eE][+-]?\d+)?$|^[+-]?\d+(?:\.\d+)?[eE][+-]?\d+$")
DECIMAL_COMMA = re.compile(r"^[+-]?(?:\d{1,3}(?:\.\d{3})+|\d+)?,\d+$")
DATES = ("%Y-%m-%d", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S.%f", "%d.%m.%Y", "%d.%m.%Y %H:%M",
         "%d.%m.%Y %H:%M:%S", "%m/%d/%Y", "%d/%m/%Y", "%Y/%m/%d")
BOOLS = {"true", "false", "yes", "no", "ja", "nein", "y", "n"}


def to_date(value):
    v = value.strip().rstrip("Z")
    v = re.sub(r"[+-]\d\d:?\d\d$", "", v) if "T" in v else v
    for fmt in DATES:
        try:
            return datetime.datetime.strptime(v, fmt)
        except ValueError:
            continue
    return None


def kind(value, decimal_comma=False):
    """empty, integer, decimal, boolean, date or text. With decimal_comma (semicolon files, as Excel writes them
    in Germany or France), 1,5 and 1.234,5 are decimals and 1.234 is an integer."""
    v = value.strip()
    if not v:
        return "empty"
    if decimal_comma and DECIMAL_COMMA.match(v):
        return "decimal"
    if decimal_comma and re.match(r"^[+-]?\d{1,3}(?:\.\d{3})+$", v):
        return "integer"
    if INTEGER.match(v) and not (len(v) > 1 and v[0] == "0" and v.isdigit()):  # 007 is a code, not a number
        return "integer"
    if DECIMAL_DOT.match(v) or DECIMAL_COMMA.match(v):
        return "decimal"
    if v.lower() in BOOLS:
        return "boolean"
    if to_date(v):
        return "date"
    return "text"


def to_number(value, decimal_comma=False):
    v = value.strip().replace("'", "").replace(" ", "")
    if decimal_comma and (DECIMAL_COMMA.match(v) or re.match(r"^[+-]?\d{1,3}(?:\.\d{3})+$", v)):
        v = v.replace(".", "").replace(",", ".")
    elif DECIMAL_COMMA.match(v) and not re.match(r"^[+-]?\d{1,3},\d{3}$", v):
        v = v.replace(".", "").replace(",", ".")
    elif "," in v and "." in v or re.match(r"^[+-]?\d{1,3}(,\d{3})+$", v):
        v = v.replace(",", "")
    elif re.match(r"^[+-]?\d{1,3}(\.\d{3})+$", v):  # 1.234.567 (German thousands)
        v = v.replace(".", "")
    try:
        return float(v)
    except ValueError:
        return None


def column_stats(values, decimal_comma=False):
    kinds = [kind(v, decimal_comma) for v in values]
    filled = [k for k in kinds if k != "empty"]
    counts = {k: filled.count(k) for k in set(filled)}
    if not filled:
        main = "empty"
    elif set(filled) <= {"integer", "decimal"}:
        main = "decimal" if "decimal" in counts else "integer"
    elif len(counts) == 1:
        main = filled[0]
    else:
        main = "text" if "text" in counts else "mixed"
    stats = {"type": main, "empty": len(kinds) - len(filled), "distinct": len({v.strip() for v in values if v.strip()}),
             "count": len(values)}
    present = [v for v, k in zip(values, kinds) if k != "empty"]
    if main in ("integer", "decimal"):
        numbers = [n for n in (to_number(v, decimal_comma) for v in present) if n is not None]
        if numbers:
            stats.update(min=min(numbers), max=max(numbers), sum=sum(numbers))
    elif main == "date":
        dates = [d for d in (to_date(v) for v in present) if d]
        if dates:
            stats.update(min=min(dates).isoformat(sep=" "), max=max(dates).isoformat(sep=" "))
    else:
        lengths = [len(v) for v in present]
        if lengths:
            stats["longest"] = max(lengths)
    return stats


def number_text(n):
    return str(int(n)) if float(n).is_integer() and abs(n) < 1e15 else f"{n:.6g}"


def describe(stats):
    parts = [stats["type"]]
    if "min" in stats:
        lo, hi = stats["min"], stats["max"]
        parts.append(f"{number_text(lo)} … {number_text(hi)}" if isinstance(lo, float) else f"{lo[:10]} … {hi[:10]}")
    if stats["empty"]:
        parts.append(f"{stats['empty']} empty")
    if stats["distinct"] == stats["count"] - stats["empty"] and stats["count"] > 1:
        parts.append("unique")
    else:
        parts.append(f"{stats['distinct']} distinct")
    return " · ".join(parts)


# --- output -------------------------------------------------------------------------------------------------

def table(rows, header):
    """An aligned text table of the first TABLE_ROWS rows, cells cut at CELL_WIDTH."""
    shown = rows[:TABLE_ROWS + (1 if header else 0)]
    width = max(len(r) for r in shown)
    cut = lambda c: (c if len(c) <= CELL_WIDTH else c[:CELL_WIDTH - 1] + "…").replace("\n", " ")
    cells = [[cut(r[i]) if i < len(r) else "" for i in range(width)] for r in shown]
    numeric = [all(kind(row[i]) in ("integer", "decimal", "empty") for row in cells[1 if header else 0:]) for i in range(width)]
    widths = [max(len(row[i]) for row in cells) for i in range(width)]
    lines = []
    for n, row in enumerate(cells):
        lines.append(" │ ".join(c.rjust(w) if numeric[i] and not (header and n == 0) else c.ljust(w)
                                for i, (c, w) in enumerate(zip(row, widths))).rstrip())
        if header and n == 0:
            lines.append("─┼─".join("─" * w for w in widths))
    if len(rows) > len(shown):
        lines.append(f"… {len(rows) - len(shown)} more rows")
    return "\n".join(lines) + "\n"


def to_json(rows, header, decimal_comma=False):
    """Objects keyed by the header (or column1…), numbers and booleans as JSON values."""
    names = rows[0] if header else [f"column{i + 1}" for i in range(max(len(r) for r in rows))]
    data = rows[1:] if header else rows
    types = [column_stats([r[i] if i < len(r) else "" for r in data], decimal_comma)["type"] for i in range(len(names))]
    result = []
    for row in data:
        item = {}
        for i, name in enumerate(names):
            v = row[i] if i < len(row) else ""
            if not v.strip():
                item[name] = None
            elif types[i] == "integer" and to_number(v, decimal_comma) is not None:
                item[name] = int(to_number(v, decimal_comma))
            elif types[i] == "decimal" and to_number(v, decimal_comma) is not None:
                item[name] = to_number(v, decimal_comma)
            elif types[i] == "boolean":
                item[name] = v.strip().lower() in ("true", "yes", "ja", "y")
            else:
                item[name] = v
        result.append(item)
    return json.dumps(result, indent=2, ensure_ascii=False) + "\n"


def cache_dir():
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    else:
        base = os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache")
    return os.path.join(base, "byteforge", "csv")


def write_output(path, suffix, text):
    folder = cache_dir()
    os.makedirs(folder, exist_ok=True)
    stamp = datetime.datetime.now().strftime("%H%M%S")
    out = os.path.join(folder, f"{os.path.splitext(os.path.basename(path))[0]}-{stamp}{suffix}")
    with open(out, "w", encoding="utf-8") as f:
        f.write(text)
    return out


SEPARATOR_NAMES = {",": "comma", ";": "semicolon", "\t": "tab", "|": "pipe"}


class CsvModule:
    def __init__(self):
        self.active = None
        self.loaded = None  # (path, mtime, rows, sep, header, truncated)

    def load(self, path):
        mtime = os.path.getmtime(path)
        if self.loaded and self.loaded[0] == path and self.loaded[1] == mtime:
            return self.loaded
        text, truncated = read_text(path)
        sep = sniff(text, path)
        rows = parse(text, sep)
        self.loaded = (path, mtime, rows, sep, has_header(rows), truncated)
        return self.loaded

    def tree(self):
        if not self.active:
            return {"type": "setTree", "view": VIEW, "items": [],
                    "message": "Open a .csv or .tsv file to see its columns."}
        try:
            _, _, rows, sep, header, truncated = self.load(self.active)
        except OSError as e:
            return {"type": "setTree", "view": VIEW, "items": [], "error": str(e)}
        if not rows:
            return {"type": "setTree", "view": VIEW, "items": [], "message": "The file is empty."}
        names = rows[0] if header else [f"Column {i + 1}" for i in range(max(len(r) for r in rows))]
        data = rows[1:] if header else rows
        widths = {len(r) for r in data}
        children = []
        for i, name in enumerate(names):
            stats = column_stats([r[i] if i < len(r) else "" for r in data], sep == ";")
            tooltip = [f"{name}: {stats['type']}", f"{stats['count'] - stats['empty']} values, {stats['distinct']} distinct"]
            if "sum" in stats:
                tooltip.append(f"sum {number_text(stats['sum'])}")
            if "longest" in stats:
                tooltip.append(f"longest {stats['longest']} characters")
            children.append({"id": f"col:{i}", "label": name or f"Column {i + 1}", "description": describe(stats),
                             "tooltip": "\n".join(tooltip), "icon": TYPE_ICONS.get(stats["type"], "fileText")})
        summary = [f"{len(data)} rows", f"{len(names)} columns", f"{SEPARATOR_NAMES[sep]} separated"]
        if not header:
            summary.append("no header row")
        if len(widths) > 1:
            summary.append(f"rows have {min(widths)}–{max(widths)} fields")
        if truncated:
            summary.append(f"first {MAX_BYTES // 1_000_000} MB")
        items = [{"id": "file", "label": os.path.basename(self.active), "header": True, "expanded": True,
                  "count": len(names), "children": children, "description": " · ".join(summary)}]
        message = " · ".join(summary)
        return {"type": "setTree", "view": VIEW, "items": items, "message": message,
                "actions": [{"id": "table", "icon": "openPreview", "tooltip": "Show as Table"},
                            {"id": "json", "icon": "fileJson", "tooltip": "Convert to JSON"},
                            {"id": "refresh", "icon": "refresh", "tooltip": "Refresh"}]}

    def refresh(self):
        send(self.tree())

    def current(self):
        if not self.active:
            log("csv: open a .csv or .tsv file first (it's read as saved).")
            return None
        try:
            return self.load(self.active)
        except OSError as e:
            log(f"csv: {e}")
            return None

    def show_table(self):
        loaded = self.current()
        if loaded and loaded[2]:
            path, _, rows, _, header, _ = loaded
            send({"type": "openFile", "path": write_output(path, ".table.txt", table(rows, header))})

    def convert(self):
        loaded = self.current()
        if loaded and loaded[2]:
            path, _, rows, _, header, _ = loaded
            out = write_output(path, ".json", to_json(rows, header, loaded[3] == ";"))
            log(f"csv: {len(rows) - (1 if header else 0)} rows → {out}")
            send({"type": "openFile", "path": out})

    def handle(self, message):
        kind_ = message.get("type")
        if kind_ == "event":
            name = message.get("name")
            if name == "editorChanged":
                path = message.get("path")
                if path and path.lower().endswith(EXTENSIONS):
                    self.active = path
                    self.refresh()
            elif name == "fileSaved" and message.get("path") == self.active:
                self.refresh()
            elif name == "viewShown":
                self.refresh()
        elif kind_ == "executeCommand":
            command = message.get("id")
            if command == "csv.table":
                self.show_table()
            elif command == "csv.json":
                self.convert()
            elif command == "csv.columns":
                send({"type": "showView", "id": VIEW})
        elif kind_ == "treeAction" and message.get("item") is None:
            action = message.get("action")
            if action == "table":
                self.show_table()
            elif action == "json":
                self.convert()
            elif action == "refresh":
                self.loaded = None
                self.refresh()


TYPE_ICONS = {"integer": "fileBinary", "decimal": "fileBinary", "date": "fileConfig", "boolean": "check",
              "text": "fileText", "mixed": "warning", "empty": "remove"}


def main():
    for command, title in (("csv.table", "Show as Table"), ("csv.json", "Convert to JSON"), ("csv.columns", "Show Columns")):
        send({"type": "registerCommand", "id": command, "title": title, "category": "CSV"})
    module = CsvModule()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        try:
            module.handle(message)
        except Exception as e:  # keep the module alive; say what went wrong
            log(f"csv: {type(e).__name__}: {e}")


if __name__ == "__main__":
    main()
