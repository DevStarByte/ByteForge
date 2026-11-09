"""SQL module for ByteForge.

A Database view with the workspace's connections and their tables, views and columns, "Run File" for .sql files,
an interactive shell per connection, and a formatter for .sql files (keyword case, trailing whitespace).

Connections come from three places:
- the `sql.connections` setting: `[{ "name", "driver", "connectionString" }]` or the fields one by one
  (`host`, `port`, `database`, `user`, `password`, `file` for SQLite, `integratedSecurity`, `trustServerCertificate`),
- the `ConnectionStrings` of the workspace's appsettings*.json files (the provider comes from the project's
  packages, else from the connection string's keys),
- SQLite database files in the workspace.

SQLite needs nothing but Python. PostgreSQL, MySQL / MariaDB and SQL Server go through their command line
clients (`psql`, `mysql` or `mariadb`, `sqlcmd`). Queries and shells run in an IDE terminal through this file
(`sql.py launch <spec>`), so passwords never show up on a command line: the module writes the connection to a
private temporary file that the launcher reads, deletes and turns into the client's environment.
"""

import hashlib
import json
import os
import queue
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

VIEW = "sql"
MAX_FILES = 100_000
SKIP_DIRS = {".git", "bin", "obj", "node_modules", ".vs", ".idea", "target", "packages"}
SQLITE_EXTENSIONS = (".db", ".sqlite", ".sqlite3", ".db3")
SQLITE_HEADER = b"SQLite format 3\x00"
WARNING_COLOR = "#d29922"
ERROR_COLOR = "#f85149"
QUERY_TIMEOUT = 30
SELECT_LIMIT = 100

DRIVERS = {"sqlite": "SQLite", "postgres": "PostgreSQL", "mysql": "MySQL", "sqlserver": "SQL Server"}
DRIVER_ALIASES = {
    "sqlite": "sqlite", "sqlite3": "sqlite",
    "postgres": "postgres", "postgresql": "postgres", "pg": "postgres", "npgsql": "postgres",
    "mysql": "mysql", "mariadb": "mysql",
    "sqlserver": "sqlserver", "mssql": "sqlserver", "sql server": "sqlserver", "sqlclient": "sqlserver",
}
DRIVER_COLORS = {"sqlite": "#44a8e0", "postgres": "#6b9bd1", "mysql": "#f29111", "sqlserver": "#e05252"}
DEFAULT_SCHEMAS = {"postgres": "public", "sqlserver": "dbo"}

# Material Design Icons (Apache 2.0): database, table, table-eye (view) and table-column.
DATABASE_ICON = ("M12,3C7.58,3 4,4.79 4,7C4,9.21 7.58,11 12,11C16.42,11 20,9.21 20,7C20,4.79 16.42,3 12,3M4,9V12C4,14.21 "
                 "7.58,16 12,16C16.42,16 20,14.21 20,12V9C20,11.21 16.42,13 12,13C7.58,13 4,11.21 4,9M4,14V17C4,19.21 "
                 "7.58,21 12,21C16.42,21 20,19.21 20,17V14C20,16.21 16.42,18 12,18C7.58,18 4,16.21 4,14Z")
TABLE_ICON = ("M5,4H19A2,2 0 0,1 21,6V18A2,2 0 0,1 19,20H5A2,2 0 0,1 3,18V6A2,2 0 0,1 5,4M5,8V12H11V8H5M13,8V12H19V8H13M5,"
              "14V18H11V14H5M13,14V18H19V14H13Z")
VIEW_ICON = ("M17,18C17.56,18 18,18.44 18,19C18,19.56 17.56,20 17,20C16.44,20 16,19.56 16,19C16,18.44 16.44,18 17,18M17,"
             "15C14.27,15 11.94,16.66 11,19C11.94,21.34 14.27,23 17,23C19.73,23 22.06,21.34 23,19C22.06,16.66 19.73,15 17,"
             "15M17,21.5A2.5,2.5 0 0,1 14.5,19A2.5,2.5 0 0,1 17,16.5A2.5,2.5 0 0,1 19.5,19A2.5,2.5 0 0,1 17,21.5M9.27,"
             "20H6V16H10V17.63C10.5,16.97 11.12,16.4 11.84,15.94L12,15.87V10H18V13.5C18.7,13.6 19.37,13.78 20,14.04V6A2,"
             "2 0 0,0 18,4H4A2,2 0 0,0 2,6V18A2,2 0 0,0 4,20H9.27C9.12,19.74 9,19.47 8.9,19.2L8.84,19L8.9,18.8C9.03,"
             "18.53 9.13,18.26 9.27,18M12,10H4V6H18V8H12V10M10,14H6V10H10V14Z")
COLUMN_ICON = ("M8,2H16A2,2 0 0,1 18,4V20A2,2 0 0,1 16,22H8A2,2 0 0,1 6,20V4A2,2 0 0,1 8,2M8,10V14H16V10H8M8,16V20H16V"
               "16H8M8,4V8H16V4H8Z")

PACKAGE_DRIVERS = [
    ("npgsql", "postgres"),
    ("pomelo.entityframeworkcore.mysql", "mysql"), ("mysqlconnector", "mysql"), ("mysql.data", "mysql"),
    ("mysql.entityframeworkcore", "mysql"),
    ("microsoft.entityframeworkcore.sqlserver", "sqlserver"), ("microsoft.data.sqlclient", "sqlserver"),
    ("system.data.sqlclient", "sqlserver"), ("aspire.microsoft.data.sqlclient", "sqlserver"),
    ("microsoft.entityframeworkcore.sqlite", "sqlite"), ("microsoft.data.sqlite", "sqlite"),
    ("system.data.sqlite", "sqlite"),
]
SQLSERVER_KEYS = {"initial catalog", "trusted_connection", "integrated security", "trustservercertificate",
                  "multipleactiveresultsets", "encrypt", "user id", "application name", "attachdbfilename"}
MYSQL_KEYS = {"uid", "pwd", "sslmode", "allowuservariables", "allowpublickeyretrieval", "treattinyasboolean",
              "convert zero datetime", "convertzerodatetime", "charset"}


class SqlError(Exception):
    pass


# ───────────── connection strings ─────────────

