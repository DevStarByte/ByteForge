using System.Text.Json.Nodes;
using ByteForge.Docker;
using ByteForge.Modules;
using ByteForge.Modules.Testing;

namespace ByteForge.Docker.Tests;

/// <summary>The machine as the module sees it, without docker, sg or systemctl: canned results per command line.</summary>
sealed class FakeSystem : DockerSystem
{
    public readonly Dictionary<string, ProcessResult> Results = [];
    public readonly HashSet<string> Missing = [];     // command lines that can't start
    public readonly HashSet<string> OnPath = [];
    public readonly List<string[]> Runs = [];
    public bool Linux;
    public (bool Listed, bool Active)? Group;

    public override bool IsLinux => Linux;

    public override string? Which(string name) => OnPath.Contains(name) ? "/usr/bin/" + name : null;

    public override (bool Listed, bool Active)? DockerGroup() => Group;

    public override Task<ProcessResult> RunAsync(IReadOnlyList<string> argv, TimeSpan timeout, CancellationToken cancel)
    {
        var line = string.Join(" ", argv);
        lock (Runs)
        {
            Runs.Add([.. argv]);
        }
        if (Missing.Contains(line) || Missing.Contains("*"))
        {
            throw new ModuleException("docker was not found on PATH");
        }
        return Task.FromResult(Results.TryGetValue(line, out var result) ? result : new ProcessResult(0, "", ""));
    }

    public bool Ran(params string[] argv)
    {
        lock (Runs)
        {
            return Runs.Any(r => r.SequenceEqual(argv));
        }
    }

    /// <summary>The engine of the fixtures: docker.json's outputs; Compose missing, Buildx there.</summary>
    public static FakeSystem WithFixtures()
    {
        var docker = Fixtures.Load("docker.json");
        var system = new FakeSystem();
        system.Results["docker ps --all --no-trunc --format {{json .}}"] = new(0, S(docker["ps"]), "");
        system.Results["docker images --format {{json .}}"] = new(0, S(docker["images"]), "");
        system.Results["docker volume ls --format {{json .}}"] = new(0, S(docker["volumes"]), "");
        system.Results["docker network ls --no-trunc --format {{json .}}"] = new(0, S(docker["networks"]), "");
        system.Results["docker compose version"] = new(1, "", "docker: 'compose' is not a docker command.");
        return system;
    }

    static string S(JsonNode? node) => node!.GetValue<string>();
}

static class Fixtures
{
    public static string Dir => Path.Combine(AppContext.BaseDirectory, "fixtures");

    public static JsonObject Load(string name) => JsonNode.Parse(File.ReadAllText(Path.Combine(Dir, name)))!.AsObject();

    /// <summary>Lays out workspace.json's files below <paramref name="root"/>.</summary>
    public static void Workspace(string root)
    {
        foreach (var (rel, text) in Load("workspace.json"))
        {
            var path = Path.Combine([root, .. rel.Split('/')]);
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(path, text!.GetValue<string>());
        }
    }
}

public sealed class DockerTests : IDisposable
{
    /// <summary>A config folder laid out like ByteForge's: the module in &lt;tmp&gt;/modules/docker, the workspace next to it.</summary>
    readonly string config = Directory.CreateTempSubdirectory("bf-docker-").FullName;

    string ModuleDir => Path.Combine(config, "modules", "docker");
    string Ws => Path.Combine(config, "ws");

    public DockerTests()
    {
        Directory.CreateDirectory(ModuleDir);
        Directory.CreateDirectory(Ws);
    }

