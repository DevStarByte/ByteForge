"""Tests for the SQL module: python3 -m unittest discover -s modules/sql -p 'test_*.py' -v"""

import io
import json
import os
import re
import sqlite3
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import sql  # noqa: E402


def load_manifest():
    with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
        text = f.read()
    return json.loads(re.sub(r"^\s*//.*$", "", text, flags=re.M))


def write(path, text=""):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def make_db(path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    db = sqlite3.connect(path)
    db.executescript("""
        CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT);
        CREATE TABLE "order items" (id INTEGER PRIMARY KEY, qty INT);
        CREATE VIEW named AS SELECT name FROM customers;
        INSERT INTO customers (name, email) VALUES ('Ada', 'ada@example.com'), ('Linus', NULL);
    """)
    db.close()


class ConnectionStrings(unittest.TestCase):
    def test_parses_quoted_values_and_keys(self):
        fields = sql.parse_connection_string('Server=localhost; Database = Shop;Password="a;b""c";;User Id=sa')
        self.assertEqual(fields, {"server": "localhost", "database": "Shop", "password": 'a;b"c', "user id": "sa"})

    def test_guesses_the_driver(self):
        guess = lambda text, hint=None: sql.guess_driver(sql.parse_connection_string(text), hint)  # noqa: E731
        self.assertEqual(guess("Data Source=app.db"), "sqlite")
        self.assertEqual(guess("Data Source=app.db", "sqlserver"), "sqlite")
        self.assertEqual(guess("Host=localhost;Database=app;Username=postgres;Password=x"), "postgres")
        self.assertEqual(guess("Server=(localdb)\\MSSQLLocalDB;Database=Shop;Trusted_Connection=True"), "sqlserver")
        self.assertEqual(guess("Server=localhost;Database=shop;Uid=root;Pwd=secret"), "mysql")
        self.assertEqual(guess("Server=localhost;Database=shop"), "sqlserver")
        self.assertEqual(guess("Server=localhost;Database=shop", "postgres"), "postgres")
        self.assertIsNone(guess("Foo=bar"))

    def test_sql_server_fields(self):
        fields = sql.parse_connection_string(
            "Server=tcp:db.example.com,1433;Initial Catalog=Shop;User ID=app;Password=pw;TrustServerCertificate=True")
        conn = sql.connection_from_fields("sqlserver", fields)
        self.assertEqual(conn, {"driver": "sqlserver", "host": "db.example.com,1433", "database": "Shop",
                                "user": "app", "password": "pw", "integrated": False, "trust": True})

    def test_sqlite_paths_are_relative_to_the_project(self):
        conn = sql.connection_from_fields("sqlite", {"data source": "data/app.db"}, "/src/Web")
        self.assertEqual(conn["file"], os.path.normpath("/src/Web/data/app.db"))

    def test_settings_entries(self):
        conn = sql.connection_from_setting({"name": "Local", "driver": "PostgreSQL", "host": "localhost",
                                            "port": 5432, "database": "app"}, "/ws")
        self.assertEqual((conn["driver"], conn["port"], conn["name"], conn["origin"]), ("postgres", "5432", "Local", "settings"))
        conn = sql.connection_from_setting({"driver": "sqlite", "file": "cache.db"}, "/ws")
        self.assertEqual((conn["file"], conn["name"]), (os.path.normpath("/ws/cache.db"), "cache.db"))
        self.assertIsNone(sql.connection_from_setting({"name": "no driver"}, "/ws"))
        conn = sql.connection_from_setting({"connectionString": "Host=h;Database=d"}, "/ws")
        self.assertEqual(conn["driver"], "postgres")


class Workspace(unittest.TestCase):
    def test_finds_appsettings_connections_and_sqlite_files(self):
        with tempfile.TemporaryDirectory() as root:
            write(os.path.join(root, "Web", "Web.csproj"),
                  '<Project><ItemGroup><PackageReference Include="Npgsql.EntityFrameworkCore.PostgreSQL" Version="9" />'
                  "</ItemGroup></Project>")
            write(os.path.join(root, "Web", "appsettings.json"), """{
                // comments and trailing commas are fine here
                "ConnectionStrings": {
                    "Default": "Server=localhost;Database=web;User Id=me;Password=pw",
                    "Cache": "Data Source=cache.db",
                },
            }""")
            make_db(os.path.join(root, "Web", "cache.db"))
            make_db(os.path.join(root, "tools", "other.sqlite"))
            write(os.path.join(root, "tools", "fake.db"), "not a database")
            os.makedirs(os.path.join(root, "Web", "bin"))
            make_db(os.path.join(root, "Web", "bin", "copy.db"))
            found = sql.scan_workspace(root)
        names = [(c["name"], c["driver"]) for c in found]
        self.assertEqual(names, [("Default", "postgres"), ("Cache", "sqlite"), ("other.sqlite", "sqlite")])
        self.assertEqual(found[0]["origin"], os.path.join("Web", "appsettings.json"))
        self.assertEqual(found[0]["user"], "me")

    def test_merge_drops_duplicates(self):
        a = {"driver": "sqlite", "file": "/x.db", "name": "x"}
        merged = sql.merge_connections([[a], [dict(a), {"driver": "sqlite", "file": "/y.db", "name": "y"}]])
        self.assertEqual([c["name"] for c in merged], ["x", "y"])
        self.assertTrue(all(len(c["id"]) == 12 for c in merged))


class Sqlite(unittest.TestCase):
    def test_tables_views_and_columns(self):
        with tempfile.TemporaryDirectory() as root:
            path = os.path.join(root, "app.db")
            make_db(path)
            tables = sql.sqlite_tables(path)
        self.assertEqual([(t["name"], t["view"]) for t in tables],
                         [("customers", False), ("order items", False), ("named", True)])
        columns = tables[0]["columns"]
        self.assertEqual([(c["name"], c["pk"], c["nullable"]) for c in columns],
                         [("id", True, False), ("name", False, False), ("email", False, True)])

    def test_missing_file(self):
        with self.assertRaises(sql.SqlError):
            sql.sqlite_tables("/nonexistent/app.db")

    def test_runs_a_script(self):
        with tempfile.TemporaryDirectory() as root:
            path = os.path.join(root, "app.db")
            make_db(path)
            out = io.StringIO()
            code = sql.run_sqlite_script(path, """
                -- a comment; with a semicolon
                INSERT INTO customers (name) VALUES ('Grace; Hopper');
                SELECT id, name FROM customers ORDER BY id;
                UPDATE customers SET email = 'x' WHERE id > 1
            """, out)
            text = out.getvalue()
            self.assertEqual(code, 0, text)
            self.assertIn("OK, 1 row affected", text)
            self.assertIn("Grace; Hopper", text)
            self.assertIn("(3 rows)", text)
            self.assertIn("OK, 2 rows affected", text)
            self.assertEqual(sqlite3.connect(path).execute("SELECT count(*) FROM customers WHERE email = 'x'").fetchone()[0], 2)

    def test_stops_at_the_first_error(self):
        with tempfile.TemporaryDirectory() as root:
            path = os.path.join(root, "app.db")
            make_db(path)
            out = io.StringIO()
            code = sql.run_sqlite_script(path, "SELECT 1; SELECT * FROM nope; SELECT 2;", out)
        self.assertEqual(code, 1)
        self.assertIn("Error in statement 2", out.getvalue())
        self.assertIn("no such table", out.getvalue())

    def test_splits_statements(self):
        self.assertEqual(sql.split_sqlite_statements("SELECT ';'; -- x;\nSELECT 2;\n-- only a comment"),
                         ["SELECT ';';", "-- x;\nSELECT 2;"])
        self.assertEqual(sql.split_sqlite_statements("SELECT 1"), ["SELECT 1"])

    def test_formats_a_table(self):
        text = sql.format_table(["id", "name"], [(1, "Ada"), (10, None)])
        self.assertEqual(text.splitlines(), ["id | name", "---+-----", " 1 | Ada", "10 | NULL"])


class Servers(unittest.TestCase):
    def test_client_commands_keep_passwords_in_the_environment(self):
        pg = {"driver": "postgres", "host": "h", "port": "5433", "user": "u", "database": "d", "password": "pw"}
        args, env = sql.client_command(pg, "psql", "run", file="/q.sql")
        self.assertNotIn("pw", args)
        self.assertEqual(env["PGPASSWORD"], "pw")
        self.assertEqual(args[:10], ["psql", "-X", "-h", "h", "-p", "5433", "-U", "u", "-d", "d"])
        self.assertEqual(args[-2:], ["-f", "/q.sql"])

        my = {"driver": "mysql", "host": "h", "user": "root", "database": "shop", "password": "pw"}
        args, env = sql.client_command(my, "mysql", "rows", query="SELECT 1")
        self.assertEqual((env["MYSQL_PWD"], args[-3:]), ("pw", ["-N", "-e", "SELECT 1"]))

        ms = {"driver": "sqlserver", "host": "db", "port": "1444", "user": "sa", "password": "pw", "trust": True}
        args, env = sql.client_command(ms, "sqlcmd", "shell")
        self.assertEqual(args, ["sqlcmd", "-S", "db,1444", "-U", "sa", "-C", "-l", "10"])
        self.assertEqual(env, {"SQLCMDPASSWORD": "pw"})
        args, _ = sql.client_command({"driver": "sqlserver", "host": "db", "integrated": True}, "sqlcmd", "shell")
        self.assertIn("-E", args)

    def test_reads_schema_rows(self):
        rows = [["public", "users", "BASE TABLE", "id", "integer", "NO"],
                ["public", "users", "BASE TABLE", "name", "text", "YES"],
                ["public", "active", "VIEW", "id", "integer", "YES"], ["broken"]]
        tables = sql.tables_from_rows(rows)
        self.assertEqual([(t["name"], t["view"], len(t["columns"])) for t in tables], [("users", False, 2), ("active", True, 1)])
        self.assertFalse(tables[0]["columns"][0]["nullable"])

    def test_select_queries(self):
        pg = {"driver": "postgres"}
        self.assertEqual(sql.select_query(pg, {"schema": "public", "name": "users"}), "SELECT * FROM users LIMIT 100;")
        self.assertEqual(sql.select_query(pg, {"schema": "audit", "name": "Log"}), 'SELECT * FROM audit."Log" LIMIT 100;')
        self.assertEqual(sql.select_query({"driver": "sqlserver"}, {"schema": "dbo", "name": "order"}),
                         "SELECT TOP 100 * FROM [order];")
        self.assertEqual(sql.select_query({"driver": "mysql"}, {"schema": "shop", "name": "a b"}),
                         "SELECT * FROM `a b` LIMIT 100;")


class Formatter(unittest.TestCase):
    def test_uppercases_keywords_only_outside_strings_comments_and_names(self):
        text = "select id, \"select\" from t -- from here\nwhere name = 'from' and t.key = 1   \n\n\n"
        self.assertEqual(sql.format_sql(text),
                         "SELECT id, \"select\" FROM t -- from here\nWHERE name = 'from' AND t.key = 1\n")

    def test_lowercase_and_preserve(self):
        self.assertEqual(sql.format_sql("SELECT X FROM [Order] /* SELECT */", "lower"), "select X from [Order] /* SELECT */\n")
        self.assertEqual(sql.format_sql("Select 1", "preserve"), "Select 1\n")

    def test_dollar_quoted_bodies_stay(self):
        text = "create function f() returns int as $$ select 1 $$ language sql;"
        self.assertEqual(sql.format_sql(text), "CREATE FUNCTION f() RETURNS INT AS $$ select 1 $$ LANGUAGE sql;\n")

    def test_variables_stay(self):
        self.assertEqual(sql.format_sql("declare @table int; select :limit"), "DECLARE @table INT; SELECT :limit\n")


class Protocol(unittest.TestCase):
    def run_module(self, module, messages):
        out = io.StringIO()
        with redirect_stdout(out):
            for message in messages:
                module.handle(message)
            while not module.work.empty():
                function, args = module.work.get()
                function(*args)
        return [json.loads(line) for line in out.getvalue().splitlines()]

    def test_view_status_and_run(self):
        with tempfile.TemporaryDirectory() as root:
            make_db(os.path.join(root, "app.db"))
            query = os.path.join(root, "q.sql")
            write(query, "SELECT 1;")
            module = sql.Module()
            module.remember_active = lambda: None
            module.load_state = lambda: {}
            sent = self.run_module(module, [{"type": "activate", "workspace": root, "settings": {}},
                                            {"type": "event", "name": "editorChanged", "path": query},
                                            {"type": "executeCommand", "id": "sql.runFile"}])
            trees = [m for m in sent if m["type"] == "setTree"]
            connections = trees[-1]["items"][0]["children"]
            self.assertEqual([c["label"] for c in connections], ["app.db  ✓"])
            status = [m for m in sent if m["type"] == "status"][-1]
            self.assertEqual(status["text"], "SQL: app.db")
            run = [m for m in sent if m["type"] == "runInTerminal"][-1]
            self.assertTrue(run["saveAll"])
            spec_path = run["command"].split()[-1].strip("'")
            with open(spec_path, encoding="utf-8") as f:
                spec = json.load(f)
            os.remove(spec_path)
            self.assertEqual((spec["mode"], spec["file"], spec["connection"]["driver"]), ("run", query, "sqlite"))

    def test_loading_tables_and_inserting_names(self):
        with tempfile.TemporaryDirectory() as root:
            make_db(os.path.join(root, "app.db"))
            module = sql.Module()
            module.remember_active = lambda: None
            module.load_state = lambda: {}
            self.run_module(module, [{"type": "activate", "workspace": root, "settings": {}}])
            cid = module.connections[0]["id"]
            with redirect_stdout(io.StringIO()):
                module.load(cid)
                for _ in range(100):
                    if cid in module.schemas:
                        break
                    import time
                    time.sleep(0.02)
            sent = self.run_module(module, [
                {"type": "treeAction", "view": "sql", "item": f"table:{cid}:1", "action": "insert"},
                {"type": "treeAction", "view": "sql", "item": f"col:{cid}:0:1", "action": "click"}])
            self.assertEqual([m["text"] for m in sent if m["type"] == "insertText"], ['"order items"', "name"])
            tree = module.tree()
            tables = tree["items"][0]["children"][0]["children"]
            self.assertEqual([t["label"] for t in tables], ["customers", "order items", "named"])

    def test_formatter_requests(self):
        module = sql.Module()
        sent = self.run_module(module, [{"type": "format", "requestId": 7, "path": "/a.sql", "text": "select 1"}])
        self.assertEqual(sent, [{"type": "formatResult", "requestId": 7, "text": "SELECT 1\n"}])


class Manifest(unittest.TestCase):
    def test_manifest(self):
        manifest = load_manifest()
        self.assertEqual(manifest["id"], "byteforge.sql")
        self.assertEqual(manifest["views"][0]["id"], sql.VIEW)
        for rule in manifest["languages"][0]["rules"]:
            re.compile(rule["match"])

    def test_launcher_runs_sqlite_from_a_spec(self):
        with tempfile.TemporaryDirectory() as root:
            path = os.path.join(root, "app.db")
            make_db(path)
            spec = sql.write_spec({"connection": {"driver": "sqlite", "file": path, "name": "app"}, "mode": "run",
                                   "query": "SELECT count(*) AS n FROM customers;"})
            out = io.StringIO()
            with redirect_stdout(out):
                code = sql.launch(spec)
            self.assertEqual(code, 0)
            self.assertFalse(os.path.exists(spec))
            self.assertIn("(1 row)", out.getvalue())


if __name__ == "__main__":
    unittest.main()