def parse_connection_string(text):
    """`Key=Value;…` (ADO.NET style) as a dict with lowercase keys; values may be quoted."""
    fields = {}
    i, n = 0, len(text)
    while i < n:
        while i < n and text[i] in "; \t\r\n":
            i += 1
        start = i
        while i < n and text[i] not in "=;":
            i += 1
        key = text[start:i].strip().lower()
        if i >= n or text[i] == ";":
            continue
        i += 1  # '='
        while i < n and text[i] in " \t":
            i += 1
        if i < n and text[i] in "\"'":
            quote = text[i]
            i += 1
            value = []
            while i < n:
                if text[i] == quote:
                    if i + 1 < n and text[i + 1] == quote:
                        value.append(quote)
                        i += 2
                        continue
                    i += 1
                    break
                value.append(text[i])
                i += 1
            value = "".join(value)
            while i < n and text[i] != ";":
                i += 1
        else:
            start = i
            while i < n and text[i] != ";":
                i += 1
            value = text[start:i].strip()
        if key:
            fields[key] = value
    return fields


def is_sqlite_path(value):
    value = value.strip().lower()
    return value == ":memory:" or value.endswith(SQLITE_EXTENSIONS)


def guess_driver(fields, hint=None):
    """The driver of a parsed connection string: a SQLite file wins, then the project's provider (`hint`),
    then the keys that only one provider uses."""
    keys = set(fields)
    source = fields.get("data source") or fields.get("datasource") or fields.get("filename") or ""
    if is_sqlite_path(source) or ("filename" in keys and "server" not in keys):
        return "sqlite"
    if hint:
        return hint
    server = (fields.get("server") or fields.get("data source") or "").lower()
    if keys & SQLSERVER_KEYS or "\\" in server or server.startswith(("tcp:", "np:", "(localdb)", "lpc:")):
        return "sqlserver"
    if keys & MYSQL_KEYS:
        return "mysql"
    if "host" in keys:
        return "postgres"
    if "server" in keys or "data source" in keys:
        return "sqlserver"
    return None


def truthy(value):
    return isinstance(value, str) and value.strip().lower() in ("true", "yes", "sspi", "1")


def connection_from_fields(driver, fields, base_dir=None):
    """A connection dict from a parsed connection string."""
    def get(*names):
        for name in names:
            value = fields.get(name)
            if value:
                return value
        return None

    conn = {"driver": driver}
    if driver == "sqlite":
        path = get("data source", "datasource", "filename") or ""
        path = path.replace("|DataDirectory|", "").lstrip("\\/") if "|DataDirectory|" in path else path
        if path and path != ":memory:" and not os.path.isabs(path) and base_dir:
            path = os.path.normpath(os.path.join(base_dir, path))
        conn["file"] = path
    elif driver == "postgres":
        host = get("host", "server")
        conn["host"] = host.split(",")[0] if host else None
        conn["port"] = get("port")
        conn["database"] = get("database", "db", "initial catalog")
        conn["user"] = get("username", "user id", "userid", "user name", "user", "uid")
        conn["password"] = get("password", "pwd")
    elif driver == "mysql":
        conn["host"] = get("server", "host", "data source", "address", "addr")
        conn["port"] = get("port")
        conn["database"] = get("database", "initial catalog")
        conn["user"] = get("uid", "user id", "userid", "user", "username", "user name")
        conn["password"] = get("pwd", "password")
    elif driver == "sqlserver":
        server = get("server", "data source", "address", "addr", "network address") or ""
        if server.lower().startswith("tcp:"):
            server = server[4:]
        conn["host"] = server
        conn["database"] = get("database", "initial catalog")
        conn["user"] = get("user id", "uid", "user", "username")
        conn["password"] = get("password", "pwd")
        conn["integrated"] = truthy(get("integrated security", "trusted_connection"))
        conn["trust"] = truthy(get("trustservercertificate", "trust server certificate"))
    return {k: v for k, v in conn.items() if v not in (None, "")}


def normalize_driver(value):
    return DRIVER_ALIASES.get(value.strip().lower()) if isinstance(value, str) else None


def connection_from_setting(entry, workspace):
    """A connection from one `sql.connections` entry, or None when it doesn't say enough."""
    if not isinstance(entry, dict):
        return None
    driver = normalize_driver(entry.get("driver"))
    text = entry.get("connectionString")
    if isinstance(text, str) and text.strip():
        fields = parse_connection_string(text)
        driver = driver or guess_driver(fields)
        if not driver:
            return None
        conn = connection_from_fields(driver, fields, workspace)
    else:
        if not driver:
            return None
        conn = {"driver": driver}
        for key in ("host", "port", "database", "user", "password", "file"):
            value = entry.get(key)
            if isinstance(value, (str, int)) and str(value):
                conn[key] = str(value)
        if driver == "sqlite" and conn.get("file"):
            path = os.path.expanduser(conn["file"])
            if not os.path.isabs(path) and workspace:
                path = os.path.normpath(os.path.join(workspace, path))
            conn["file"] = path
        if entry.get("integratedSecurity"):
            conn["integrated"] = True
        if entry.get("trustServerCertificate"):
            conn["trust"] = True
    name = entry.get("name")
    conn["name"] = name if isinstance(name, str) and name else default_name(conn)
    conn["origin"] = "settings"
    return conn


def default_name(conn):
    if conn["driver"] == "sqlite":
        return os.path.basename(conn.get("file") or "") or "SQLite"
    target = conn.get("database") or conn.get("host") or ""
    return f"{DRIVERS[conn['driver']]} {target}".strip()


def connection_id(conn):
    key = "\n".join(str(conn.get(k) or "") for k in ("driver", "name", "host", "port", "database", "user", "file"))
    return hashlib.sha1(key.encode()).hexdigest()[:12]


def connection_target(conn):
    """Where the connection points, for the view: `user@host:port/database` or the file."""
    if conn["driver"] == "sqlite":
        return conn.get("file") or ""
    host = conn.get("host") or "localhost"
    if conn.get("port"):
        host += f":{conn['port']}"
    if conn.get("user"):
        host = f"{conn['user']}@{host}"
    return f"{host}/{conn['database']}" if conn.get("database") else host


# ───────────── the workspace ─────────────

def loads_lenient(text):
    """JSON with comments and trailing commas (appsettings files allow both)."""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append(text[i:j + 1])
            i = j + 1
        elif text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            i = n if end < 0 else end + 2
        else:
            out.append(c)
            i += 1
    cleaned = re.sub(r",(\s*[}\]])", r"\1", "".join(out))
    return json.loads(cleaned.lstrip("﻿"))


