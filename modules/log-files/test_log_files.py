"""Tests for the Log Files module: python3 -m unittest discover -s modules/log-files -p 'test_*.py' -v"""

import io
import json
import os
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import log_files as lf  # noqa: E402

RED, YELLOW, BLUE, DIM = "regex - character class", "string - escape character", "keyword", "excluded code"

ASPNET = """info: Microsoft.Hosting.Lifetime[14]
      Now listening on: http://localhost:5031
warn: Microsoft.AspNetCore.HttpsPolicy.HttpsRedirectionMiddleware[3]
      Failed to determine the https port for redirect.
fail: Microsoft.AspNetCore.Diagnostics.ExceptionHandlerMiddleware[1]
      An unhandled exception has occurred while executing the request.
      System.InvalidOperationException: Something broke on purpose.
         at Program.<>c.<<Main>$>b__0_9() in /src/ModuleTest/Program.cs:line 71
         at lambda_method12(Closure, Object, HttpContext)
dbug: Microsoft.AspNetCore.Routing.Matching.DfaMatcher[1001]
      1 candidate(s) found for the request path '/boom'
info: Microsoft.Hosting.Lifetime[0]
      Application is shutting down..."""

SERILOG = """[10:00:01 INF] Starting up
[10:00:02 DBG] Config loaded from appsettings.json
[10:00:03 WRN] Slow query took 1200 ms
[10:00:04 ERR] Order 42 failed
System.Data.SqlClient.SqlException (0x80131904): Timeout expired.
   at System.Data.SqlClient.SqlConnection.OnError(SqlException exception)
[10:00:05 INF] Retrying"""

PLAIN = """2026-11-20 10:00:00.123 INFO  Server started on 0.0.0.0:8080
2026-11-20 10:00:01.456 ERROR Request GET /api/orders failed with status 500
2026-11-20 10:00:02.789 INFO  an error occurred? no: this is just a message about error handling
2026-11-20 10:00:03.000 FATAL Out of memory"""


def manifest():
    with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
        return json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))


def token_of(line, text, nth=0):
    """The token the IDE gives `text` (its nth occurrence) on `line`: the earliest rule covering it wins."""
    start = [m.start() for m in re.finditer(re.escape(text), line)][nth]
    for rule in manifest()["languages"][0]["rules"]:
        for m in re.finditer(rule["match"], line):
            g = rule.get("group", 0)
            if m.start(g) <= start < m.end(g):
                return rule["token"]
    return None


class HighlightTests(unittest.TestCase):
    def test_aspnet_levels(self):
        self.assertEqual(token_of("fail: Microsoft.AspNetCore.Diagnostics[1]", "fail"), RED)
        self.assertEqual(token_of("warn: Microsoft.AspNetCore.HttpsPolicy[3]", "warn"), YELLOW)
        self.assertEqual(token_of("info: Microsoft.Hosting.Lifetime[14]", "info"), BLUE)
        self.assertEqual(token_of("dbug: Microsoft.AspNetCore.Routing[1001]", "Routing"), DIM)

    def test_serilog_and_plain_levels(self):
        self.assertEqual(token_of("[10:00:04 ERR] Order 42 failed", "ERR"), RED)
        self.assertEqual(token_of("[10:00:03 WRN] Slow query", "WRN"), YELLOW)
        self.assertEqual(token_of("[10:00:02 DBG] Config loaded", "Config"), DIM)
        line = "2026-11-20 10:00:03.000 FATAL Out of memory"
        self.assertEqual(token_of(line, "FATAL"), RED)
        self.assertEqual(token_of(line, "2026-11-20"), "comment")
        self.assertEqual(token_of("2026-11-20 | Warning | Disk almost full", "Warning"), YELLOW)

    def test_words_in_messages_stay_plain(self):
        line = "2026-11-20 10:00:02 INFO  an error occurred? no: about error handling"
        self.assertIsNone(token_of(line, "error"))
        self.assertEqual(token_of(line, "INFO"), BLUE)

    def test_stack_frames_and_exceptions(self):
        frame = "   at Program.Main(String[] args) in /src/App/Program.cs:line 71"
        self.assertEqual(token_of(frame, "Program.Main"), "method name")
        self.assertEqual(token_of(frame, "/src/App/Program.cs"), "string")
        self.assertEqual(token_of(frame, "71"), "number")
        self.assertEqual(token_of("System.InvalidOperationException: broke", "System.InvalidOperationException"), "class name")
        self.assertEqual(token_of('  File "/app/main.py", line 3, in <module>', "/app/main.py"), "string")
        self.assertEqual(token_of("##[error]Process completed with exit code 1.", "Process"), RED)

    def test_values(self):
        line = "GET /api/orders status=503 from 10.0.0.7:5000 id 3f2b9c1e-1111-2222-3333-444455556666 http://x.io/a"
        self.assertEqual(token_of(line, "GET"), "keyword - control")
        self.assertEqual(token_of(line, "503"), RED)
        self.assertEqual(token_of(line, "10.0.0.7"), "constant name")
        self.assertEqual(token_of(line, "3f2b9c1e"), "constant name")
        self.assertEqual(token_of(line, "http://x.io/a"), "string")

    def test_rules_have_no_lookaround(self):
        language = manifest()["languages"][0]
        for pattern in [r["match"] for r in language["rules"]] + language["detect"]:
            re.compile(pattern)
            self.assertNotRegex(pattern, r"\(\?<?[=!]", "Rust's regex crate has no lookaround")


