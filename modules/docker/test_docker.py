"""Tests for the Docker module: python3 -m unittest discover -s modules/docker -p 'test_*.py' -v"""

import json
import os
import re
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import docker as dk  # noqa: E402


def load_manifest():
    with open(os.path.join(HERE, "module.json"), encoding="utf-8") as f:
        text = f.read()
    return json.loads(re.sub(r"^\s*//.*$", "", text, flags=re.M))


def highlight(rules, line):
    """Per-character token of `line`, the earlier rule winning where matches overlap (what the IDE does)."""
    tokens = [None] * len(line)
    for rule in rules:
        group = rule.get("group", 0)
        for m in re.finditer(rule["match"], line):
            if m.group(group) is None:
                continue
            for i in range(m.start(group), m.end(group)):
                if tokens[i] is None:
                    tokens[i] = rule["token"]
    return tokens


class ParseTests(unittest.TestCase):
    def test_parses_containers_and_images(self):
        ps = ('{"Command":"\\"dotnet Api.dll\\"","CreatedAt":"2026-11-07 10:00:00 +0100 CET","ID":"0123456789abcdef0123",'
              '"Image":"api","Labels":"com.docker.compose.project=shop,com.docker.compose.service=api,x=1","Names":"shop-api-1",'
              '"Ports":"0.0.0.0:8080->8080/tcp","State":"running","Status":"Up 5 minutes"}\n'
              '{"ID":"fedcba987654","Image":"postgres:17","Labels":"","Names":"db","Ports":"","Status":"Exited (0) 2 hours ago"}\n'
              'not json')
        containers = dk.parse_containers(ps)
        self.assertEqual(len(containers), 2)
        api = containers[0]
        self.assertEqual(api["id"], "0123456789ab")
        self.assertEqual(api["name"], "shop-api-1")
        self.assertTrue(dk.is_running(api))
        self.assertEqual(api["compose_project"], "shop")
        self.assertEqual(api["compose_service"], "api")
        db = containers[1]
        self.assertEqual(db["state"], "exited")
        self.assertIsNone(db["compose_project"])

        images = dk.parse_images(
            '{"Containers":"N/A","CreatedAt":"","CreatedSince":"2 days ago","ID":"sha256:abcdef0123456789","Repository":"<none>","Tag":"<none>","Size":"220MB"}\n'
            '{"CreatedSince":"3 weeks ago","ID":"1234567890ab","Repository":"mcr.microsoft.com/dotnet/aspnet","Tag":"9.0","Size":"225MB"}')
        self.assertEqual(len(images), 2)
        self.assertEqual(dk.image_reference(images[0]), "mcr.microsoft.com/dotnet/aspnet:9.0")
        self.assertEqual(dk.image_reference(images[1]), "abcdef012345")

    def test_state_from_status_and_bad_input(self):
        self.assertEqual(dk.state_from_status("Up 2 minutes (Paused)"), "paused")
        self.assertEqual(dk.state_from_status("Restarting (1) 3 seconds ago"), "restarting")
        self.assertEqual(dk.state_from_status("Created"), "created")
        self.assertEqual(dk.parse_containers('{"ID":1,"Names":"x","Image":"y"}\n[]\n{"Names":"x"}'), [])
        self.assertEqual(dk.parse_images('{"ID":"a","Repository":"r"}'), [])


class NameTests(unittest.TestCase):
    def test_names(self):
        self.assertTrue(dk.is_compose_file_name("compose.yaml"))
        self.assertTrue(dk.is_compose_file_name("docker-compose.override.yml"))
        self.assertFalse(dk.is_compose_file_name("compose.json"))
        self.assertFalse(dk.is_compose_file_name("my-compose.yml"))
        self.assertFalse(dk.is_compose_file_name("compose..yml"))  # stem "compose." has no variant
        self.assertEqual(dk.image_name("My.Shop Api"), "my.shop-api")
        self.assertEqual(dk.image_name("__"), "app")
        self.assertEqual(dk.image_name_for("/w/Shop.Api/Dockerfile"), "shop.api")
        self.assertEqual(dk.image_name_for("/w/Shop/Dockerfile.worker"), "shop-worker")
        self.assertEqual(dk.shell_quote("a b'c"), "'a b'\\''c'")
        self.assertEqual(dk.command_line(["docker", "build", "-t", "api", "/x y"]), "docker build -t api '/x y'")
        self.assertEqual(dk.shell_quote(""), "''")

    def test_dockerfile_names(self):
        for name in ("Dockerfile", "dockerfile", "Containerfile", "Dockerfile.worker", "api.dockerfile"):
            self.assertTrue(dk.is_dockerfile_name(name), name)
        for name in ("Dockerfile.", ".dockerfile", "Dockerfiles", "README.md"):
            self.assertFalse(dk.is_dockerfile_name(name), name)

    def test_shell_command(self):
        line = dk.command_line(["docker", "exec", "-it", "abc", "sh", "-c", dk.SHELL_SCRIPT])
        self.assertEqual(line, "docker exec -it abc sh -c 'command -v bash >/dev/null && exec bash || exec sh'")


class DotnetTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="bf-docker-")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def write(self, rel, text):
        path = os.path.join(self.dir, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)

    def p(self, rel):
        return os.path.join(self.dir, rel)

    def test_generates_dotnet_docker_files(self):
        self.write("Shop.slnx", "<Solution />")
        self.write("Directory.Packages.props", "<Project />")
        self.write("Shop.Api/Shop.Api.csproj", '<Project Sdk="Microsoft.NET.Sdk.Web"><PropertyGroup><TargetFrameworks>net8.0;net9.0;net9.0-windows</TargetFrameworks></PropertyGroup><ItemGroup><ProjectReference Include="..\\Shop.Core\\Shop.Core.csproj" /></ItemGroup></Project>')
        self.write("Shop.Core/Shop.Core.csproj", '<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><ProjectReference Include="../Shop.Data/Shop.Data.csproj" /></ItemGroup></Project>')
        self.write("Shop.Data/Shop.Data.csproj", '<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><ProjectReference Include="..\\Shop.Core\\Shop.Core.csproj" /></ItemGroup></Project>')
        self.write("Tool/Tool.csproj", '<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><AssemblyName>shoptool</AssemblyName></PropertyGroup></Project>')

        api = dk.DotnetProject.read(self.p("Shop.Api/Shop.Api.csproj"))
        self.assertEqual((api.assembly, api.version, api.web), ("Shop.Api", "9.0", True))
        self.assertEqual(api.references(), [self.p("Shop.Core/Shop.Core.csproj"), self.p("Shop.Data/Shop.Data.csproj")])
        text = dk.dotnet_dockerfile(api, self.dir)
        self.assertIn("FROM mcr.microsoft.com/dotnet/aspnet:9.0 AS base", text)
        self.assertIn('COPY ["Directory.Packages.props", "."]', text)
        self.assertIn('COPY ["Shop.Api/Shop.Api.csproj", "Shop.Api/"]', text)
        self.assertIn('COPY ["Shop.Data/Shop.Data.csproj", "Shop.Data/"]', text)
        self.assertIn('WORKDIR "/src/Shop.Api"', text)
        self.assertIn('ENTRYPOINT ["dotnet", "Shop.Api.dll"]', text)
        self.assertTrue(text.startswith("# Build:  docker build -f Shop.Api/Dockerfile -t shop.api .\n"
                                        "# (run from the build context: the folder that contains the solution)\n\n"), text)

        tool = dk.DotnetProject.read(self.p("Tool/Tool.csproj"))
        self.assertEqual((tool.assembly, tool.version, tool.web), ("shoptool", dk.DEFAULT_DOTNET_VERSION, False))
        text = dk.dotnet_dockerfile(tool, self.p("Tool"))
        self.assertIn("FROM mcr.microsoft.com/dotnet/runtime:", text)
        self.assertNotIn("EXPOSE", text)
        self.assertIn('WORKDIR "/src"\n', text)
        self.assertIn('RUN dotnet restore "./Tool.csproj"', text)
        self.assertEqual(text, (
            "# Build:  docker build -f ./Dockerfile -t shoptool .\n"
            "# (run from the build context: the folder that contains this file)\n\n"
            "FROM mcr.microsoft.com/dotnet/runtime:10.0 AS base\nUSER $APP_UID\nWORKDIR /app\n\n"
            "FROM mcr.microsoft.com/dotnet/sdk:10.0 AS build\nARG BUILD_CONFIGURATION=Release\nWORKDIR /src\n"
            'COPY ["Tool.csproj", "./"]\n'
            'RUN dotnet restore "./Tool.csproj"\nCOPY . .\nWORKDIR "/src"\n'
            'RUN dotnet build "./Tool.csproj" -c $BUILD_CONFIGURATION -o /app/build\n\n'
            "FROM build AS publish\nARG BUILD_CONFIGURATION=Release\n"
            'RUN dotnet publish "./Tool.csproj" -c $BUILD_CONFIGURATION -o /app/publish /p:UseAppHost=false\n\n'
            'FROM base AS final\nWORKDIR /app\nCOPY --from=publish /app/publish .\nENTRYPOINT ["dotnet", "shoptool.dll"]\n'))

        # A Dockerfile next to a project in a solution builds from the solution folder.
        self.write("Shop.Api/Dockerfile", dk.dotnet_dockerfile(api, self.dir))
        self.write("Tool/Dockerfile", "FROM scratch\n")
        self.write("Tool/.dockerignore", "bin\n")
        self.write("deploy/compose.yaml", "services: {}\n")
        self.assertEqual(dk.build_context(self.p("Shop.Api/Dockerfile"), self.dir), self.dir)
        self.assertEqual(dk.build_context(self.p("Tool/Dockerfile"), self.dir), self.p("Tool"))
        files = [self.p("Shop.Api/Dockerfile"), self.p("Tool/Dockerfile"), self.p("Tool/.dockerignore"), self.p("deploy/compose.yaml")]
        dockerfiles, compose = dk.docker_files(files, lambda n: n == "Dockerfile")
        self.assertEqual(dockerfiles, [self.p("Shop.Api/Dockerfile"), self.p("Tool/Dockerfile")])
        self.assertEqual(compose, [self.p("deploy/compose.yaml")])

        services = [
            {"name": "shop.api", "dockerfile": self.p("Shop.Api/Dockerfile"), "context": self.dir, "web": True},
            {"name": "tool", "dockerfile": self.p("Tool/Dockerfile"), "context": self.p("Tool"), "web": False},
        ]
        compose = dk.compose_file(services, self.dir)
        self.assertIn('  shop.api:\n    image: shop.api\n    build:\n      context: .\n      dockerfile: Shop.Api/Dockerfile\n    ports:\n      - "8080:8080"', compose)
        self.assertIn("  tool:\n    image: tool\n    build:\n      context: ./Tool\n      dockerfile: Dockerfile\n", compose)
        self.assertEqual(dk.compose_services([self.p("Shop.Api/Dockerfile"), self.p("Tool/Dockerfile")], self.dir), services)

        # The workspace scan skips build output and dot-folders; projects are found shallowest first.
        self.write("Shop.Api/bin/Dockerfile", "")
        self.write(".hidden/Dockerfile", "")
        dockerfiles, compose = dk.docker_files(dk.workspace_files(self.dir))
        self.assertEqual(dockerfiles, [self.p("Shop.Api/Dockerfile"), self.p("Tool/Dockerfile")])
        self.assertEqual(compose, [self.p("deploy/compose.yaml")])
        self.assertEqual(dk.dotnet_projects(self.dir)[-1], self.p("Tool/Tool.csproj"))
        self.assertTrue(dk.is_app(self.p("Tool/Tool.csproj")))
        self.assertFalse(dk.is_app(self.p("Shop.Core/Shop.Core.csproj")))

    def test_dockerignore_text(self):
        self.assertTrue(dk.DOTNET_DOCKERIGNORE.startswith("**/.classpath\n**/.dockerignore\n"))
        self.assertTrue(dk.DOTNET_DOCKERIGNORE.endswith("!.git/refs/heads/**\n"))
        self.assertEqual(len(dk.DOTNET_DOCKERIGNORE.splitlines()), 32)


