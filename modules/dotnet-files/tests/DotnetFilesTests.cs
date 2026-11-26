using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Nodes;
using ByteForge.DotnetFiles;
using ByteForge.Modules.Testing;

namespace ByteForge.DotnetFiles.Tests;

public sealed class DotnetFilesTests : IDisposable
{
    readonly string dir = Directory.CreateTempSubdirectory("bf-dotnet-files-").FullName;

    public void Dispose()
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    /// <summary>`dotnet --version` without running dotnet.</summary>
    sealed class FakeSdk(string? version) : DotnetFilesModule
    {
        public override Task<string?> SdkVersionAsync() => Task.FromResult(version);
    }

    string ModuleDir => Path.Combine(dir, "modules", "dotnet-files");

    string Workspace => Directory.CreateDirectory(Path.Combine(dir, "repo")).FullName;

    static JsonObject Run(string id) => new() { ["type"] = "executeCommand", ["id"] = id };

    static string Sha256(string text) => Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(text)));

    [Fact]
    public void The_files_are_byte_identical_to_the_python_modules()
    {
        // Sizes and SHA-256 of the Python module's strings (UTF-8).
        Assert.Equal((367, "c6d537166bae29a17d8f9c9e3890d52d4cbc234949b22d6a225ab0594ac927dd"),
            (Encoding.UTF8.GetByteCount(DotnetFilesModule.Gitignore), Sha256(DotnetFilesModule.Gitignore)));
        Assert.Equal((2189, "88274030308a1db975f1aae50a66a7bc73148846c492851aff436bfbecb12b6d"),
            (Encoding.UTF8.GetByteCount(DotnetFilesModule.EditorConfig), Sha256(DotnetFilesModule.EditorConfig)));
        Assert.Equal((398, "f1c9f7a6a99ab190428aea094659b0fa41b1a2fe607bbb778901704d0c8cd604"),
            (Encoding.UTF8.GetByteCount(DotnetFilesModule.BuildProps), Sha256(DotnetFilesModule.BuildProps)));
        Assert.Equal((93, "ca1091f8f457c0738f2f55837269f1a6394dc78029d9e5444cc98e1c0b1697b9"),
            (Encoding.UTF8.GetByteCount(DotnetFilesModule.Gitattributes), Sha256(DotnetFilesModule.Gitattributes)));
    }

    [Fact]
    public async Task Registers_a_command_per_file()
    {
        await using var module = ModuleHarness.Start(new FakeSdk("10.0.100"), ModuleDir);
        var expected = new[]
        {
            ("files.gitignore", "Add .gitignore (.NET)"), ("files.editorconfig", "Add .editorconfig (C# style)"),
            ("files.buildProps", "Add Directory.Build.props"), ("files.globalJson", "Add global.json (pin the SDK)"),
            ("files.gitattributes", "Add .gitattributes"),
        };
        foreach (var (id, title) in expected)
        {
            var command = await module.Next("registerCommand");
            Assert.Equal(id, command["id"]!.GetValue<string>());
            Assert.Equal(title, command["title"]!.GetValue<string>());
            Assert.Equal("Files", command["category"]!.GetValue<string>());
        }
    }

    [Fact]
    public async Task Adds_each_file_with_lf_and_no_bom_then_opens_it()
    {
        var workspace = Workspace;
        await using var module = ModuleHarness.Start(new FakeSdk("10.0.100"), ModuleDir);
        module.Activate(workspace);
        var expected = new Dictionary<string, string>
        {
            ["files.gitignore"] = DotnetFilesModule.Gitignore,
            ["files.editorconfig"] = DotnetFilesModule.EditorConfig,
            ["files.buildProps"] = DotnetFilesModule.BuildProps,
            ["files.gitattributes"] = DotnetFilesModule.Gitattributes,
            ["files.globalJson"] = "{\n  \"sdk\": {\n    \"version\": \"10.0.100\",\n    \"rollForward\": \"latestFeature\"\n  }\n}\n",
        };
        foreach (var (id, content) in expected)
        {
            var name = DotnetFilesModule.Files[id].Name;
            module.Send(Run(id));
            var log = await module.Next("log");
            Assert.Equal($"Added {name}.", log["text"]!.GetValue<string>());
            var open = await module.Next("openFile");
            Assert.Equal(Path.Combine(workspace, name), open["path"]!.GetValue<string>());
            Assert.Equal(Encoding.UTF8.GetBytes(content), File.ReadAllBytes(Path.Combine(workspace, name)));
        }
    }

    [Fact]
    public async Task Never_overwrites_an_existing_file()
    {
        var workspace = Workspace;
        File.WriteAllText(Path.Combine(workspace, ".gitignore"), "mine\n");
        await using var module = ModuleHarness.Start(new FakeSdk("10.0.100"), ModuleDir);
        module.Activate(workspace);
        module.Send(Run("files.gitignore"));
        Assert.Equal(".gitignore already exists; opening it.", (await module.Next("log"))["text"]!.GetValue<string>());
        await module.Next("openFile");
        Assert.Equal("mine\n", File.ReadAllText(Path.Combine(workspace, ".gitignore")));
    }

    [Fact]
    public async Task Needs_a_folder()
    {
        await using var module = ModuleHarness.Start(new FakeSdk("10.0.100"), ModuleDir);
        module.Activate(null);
        module.Send(Run("files.gitignore"));
        Assert.Equal("Open a folder first.", (await module.Next("log"))["text"]!.GetValue<string>());
    }

    [Fact]
    public async Task Global_json_without_an_sdk_is_an_error_and_leaves_no_file()
    {
        var workspace = Workspace;
        await using var module = ModuleHarness.Start(new FakeSdk(null), ModuleDir);
        module.Activate(workspace);
        module.Send(Run("files.globalJson"));
        Assert.Equal("Could not add global.json: dotnet --version failed (is the .NET SDK installed?)", (await module.Next("log"))["text"]!.GetValue<string>());
        Assert.Equal("output", (await module.Next("showView"))["id"]!.GetValue<string>());
        Assert.False(File.Exists(Path.Combine(workspace, "global.json")));
    }

    [Fact]
    public async Task Follows_the_opened_folder()
    {
        var workspace = Workspace;
        await using var module = ModuleHarness.Start(new FakeSdk("10.0.100"), ModuleDir);
        module.Activate(null);
        module.Send(new() { ["type"] = "event", ["name"] = "folderOpened", ["path"] = workspace });
        module.Send(Run("files.gitattributes"));
        Assert.Equal(Path.Combine(workspace, ".gitattributes"), (await module.Next("openFile"))["path"]!.GetValue<string>());
    }

    [Fact]
    public async Task The_sdk_version_comes_from_dotnet_on_the_path()
    {
        var path = Environment.GetEnvironmentVariable("PATH");
        var bin = Directory.CreateDirectory(Path.Combine(dir, "bin")).FullName;
        try
        {
            Environment.SetEnvironmentVariable("PATH", bin);
            Assert.Null(await new DotnetFilesModule().SdkVersionAsync()); // no dotnet on PATH (the current folder doesn't count)

            if (OperatingSystem.IsWindows())
            {
                File.WriteAllText(Path.Combine(bin, "dotnet.cmd"), "@echo 9.0.300\r\n");
            }
            else
            {
                var script = Path.Combine(bin, "dotnet");
                File.WriteAllText(script, "#!/bin/sh\necho 9.0.300\n");
                File.SetUnixFileMode(script, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
            }
            Assert.Equal("9.0.300", await new DotnetFilesModule().SdkVersionAsync());
        }
        finally
        {
            Environment.SetEnvironmentVariable("PATH", path);
        }
    }
}
