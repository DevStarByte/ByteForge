"""Tests for the HTTP Client module: python3 -m unittest discover -s modules/http-client -p 'test_*.py' -v"""

import http.server
import io
import json
import os
import re
import sys
import tempfile
import threading
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import http_client as hc  # noqa: E402

SAMPLE = """@host = http://localhost:5000
@api = {{host}}/api

### Log in
# @name login
POST {{api}}/login HTTP/1.1
Content-Type: application/json
# a comment between headers

{
  "user": "admin"
}

> {%
    client.global.set("token", response.body.token);
%}

###
GET {{api}}/orders
    ?page=1
    &size=20
Authorization: Bearer {{login.response.body.$.token}}

###
// @name upload
PUT https://example.com/files
Content-Type: application/octet-stream

< ./data.bin

### Just a URL
https://example.com/health
"""


class ParseTests(unittest.TestCase):
    def setUp(self):
        self.variables, self.requests = hc.parse(SAMPLE)

    def test_variables(self):
        self.assertEqual(self.variables, {"host": "http://localhost:5000", "api": "{{host}}/api"})

    def test_requests(self):
        self.assertEqual([r["method"] for r in self.requests], ["POST", "GET", "PUT", "GET"])
        login, orders, upload, health = self.requests
        self.assertEqual(login["name"], "login")
        self.assertEqual(login["title"], "Log in")
        self.assertEqual(login["url"], "{{api}}/login")
        self.assertEqual(login["headers"], [("Content-Type", "application/json")])
        self.assertEqual(json.loads(login["body"]), {"user": "admin"})
        self.assertEqual(orders["url"], "{{api}}/orders?page=1&size=20")
        self.assertEqual(orders["headers"][0][0], "Authorization")
        self.assertIsNone(orders["body"])
        self.assertEqual(upload["name"], "upload")
        self.assertEqual(upload["bodyFile"], "./data.bin")
        self.assertIsNone(upload["body"])
        self.assertEqual(health["url"], "https://example.com/health")

    def test_lines_are_zero_based_request_lines(self):
        lines = SAMPLE.splitlines()
        for r in self.requests:
            self.assertIn(r["url"].split("?")[0], lines[r["line"]])

    def test_labels(self):
        self.assertEqual([hc.label(r) for r in self.requests], ["Log in", "GET /orders?page=1&size=20", "upload", "Just a URL"])

    def test_empty_and_comment_only_blocks(self):
        self.assertEqual(hc.parse("# nothing\n\n###\n// still nothing\n")[1], [])


