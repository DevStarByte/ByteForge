"""Tests for the Aspire module: python3 -m unittest discover -s modules/aspire -p 'test_*.py' -v"""

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

import aspire as asp  # noqa: E402


def load_manifest():
    with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
        text = f.read()
    return json.loads(re.sub(r"^\s*//.*$", "", text, flags=re.M))


def write(path, text=""):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


APPHOST_CSPROJ = """<Project Sdk="Microsoft.NET.Sdk">
  <Sdk Name="Aspire.AppHost.Sdk" Version="13.0.0" />
  <PropertyGroup><OutputType>Exe</OutputType></PropertyGroup>
</Project>"""

OLD_APPHOST_CSPROJ = """<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><IsAspireHost>true</IsAspireHost></PropertyGroup>
</Project>"""

WEB_CSPROJ = '<Project Sdk="Microsoft.NET.Sdk.Web"><PropertyGroup /></Project>'

PS_OUTPUT = """[
  {"appHostPath": "/src/Shop/Shop.AppHost/Shop.AppHost.csproj", "appHostPid": 4242, "cliPid": 4200,
   "dashboardUrl": "https://localhost:17043/login?t=abc"}
]"""

DESCRIBE_OUTPUT = json.dumps({"resources": [
    {"name": "webfrontend", "displayName": "webfrontend", "resourceType": "Project", "state": "Running",
     "stateStyle": "success", "healthStatus": "Healthy", "exitCode": None,
     "urls": [{"name": "https", "displayName": None, "url": "https://localhost:7001", "isInternal": False},
              {"name": "http", "displayName": None, "url": "http://localhost:5001", "isInternal": True}],
     "commands": {"stop": {"state": "Enabled"}, "restart": {"state": "Enabled"},
                  "start": {"state": "Hidden", "visibility": "Hidden"}}},
    {"name": "cache-xyz", "displayName": "cache", "resourceType": "Container", "state": "Exited",
     "stateStyle": "error", "exitCode": 1, "urls": None,
     "commands": {"start": {"state": "Enabled"}, "stop": {"state": "Disabled"}}},
]})


class ParsingTests(unittest.TestCase):
    def test_find_json_skips_banners(self):
        self.assertEqual(asp.find_json("Aspire CLI 13.0\n[1, 2]\n"), [1, 2])
        self.assertEqual(asp.find_json('Note: [preview]\n{"a": 1}'), {"a": 1})
        with self.assertRaises(asp.AspireError):
            asp.find_json("no json here")

    def test_parse_ps(self):
        hosts = asp.parse_ps(PS_OUTPUT)
        self.assertEqual(len(hosts), 1)
        self.assertEqual(hosts[0]["path"], "/src/Shop/Shop.AppHost/Shop.AppHost.csproj")
        self.assertEqual(hosts[0]["pid"], 4242)
        self.assertEqual(hosts[0]["dashboard"], "https://localhost:17043/login?t=abc")

    def test_parse_ps_single_object_and_empty(self):
        self.assertEqual(len(asp.parse_ps('{"appHostPath": "/a/apphost.ts", "appHostPid": 1}')), 1)
        self.assertEqual(asp.parse_ps("[]"), [])
        self.assertEqual(asp.parse_ps(""), [])
        self.assertEqual(asp.parse_ps('[{"nope": 1}]'), [])

    def test_parse_resources_sorted_by_display_name(self):
        resources = asp.parse_resources(DESCRIBE_OUTPUT)
        self.assertEqual([asp.display_name(r) for r in resources], ["cache", "webfrontend"])

    def test_commands(self):
        web, cache = asp.parse_resources(DESCRIBE_OUTPUT)[1], asp.parse_resources(DESCRIBE_OUTPUT)[0]
        self.assertTrue(asp.command_enabled(web, "stop"))
        self.assertTrue(asp.command_enabled(web, "restart"))
        self.assertFalse(asp.command_enabled(web, "start"))
        self.assertTrue(asp.command_enabled(cache, "start"))
        self.assertFalse(asp.command_enabled(cache, "stop"))
        self.assertTrue(asp.command_enabled({"commands": {"resource-stop": {}}}, "stop"))
        self.assertFalse(asp.command_enabled({"commands": None}, "stop"))

    def test_status(self):
        self.assertEqual(asp.resource_status({"state": "Running"}), "running")
        self.assertEqual(asp.resource_status({"state": "Running", "healthStatus": "Unhealthy"}), "warning")
        self.assertEqual(asp.resource_status({"state": "Running", "stateStyle": "error"}), "error")
        self.assertEqual(asp.resource_status({"state": "Starting"}), "busy")
        self.assertEqual(asp.resource_status({"state": "Exited", "exitCode": 0}), "idle")
        self.assertEqual(asp.resource_status({"state": "Exited", "exitCode": 137}), "error")
        self.assertEqual(asp.resource_status({"state": "FailedToStart"}), "warning")
        self.assertEqual(asp.resource_status({"state": None}), "idle")

    def test_urls_hide_internal_ones(self):
        web = asp.parse_resources(DESCRIBE_OUTPUT)[1]
        self.assertEqual(asp.resource_urls(web), [("https", "https://localhost:7001")])


class WorkspaceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def test_finds_project_and_file_apphosts(self):
        write(os.path.join(self.root, "Shop.AppHost", "Shop.AppHost.csproj"), APPHOST_CSPROJ)
        write(os.path.join(self.root, "Shop.AppHost", "AppHost.cs"), "var builder = DistributedApplication.CreateBuilder(args);")
        write(os.path.join(self.root, "Legacy", "Legacy.AppHost.csproj"), OLD_APPHOST_CSPROJ)
        write(os.path.join(self.root, "Web", "Web.csproj"), WEB_CSPROJ)
        write(os.path.join(self.root, "single", "apphost.cs"), "#:sdk Aspire.AppHost.Sdk@13.0.0\n")
        write(os.path.join(self.root, "ts", "apphost.ts"), "import { createBuilder } from '@aspire/apphost';")
        write(os.path.join(self.root, "other", "apphost.ts"), "console.log('unrelated');")
        write(os.path.join(self.root, "Web", "bin", "Debug", "Copy.AppHost.csproj"), APPHOST_CSPROJ)
        found = sorted(os.path.relpath(p, self.root) for p in asp.find_apphosts(self.root))
        self.assertEqual(found, sorted([
            os.path.join("Legacy", "Legacy.AppHost.csproj"),
            os.path.join("Shop.AppHost", "Shop.AppHost.csproj"),
            os.path.join("single", "apphost.cs"),
            os.path.join("ts", "apphost.ts"),
        ]))

    def test_no_workspace(self):
        self.assertEqual(asp.find_apphosts(None), [])
        self.assertEqual(asp.find_apphosts(os.path.join(self.root, "missing")), [])

    def test_labels_and_source(self):
        project = os.path.join(self.root, "Shop.AppHost", "Shop.AppHost.csproj")
        write(project, APPHOST_CSPROJ)
        self.assertEqual(asp.apphost_label(project), "Shop.AppHost")
        self.assertEqual(asp.apphost_label(os.path.join(self.root, "frontend", "apphost.ts")), "frontend")
        self.assertEqual(asp.source_file(project), project)
        write(os.path.join(self.root, "Shop.AppHost", "AppHost.cs"))
        self.assertEqual(asp.source_file(project), os.path.join(self.root, "Shop.AppHost", "AppHost.cs"))

    def test_find_cli_from_setting(self):
        fake = os.path.join(self.root, "aspire")
        write(fake)
        self.assertEqual(asp.find_cli({"aspire.cliPath": fake}), fake)
        self.assertIsNone(asp.find_cli({"aspire.cliPath": os.path.join(self.root, "nope")}))