def project_driver(directory):
    """The database provider the project in `directory` references, when it's exactly one."""
    found = set()
    try:
        names = os.listdir(directory)
    except OSError:
        return None
    for name in names:
        if not name.endswith(".csproj"):
            continue
        try:
            with open(os.path.join(directory, name), encoding="utf-8", errors="replace") as f:
                text = f.read(500_000).lower()
        except OSError:
            continue
        for package in re.findall(r'<packagereference\s+include="([^"]+)"', text):
            for prefix, driver in PACKAGE_DRIVERS:
                if package.startswith(prefix):
                    found.add(driver)
                    break
    return found.pop() if len(found) == 1 else None


def appsettings_connections(path, workspace):
    """The connections in one appsettings*.json file."""
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            data = loads_lenient(f.read(1_000_000))
    except (OSError, ValueError):
        return []
    strings = data.get("ConnectionStrings") if isinstance(data, dict) else None
    if not isinstance(strings, dict):
        return []
    directory = os.path.dirname(path)
    hint = project_driver(directory)
    result = []
    for name, text in strings.items():
        if not isinstance(text, str) or "=" not in text:
            continue
        fields = parse_connection_string(text)
        driver = guess_driver(fields, hint)
        if not driver:
            continue
        conn = connection_from_fields(driver, fields, directory)
        conn["name"] = name
        conn["origin"] = relative(path, workspace)
        result.append(conn)
    return result


def is_sqlite_file(path):
    try:
        with open(path, "rb") as f:
            return f.read(16) == SQLITE_HEADER
    except OSError:
        return False


def scan_workspace(root):
    """Connections found in the workspace: appsettings connection strings, then SQLite files."""
    found, files = [], []
    if not root or not os.path.isdir(root):
        return found
    seen = 0
    for directory, dirs, names in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS and not d.startswith("."))
        seen += len(names)
        if seen > MAX_FILES:
            break
        for name in sorted(names):
            lower = name.lower()
            path = os.path.join(directory, name)
            if lower.startswith("appsettings") and lower.endswith(".json"):
                found += appsettings_connections(path, root)
            elif lower.endswith(SQLITE_EXTENSIONS) and is_sqlite_file(path):
                files.append(path)
    known = {path_key(c["file"]) for c in found if c.get("file")}
    for path in files:
        if path_key(path) not in known:
            found.append({"driver": "sqlite", "file": path, "name": os.path.basename(path),
                          "origin": relative(os.path.dirname(path), root) or "."})
    return found


def merge_connections(lists):
    """All connections without duplicates (the same target under the same name), in order, with ids."""
    result, seen = [], set()
    for conn in (c for group in lists for c in group):
        conn = dict(conn)
        conn["id"] = connection_id(conn)
        if conn["id"] in seen:
            continue
        seen.add(conn["id"])
        result.append(conn)
    return result


def path_key(path):
    path = os.path.normpath(os.path.abspath(path))
    return path.lower() if os.name == "nt" else path


def relative(path, base):
    if base:
        try:
            rel = os.path.relpath(path, base)
            if not rel.startswith(".."):
                return "" if rel == "." else rel
        except ValueError:
            pass
    return path


# ───────────── clients ─────────────

def find_client(driver, settings=None):
    """The command line client for a server driver: the `sql.<driver>Path` setting or PATH."""
    configured = (settings or {}).get(f"sql.{driver}Path")
    if isinstance(configured, str) and configured.strip():
        path = os.path.expanduser(configured.strip())
        return path if os.path.isfile(path) else None
    names = {"postgres": ["psql"], "mysql": ["mysql", "mariadb"], "sqlserver": ["sqlcmd"]}[driver]
    for name in names:
        found = shutil.which(name)
        if found:
            return found
    return None


CLIENT_NAMES = {"postgres": "psql", "mysql": "mysql (or mariadb)", "sqlserver": "sqlcmd"}


