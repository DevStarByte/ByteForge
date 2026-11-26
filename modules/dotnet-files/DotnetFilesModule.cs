using System.Text;
using ByteForge.Modules;

namespace ByteForge.DotnetFiles;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<DotnetFilesModule>(input, output, moduleDir, cancel);
}

/// <summary>'.NET Project Files': adds common repo files to the workspace root and opens them.</summary>
public class DotnetFilesModule : Module
{
    // The source file may be checked out with CRLF; the files written always have LF line ends.
    static string Lf(string text) => text.ReplaceLineEndings("\n") + "\n";

    public static readonly string Gitignore = Lf("""
        # Build output
        [Bb]in/
        [Oo]bj/
        [Ll]og/
        [Ll]ogs/
        artifacts/
        publish/
        *.nupkg
        *.snupkg

        # Test results and coverage
        TestResults/
        coverage*.json
        coverage*.xml
        *.trx

        # User and IDE files
        *.user
        *.suo
        *.userprefs
        .vs/
        .idea/
        .vscode/*
        !.vscode/settings.json
        !.vscode/launch.json
        *.swp
        .DS_Store
        Thumbs.db

        # Secrets and local settings
        appsettings.*.local.json
        *.pfx
        .env
        """);

    public static readonly string EditorConfig = Lf("""
        root = true

        [*]
        charset = utf-8
        end_of_line = lf
        insert_final_newline = true
        trim_trailing_whitespace = true
        indent_style = space
        indent_size = 4

        [*.{json,yml,yaml,xml,csproj,props,targets,slnx,config}]
        indent_size = 2

        [*.md]
        trim_trailing_whitespace = false

        [*.cs]
        # Usings and namespaces
        dotnet_sort_system_directives_first = true
        csharp_using_directive_placement = outside_namespace:warning
        csharp_style_namespace_declarations = file_scoped:warning

        # var and expression bodies
        csharp_style_var_for_built_in_types = true:suggestion
        csharp_style_var_when_type_is_apparent = true:suggestion
        csharp_style_expression_bodied_properties = true:suggestion
        csharp_style_expression_bodied_methods = when_on_single_line:suggestion

        # Braces and new lines
        csharp_prefer_braces = true:suggestion
        csharp_new_line_before_open_brace = all

        # Modern C#
        csharp_style_prefer_primary_constructors = true:suggestion
        csharp_style_prefer_pattern_matching = true:suggestion
        dotnet_style_prefer_collection_expression = when_types_loosely_match:suggestion

        # Naming: _camelCase private fields, PascalCase constants, I-prefixed interfaces
        dotnet_naming_rule.private_fields.symbols = private_fields
        dotnet_naming_rule.private_fields.style = underscore_camel
        dotnet_naming_rule.private_fields.severity = suggestion
        dotnet_naming_symbols.private_fields.applicable_kinds = field
        dotnet_naming_symbols.private_fields.applicable_accessibilities = private
        dotnet_naming_style.underscore_camel.required_prefix = _
        dotnet_naming_style.underscore_camel.capitalization = camel_case

        dotnet_naming_rule.constants.symbols = constants
        dotnet_naming_rule.constants.style = pascal
        dotnet_naming_rule.constants.severity = suggestion
        dotnet_naming_symbols.constants.applicable_kinds = field
        dotnet_naming_symbols.constants.required_modifiers = const
        dotnet_naming_style.pascal.capitalization = pascal_case

        dotnet_naming_rule.interfaces.symbols = interfaces
        dotnet_naming_rule.interfaces.style = i_prefix
        dotnet_naming_rule.interfaces.severity = suggestion
        dotnet_naming_symbols.interfaces.applicable_kinds = interface
        dotnet_naming_style.i_prefix.required_prefix = I
        dotnet_naming_style.i_prefix.capitalization = pascal_case
        """);

    public static readonly string BuildProps = Lf("""
        <Project>
          <!-- Applies to every project below this folder. -->
          <PropertyGroup>
            <Nullable>enable</Nullable>
            <ImplicitUsings>enable</ImplicitUsings>
            <LangVersion>latest</LangVersion>
            <TreatWarningsAsErrors>false</TreatWarningsAsErrors>
            <EnforceCodeStyleInBuild>true</EnforceCodeStyleInBuild>
            <AnalysisLevel>latest-recommended</AnalysisLevel>
          </PropertyGroup>
        </Project>
        """);