class TreeTests(unittest.TestCase):
    def module(self):
        module = asp.Module()
        module.workspace = "/src/Shop"
        module.cli = "/usr/bin/aspire"
        return module

    def test_stopped_apphost_offers_run(self):
        module = self.module()
        module.apphosts = ["/src/Shop/Shop.AppHost/Shop.AppHost.csproj"]
        tree = module.tree()
        host = tree["items"][0]["children"][0]
        self.assertEqual(host["label"], "Shop.AppHost")
        self.assertEqual([a["id"] for a in host["actions"]], ["run"])
        self.assertEqual(tree["badge"], 0)

    def test_running_apphost_lists_resources(self):
        module = self.module()
        path = "/src/Shop/Shop.AppHost/Shop.AppHost.csproj"
        module.apphosts = [path]
        host = asp.parse_ps(PS_OUTPUT)[0]
        host["resources"], host["error"] = asp.parse_resources(DESCRIBE_OUTPUT), None
        module.running = {asp.path_key(path): host}
        tree = module.tree()
        item = tree["items"][0]["children"][0]
        self.assertEqual([a["id"] for a in item["actions"]], ["dashboard", "stop"])
        self.assertEqual(item["description"], "running · 1/2 up")
        labels = [c["label"] for c in item["children"]]
        self.assertEqual(labels, ["Dashboard", "cache", "webfrontend"])
        web = item["children"][2]
        self.assertEqual([a["id"] for a in web["actions"]], ["restart", "stop", "logs"])
        self.assertEqual(web["children"][0]["description"], "https://localhost:7001")
        self.assertEqual(tree["badge"], 1)

    def test_running_apphost_outside_the_workspace_is_listed(self):
        module = self.module()
        host = asp.parse_ps('[{"appHostPath": "/elsewhere/apphost.ts", "appHostPid": 7}]')[0]
        host["resources"], host["error"] = [], None
        module.running = {asp.path_key(host["path"]): host}
        children = module.tree()["items"][0]["children"]
        self.assertEqual(children[0]["label"], "elsewhere")
        self.assertEqual(children[0]["children"][0]["label"], "Waiting for resources…")

    def test_missing_cli_shows_setup(self):
        module = self.module()
        module.cli = False
        tree = module.tree()
        self.assertEqual(tree["items"][0]["label"], "SETUP")
        self.assertIn("isn't installed", tree["message"])

    def test_resource_action_runs_the_cli_command(self):
        module = self.module()
        calls = []
        module.run_op = lambda label, args, then=None: calls.append(args)
        module.tree_action("res:/a/AppHost.csproj\ncache-xyz", "restart")
        self.assertEqual(calls, [["resource", "cache-xyz", "restart", "--non-interactive", "--apphost",
                                  "/a/AppHost.csproj"]])

    def test_run_opens_a_terminal(self):
        module = self.module()
        module.require_cli = lambda: "/usr/bin/aspire"
        out = io.StringIO()
        with redirect_stdout(out):
            module.tree_action("host:/src/Shop/Shop.AppHost/Shop.AppHost.csproj", "run")
        message = json.loads(out.getvalue().splitlines()[0])
        self.assertEqual(message["type"], "runInTerminal")
        self.assertEqual(message["command"],
                         "/usr/bin/aspire run --apphost /src/Shop/Shop.AppHost/Shop.AppHost.csproj")
        self.assertEqual(message["cwd"], "/src/Shop/Shop.AppHost")


class ManifestTests(unittest.TestCase):
    def test_manifest(self):
        manifest = load_manifest()
        self.assertEqual(manifest["id"], "byteforge.aspire")
        self.assertEqual(manifest["args"], ["aspire.py"])
        self.assertEqual(manifest["views"][0]["id"], asp.VIEW)


if __name__ == "__main__":
    unittest.main()