def client_command(conn, client, mode, query=None, file=None):
    """(argv, environment additions) for a server client. `mode`: "rows" (machine-readable output of `query`),
    "run" (a `query` or a `file` for people) or "shell"."""
    driver = conn["driver"]
    env = {}
    args = [client]
    if driver == "postgres":
        args += ["-X"]
        if conn.get("host"):
            args += ["-h", conn["host"]]
        if conn.get("port"):
            args += ["-p", str(conn["port"])]
        if conn.get("user"):
            args += ["-U", conn["user"]]
        if conn.get("database"):
            args += ["-d", conn["database"]]
        if conn.get("password"):
            env["PGPASSWORD"] = conn["password"]
        env["PGCONNECT_TIMEOUT"] = "10"
        if mode == "rows":
            args += ["-w", "-q", "-A", "-t", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-c", query]
        elif mode == "run":
            args += ["-v", "ON_ERROR_STOP=1", "-P", "pager=off"]
            args += ["-f", file] if file else ["-c", query]
    elif driver == "mysql":
        if conn.get("host"):
            args += ["-h", conn["host"]]
        if conn.get("port"):
            args += ["-P", str(conn["port"])]
        if conn.get("user"):
            args += ["-u", conn["user"]]
        if conn.get("database"):
            args += ["-D", conn["database"]]
        if conn.get("password"):
            env["MYSQL_PWD"] = conn["password"]
        args += ["--connect-timeout=10"]
        if mode == "rows":
            args += ["-B", "-N", "-e", query]
        elif mode == "run":
            args += ["-t", "-v", "-e", f"source {file}" if file else query]
    elif driver == "sqlserver":
        server = conn.get("host") or "localhost"
        if conn.get("port") and "," not in server:
            server += f",{conn['port']}"
        args += ["-S", server]
        if conn.get("database"):
            args += ["-d", conn["database"]]
        if conn.get("integrated") or not conn.get("user"):
            args += ["-E"]
        else:
            args += ["-U", conn["user"]]
            if conn.get("password"):
                env["SQLCMDPASSWORD"] = conn["password"]
        if conn.get("trust"):
            args += ["-C"]
        args += ["-l", "10"]
        if mode == "rows":
            args += ["-b", "-h", "-1", "-W", "-s", "|", "-Q", "SET NOCOUNT ON; " + query]
        elif mode == "run":
            args += ["-b"] + (["-i", file] if file else ["-Q", query])
    return args, env


def run_rows(conn, query, settings=None):
    """The rows of a query against a server, as lists of strings."""
    client = find_client(conn["driver"], settings)
    if not client:
        raise SqlError(f"{CLIENT_NAMES[conn['driver']]} isn't installed")
    args, extra = client_command(conn, client, "rows", query=query)
    env = dict(os.environ, **extra)
    kwargs = {}
    if os.name == "nt":
        kwargs["creationflags"] = 0x08000000  # CREATE_NO_WINDOW
    try:
        done = subprocess.run(args, capture_output=True, text=True, timeout=QUERY_TIMEOUT, stdin=subprocess.DEVNULL,
                              env=env, encoding="utf-8", errors="replace", **kwargs)
    except OSError as error:
        raise SqlError(f"couldn't run {os.path.basename(client)}: {error}")
    except subprocess.TimeoutExpired:
        raise SqlError("the server didn't answer in time")
    if done.returncode != 0:
        text = done.stderr.strip() or done.stdout.strip() or f"exit code {done.returncode}"
        raise SqlError(text.splitlines()[0])
    separator = "|" if conn["driver"] == "sqlserver" else "\t"
    return [line.split(separator) for line in done.stdout.splitlines() if line.strip()]


SCHEMA_QUERIES = {
    "postgres": ("SELECT c.table_schema, c.table_name, t.table_type, c.column_name, c.data_type, c.is_nullable "
                 "FROM information_schema.columns c JOIN information_schema.tables t "
                 "ON t.table_schema = c.table_schema AND t.table_name = c.table_name "
                 "WHERE c.table_schema NOT IN ('pg_catalog', 'information_schema') "
                 "ORDER BY c.table_schema, c.table_name, c.ordinal_position"),
    "mysql": ("SELECT c.table_schema, c.table_name, t.table_type, c.column_name, c.column_type, c.is_nullable "
              "FROM information_schema.columns c JOIN information_schema.tables t "
              "ON t.table_schema = c.table_schema AND t.table_name = c.table_name "
              "WHERE c.table_schema = DATABASE() "
              "ORDER BY c.table_schema, c.table_name, c.ordinal_position"),
    "sqlserver": ("SELECT c.TABLE_SCHEMA, c.TABLE_NAME, t.TABLE_TYPE, c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE "
                  "FROM INFORMATION_SCHEMA.COLUMNS c JOIN INFORMATION_SCHEMA.TABLES t "
                  "ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME "
                  "ORDER BY c.TABLE_SCHEMA, c.TABLE_NAME, c.ORDINAL_POSITION"),
}


def tables_from_rows(rows):
    """Tables from (schema, table, type, column, data type, nullable) rows, in order."""
    tables, index = [], {}
    for row in rows:
        if len(row) < 6:
            continue
        schema, name, kind, column, data_type, nullable = (v.strip() for v in row[:6])
        key = (schema, name)
        if key not in index:
            index[key] = {"schema": schema, "name": name, "view": "VIEW" in kind.upper(), "columns": []}
            tables.append(index[key])
        index[key]["columns"].append({"name": column, "type": data_type, "nullable": nullable.upper() == "YES"})
    return tables


def open_sqlite(path, readonly=True):
    if not path or (path != ":memory:" and not os.path.isfile(path)):
        raise SqlError(f"{path or 'the database file'} doesn't exist")
    if readonly and path != ":memory:":
        from urllib.parse import quote
        return sqlite3.connect(f"file:{quote(os.path.abspath(path))}?mode=ro", uri=True, timeout=5)
    return sqlite3.connect(path, timeout=5, isolation_level=None)


def sqlite_tables(path):
    db = open_sqlite(path)
    try:
        tables = []
        for name, kind in db.execute("SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') "
                                     "AND name NOT LIKE 'sqlite_%' ORDER BY type, name"):
            columns = [{"name": c[1], "type": c[2] or "", "nullable": not c[3] and not c[5], "pk": bool(c[5])}
                       for c in db.execute(f"PRAGMA table_info({quote_identifier('sqlite', name)})")]
            tables.append({"schema": "", "name": name, "view": kind == "view", "columns": columns})
        return tables
    except sqlite3.Error as error:
        raise SqlError(str(error))
    finally:
        db.close()


def load_tables(conn, settings=None):
    if conn["driver"] == "sqlite":
        return sqlite_tables(conn.get("file"))
    return tables_from_rows(run_rows(conn, SCHEMA_QUERIES[conn["driver"]], settings))


def quote_identifier(driver, name):
    if driver == "mysql":
        return "`" + name.replace("`", "``") + "`"
    if driver == "sqlserver":
        return "[" + name.replace("]", "]]") + "]"
    return '"' + name.replace('"', '""') + '"'


def plain_identifier(name, driver=None):
    """Whether a name can go into a query unquoted (PostgreSQL folds unquoted names to lowercase)."""
    if driver == "postgres" and name != name.lower():
        return False
    return re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) is not None and name.upper() not in KEYWORDS


def table_reference(conn, table, always_quote=False):
    """How to write a table in a query: schema-qualified unless it's in the default schema."""
    driver = conn["driver"]

    def part(name):
        return name if plain_identifier(name, driver) and not always_quote else quote_identifier(driver, name)
    schema = table.get("schema")
    if schema and driver != "mysql" and schema != DEFAULT_SCHEMAS.get(driver):
        return f"{part(schema)}.{part(table['name'])}"
    return part(table["name"])


def select_query(conn, table, limit=SELECT_LIMIT):
    name = table_reference(conn, table)
    if conn["driver"] == "sqlserver":
        return f"SELECT TOP {limit} * FROM {name};"
    return f"SELECT * FROM {name} LIMIT {limit};"


# ───────────── the launcher (runs in a terminal) ─────────────

def split_sqlite_statements(text):
    """SQLite statements of a script, each ending with its ';' (the last one may not have one)."""
    statements, start = [], 0
    for i, c in enumerate(text):
        if c == ";" and sqlite3.complete_statement(text[start:i + 1]):
            statements.append(text[start:i + 1])
            start = i + 1
    rest = text[start:]
    if strip_sql_comments(rest).strip():
        statements.append(rest)
    return [s.strip() for s in statements if strip_sql_comments(s).strip(" \t\r\n;")]


def strip_sql_comments(text):
    return "".join(token for kind, token in tokenize(text) if kind != "comment")


def format_table(columns, rows, max_width=60):
    def cell(value):
        if value is None:
            return "NULL"
        if isinstance(value, bytes):
            return "0x" + value[:32].hex() + ("…" if len(value) > 32 else "")
        text = str(value).replace("\r", "").replace("\n", "⏎")
        return text if len(text) <= max_width else text[:max_width - 1] + "…"
    cells = [[cell(v) for v in row] for row in rows]
    widths = [len(c) for c in columns]
    for row in cells:
        widths = [max(w, len(v)) for w, v in zip(widths, row)]
    lines = [" | ".join(c.ljust(w) for c, w in zip(columns, widths)).rstrip(), "-+-".join("-" * w for w in widths)]
    for raw, row in zip(rows, cells):
        # Numbers line up on the right, everything else on the left.
        lines.append(" | ".join(text.rjust(w) if isinstance(value, (int, float)) and not isinstance(value, bool)
                                else text.ljust(w) for value, text, w in zip(raw, row, widths)).rstrip())
    return "\n".join(lines)