    public static readonly string Gitattributes = Lf("""
        * text=auto eol=lf
        *.sln text eol=crlf
        *.{png,jpg,jpeg,gif,ico,pfx,snk,dll,exe,nupkg} binary
        """);

    /// <summary>A file the module adds: its name, how its content is made, the command's title.</summary>
    public sealed record FileKind(string Name, Func<DotnetFilesModule, Task<string>> Content, string Title);

    public static readonly IReadOnlyDictionary<string, FileKind> Files = new Dictionary<string, FileKind>
    {
        ["files.gitignore"] = new(".gitignore", _ => Task.FromResult(Gitignore), "Add .gitignore (.NET)"),
        ["files.editorconfig"] = new(".editorconfig", _ => Task.FromResult(EditorConfig), "Add .editorconfig (C# style)"),
        ["files.buildProps"] = new("Directory.Build.props", _ => Task.FromResult(BuildProps), "Add Directory.Build.props"),
        ["files.globalJson"] = new("global.json", m => m.GlobalJsonAsync(), "Add global.json (pin the SDK)"),
        ["files.gitattributes"] = new(".gitattributes", _ => Task.FromResult(Gitattributes), "Add .gitattributes"),
    };

    // Dictionary enumeration order is insertion order here (no removals): the commands register in this order.
    static readonly string[] Order = ["files.gitignore", "files.editorconfig", "files.buildProps", "files.globalJson", "files.gitattributes"];

    protected override Task OnStartAsync()
    {
        foreach (var id in Order)
        {
            RegisterCommand(id, Files[id].Title, "Files");
        }
        return Task.CompletedTask;
    }

    protected override async Task OnCommandAsync(string id)
    {
        if (Files.TryGetValue(id, out var file))
        {
            await AddAsync(file);
        }
    }

    /// <summary>The installed SDK's version (<c>dotnet --version</c>), or null.</summary>
    public virtual async Task<string?> SdkVersionAsync()
    {
        if (Programs.Which("dotnet") is not { } dotnet)
        {
            return null; // only from PATH, never the current folder
        }
        try
        {
            var result = await Programs.RunAsync(dotnet, ["--version"], workingDirectory: ModuleDir,
                timeout: TimeSpan.FromSeconds(20), cancel: Stopping);
            var version = result.Output.Trim();
            return version.Length > 0 ? version : null;
        }
        catch (Exception e) when (e is ModuleException or TimeoutException)
        {
            return null;
        }
    }

    public async Task<string> GlobalJsonAsync()
    {
        var version = await SdkVersionAsync() ?? throw new ModuleException("dotnet --version failed (is the .NET SDK installed?)");
        return "{\n  \"sdk\": {\n    \"version\": " + JsonString(version) + ",\n    \"rollForward\": \"latestFeature\"\n  }\n}\n";
    }

    /// <summary>A JSON string as Python's json.dumps writes it (ASCII only, \uXXXX for the rest).</summary>
    static string JsonString(string text)
    {
        var json = new StringBuilder("\"");
        foreach (var c in text)
        {
            json.Append(c switch
            {
                '"' => "\\\"",
                '\\' => "\\\\",
                '\n' => "\\n",
                '\r' => "\\r",
                '\t' => "\\t",
                '\b' => "\\b",
                '\f' => "\\f",
                _ when c < 0x20 || c >= 0x7f => $"\\u{(int)c:x4}", // ensure_ascii: everything but space…~
                _ => c.ToString(),
            });
        }
        return json.Append('"').ToString();
    }

    async Task AddAsync(FileKind file)
    {
        if (string.IsNullOrEmpty(Workspace))
        {
            Log("Open a folder first.");
            return;
        }
        var path = Path.Combine(Workspace, file.Name);
        if (File.Exists(path) || Directory.Exists(path))
        {
            Log($"{file.Name} already exists; opening it.");
        }
        else
        {
            try
            {
                // The content first: a failing `dotnet --version` mustn't leave an empty global.json behind.
                var content = await file.Content(this);
                await File.WriteAllTextAsync(path, content, new UTF8Encoding(false), Stopping);
                Log($"Added {file.Name}.");
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException or ModuleException)
            {
                Log($"Could not add {file.Name}: {e.Message}");
                ShowView("output");
                return;
            }
        }
        OpenFile(path);
    }
}