class LevelTests(unittest.TestCase):
    def test_level_of(self):
        self.assertEqual(lf.level_of("fail: Microsoft.AspNetCore[1]"), "error")
        self.assertEqual(lf.level_of("[10:00:03 WRN] Slow"), "warning")
        self.assertEqual(lf.level_of("2026-11-20 10:00:00 INFO  Started"), "info")
        self.assertEqual(lf.level_of("2026-11-20 10:00:00 | Error | x"), "error")
        self.assertEqual(lf.level_of("##[error]exit code 1"), "error")
        self.assertIsNone(lf.level_of("      Now listening on: http://localhost:5031"))

    def test_entries_keep_stack_traces_together(self):
        blocks = list(lf.entries(ASPNET.split("\n")))
        self.assertEqual([b[1] for b in blocks], ["info", "warning", "error", "debug", "info"])
        error = blocks[2]
        self.assertEqual(len(error[2]), 5)
        self.assertEqual(lf.exception_of(error[2]), "System.InvalidOperationException")
        self.assertTrue(lf.first_frame(error[2]).startswith("at Program."))

    def test_serilog_exception_lines_belong_to_the_error(self):
        blocks = list(lf.entries(SERILOG.split("\n")))
        error = next(b for b in blocks if b[1] == "error")
        self.assertEqual(len(error[2]), 3)

    def test_list_errors_points_at_the_entry(self):
        out = lf.list_errors("/x/app.log", ASPNET.split("\n"))
        self.assertEqual(len(out), 1)
        self.assertTrue(out[0].startswith("/x/app.log:5:1: fail: Microsoft.AspNetCore.Diagnostics"))
        self.assertIn("System.InvalidOperationException", out[0])
        out = lf.list_errors("/x/plain.log", PLAIN.split("\n"))
        self.assertEqual([o.split(":")[1] for o in out], ["2", "4"])

    def test_filters(self):
        lines = ASPNET.split("\n")
        self.assertEqual(lf.apply_filter(lines, "errors")[0], "fail: Microsoft.AspNetCore.Diagnostics.ExceptionHandlerMiddleware[1]")
        self.assertEqual(len(lf.apply_filter(lines, "warnings")), 7)
        self.assertNotIn("dbug", "\n".join(lf.apply_filter(lines, "nodebug")))
        self.assertIn("SqlException", "\n".join(lf.apply_filter(SERILOG.split("\n"), "exceptions")))

    def test_counts(self):
        self.assertEqual(lf.counts(PLAIN.split("\n")),
                         {"trace": 0, "debug": 0, "info": 2, "warning": 0, "error": 1, "fatal": 1})


class ModuleTests(unittest.TestCase):
    def test_status_and_filter_file(self):
        with tempfile.TemporaryDirectory() as folder:
            os.environ["XDG_CACHE_HOME"] = folder
            path = os.path.join(folder, "app.log.1")
            with open(path, "w") as f:
                f.write(ASPNET)
            module = lf.LogModule()
            out = io.StringIO()
            with redirect_stdout(out):
                module.handle({"type": "event", "name": "editorChanged", "path": path})
                module.filter("errors")
            messages = [json.loads(l) for l in out.getvalue().splitlines()]
            status = next(m for m in messages if m["type"] == "status")
            self.assertEqual(status["text"], "✗ 1  ⚠ 1")
            opened = next(m for m in messages if m["type"] == "openFile")["path"]
            self.assertTrue(opened.endswith(".log"))
            with open(opened) as f:
                self.assertTrue(f.read().startswith("fail:"))

    def test_follow_command(self):
        module = lf.LogModule()
        module.active = "/var/log/my app.log"
        out = io.StringIO()
        with redirect_stdout(out):
            module.follow()
        command = json.loads(out.getvalue())["command"]
        if os.name != "nt":
            self.assertEqual(command, "tail -n 200 -F '/var/log/my app.log'")

    def test_is_log(self):
        for name in ("app.log", "app.log.1", "app.log.2026-11-20", "build.out"):
            self.assertTrue(lf.is_log("/x/" + name), name)
        self.assertFalse(lf.is_log("/x/catalog.json"))


if __name__ == "__main__":
    unittest.main()