    public void Dispose()
    {
        try
        {
            Directory.Delete(config, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    ModuleHarness Start(FakeSystem system, int pollMs = 60_000) => ModuleHarness.Start(new DockerModule(system, TimeSpan.FromMilliseconds(pollMs)), ModuleDir);

    /// <summary>Paths below the workspace as the Python reference wrote them: "&lt;ws&gt;/…" with "/".</summary>
    JsonNode? Norm(JsonNode? node) => node switch
    {
        JsonValue v when v.TryGetValue<string>(out var s) => JsonValue.Create(s.Replace(Ws, "<ws>").Replace('\\', '/')),
        JsonArray a => new JsonArray([.. a.Select(Norm)]),
        JsonObject o => new JsonObject(o.Select(p => KeyValuePair.Create(Norm(JsonValue.Create(p.Key))!.GetValue<string>(), Norm(p.Value)))),
        null => null,
        _ => node.DeepClone(),
    };

    JsonNode? Norm(object value) => Norm(System.Text.Json.JsonSerializer.SerializeToNode(value));

    static void AssertJson(JsonNode? expected, JsonNode? actual) =>
        Assert.True(JsonNode.DeepEquals(expected, actual), $"expected:\n{expected?.ToJsonString()}\nactual:\n{actual?.ToJsonString()}");

    static string S(JsonNode? node) => node!.GetValue<string>();

    static void Command(ModuleHarness module, string id) => module.Send(new() { ["type"] = "executeCommand", ["id"] = id });

    static void TreeAction(ModuleHarness module, string? item, string action) =>
        module.Send(new() { ["type"] = "treeAction", ["view"] = "docker", ["item"] = item, ["action"] = action });

    static void Event(ModuleHarness module, string name, JsonObject? fields = null)
    {
        var message = new JsonObject { ["type"] = "event", ["name"] = name };
        foreach (var (key, value) in fields ?? [])
        {
            message[key] = value?.DeepClone();
        }
        module.Send(message);
    }

    /// <summary>Shows the view and waits for the tree with the engine's sections (or <paramref name="until"/>).</summary>
    static async Task<JsonObject> Shown(ModuleHarness module, Func<JsonObject, bool>? until = null)
    {
        Event(module, "viewShown", new() { ["view"] = "docker" });
        return await module.Next("setTree", until ?? (t => t["items"]!.AsArray().Any(i => S(i!["id"]) == "section:containers")));
    }

    /// <summary>Waits until a quick operation is over (the view's message is no longer "docker …").</summary>
    static Task Idle(ModuleHarness module) => module.Next("setTree", t => !S(t["message"]).StartsWith("docker ", StringComparison.Ordinal));

    static JsonObject? Find(JsonNode? items, string id)
    {
        foreach (var item in items?.AsArray() ?? [])
        {
            if (S(item!["id"]) == id)
            {
                return item.AsObject();
            }
            if (Find(item["children"], id) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    // ───────────── the data, compared with the Python module ─────────────

    [Fact]
    public void Workspace_files_and_projects_match_python()
    {
        Fixtures.Workspace(Ws);
        var expected = Fixtures.Load("python-expected.json");
        var files = DockerData.WorkspaceFiles(Ws);
        var (dockerfiles, compose) = DockerData.DockerFiles(files);
        AssertJson(expected["files"], Norm(files));
        AssertJson(expected["dockerfiles"], Norm(dockerfiles));
        AssertJson(expected["compose"], Norm(compose));
        AssertJson(expected["contexts"], Norm(dockerfiles.ToDictionary(f => f, f => DockerData.BuildContext(f, Ws))));
        AssertJson(expected["imageNames"], Norm(dockerfiles.Concat(compose).ToDictionary(f => f, DockerData.ImageNameFor)));
        var projects = DockerData.DotnetProjects(Ws);
        AssertJson(expected["projects"], Norm(projects));
        var info = new Dictionary<string, object>();
        var texts = new Dictionary<string, string>();
        foreach (var p in projects)
        {
            var project = DotnetProject.Read(p);
            info[p] = new object[] { project.Assembly, project.Version, project.Web, DockerData.IsApp(p), project.References() };
            var context = DockerData.HasSolution(Ws) || project.References().Count > 0 ? Ws : PyPath.Dirname(p);
            texts[p] = DockerData.DotnetDockerfile(project, context);
        }
        AssertJson(expected["projectInfo"], Norm(info));
        AssertJson(expected["dockerfileTexts"], Norm(texts));
        AssertJson(expected["composeFile"], Norm(DockerData.ComposeFile(DockerData.ComposeServices(dockerfiles, Ws), Ws)));
    }

    [Fact]
    public void Parsing_matches_python()
    {
        var docker = Fixtures.Load("docker.json");
        var expected = Fixtures.Load("python-expected.json")["parsed"]!;
        var containers = DockerData.ParseContainers(S(docker["ps"])).Select(c => new Dictionary<string, object?>
        {
            ["id"] = c.Id, ["name"] = c.Name, ["image"] = c.Image, ["state"] = c.State, ["status"] = c.Status, ["ports"] = c.Ports,
            ["compose_project"] = c.ComposeProject, ["compose_service"] = c.ComposeService, ["mounts"] = c.Mounts, ["networks"] = c.Networks,
            ["command"] = c.Command, ["age"] = c.Age,
        });
        AssertJson(expected["containers"], Norm(containers));
        var images = DockerData.ParseImages(S(docker["images"])).Select(i => new Dictionary<string, object?>
        {
            ["id"] = i.Id, ["repository"] = i.Repository, ["tag"] = i.Tag, ["size"] = i.Size, ["created"] = i.Created,
        });
        AssertJson(expected["images"], Norm(images));
        var volumes = DockerData.ParseVolumes(S(docker["volumes"])).Select(v => new Dictionary<string, object?>
        {
            ["name"] = v.Name, ["driver"] = v.Driver, ["mountpoint"] = v.Mountpoint, ["compose_project"] = v.ComposeProject, ["anonymous"] = v.Anonymous,
        });
        AssertJson(expected["volumes"], Norm(volumes));
        var networks = DockerData.ParseNetworks(S(docker["networks"])).Select(n => new Dictionary<string, object?>
        {
            ["id"] = n.Id, ["name"] = n.Name, ["driver"] = n.Driver, ["scope"] = n.Scope, ["compose_project"] = n.ComposeProject,
        });
        AssertJson(expected["networks"], Norm(networks));
    }

    [Fact]
    public void Names_and_command_lines_match_python()
    {
        var expected = Fixtures.Load("python-expected.json");
        foreach (var (input, output) in expected["imageName"]!.AsObject())
        {
            Assert.Equal(S(output), DockerData.ImageName(input));
        }
        foreach (var (input, output) in expected["shellQuote"]!.AsObject())
        {
            Assert.Equal(S(output), DockerData.ShellQuote(input));
        }
        foreach (var (input, output) in expected["isDockerfile"]!.AsObject())
        {
            Assert.True(output!.GetValue<bool>() == DockerData.IsDockerfileName(input), input);
        }
        foreach (var (input, output) in expected["isCompose"]!.AsObject())
        {
            Assert.True(output!.GetValue<bool>() == DockerData.IsComposeFileName(input), input);
        }
        foreach (var (input, output) in expected["classify"]!.AsObject())
        {
            Assert.Equal(S(output), DockerData.ClassifyEngineError(input));
        }
        Assert.Equal(S(expected["runShell"]), DockerData.CommandLine(DockerData.RunShellArgs("my/image:1.0")));
    }

    [Fact]
    public async Task Tree_matches_python()
    {
        Fixtures.Workspace(Ws);
        var system = FakeSystem.WithFixtures();
        await using var module = Start(system);
        module.Activate(Ws);
        var tree = await Shown(module);
        AssertJson(Fixtures.Load("python-expected.json")["tree"], Norm(tree));
        Assert.True(system.Ran("docker", "--version"));
        Assert.True(system.Ran("docker", "buildx", "version"));
    }

    [Fact]
    public async Task Stopped_engine_tree_matches_python()
    {
        Fixtures.Workspace(Ws);
        var system = FakeSystem.WithFixtures();
        system.Results["docker ps --all --no-trunc --format {{json .}}"] =
            new(1, "", "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?\n");
        await using var module = Start(system);
        module.Activate(Ws);
        var tree = await Shown(module, t => t["error"] != null);
        AssertJson(Fixtures.Load("python-expected.json")["treeStopped"], Norm(tree));
    }

    // ───────────── the module ─────────────

    [Fact]
    public async Task Registers_its_commands()
    {
        await using var module = Start(new FakeSystem());
        foreach (var (id, title) in DockerModule.Commands)
        {
            var command = await module.Next("registerCommand", m => S(m["id"]) == id);
            Assert.Equal(title, S(command["title"]));
            Assert.Equal("Docker", S(command["category"]));
        }
        Assert.Equal(7, DockerModule.Commands.Length);
        module.Activate(null);
        var tree = await module.Next("setTree");
        Assert.Equal("Looking for Docker…", S(tree["message"]));
        Assert.Equal("No Dockerfile yet: run \"Docker: Add Docker Support to Project…\"", S(Find(tree["items"], "note:files")!["label"]));
    }

    [Fact]
    public async Task Docker_not_installed()
    {
        var system = new FakeSystem();
        system.Missing.Add("*");
        await using var module = Start(system);
        module.Activate(Ws);
        var tree = await Shown(module, t => Find(t["items"], "setup:install") != null);
        Assert.Equal("Docker isn't installed (no docker command found).", S(tree["message"]));
        Assert.Equal(["section:setup", "section:files"], tree["items"]!.AsArray().Select(i => S(i!["id"])));
    }

    [Fact]
    public async Task Linux_setup_checks_and_fixes()
    {
        var system = FakeSystem.WithFixtures();
        system.Linux = true;
        system.Group = (false, false);
        system.OnPath.UnionWith(["systemctl", "pacman"]);
        system.Results["docker ps --all --no-trunc --format {{json .}}"] = new(1, "", "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.");
        system.Results["docker buildx version"] = new(1, "", "unknown command");
        await using var module = Start(system);
        module.Activate(Ws);
        var tree = await Shown(module, t => Find(t["items"], "setup:engine") != null);
        var engine = Find(tree["items"], "setup:engine")!;
        Assert.Equal("▶ start it and enable it at boot", S(engine["description"]));
        Assert.Equal("start-engine", S(engine["actions"]![0]!["id"]));
        Assert.NotNull(Find(tree["items"], "setup:group"));
        var plugins = Find(tree["items"], "setup:plugins")!;
        Assert.Equal("Docker Compose and Buildx are missing", S(plugins["label"]));
        Assert.Equal("▶ install docker-compose docker-buildx", S(plugins["description"]));
        Assert.Equal("sudo pacman -S --needed docker-compose docker-buildx", S(plugins["actions"]![0]!["tooltip"]));

        TreeAction(module, "setup:engine", "start-engine");
        var start = await module.Next("runInTerminal");
        Assert.Equal("start Docker", S(start["title"]));
        Assert.Equal("sudo systemctl enable --now docker", S(start["command"]));
        TreeAction(module, "setup:group", "click");
        Assert.Equal("sudo usermod -aG docker \"$USER\"", S((await module.Next("runInTerminal"))["command"]));
        TreeAction(module, "setup:plugins", "install-plugins");
        Assert.Equal("sudo pacman -S --needed docker-compose docker-buildx", S((await module.Next("runInTerminal"))["command"]));
    }

    [Theory]
    [InlineData("permission denied while trying to connect to the Docker daemon socket", "setup:permission", "No permission to use Docker. See SETUP below.")]
    [InlineData("context deadline exceeded", "setup:other", "The Docker engine isn't reachable. Check your Docker context.")]
    public async Task Engine_errors_explained(string error, string item, string message)
    {
        var system = FakeSystem.WithFixtures();
        system.Linux = true;
        system.Group = (true, true);
        system.Results["docker ps --all --no-trunc --format {{json .}}"] = new(1, "", error + "\n");
        await using var module = Start(system);
        module.Activate(Ws);
        var tree = await Shown(module, t => t["error"] != null);
        Assert.Equal(message, S(tree["message"]));
        Assert.Equal(error, S(tree["error"]));
        Assert.Equal(error, S(Find(tree["items"], item)!["tooltip"]));
        Assert.Null(Find(tree["items"], "setup:group"));
        Assert.Null(Find(tree["items"], "section:containers"));
        // Compose is missing (fixtures), no package manager on PATH.
        Assert.Equal("install them with your package manager", S(Find(tree["items"], "setup:plugins")!["description"]));
    }

    [Fact]
    public async Task Uses_sg_while_the_docker_group_is_not_active_yet()
    {
        var system = FakeSystem.WithFixtures();
        system.Linux = true;
        system.Group = (true, false);
        system.OnPath.Add("sg");
        // Through sg, the docker command line is one argument of `sg docker -c`.
        string[][] commands =
        [
            ["docker", "ps", "--all", "--no-trunc", "--format", "{{json .}}"],
            ["docker", "images", "--format", "{{json .}}"],
            ["docker", "volume", "ls", "--format", "{{json .}}"],
            ["docker", "network", "ls", "--no-trunc", "--format", "{{json .}}"],
            ["docker", "compose", "version"],
        ];
        foreach (var argv in commands)
        {
            system.Results["sg docker -c " + DockerData.CommandLine(argv)] = system.Results[string.Join(" ", argv)];
        }
        await using var module = Start(system);
        module.Activate(Ws);
        var tree = await Shown(module, t => Find(t["items"], "setup:relogin") != null);
        Assert.True(system.Ran("sg", "docker", "-c", "docker ps --all --no-trunc --format '{{json .}}'"));
        TreeAction(module, "container:aaaaaaaaaaaa", "logs");
        var logs = await module.Next("runInTerminal");
        Assert.Equal("logs: shop-db-1", S(logs["title"]));
        Assert.Equal("sg docker -c 'docker logs --follow --tail 500 aaaaaaaaaaaa'", S(logs["command"]));
        Assert.NotNull(tree);
    }

    [Fact]
    public async Task Container_actions()
    {
        var system = FakeSystem.WithFixtures();
        system.Results["docker stop aaaaaaaaaaaa"] = new(0, "aaaaaaaaaaaa\n", "");
        system.Results["docker start dddddddddddd"] = new(1, "", "Error response from daemon: no such container\n");
        await using var module = Start(system);
        module.Activate(Ws);
        await Shown(module);

        TreeAction(module, "container:aaaaaaaaaaaa", "stop");
        await module.Next("setTree", t => S(t["message"]) == "docker stop shop-db-1…");
        await module.Next("log", m => S(m["text"]) == "docker stop shop-db-1: aaaaaaaaaaaa");
        await Idle(module);
        TreeAction(module, "container:dddddddddddd", "start");
        await module.Next("log", m => S(m["text"]) == "docker start old failed: Error response from daemon: no such container");
        await Idle(module);

        TreeAction(module, "container:aaaaaaaaaaaa", "shell");
        var shell = await module.Next("runInTerminal");
        Assert.Equal("docker exec -it aaaaaaaaaaaa sh -c 'command -v bash >/dev/null && exec bash || exec sh'", S(shell["command"]));

        // Remove asks first; Cancel does nothing, Remove removes it (forced).
        TreeAction(module, "container:cccccccccccc", "remove");
        var prompt = await module.Next("prompt");
        Assert.Equal("Remove container 'web'?", S(prompt["message"]));
        Assert.Equal("Its file system changes are lost (volumes are kept).", S(prompt["detail"]));
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 1 });
        TreeAction(module, "container:cccccccccccc", "remove");
        prompt = await module.Next("prompt");
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 0 });
        await module.Next("log", m => S(m["text"]) == "docker rm web: done");
        Assert.Equal(1, system.Runs.Count(r => r.SequenceEqual(["docker", "rm", "--force", "cccccccccccc"])));
    }

    [Fact]
    public async Task Image_volume_and_network_actions()
    {
        var system = FakeSystem.WithFixtures();
        await using var module = Start(system);
        module.Activate(Ws);
        await Shown(module);

        TreeAction(module, "image:3333bbbb4444:postgres:16", "run");
        var run = await module.Next("runInTerminal");
        Assert.Equal("run: postgres:16", S(run["title"]));
        Assert.Equal(DockerData.CommandLine(DockerData.RunShellArgs("postgres:16")), S(run["command"]));

        TreeAction(module, "volume:shop_data", "remove");
        await module.Next("log", m => S(m["text"]) == "Volume shop_data is used by shop-db-1; remove those containers first.");
        Assert.Equal("output", S((await module.Next("showView"))["id"]));

        TreeAction(module, "volume:unused", "remove");
        var prompt = await module.Next("prompt");
        Assert.Equal("Remove volume 'unused'?", S(prompt["message"]));
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 0 });
        await module.Next("log", m => S(m["text"]) == "docker volume rm unused: done");
        await Idle(module);

        TreeAction(module, "network:bridge", "inspect");
        Assert.Equal("docker network inspect bridge", S((await module.Next("runInTerminal"))["command"]));

        TreeAction(module, null, "prune");
        prompt = await module.Next("prompt");
        Assert.Equal("Remove all stopped containers, unused networks, dangling images and the build cache?", S(prompt["message"]));
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 0 });
        await module.Next("log", m => S(m["text"]) == "docker system prune: done");
        Assert.True(system.Ran("docker", "system", "prune", "--force"));
    }

    [Fact]
    public async Task Builds_the_active_dockerfile()
    {
        Fixtures.Workspace(Ws);
        await using var module = Start(new FakeSystem());
        module.Activate(Ws);
        var dockerfile = Path.Combine(Ws, "Api", "Dockerfile");
        Event(module, "editorChanged", new() { ["path"] = dockerfile });
        Command(module, "docker.build");
        var build = await module.Next("runInTerminal");
        Assert.Equal("build: api", S(build["title"]));
        Assert.Equal(DockerData.CommandLine(["docker", "build", "--file", dockerfile, "--tag", "api", Ws]), S(build["command"]));
        Assert.True(build["saveAll"]!.GetValue<bool>());
    }

    [Fact]
    public async Task Compose_up_picks_a_file_and_uses_the_compose_command_setting()
    {
        Fixtures.Workspace(Ws);
        await using var module = Start(new FakeSystem());
        module.Activate(Ws, new() { ["docker.composeCommand"] = "podman-compose" });
        Command(module, "docker.composeUp");
        var pick = await module.Next("quickPick");
        Assert.Equal("Select the Compose file to start", S(pick["placeholder"]));
        Assert.Equal(["deploy/compose.prod.yaml", "deploy/docker-compose.yml"], pick["items"]!.AsArray().Select(i => S(i!["label"])));
        var file = S(pick["items"]![1]!["value"]);
        module.Send(new() { ["type"] = "quickPickResult", ["requestId"] = pick["requestId"]!.DeepClone(), ["value"] = file });
        var up = await module.Next("runInTerminal");
        Assert.Equal("compose up: deploy", S(up["title"]));
        Assert.Equal(DockerData.CommandLine(["podman-compose", "--file", file, "up", "--build", "--detach"]), S(up["command"]));
        Assert.Equal("docker", S((await module.Next("showView"))["id"]));

        Event(module, "editorChanged", new() { ["path"] = file });
        Command(module, "docker.composeDown");
        var down = await module.Next("runInTerminal");
        Assert.Equal(DockerData.CommandLine(["podman-compose", "--file", file, "down"]), S(down["command"]));
        Assert.False(down["saveAll"]!.GetValue<bool>());
    }

    [Fact]
    public async Task No_dockerfile_to_build()
    {
        await using var module = Start(new FakeSystem());
        module.Activate(Ws);
        Command(module, "docker.build");
        await module.Next("log", m => S(m["text"]) == "No Dockerfile in the workspace. Create one with \"Docker: Add Docker Support to Project…\".");
        Command(module, "docker.addCompose");
        await module.Next("log", m => S(m["text"]) == "The workspace has no Dockerfile yet. Run \"Docker: Add Docker Support to Project…\" first.");
    }

    [Fact]
    public async Task Adds_docker_support_to_the_only_project()
    {
        var project = Path.Combine(Ws, "Hello", "Hello.csproj");
        Directory.CreateDirectory(Path.GetDirectoryName(project)!);
        File.WriteAllText(project, "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n    <TargetFramework>net9.0</TargetFramework>\n  </PropertyGroup>\n</Project>\n");
        await using var module = Start(new FakeSystem());
        module.Activate(Ws);
        Command(module, "docker.addDockerSupport");
        var dockerfile = Path.Combine(Ws, "Hello", "Dockerfile");
        var opened = await module.Next("openFile");
        Assert.Equal(dockerfile, S(opened["path"]));
        var text = File.ReadAllText(dockerfile);
        Assert.StartsWith("# Build:  docker build -f ./Dockerfile -t hello .\n# (run from the build context: the folder that contains this file)\n\nFROM mcr.microsoft.com/dotnet/runtime:9.0 AS base\n", text);
        Assert.Contains("WORKDIR \"/src\"\n", text);
        Assert.Equal(DockerData.DotnetDockerignore, File.ReadAllText(Path.Combine(Ws, "Hello", ".dockerignore")));
        Assert.Contains(module.Sent, m => S(m["type"]) == "log" && S(m["text"]) ==
            $"Added {dockerfile} (console app on .NET 9.0, build context {Path.Combine(Ws, "Hello")}). Build it with \"Docker: Build Image…\".");

        // Again: it asks before replacing it.
        File.WriteAllText(dockerfile, "mine");
        Command(module, "docker.addDockerSupport");
        var prompt = await module.Next("prompt");
        Assert.Equal("This project already has a Dockerfile.", S(prompt["message"]));
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = null });
        Command(module, "docker.addDockerSupport");
        prompt = await module.Next("prompt");
        Assert.Equal("mine", File.ReadAllText(dockerfile));
        module.Send(new() { ["type"] = "promptResult", ["requestId"] = prompt["requestId"]!.DeepClone(), ["index"] = 0 });
        await module.Next("openFile");
        Assert.Equal(text, File.ReadAllText(dockerfile));
    }

    [Fact]
    public async Task Add_docker_support_lists_apps_first()
    {
        Fixtures.Workspace(Ws);
        await using var module = Start(new FakeSystem());
        module.Activate(Ws);
        Command(module, "docker.addDockerSupport");
        var pick = await module.Next("quickPick");
        Assert.Equal("Select the project to add Docker support to", S(pick["placeholder"]));
        var items = pick["items"]!.AsArray().Select(i => $"{S(i!["label"])}|{i["detail"]?.GetValue<string>()}|{S(i["hint"])}").ToList();
        Assert.Equal(["Api|Api|web app · has a Dockerfile", "Tool|Tool|app · has a Dockerfile", "Worker|src/Worker|app", "Core|Core|library", "Lib|Lib|library"], items);
    }

    [Fact]
    public async Task Adds_a_compose_file()
    {
        Fixtures.Workspace(Ws);
        await using var module = Start(new FakeSystem());
        module.Activate(Ws);
        Command(module, "docker.addCompose");
        var path = Path.Combine(Ws, "compose.yaml");
        await module.Next("log", m => S(m["text"]) == $"Added {path} with 4 service(s). Start it with \"Docker: Compose Up\".");
        Assert.Equal(path, S((await module.Next("openFile"))["path"]));
        Assert.Equal(S(Fixtures.Load("python-expected.json")["composeFile"]), File.ReadAllText(path));
        Command(module, "docker.addCompose");
        await module.Next("log", m => S(m["text"]) == $"{path} already exists.");
    }

    [Fact]
    public async Task Needs_a_folder()
    {
        await using var module = Start(new FakeSystem());
        module.Activate(null);
        Command(module, "docker.addDockerSupport");
        await module.Next("log", m => S(m["text"]) == "Open a folder containing a .NET project first.");
        Command(module, "docker.addCompose");
        await module.Next("log", m => S(m["text"]) == "Open a folder first.");
    }

    [Fact]
    public async Task Polls_only_while_the_view_is_shown()
    {
        var system = FakeSystem.WithFixtures();
        await using var module = Start(system, pollMs: 50);
        module.Activate(Ws);
        await Task.Delay(300);
        lock (system.Runs)
        {
            Assert.Empty(system.Runs);
        }
        await Shown(module);
        await Task.Delay(400);
        int count;
        lock (system.Runs)
        {
            count = system.Runs.Count(r => r.Length > 1 && r[1] == "ps");
        }
        Assert.True(count >= 2, $"ps ran {count} times");
    }

    // ───────────── helpers ─────────────

    [Fact]
    public void Docker_group_from_proc_and_etc()
    {
        const string groups = "root:x:0:\nwheel:x:10:alice\ndocker:x:972:bob,alice\n";
        const string passwd = "root:x:0:0::/root:/bin/bash\nalice:x:1000:1000::/home/alice:/bin/bash\n";
        Assert.Equal((true, false), DockerData.DockerGroup("Name:\tbf\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\nGroups:\t10 1000\n", groups, passwd));
        Assert.Equal((true, true), DockerData.DockerGroup("Uid:\t1000\t1000\nGid:\t1000\t1000\nGroups:\t10 972 1000\n", groups, passwd));
        Assert.Equal((false, false), DockerData.DockerGroup("Uid:\t1000\nGid:\t1000\nGroups:\t1000\n", "docker:x:972:bob\n", passwd));
        Assert.Null(DockerData.DockerGroup("Uid:\t0\nGid:\t0\nGroups:\t0\n", groups, passwd));
        Assert.Null(DockerData.DockerGroup("Uid:\t1000\nGid:\t1000\nGroups:\n", "wheel:x:10:\n", passwd));
    }

    [Fact]
    public void Paths()
    {
        Assert.Equal("a/b", PyPath.Relative("/w/a/b", "/w"));
        Assert.Equal("", PyPath.Relative("/w", "/w/"));
        Assert.Equal("/x/y", PyPath.Relative("/x/y", "/w"));
        Assert.True(PyPath.StartsWith("/w/a/../b", "/w/b"));
        Assert.False(PyPath.StartsWith("/wx/a", "/w"));
        Assert.Equal("Dockerfile", PyPath.Basename("/w/Api/Dockerfile"));
        Assert.Equal("/w/Api", PyPath.Dirname("/w/Api/Dockerfile"));
        Assert.Equal("", PyPath.Dirname("Dockerfile"));
        Assert.Equal("/", PyPath.Dirname("/x"));
        Assert.Equal(("App", ".csproj"), PyPath.SplitExt("App.csproj"));
        Assert.Equal((".sln", ""), PyPath.SplitExt(".sln"));
        Assert.Equal("..", PyPath.Normpath("a/../.."));
        Assert.Equal(".", PyPath.Normpath(""));
        if (OperatingSystem.IsWindows())
        {
            Assert.Equal(@"C:\w\b", PyPath.Normpath("C:/w/a/../b"));
            Assert.Equal(@"C:\w", PyPath.Dirname(@"C:\w\x"));
            Assert.Equal("Api/Dockerfile", PyPath.Relative(@"C:\w\Api\Dockerfile", "C:/w"));
        }
        else
        {
            Assert.Equal("/w/b", PyPath.Normpath("/w/a/../b"));
            Assert.Equal("//w", PyPath.Normpath("//w/"));
        }
        List<string> sorted = ["/w/b", "/w/a/z", "/w/a", "/w/A"];
        sorted.Sort(PyPath.Order);
        Assert.Equal(["/w/A", "/w/a", "/w/a/z", "/w/b"], sorted);
    }

    [Fact]
    public void Dotnet_project_details()
    {
        var path = Path.Combine(Ws, "Multi.csproj");
        File.WriteAllText(path, "<Project><PropertyGroup><TargetFrameworks>net48;net6.0;net10.0;net9.0</TargetFrameworks><AssemblyName> </AssemblyName></PropertyGroup></Project>");
        var project = DotnetProject.Read(path);
        Assert.Equal(("Multi", "10.0", false), (project.Assembly, project.Version, project.Web));
        File.WriteAllText(path, "<Project Sdk=\"Microsoft.NET.Sdk.Razor\"><PropertyGroup><TargetFramework>netcoreapp3.1</TargetFramework></PropertyGroup></Project>");
        project = DotnetProject.Read(path);
        Assert.Equal(("Multi", DockerData.DefaultDotnetVersion, true), (project.Assembly, project.Version, project.Web));
        Assert.Throws<FileNotFoundException>(() => DotnetProject.Read(Path.Combine(Ws, "missing.csproj")));
        Assert.Equal(["a.csproj", "b"], DockerData.AttributeValues("<ProjectReference Include=\"a.csproj\"/><ProjectReference Include='b' /><ProjectReference Include=c />", "ProjectReference", "Include"));
    }
}