class ManifestTests(unittest.TestCase):
    def test_manifest_parses_and_rules_compile(self):
        manifest = load_manifest()
        self.assertEqual(manifest["id"], "byteforge.docker")
        self.assertEqual(manifest["args"], ["docker.py"])
        self.assertEqual(manifest["views"][0]["id"], dk.VIEW)
        self.assertEqual(len(manifest["fileIcons"]), 3)
        language = manifest["languages"][0]
        self.assertEqual(language["id"], "dockerfile")
        for rule in language["rules"]:
            pattern = re.compile(rule["match"])
            self.assertLessEqual(rule.get("group", 0), pattern.groups, rule["match"])
            self.assertNotIn("(?<", rule["match"])  # no lookbehind in the Rust regex crate

    def test_dockerfile_highlighting(self):
        """The old built-in highlighting's expectations (bf-syntax `dockerfile_kinds`)."""
        rules = load_manifest()["languages"][0]["rules"]

        def kind(line, needle, nth=0):
            at = -1
            for _ in range(nth + 1):
                at = line.index(needle, at + 1)
            tokens = highlight(rules, line)[at:at + len(needle)]
            self.assertEqual(len(set(tokens)), 1, f"{needle!r} in {line!r}: {tokens}")
            return tokens[0]

        self.assertEqual(kind("# syntax=docker/dockerfile:1", "# syntax=docker/dockerfile:1"), "comment")
        line = "FROM mcr.microsoft.com/dotnet/sdk:9.0 AS build"
        self.assertEqual(kind(line, "FROM"), "keyword")
        self.assertEqual(kind(line, ":9.0"), "number")
        self.assertEqual(kind(line, "AS"), "keyword")
        self.assertEqual(kind(line, "mcr.microsoft.com/dotnet/sdk"), "class name")
        self.assertEqual(kind(line, "build"), "label name")
        self.assertEqual(kind("ARG BUILD_CONFIGURATION=Release", "BUILD_CONFIGURATION"), "property name")
        self.assertEqual(kind("ENV ASPNETCORE_HTTP_PORTS=8080", "ASPNETCORE_HTTP_PORTS"), "property name")
        line = 'COPY ["Api/Api.csproj", "Api/"]'
        self.assertEqual(kind(line, '"Api/Api.csproj"'), "string")
        self.assertEqual(kind(line, "["), "punctuation")
        self.assertEqual(kind(line, ","), "punctuation")
        self.assertEqual(kind(line, "]"), "punctuation")
        self.assertEqual(kind('RUN dotnet restore "Api/Api.csproj"', '"Api/Api.csproj"'), "string")
        self.assertEqual(kind("COPY --from=build /app/publish .", "--from=build"), "parameter name")
        self.assertEqual(kind("EXPOSE 8080", "8080"), "number")
        self.assertEqual(kind('ENTRYPOINT ["dotnet", "Api.dll"]', "ENTRYPOINT"), "keyword")
        line = 'RUN echo "${HOME:-/root}" $USER \\'
        self.assertEqual(kind(line, "$USER"), "local name")
        self.assertEqual(kind(line, ":-"), "operator")
        self.assertEqual(kind(line, "\\"), "operator")
        self.assertEqual(kind("FROM localhost:5000/app:dev AS x", "localhost:5000/app"), "class name")
        self.assertEqual(highlight(rules, "run dotnet build")[0:3], ["keyword"] * 3)
        self.assertIsNone(highlight(rules, "RUN apt-get install -y curl")[10])


