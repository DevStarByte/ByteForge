"""'.NET Project Files' module for ByteForge: adds common repo files to the workspace root and opens them."""

import json
import os
import subprocess
import sys

GITIGNORE = """# Build output
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
"""

EDITORCONFIG = """root = true

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
"""

BUILD_PROPS = """<Project>
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
"""

GITATTRIBUTES = """* text=auto eol=lf
*.sln text eol=crlf
*.{png,jpg,jpeg,gif,ico,pfx,snk,dll,exe,nupkg} binary
"""

workspace = None


def send(message):
    print(json.dumps(message), flush=True)


def sdk_version():
    try:
        out = subprocess.run(["dotnet", "--version"], capture_output=True, text=True, timeout=20).stdout.strip()
        return out or None
    except (OSError, subprocess.SubprocessError):
        return None


def global_json():
    version = sdk_version()
    if not version:
        raise RuntimeError("dotnet --version failed (is the .NET SDK installed?)")
    return json.dumps({"sdk": {"version": version, "rollForward": "latestFeature"}}, indent=2) + "\n"


FILES = {
    "files.gitignore": (".gitignore", lambda: GITIGNORE, "Add .gitignore (.NET)"),
    "files.editorconfig": (".editorconfig", lambda: EDITORCONFIG, "Add .editorconfig (C# style)"),
    "files.buildProps": ("Directory.Build.props", lambda: BUILD_PROPS, "Add Directory.Build.props"),
    "files.globalJson": ("global.json", global_json, "Add global.json (pin the SDK)"),
    "files.gitattributes": (".gitattributes", lambda: GITATTRIBUTES, "Add .gitattributes"),
}


def add(command):
    name, content, _ = FILES[command]
    if not workspace:
        send({"type": "log", "text": "Open a folder first."})
        return
    path = os.path.join(workspace, name)
    if os.path.exists(path):
        send({"type": "log", "text": f"{name} already exists; opening it."})
    else:
        try:
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                f.write(content())
            send({"type": "log", "text": f"Added {name}."})
        except (OSError, RuntimeError) as e:
            send({"type": "log", "text": f"Could not add {name}: {e}"})
            send({"type": "showView", "id": "output"})
            return
    send({"type": "openFile", "path": path})


for command, (_, _, title) in FILES.items():
    send({"type": "registerCommand", "id": command, "title": title, "category": "Files"})

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    kind = message.get("type")
    if kind == "activate":
        workspace = message.get("workspace")
    elif kind == "event" and message.get("name") == "folderOpened":
        workspace = message.get("path")
    elif kind == "executeCommand" and message.get("id") in FILES:
        add(message["id"])