def run_sqlite_script(path, text, out=None):
    """Runs a script statement by statement and prints each result. Returns the exit code."""
    out = out or sys.stdout
    try:
        db = open_sqlite(path, readonly=False)
    except (SqlError, sqlite3.Error) as error:
        print(f"Error: {error}", file=out)
        return 1
    started = time.monotonic()
    try:
        for number, statement in enumerate(split_sqlite_statements(text), 1):
            try:
                cursor = db.execute(statement)
                if cursor.description:
                    rows = cursor.fetchall()
                    print(format_table([d[0] for d in cursor.description], rows), file=out)
                    print(f"({len(rows)} row{'s' if len(rows) != 1 else ''})\n", file=out)
                elif cursor.rowcount >= 0:
                    print(f"OK, {cursor.rowcount} row{'s' if cursor.rowcount != 1 else ''} affected\n", file=out)
            except sqlite3.Error as error:
                first = statement.splitlines()[0][:80]
                print(f"Error in statement {number} ({first}): {error}", file=out)
                return 1
        print(f"Done in {time.monotonic() - started:.2f}s", file=out)
        return 0
    finally:
        db.close()


def sqlite_shell(path):
    """A small interactive SQLite shell (used when the sqlite3 program isn't installed)."""
    print(f"SQLite {sqlite3.sqlite_version} · {path}\nEnd statements with ';'. .tables lists the tables, .quit exits.")
    buffer = ""
    while True:
        try:
            line = input("sqlite> " if not buffer else "   ...> ")
        except (EOFError, KeyboardInterrupt):
            print()
            return 0
        if not buffer and line.strip().startswith("."):
            command = line.strip().split()
            if command[0] in (".quit", ".exit", ".q"):
                return 0
            if command[0] == ".tables":
                for table in sqlite_tables(path):
                    print(table["name"] + (" (view)" if table["view"] else ""))
            else:
                print("Commands: .tables, .quit")
            continue
        buffer += line + "\n"
        if sqlite3.complete_statement(buffer):
            run_sqlite_script(path, buffer)
            buffer = ""


def launch(spec_path):
    """`sql.py launch <spec>`: runs a query, a file or a shell for the connection in the spec file (and deletes
    it: it may hold a password)."""
    try:
        with open(spec_path, encoding="utf-8") as f:
            spec = json.load(f)
    finally:
        try:
            os.remove(spec_path)
        except OSError:
            pass
    conn, mode = spec["connection"], spec["mode"]
    print(f"── {conn.get('name')} · {DRIVERS[conn['driver']]} · {connection_target(conn)}\n", flush=True)
    if conn["driver"] == "sqlite":
        if mode == "shell":
            program = shutil.which("sqlite3")
            if program:
                return subprocess.call([program, "-header", "-column", conn["file"]])
            return sqlite_shell(conn["file"])
        text = spec.get("query")
        if spec.get("file"):
            with open(spec["file"], encoding="utf-8-sig", errors="replace") as f:
                text = f.read()
        return run_sqlite_script(conn["file"], text or "")
    client = find_client(conn["driver"], spec.get("settings"))
    if not client:
        print(f"{CLIENT_NAMES[conn['driver']]} isn't installed (or set \"sql.{conn['driver']}Path\").")
        return 127
    args, extra = client_command(conn, client, mode, query=spec.get("query"), file=spec.get("file"))
    try:
        return subprocess.call(args, env=dict(os.environ, **extra))
    except KeyboardInterrupt:
        return 130


# ───────────── the formatter ─────────────

KEYWORDS = set("""
ADD ALL ALTER AND ANY AS ASC AUTOINCREMENT AUTO_INCREMENT BEGIN BETWEEN BIGINT BINARY BIT BLOB BOOLEAN BOTH BY CASCADE
CASE CAST CHAR CHARACTER CHECK CLUSTERED COALESCE COLLATE COLUMN COMMIT CONFLICT CONSTRAINT CONTINUE CREATE CROSS
CURRENT_DATE CURRENT_TIME CURRENT_TIMESTAMP CURSOR DATABASE DATE DATETIME DATETIME2 DECIMAL DECLARE DEFAULT DEFERRABLE
DELETE DESC DISTINCT DO DOUBLE DROP EACH ELSE ELSIF END ESCAPE EXCEPT EXEC EXECUTE EXISTS EXPLAIN FALSE FETCH FILTER
FIRST FLOAT FOR FOREIGN FROM FULL FUNCTION GO GRANT GROUP HAVING IDENTITY IF IGNORE ILIKE IN INDEX INNER INSERT INT
INTEGER INTERSECT INTERVAL INTO IS JOIN JSON JSONB KEY LANGUAGE LAST LATERAL LEADING LEFT LIKE LIMIT MATERIALIZED MERGE
NATURAL NCHAR NEXT NO NOCOUNT NONCLUSTERED NOT NOTHING NULL NULLS NUMERIC NVARCHAR OF OFFSET ON ONLY OR ORDER OUTER OUTPUT
OVER PARTITION PRAGMA PRECISION PRIMARY PROCEDURE RAISE REAL RECURSIVE REFERENCES REINDEX RELEASE RENAME REPLACE
RESTRICT RETURN RETURNING RETURNS REVOKE RIGHT ROLLBACK ROW ROWS SAVEPOINT SCHEMA SELECT SEQUENCE SERIAL SET SMALLINT
TABLE TEMP TEMPORARY TEXT THEN TIME TIMESTAMP TIMESTAMPTZ TINYINT TO TOP TRAILING TRAN TRANSACTION TRIGGER TRUE
TRUNCATE UNION UNIQUE UNIQUEIDENTIFIER UNSIGNED UPDATE UPSERT USE USING UUID VACUUM VALUES VARBINARY VARCHAR VIEW WHEN
WHERE WHILE WINDOW WITH WITHOUT
""".split())

TOKEN_RE = re.compile(r"""
    (?P<comment>--[^\n]*|/\*.*?(?:\*/|\Z))
  | (?P<dollar>\$\$.*?(?:\$\$|\Z)|\$(?P<tag>[A-Za-z_][A-Za-z0-9_]*)\$.*?(?:\$(?P=tag)\$|\Z))
  | (?P<string>[NnEeXxBb]?'(?:[^']|'')*(?:'|\Z))
  | (?P<quoted>"(?:[^"]|"")*(?:"|\Z)|\[[^\]\n]*(?:\]|\Z)|`(?:[^`]|``)*(?:`|\Z))
  | (?P<word>[A-Za-z_][A-Za-z0-9_$]*)
  | (?P<other>\s+|.)
""", re.S | re.X)