class ProtocolTests(unittest.TestCase):
    def test_tree_without_docker(self):
        module = dk.Module()
        module.cli = False
        tree = module.tree()
        self.assertEqual(tree["type"], "setTree")
        self.assertEqual(tree["message"], "Docker isn't installed (no docker command found).")
        self.assertEqual([i["id"] for i in tree["items"]], ["section:files"])
        self.assertEqual(tree["items"][0]["children"][0]["id"], "note:files")

    def test_tree_with_containers(self):
        module = dk.Module()
        module.workspace = "/w"
        module.cli = True
        module.dockerfiles = ["/w/Api/Dockerfile"]
        module.compose_files = ["/w/compose.yaml"]
        module.containers = dk.parse_containers(
            '{"ID":"aaa","Names":"shop-api-1","Image":"api","State":"running","Status":"Up","Ports":"0.0.0.0:80->80/tcp",'
            '"Labels":"com.docker.compose.project=shop,com.docker.compose.service=api"}\n'
            '{"ID":"bbb","Names":"db","Image":"pg","State":"paused","Status":"Up (Paused)"}')
        tree = module.tree()
        self.assertEqual(tree["badge"], 1)
        self.assertEqual(tree["message"], "1 running · 2 containers · 0 images")
        files, containers, images = tree["items"]
        self.assertEqual([c["id"] for c in files["children"]], ["file:/w/compose.yaml", "file:/w/Api/Dockerfile"])
        self.assertEqual(files["children"][1]["icon"], "file:Dockerfile")
        self.assertEqual(files["children"][1]["description"], "Api")
        group, db = containers["children"]
        self.assertEqual(group["id"], "group:shop")
        api = group["children"][0]
        self.assertEqual((api["label"], api["iconColor"], api["description"]), ("api", "#3fb950", "0.0.0.0:80->80/tcp"))
        self.assertEqual([a["id"] for a in api["actions"]], ["logs", "shell", "restart", "stop", "remove"])
        self.assertEqual([a["id"] for a in db["actions"]], ["logs", "unpause", "remove"])
        self.assertEqual(db["iconColor"], "#d29922")
        self.assertEqual(images["count"], 0)


if __name__ == "__main__":
    unittest.main()