class ResolverTests(unittest.TestCase):
    def resolver(self, variables=None, env=None, responses=None):
        return hc.Resolver(variables or {}, env or {}, responses or {}, os.path.join(HERE, "x.http"), HERE)

    def test_nested_file_and_environment_variables(self):
        r = self.resolver({"api": "{{host}}/api"}, {"host": "https://h", "port": 5})
        self.assertEqual(r.resolve("{{api}}/x:{{port}}"), "https://h/api/x:5")

    def test_file_variables_win_over_environment(self):
        self.assertEqual(self.resolver({"a": "file"}, {"a": "env"}).resolve("{{a}}"), "file")

    def test_dynamic_variables(self):
        r = self.resolver()
        self.assertRegex(r.resolve("{{$guid}}"), r"^[0-9a-f-]{36}$")
        self.assertTrue(10 <= int(r.resolve("{{$randomInt 10 20}}")) < 20)
        self.assertRegex(r.resolve("{{$timestamp}}"), r"^\d{10}$")
        self.assertRegex(r.resolve("{{$datetime iso8601}}"), r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertRegex(r.resolve("{{$datetime rfc1123}}"), r"GMT$")
        self.assertRegex(r.resolve('{{$datetime "YYYY-MM-DD" 1 d}}'), r"^\d{4}-\d\d-\d\d$")
        os.environ["BF_HTTP_TEST"] = "secret"
        self.assertEqual(r.resolve("{{$processEnv BF_HTTP_TEST}}"), "secret")

    def test_undefined_and_recursive_variables_fail(self):
        with self.assertRaises(hc.VariableError):
            self.resolver().resolve("{{nope}}")
        with self.assertRaises(hc.VariableError):
            self.resolver({"a": "{{a}}"}).resolve("{{a}}")

    def test_request_variables(self):
        responses = {"login": {"headers": [("Location", "/x")], "text": '{"token": "t1", "items": [{"id": 1}, {"id": 2}]}',
                               "request": {"headers": [], "body": ""}}}
        r = self.resolver(responses=responses)
        self.assertEqual(r.resolve("{{login.response.body.$.token}}"), "t1")
        self.assertEqual(r.resolve("{{login.response.body.$.items[1].id}}"), "2")
        self.assertEqual(r.resolve("{{login.response.body.$.items[*].id}}"), "[1, 2]")
        self.assertEqual(r.resolve("{{login.response.headers.location}}"), "/x")
        with self.assertRaises(hc.VariableError):
            r.resolve("{{other.response.body.$.a}}")


class EnvironmentTests(unittest.TestCase):
    def test_nearest_env_file_with_shared_and_private(self):
        with tempfile.TemporaryDirectory() as root:
            api = os.path.join(root, "src", "Api")
            os.makedirs(api)
            with open(os.path.join(root, "http-client.env.json"), "w") as f:
                json.dump({"$shared": {"version": "v1"}, "dev": {"host": "http://localhost"}, "prod": {"host": "https://p"}}, f)
            with open(os.path.join(root, "http-client.private.env.json"), "w") as f:
                json.dump({"dev": {"password": "pw"}}, f)
            envs = hc.load_environments(os.path.join(api, "api.http"), root)
            self.assertEqual(envs["dev"], {"version": "v1", "host": "http://localhost", "password": "pw"})
            self.assertEqual(envs["prod"], {"version": "v1", "host": "https://p"})
            client = hc.HttpClient()
            self.assertEqual(client.current_environment(envs), "dev")

    def test_dotenv(self):
        with tempfile.TemporaryDirectory() as root:
            with open(os.path.join(root, ".env"), "w") as f:
                f.write('export TOKEN="abc"\n# comment\nOTHER=1\n')
            r = hc.Resolver({}, {}, {}, os.path.join(root, "a.http"), root)
            self.assertEqual(r.resolve("{{$dotenv TOKEN}}-{{$dotenv OTHER}}"), "abc-1")


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, body, ctype="application/json", extra=()):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"{}")
        self.reply(200, json.dumps({"token": "tok-" + body.get("user", "?")}))

    def do_GET(self):
        if self.path == "/missing":
            self.reply(404, '{"title":"Not Found"}', "application/problem+json")
        elif self.path == "/redirect":
            self.reply(302, "", "text/plain", [("Location", "/orders")])
        elif self.path == "/image":
            self.reply(200, b"\x89PNG\r\n\x1a\n\0\0", "image/png")
        else:
            self.reply(200, json.dumps({"auth": self.headers.get("Authorization"), "path": self.path}))


class SendTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.base = f"http://127.0.0.1:{cls.server.server_address[1]}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        os.environ["XDG_CACHE_HOME"] = os.path.join(self.tmp.name, "cache")
        self.path = os.path.join(self.tmp.name, "api.http")
        with open(self.path, "w") as f:
            f.write(f"""@host = {self.base}

# @name login
POST {{{{host}}}}/login
Content-Type: application/json

{{"user": "{{{{$processEnv USER_FOR_TEST}}}}"}}

###
GET {{{{host}}}}/orders?x=1
Authorization: Bearer {{{{login.response.body.$.token}}}}

###
GET {{{{host}}}}/missing

###
GET {{{{host}}}}/redirect

###
GET {{{{host}}}}/image
""")
        os.environ["USER_FOR_TEST"] = "ada"
        self.client = hc.HttpClient()
        self.client.root = self.tmp.name
        self.requests = hc.parse(hc.read(self.path))[1]

    def tearDown(self):
        self.tmp.cleanup()

    def send(self, index):
        out = io.StringIO()
        with redirect_stdout(out):
            response = self.client._send_inner(self.path, self.requests[index]["line"])
        messages = [json.loads(line) for line in out.getvalue().splitlines()]
        return response, messages

    def opened(self, messages):
        path = next(m["path"] for m in messages if m["type"] == "openFile")
        with open(path) as f:
            return path, f.read()

    def test_chained_requests(self):
        response, messages = self.send(0)
        self.assertEqual(response["status"], 200)
        _, document = self.opened(messages)
        self.assertRegex(document, r"(?m)^HTTP/1\.[01] 200 OK$")
        self.assertIn('"token": "tok-ada"', document)  # pretty-printed
        response, messages = self.send(1)
        _, document = self.opened(messages)
        self.assertIn('"auth": "Bearer tok-ada"', document)
        self.assertIn('"path": "/orders?x=1"', document)

    def test_missing_named_response_is_a_variable_error(self):
        response, messages = self.send(1)
        self.assertIsNone(response)
        self.assertTrue(any("send the request named login first" in m.get("text", "") for m in messages))

    def test_error_status_and_redirects(self):
        response, _ = self.send(2)
        self.assertEqual(response["status"], 404)
        response, _ = self.send(3)
        self.assertEqual(response["status"], 200)
        self.client.settings = {"http.followRedirects": False}
        response, _ = self.send(3)
        self.assertEqual(response["status"], 302)

    def test_binary_body_is_saved_next_to_the_response(self):
        _, messages = self.send(4)
        path, document = self.opened(messages)
        self.assertIn("image/png", document)
        self.assertTrue(os.path.exists(path[:-5] + ".png"))

    def test_connection_failure_is_logged(self):
        with open(self.path, "w") as f:
            f.write("GET http://127.0.0.1:9/nothing\n")
        self.requests = hc.parse(hc.read(self.path))[1]
        response, messages = self.send(0)
        self.assertIsNone(response)
        self.assertTrue(any(m.get("text", "").startswith("http: GET http://127.0.0.1:9/nothing:") for m in messages))

    def test_tree_lists_requests_with_results(self):
        self.send(0)
        out = io.StringIO()
        with redirect_stdout(out):
            self.client.refresh()
        tree = [json.loads(line) for line in out.getvalue().splitlines() if '"setTree"' in line][-1]
        file_item = tree["items"][0]
        self.assertEqual(file_item["label"], "api.http")
        self.assertEqual(file_item["count"], 5)
        self.assertTrue(file_item["children"][0]["description"].startswith("200 OK"))


class PruneTests(unittest.TestCase):
    def test_keeps_the_newest_responses_and_their_binaries(self):
        with tempfile.TemporaryDirectory() as folder:
            for i in range(hc.KEEP_RESPONSES + 3):
                open(os.path.join(folder, f"{i:04}-r.http"), "w").close()
            open(os.path.join(folder, "0000-r.png"), "w").close()
            hc.prune(folder)
            names = sorted(os.listdir(folder))
            self.assertEqual(len(names), hc.KEEP_RESPONSES)
            self.assertEqual(names[0], "0003-r.http")


class ManifestTests(unittest.TestCase):
    def test_manifest_rules_compile_without_lookaround(self):
        with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
            manifest = json.loads(re.sub(r"(?m)^\s*//.*$", "", f.read()))
        for rule in manifest["languages"][0]["rules"] + [{"match": d} for d in manifest["languages"][0]["detect"]]:
            re.compile(rule["match"])
            self.assertNotRegex(rule["match"], r"\(\?<?[=!]", "Rust's regex crate has no lookaround")
        self.assertEqual({s["key"].split(".")[0] for s in manifest["settings"]}, {"http"})


if __name__ == "__main__":
    unittest.main()