def tokenize(text):
    """(kind, text) tokens: comment, string, quoted (identifier), word or other."""
    for m in TOKEN_RE.finditer(text):
        kind = m.lastgroup if m.lastgroup != "tag" else "dollar"
        if kind == "dollar":
            kind = "string"
        yield kind, m.group(0)


def format_sql(text, keyword_case="upper"):
    """Keywords in one case (outside strings, comments and quoted names), no trailing whitespace, one final
    newline. Everything else stays as written."""
    parts = []
    previous = ""
    for kind, token in tokenize(text):
        # A word after '.' or '@' / ':' is a name (t.key, @user), not a keyword.
        if kind == "word" and token.upper() in KEYWORDS and previous not in (".", "@", ":", "#"):
            if keyword_case == "upper":
                token = token.upper()
            elif keyword_case == "lower":
                token = token.lower()
        parts.append(token)
        if kind != "other" or token.strip():
            previous = token[-1:]
    lines = [line.rstrip() for line in "".join(parts).replace("\r\n", "\n").split("\n")]
    while lines and not lines[-1]:
        lines.pop()
    return "\n".join(lines) + "\n" if lines else ""


# ───────────── protocol ─────────────

output_lock = threading.Lock()


def send(message):
    with output_lock:
        try:
            print(json.dumps(message), flush=True)
        except (OSError, ValueError):
            pass  # the IDE went away


def log(text):
    send({"type": "log", "text": text})


def write_spec(spec):
    """A private temporary file with what the launcher needs (it deletes the file when it reads it)."""
    fd, path = tempfile.mkstemp(prefix="byteforge-sql-", suffix=".json")
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(spec, f)
    return path


def shell_quote(arg):
    if arg and all(c.isalnum() or c in "-_./:=@%+," for c in arg):
        return arg
    if os.name == "nt":
        return '"' + arg.replace('"', '\\"') + '"'
    return "'" + arg.replace("'", "'\\''") + "'"


def state_file():
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "state.json")


class Module:
    def __init__(self):
        self.lock = threading.RLock()
        self.workspace = None
        self.settings = {}
        self.connections = []
        self.schemas = {}         # id -> {"tables": [...]} or {"error": "..."}
        self.loading = set()      # ids being loaded
        self.active = None        # id of the connection "Run File" uses
        self.editor_path = None
        self.next_request = 1
        self.pending = {}
        self.last_tree = None
        self.last_status = None
        self.scanned = False
        self.visible = False
        self.stale = False        # files changed while the view was hidden
        self.work = queue.Queue()

    # ── state ──

    def load_state(self):
        try:
            with open(state_file(), encoding="utf-8") as f:
                state = json.load(f)
            return state if isinstance(state, dict) else {}
        except (OSError, ValueError):
            return {}

    def remember_active(self):
        state = self.load_state()
        state.setdefault("active", {})[self.workspace or ""] = self.active
        try:
            with open(state_file(), "w", encoding="utf-8") as f:
                json.dump(state, f)
        except OSError:
            pass

    def rescan(self):
        configured = self.settings.get("sql.connections")
        from_settings = [c for c in (connection_from_setting(e, self.workspace)
                                     for e in (configured if isinstance(configured, list) else [])) if c]
        found = scan_workspace(self.workspace) if self.settings.get("sql.discover", True) is not False else []
        connections = merge_connections([from_settings, found])
        with self.lock:
            self.connections = connections
            ids = {c["id"] for c in connections}
            self.schemas = {k: v for k, v in self.schemas.items() if k in ids}
            if self.active not in ids:
                remembered = self.load_state().get("active", {}).get(self.workspace or "")
                # With several connections and none chosen yet, "Run File" asks (and remembers the answer).
                only = connections[0]["id"] if len(connections) == 1 else None
                self.active = remembered if remembered in ids else only
            self.scanned = True
        self.publish()

    def connection(self, cid):
        with self.lock:
            return next((c for c in self.connections if c["id"] == cid), None)

    # ── state → view ──

    def table_item(self, conn, table, index):
        tid = f"table:{conn['id']}:{index}"
        columns = [{"id": f"col:{conn['id']}:{index}:{i}", "label": c["name"],
                    "description": (c.get("type") or "").lower() + ("" if c.get("nullable", True) else " not null"),
                    "icon": COLUMN_ICON, "iconColor": DRIVER_COLORS[conn["driver"]] if c.get("pk") else None,
                    "tooltip": f"{c['name']} {c.get('type') or ''}{' · primary key' if c.get('pk') else ''}\n"
                               "Click to insert the name"}
                   for i, c in enumerate(table["columns"])]
        for column in columns:
            if column["iconColor"] is None:
                del column["iconColor"]
        schema = table.get("schema")
        show_schema = schema and conn["driver"] != "mysql" and schema != DEFAULT_SCHEMAS.get(conn["driver"])
        return {"id": tid, "label": f"{schema}.{table['name']}" if show_schema else table["name"],
                "description": "view" if table["view"] else None, "icon": VIEW_ICON if table["view"] else TABLE_ICON,
                "count": len(columns), "expanded": False, "children": columns,
                "actions": [{"id": "select", "icon": "play", "tooltip": f"Select Top {SELECT_LIMIT}"},
                            {"id": "insert", "icon": "add", "tooltip": "Insert the Name"}]}

    def connection_item(self, conn):
        cid = conn["id"]
        with self.lock:
            schema = self.schemas.get(cid)
            loading = cid in self.loading
            active = cid == self.active
        description = DRIVERS[conn["driver"]] + (f" · {conn['origin']}" if conn.get("origin") not in (None, "", "settings") else "")
        tooltip = [f"{conn['name']} ({DRIVERS[conn['driver']]})", connection_target(conn)]
        if conn.get("origin"):
            tooltip.append("From settings" if conn["origin"] == "settings" else f"From {conn['origin']}")
        if active:
            tooltip.append("Used by \"SQL: Run File\"")
        tooltip.append("Click to load the tables" if schema is None else "")
        item = {"id": f"conn:{cid}", "label": conn["name"] + ("  ✓" if active else ""), "description": description,
                "tooltip": "\n".join(t for t in tooltip if t), "icon": DATABASE_ICON,
                "iconColor": DRIVER_COLORS[conn["driver"]],
                "actions": [{"id": "run", "icon": "play", "tooltip": "Run the Active .sql File Here", "label": "Run Active File Here"},
                            {"id": "shell", "icon": "terminal", "tooltip": "Open Shell"},
                            {"id": "refresh", "icon": "refresh", "tooltip": "Reload Tables"}]}
        if loading:
            item["children"] = [{"id": f"loading:{cid}", "label": "Loading…", "icon": "sync"}]
        elif schema is not None and schema.get("error"):
            item["children"] = [{"id": f"error:{cid}", "label": "Couldn't load the tables", "icon": "warning",
                                 "iconColor": WARNING_COLOR, "description": schema["error"],
                                 "tooltip": schema["error"] + "\nClick to try again"}]
        elif schema is not None:
            tables = schema["tables"]
            item["count"] = len(tables)
            item["children"] = [self.table_item(conn, t, i) for i, t in enumerate(tables)] or [
                {"id": f"empty:{cid}", "label": "No tables"}]
        return item

    def tree(self):
        with self.lock:
            connections = list(self.connections)
            scanned = self.scanned
        children = [self.connection_item(c) for c in connections] or [
            {"id": "note:none", "label": "No connections yet",
             "tooltip": "Add one to \"sql.connections\" in settings, put ConnectionStrings in appsettings.json, "
                        "or add a SQLite database file to the workspace"}]
        tree = {"type": "setTree", "view": VIEW, "badge": 0,
                "items": [{"id": "section:connections", "label": "CONNECTIONS", "header": True,
                           "count": len(connections), "children": children}],
                "actions": [{"id": "add", "icon": "add", "tooltip": "Add a Connection (opens settings)", "label": "Add Connection…"},
                            {"id": "refresh", "icon": "refresh", "tooltip": "Refresh"}]}
        if not scanned:
            tree["message"] = "Looking for connections…"
        return tree

    def publish(self, force=False):
        tree = self.tree()
        text = json.dumps(tree, sort_keys=True)
        with self.lock:
            if not force and text == self.last_tree:
                return
            self.last_tree = text
        send(tree)
        self.publish_status()

    def publish_status(self):
        """"SQL: <connection>" in the status bar while a .sql file is active."""
        with self.lock:
            conn = self.connection(self.active) if self.active else None
            show = bool(self.editor_path and self.editor_path.lower().endswith(".sql"))
        text = ""
        if show:
            text = f"SQL: {conn['name']}" if conn else "SQL: no connection"
        status = {"type": "status", "id": "sql.connection", "text": text, "command": "sql.selectConnection",
                  "tooltip": "The connection \"SQL: Run File\" uses. Click to change it.", "alignment": "right"}
        if status != self.last_status:
            self.last_status = status
            send(status)

    # ── loading tables ──

    def load(self, cid):
        conn = self.connection(cid)
        if not conn:
            return
        with self.lock:
            if cid in self.loading:
                return
            self.loading.add(cid)
        self.publish()

        def work():
            try:
                result = {"tables": load_tables(conn, self.settings)}
            except SqlError as error:
                result = {"error": str(error)}
            except Exception as error:  # a bug must not leave the row loading forever
                result = {"error": f"{type(error).__name__}: {error}"}
            with self.lock:
                self.loading.discard(cid)
                self.schemas[cid] = result
            self.publish()
        threading.Thread(target=work, daemon=True).start()

    # ── requests to the IDE ──

    def ask(self, message, callback):
        with self.lock:
            request = self.next_request
            self.next_request += 1
            self.pending[request] = callback
        message["requestId"] = request
        send(message)

    def answer(self, request, value):
        with self.lock:
            callback = self.pending.pop(request, None)
        if callback:
            callback(value)

    def pick_connection(self, placeholder, on_pick, prefer_active=False):
        with self.lock:
            connections = list(self.connections)
            active = self.active
        if not connections:
            log("sql: no connections. Add one to \"sql.connections\" in settings, put ConnectionStrings in "
                "appsettings.json, or add a SQLite database file to the workspace.")
            send({"type": "showView", "id": "output"})
            return
        if prefer_active and active and self.connection(active):
            on_pick(active)
            return
        if len(connections) == 1:
            on_pick(connections[0]["id"])
            return
        items = [{"label": c["name"], "detail": connection_target(c),
                  "hint": ("✓ " if c["id"] == active else "") + DRIVERS[c["driver"]], "value": c["id"]}
                 for c in connections]
        self.ask({"type": "quickPick", "placeholder": placeholder, "items": items},
                 lambda value: on_pick(value) if value else None)

    def set_active(self, cid):
        with self.lock:
            self.active = cid
        self.remember_active()
        self.publish()

    # ── running ──

    def launch_in_terminal(self, conn, title, mode, query=None, file=None, save_all=False):
        if conn["driver"] != "sqlite" and not find_client(conn["driver"], self.settings):
            log(f"sql: {CLIENT_NAMES[conn['driver']]} isn't installed (or set \"sql.{conn['driver']}Path\" "
                "in settings).")
            send({"type": "showView", "id": "output"})
            return
        spec = {"connection": conn, "mode": mode, "query": query, "file": file,
                "settings": {k: v for k, v in self.settings.items() if k.startswith("sql.") and k.endswith("Path")}}
        path = write_spec(spec)
        script = os.path.abspath(__file__)
        python = sys.executable or "python3"
        message = {"type": "runInTerminal", "title": title,
                   "command": " ".join(shell_quote(a) for a in (python, script, "launch", path)),
                   "cwd": os.path.dirname(file) if file else (self.workspace or os.path.expanduser("~"))}
        if save_all:
            message["saveAll"] = True
        send(message)

    def run_file(self, cid=None):
        path = self.editor_path
        if not path or not path.lower().endswith(".sql"):
            log("sql: open a .sql file first (\"SQL: Run File\" runs the active one; save new files first).")
            send({"type": "showView", "id": "output"})
            return

        def run(chosen):
            conn = self.connection(chosen)
            if conn:
                if chosen != self.active:
                    self.set_active(chosen)
                self.launch_in_terminal(conn, f"SQL: {os.path.basename(path)} → {conn['name']}", "run",
                                        file=path, save_all=True)
        if cid:
            run(cid)
        else:
            self.ensure_scanned()
            self.pick_connection("Run against which connection?", run, prefer_active=True)

    def open_shell(self, cid):
        conn = self.connection(cid)
        if conn:
            self.launch_in_terminal(conn, f"SQL: {conn['name']}", "shell")

    def select_top(self, cid, index):
        conn = self.connection(cid)
        with self.lock:
            tables = (self.schemas.get(cid) or {}).get("tables") or []
        if conn and 0 <= index < len(tables):
            table = tables[index]
            self.launch_in_terminal(conn, f"SQL: {table['name']}", "run", query=select_query(conn, table))

    def ensure_scanned(self):
        with self.lock:
            scanned = self.scanned
        if not scanned:
            self.rescan()

    def open_settings(self):
        config = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
        if sys.platform == "darwin":
            config = os.path.join(os.path.expanduser("~"), "Library", "Application Support")
        elif os.name == "nt":
            config = os.environ.get("APPDATA") or config
        path = os.path.join(config, "byteforge", "settings.json")
        log('sql: add connections to "sql.connections" in settings, for example\n'
            '  "sql.connections": [\n'
            '    { "name": "Local Postgres", "driver": "postgres", "host": "localhost", "database": "app", '
            '"user": "postgres", "password": "…" },\n'
            '    { "name": "Shop", "driver": "sqlserver", "connectionString": "Server=localhost;Database=Shop;'
            'Trusted_Connection=True;TrustServerCertificate=True" },\n'
            '    { "name": "Cache", "driver": "sqlite", "file": "data/cache.db" }\n'
            '  ]\n'
            "Drivers: sqlite, postgres, mysql, sqlserver.")
        send({"type": "showView", "id": "output"})
        if os.path.isfile(path):
            send({"type": "openFile", "path": path})

    # ── tree actions ──

    def tree_action(self, item, action):
        if item is None:
            if action == "refresh":
                with self.lock:
                    self.schemas.clear()
                self.rescan()
            elif action == "add":
                self.open_settings()
            return
        if not isinstance(item, str):
            return
        kind, _, rest = item.partition(":")
        if kind == "conn":
            if action == "run":
                self.run_file(rest)
            elif action == "shell":
                self.open_shell(rest)
            elif action in ("refresh", "click"):
                self.load(rest)
        elif kind == "error":
            self.load(rest)
        elif kind in ("table", "col"):
            parts = rest.split(":")
            cid, index = parts[0], int(parts[1])
            conn = self.connection(cid)
            with self.lock:
                tables = (self.schemas.get(cid) or {}).get("tables") or []
            if not conn or index >= len(tables):
                return
            table = tables[index]
            if kind == "table" and action == "select":
                self.select_top(cid, index)
            elif kind == "table" and action == "insert":
                send({"type": "insertText", "text": table_reference(conn, table)})
            elif kind == "col" and action == "click":
                column = table["columns"][int(parts[2])]["name"]
                send({"type": "insertText", "text": column if plain_identifier(column, conn["driver"])
                      else quote_identifier(conn["driver"], column)})

    # ── commands ──

    def execute(self, command):
        if command == "sql.runFile":
            self.run_file()
        elif command == "sql.selectConnection":
            self.ensure_scanned()
            self.pick_connection("Connection for \"SQL: Run File\"", self.set_active)
        elif command == "sql.openShell":
            self.ensure_scanned()
            self.pick_connection("Open a shell for which connection?", self.open_shell)
        elif command == "sql.addConnection":
            self.open_settings()
        elif command == "sql.refresh":
            with self.lock:
                self.schemas.clear()
            self.rescan()

    def format(self, request, path, text):
        case = self.settings.get("sql.format.keywordCase", "upper")
        if case not in ("upper", "lower", "preserve"):
            case = "upper"
        try:
            send({"type": "formatResult", "requestId": request, "text": format_sql(text, case)})
        except Exception as error:
            send({"type": "formatResult", "requestId": request, "error": str(error)})

    # ── messages from the IDE ──

    def handle(self, message):
        kind = message.get("type")
        if kind == "activate":
            self.workspace = message.get("workspace") or None
            if isinstance(message.get("settings"), dict):
                self.settings = message["settings"]
            self.publish(force=True)
            self.later(self.rescan)
        elif kind == "executeCommand":
            self.later(self.execute, message.get("id"))
        elif kind == "treeAction":
            if message.get("view") == VIEW:
                self.later(self.tree_action, message.get("item"), message.get("action"))
        elif kind == "quickPickResult":
            value = message.get("value")
            if isinstance(value, str):
                self.later(self.answer, message.get("requestId"), value)
        elif kind == "format":
            self.format(message.get("requestId"), message.get("path"), message.get("text") or "")
        elif kind == "event":
            self.event(message)

    def event(self, message):
        name = message.get("name")
        if name == "editorChanged":
            with self.lock:
                self.editor_path = message.get("path")
            self.publish_status()
        elif name == "folderOpened":
            with self.lock:
                self.workspace = message.get("path") or None
                self.schemas.clear()
                self.active = None
                self.scanned = False
            self.later(self.rescan)
        elif name == "filesChanged":
            # Scanning walks the workspace: only while the view shows, else once it's shown again.
            if self.visible:
                self.later(self.rescan)
            else:
                self.stale = True
        elif name == "viewShown" and message.get("view") == VIEW:
            self.visible = True
            if self.stale:
                self.stale = False
                self.later(self.rescan)
        elif name == "viewHidden" and message.get("view") == VIEW:
            self.visible = False
        elif name == "settingsChanged":
            if isinstance(message.get("settings"), dict):
                self.settings = message["settings"]
                self.later(self.rescan)

    def later(self, function, *args):
        self.work.put((function, args))

    def worker(self):
        while True:
            function, args = self.work.get()
            self.safely(function, *args)

    def safely(self, function, *args):
        try:
            function(*args)
        except Exception as error:  # a bug here must never take the module down
            log(f"sql: {error}")


COMMANDS = [
    ("sql.runFile", "Run File", "ctrl-shift-enter"),
    ("sql.selectConnection", "Select Connection…", None),
    ("sql.openShell", "Open Shell…", None),
    ("sql.addConnection", "Add Connection…", None),
    ("sql.refresh", "Refresh Connections", None),
]


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "launch":
        sys.exit(launch(sys.argv[2]))
    module = Module()
    for command, title, key in COMMANDS:
        message = {"type": "registerCommand", "id": command, "title": title, "category": "SQL"}
        if key:
            message["keybinding"] = key
        send(message)
    send({"type": "registerFormatter", "id": "sql.format", "name": "SQL (keyword case)", "extensions": ["sql"]})
    threading.Thread(target=module.worker, daemon=True).start()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if isinstance(message, dict):
            module.safely(module.handle, message)


if __name__ == "__main__":
    main()
