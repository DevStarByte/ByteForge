using System.Numerics;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace ByteForge.Docker;

public sealed record Container(string Id, string Name, string Image, string State, string Status, string Ports, string? ComposeProject,
    string? ComposeService, List<string> Mounts, List<string> Networks, string Command, string Age)
{
    public bool Running => State is "running" or "restarting";
    public bool Paused => State == "paused";
}

public sealed record Image(string Id, string Repository, string Tag, string Size, string Created)
{
    /// <summary>`repository:tag`, or the id for dangling images (`&lt;none&gt;`).</summary>
    public string Reference => Repository == "<none>" ? Id : Tag is "<none>" or "" ? Repository : $"{Repository}:{Tag}";

    /// <summary>A stable item id (one image id can have several tags).</summary>
    public string Key => $"image:{Id}:{Reference}";
}

public sealed record Volume(string Name, string Driver, string Mountpoint, string? ComposeProject, bool Anonymous);

public sealed record Network(string Id, string Name, string Driver, string Scope, string? ComposeProject);

/// <summary>Parsing the docker CLI's output, and the names and command lines the module builds.</summary>
public static partial class DockerData
{
    public static readonly HashSet<string> BuiltinNetworks = ["bridge", "host", "none"];

    /// <summary>"permission" (no access to the socket), "stopped" (no daemon) or "other".</summary>
    public static string ClassifyEngineError(string? text)
    {
        var t = (text ?? "").ToLowerInvariant();
        if (t.Contains("permission denied"))
        {
            return "permission";
        }
        string[] stopped = ["no such file or directory", "is the docker daemon running", "cannot connect to the docker", "connection refused", "failed to connect to the docker api"];
        return stopped.Any(t.Contains) ? "stopped" : "other";
    }

    /// <summary>The JSON objects of `--format '{{json .}}'` output (one per line; others skipped; a repeated key: the last wins).</summary>
    public static IEnumerable<Dictionary<string, JsonElement>> JsonLines(string text)
    {
        foreach (var line in text.Split('\n'))
        {
            Dictionary<string, JsonElement>? fields = null;
            try
            {
                using var document = JsonDocument.Parse(line.Trim());
                if (document.RootElement.ValueKind == JsonValueKind.Object)
                {
                    fields = [];
                    foreach (var property in document.RootElement.EnumerateObject())
                    {
                        fields[property.Name] = property.Value.Clone();
                    }
                }
            }
            catch (JsonException)
            {
            }
            if (fields != null)
            {
                yield return fields;
            }
        }
    }

    /// <summary>A string field: null (skip the object) when required and missing, or not a string; "" when optional and missing.</summary>
    static string? Field(Dictionary<string, JsonElement> raw, string key, bool required)
    {
        if (!raw.TryGetValue(key, out var value) || value.ValueKind == JsonValueKind.Null)
        {
            return required ? null : "";
        }
        return value.ValueKind == JsonValueKind.String ? value.GetString() : null;
    }

    static string Str(Dictionary<string, JsonElement> raw, string key) =>
        raw.TryGetValue(key, out var value) && value.ValueKind == JsonValueKind.String ? value.GetString()! : "";

    /// <summary>Older engines don't report `State`; derive it from the status text.</summary>
    public static string StateFromStatus(string status)
    {
        var s = status.ToLowerInvariant();
        if (s.Contains("(paused)"))
        {
            return "paused";
        }
        if (s.StartsWith("up", StringComparison.Ordinal))
        {
            return "running";
        }
        if (s.StartsWith("restarting", StringComparison.Ordinal))
        {
            return "restarting";
        }
        return s.StartsWith("created", StringComparison.Ordinal) ? "created" : "exited";
    }

    /// <summary>A `key=value` label of a comma-separated label list (null when missing or empty).</summary>
    public static string? LabelValue(string labels, string key)
    {
        foreach (var pair in labels.Split(','))
        {
            if (pair.StartsWith(key + "=", StringComparison.Ordinal))
            {
                var value = pair[(key.Length + 1)..];
                return value.Length > 0 ? value : null;
            }
        }
        return null;
    }

    public static List<string> SplitList(string text) => [.. text.Split(',').Select(p => p.Trim()).Where(p => p.Length > 0)];

    /// <summary>Parses `docker ps --format '{{json .}}'`, sorted by Compose project (none last), then name.</summary>
    public static List<Container> ParseContainers(string text)
    {
        var containers = new List<Container>();
        foreach (var raw in JsonLines(text))
        {
            string[] keys = ["ID", "Names", "Image", "State", "Status", "Ports", "Labels", "Mounts", "Networks", "Command", "RunningFor"];
            var fields = new Dictionary<string, string>();
            var valid = true;
            foreach (var key in keys)
            {
                var value = Field(raw, key, key is "ID" or "Names" or "Image");
                if (value == null)
                {
                    valid = false;
                    break;
                }
                fields[key] = value;
            }
            if (!valid)
            {
                continue;
            }
            var state = fields["State"].Length > 0 ? fields["State"].ToLowerInvariant() : StateFromStatus(fields["Status"]);
            containers.Add(new Container(
                Prefix(fields["ID"], 12),
                fields["Names"].Split(',')[0],
                fields["Image"],
                state,
                fields["Status"],
                fields["Ports"],
                LabelValue(fields["Labels"], "com.docker.compose.project"),
                LabelValue(fields["Labels"], "com.docker.compose.service"),
                SplitList(fields["Mounts"]),
                SplitList(fields["Networks"]),
                fields["Command"].Trim('"'),
                fields["RunningFor"]));
        }
        return [.. containers.OrderBy(c => c.ComposeProject == null).ThenBy(c => c.ComposeProject ?? "", StringComparer.Ordinal).ThenBy(c => c.Name, StringComparer.Ordinal)];
    }

    /// <summary>Parses `docker volume ls --format '{{json .}}'`: named volumes first, then anonymous ones.</summary>
    public static List<Volume> ParseVolumes(string text)
    {
        var volumes = new List<Volume>();
        foreach (var raw in JsonLines(text))
        {
            var name = Str(raw, "Name");
            if (name.Length == 0)
            {
                continue;
            }
            volumes.Add(new Volume(name, Str(raw, "Driver"), Str(raw, "Mountpoint"), LabelValue(Str(raw, "Labels"), "com.docker.compose.project"), IsAnonymousVolume(name)));
        }
        return [.. volumes.OrderBy(v => v.Anonymous).ThenBy(v => v.Name, StringComparer.Ordinal)];
    }

    public static bool IsAnonymousVolume(string name) => name.Length == 64 && name.All(c => c is >= '0' and <= '9' or >= 'a' and <= 'f');

    /// <summary>Parses `docker network ls --format '{{json .}}'`: the built-in ones (bridge, host, none) last.</summary>
    public static List<Network> ParseNetworks(string text)
    {
        var networks = new List<Network>();
        foreach (var raw in JsonLines(text))
        {
            if (!raw.TryGetValue("Name", out var name) || name.ValueKind != JsonValueKind.String || !raw.TryGetValue("ID", out var id) || id.ValueKind != JsonValueKind.String
                || name.GetString()!.Length == 0)
            {
                continue;
            }
            networks.Add(new Network(Prefix(id.GetString()!, 12), name.GetString()!, Str(raw, "Driver"), Str(raw, "Scope"), LabelValue(Str(raw, "Labels"), "com.docker.compose.project")));
        }
        return [.. networks.OrderBy(n => BuiltinNetworks.Contains(n.Name)).ThenBy(n => n.Name, StringComparer.Ordinal)];
    }

    /// <summary>Parses `docker images --format '{{json .}}'`: dangling images last.</summary>
    public static List<Image> ParseImages(string text)
    {
        var images = new List<Image>();
        foreach (var raw in JsonLines(text))
        {
            var id = Field(raw, "ID", true);
            var repository = Field(raw, "Repository", true);
            var tag = Field(raw, "Tag", true);
            var size = Field(raw, "Size", false);
            var created = Field(raw, "CreatedSince", false);
            if (id == null || repository == null || tag == null || size == null || created == null)
            {
                continue;
            }
            while (id.StartsWith("sha256:", StringComparison.Ordinal))
            {
                id = id["sha256:".Length..];
            }
            images.Add(new Image(Prefix(id, 12), repository, tag, size, created));
        }
        return [.. images.OrderBy(i => i.Repository == "<none>").ThenBy(i => i.Repository, StringComparer.Ordinal).ThenBy(i => i.Tag, StringComparer.Ordinal)];
    }

    static string Prefix(string text, int length) => text.Length > length ? text[..length] : text;

    // ───────────── names ─────────────

    /// <summary>`Dockerfile`, `Containerfile` and their variants (`Dockerfile.worker`, `api.dockerfile`).</summary>
    public static bool IsDockerfileName(string name)
    {
        name = name.ToLowerInvariant();
        foreach (var @base in new[] { "dockerfile", "containerfile" })
        {
            if (name == @base)
            {
                return true;
            }
            if (name.Length - @base.Length > 1 && (name.StartsWith(@base + ".", StringComparison.Ordinal) || name.EndsWith("." + @base, StringComparison.Ordinal)))
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>`compose.yaml`, `docker-compose.yml`, … and their variants (`docker-compose.override.yml`, `compose.prod.yaml`).</summary>
    public static bool IsComposeFileName(string name)
    {
        name = name.ToLowerInvariant();
        string stem;
        if (name.EndsWith(".yml", StringComparison.Ordinal))
        {
            stem = name[..^4];
        }
        else if (name.EndsWith(".yaml", StringComparison.Ordinal))
        {
            stem = name[..^5];
        }
        else
        {
            return false;
        }
        foreach (var @base in new[] { "compose", "docker-compose" })
        {
            if (stem == @base)
            {
                return true;
            }
            if (stem.StartsWith(@base, StringComparison.Ordinal) && stem[@base.Length..] is { Length: > 1 } rest && rest[0] == '.')
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>Splits workspace files into (Dockerfiles, Compose files).</summary>
    public static (List<string> Dockerfiles, List<string> Compose) DockerFiles(IEnumerable<string> files)
    {
        var list = files.ToList();
        return ([.. list.Where(p => IsDockerfileName(PyPath.Basename(p)))], [.. list.Where(p => IsComposeFileName(PyPath.Basename(p)))]);
    }

    /// <summary>An image name from a folder or project name: lowercase, with only the characters Docker allows.</summary>
    public static string ImageName(string name)
    {
        var output = new StringBuilder();
        foreach (var raw in name)
        {
            var ascii = raw < 128;
            var c = ascii ? char.ToLowerInvariant(raw) : raw;
            var separatorOk = output.Length > 0 && output[^1] is not ('.' or '_' or '-');
            if ((ascii && char.IsAsciiLetterOrDigit(c)) || (c is '.' or '_' or '-' && separatorOk))
            {
                output.Append(c);
            }
            else if (separatorOk)
            {
                output.Append('-');
            }
        }
        var result = output.ToString().TrimEnd('.', '_', '-');
        return result.Length > 0 ? result : "app";
    }

    /// <summary>The image name for a Dockerfile: its folder's name (`Api/Dockerfile` → `api`), or with the variant (`Dockerfile.worker` → `&lt;folder&gt;-worker`).</summary>
    public static string ImageNameFor(string dockerfile)
    {
        var folder = PyPath.Basename(PyPath.Dirname(dockerfile));
        if (folder.Length == 0)
        {
            folder = "app";
        }
        var file = PyPath.Basename(dockerfile).ToLowerInvariant();
        string? variant = null;
        foreach (var prefix in new[] { "dockerfile.", "containerfile." })
        {
            if (file.StartsWith(prefix, StringComparison.Ordinal))
            {
                variant = file[prefix.Length..];
                break;
            }
        }
        if (variant == null)
        {
            foreach (var suffix in new[] { ".dockerfile", ".containerfile" })
            {
                if (file.EndsWith(suffix, StringComparison.Ordinal))
                {
                    variant = file[..^suffix.Length];
                    break;
                }
            }
        }
        return variant != null ? ImageName($"{folder}-{variant}") : ImageName(folder);
    }

    // ───────────── command lines ─────────────

    public const string ShellScript = "command -v bash >/dev/null && exec bash || exec sh";

    static readonly System.Buffers.SearchValues<char> SafeChars =
        System.Buffers.SearchValues.Create("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./:=@%+,");

    /// <summary>Quotes an argument for POSIX shells and fish (`'…'`, with `'` written as `'\''`).</summary>
    public static string ShellQuote(string arg) =>
        arg.Length > 0 && !arg.AsSpan().ContainsAnyExcept(SafeChars) ? arg : "'" + arg.Replace("'", "'\\''") + "'";

    /// <summary>A command line from arguments, quoted for the shell.</summary>
    public static string CommandLine(IEnumerable<string> args) => string.Join(" ", args.Select(ShellQuote));

    /// <summary>
    /// Arguments that start an image's container in the background (ports published) and then open a shell at `/`
    /// in it, in the same terminal. Leaving the shell removes the container. When the container doesn't stay up
    /// (its command exits right away), its logs are shown instead.
    /// </summary>
    public static string[] RunShellArgs(string reference)
    {
        var script = string.Join("; ",
            "id=$(docker run --detach --interactive --tty --publish-all \"$1\") || exit",
            "docker port \"$id\"",
            "if [ \"$(docker inspect --format '{{.State.Running}}' \"$id\")\" = true ]",
            "then docker exec --interactive --tty --workdir / \"$id\" sh -c " + ShellQuote(ShellScript),
            "else echo \"The container exited:\"; docker logs \"$id\"",
            "fi",
            "docker rm --force \"$id\" >/dev/null");
        return ["sh", "-c", script, "sh", reference];
    }

    // ───────────── setup ─────────────

    /// <summary>Package names for the missing Compose and Buildx plugins.</summary>
    public static List<string> PluginPackages(string tool, bool compose, bool buildx)
    {
        var (composeName, buildxName) = tool == "pacman" ? ("docker-compose", "docker-buildx") : ("docker-compose-plugin", "docker-buildx-plugin");
        var packages = new List<string>();
        if (compose)
        {
            packages.Add(composeName);
        }
        if (buildx)
        {
            packages.Add(buildxName);
        }
        return packages;
    }

    /// <summary>
    /// Linux: (listed, active) for the `docker` group from /proc/self/status, /etc/group and /etc/passwd; null
    /// when there's no such group, the user is unknown, or we're root.
    /// </summary>
    public static (bool Listed, bool Active)? DockerGroup(string status, string groups, string passwd)
    {
        long? Field(string name, int index)
        {
            foreach (var line in status.Split('\n'))
            {
                if (line.StartsWith(name + ":", StringComparison.Ordinal))
                {
                    var values = line[(name.Length + 1)..].Split([' ', '\t'], StringSplitOptions.RemoveEmptyEntries);
                    return values.Length > index && long.TryParse(values[index], out var v) ? v : null;
                }
            }
            return null;
        }
        if (Field("Uid", 0) is not { } uid || uid == 0 || Field("Gid", 0) is not { } gid)
        {
            return null;
        }
        var active = new HashSet<long>();
        foreach (var line in status.Split('\n'))
        {
            if (line.StartsWith("Groups:", StringComparison.Ordinal))
            {
                foreach (var g in line["Groups:".Length..].Split([' ', '\t'], StringSplitOptions.RemoveEmptyEntries))
                {
                    if (long.TryParse(g, out var value))
                    {
                        active.Add(value);
                    }
                }
            }
        }
        string[]? group = groups.Split('\n').Select(l => l.TrimEnd('\r').Split(':')).FirstOrDefault(f => f.Length >= 4 && f[0] == "docker");
        if (group == null || !long.TryParse(group[2], out var dockerGid))
        {
            return null;
        }
        var user = passwd.Split('\n').Select(l => l.TrimEnd('\r').Split(':')).FirstOrDefault(f => f.Length >= 4 && long.TryParse(f[2], out var u) && u == uid);
        if (user == null || !long.TryParse(user[3], out var userGid))
        {
            return null;
        }
        var members = group[3].Split(',', StringSplitOptions.RemoveEmptyEntries);
        var listed = members.Contains(user[0]) || userGid == dockerGid;
        return (listed, active.Contains(dockerGid) || gid == dockerGid);
    }

    // ───────────── build contexts ─────────────

    static bool DirHas(string directory, params string[] extensions)
    {
        try
        {
            return Directory.EnumerateFileSystemEntries(directory).Any(p => extensions.Contains(PyPath.Extension(Path.GetFileName(p))));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or ArgumentException)
        {
            return false;
        }
    }

    public static bool HasSolution(string directory) => DirHas(directory, "sln", "slnx");

    static bool PathExists(string path) => File.Exists(path) || Directory.Exists(path);

    /// <summary>
    /// The build context for a Dockerfile: its folder, unless it sits next to a .NET project in a workspace with a
    /// solution at <paramref name="root"/> and no `.dockerignore` of its own. Visual Studio's Dockerfiles build
    /// from the solution folder (they copy `Project/Project.csproj`).
    /// </summary>
    public static string BuildContext(string dockerfile, string root)
    {
        var directory = PyPath.Dirname(dockerfile);
        if (directory.Length == 0)
        {
            return root;
        }
        if (PyPath.Same(directory, root) || PathExists(PyPath.Join(directory, ".dockerignore")))
        {
            return directory;
        }
        var solutionRoot = HasSolution(root) || PathExists(PyPath.Join(root, ".dockerignore"));
        if (PyPath.StartsWith(directory, root) && DirHas(directory, "csproj", "fsproj", "vbproj") && solutionRoot)
        {
            return root;
        }
        return directory;
    }

    // ───────────── workspace files ─────────────

    public const int MaxFiles = 100_000;
    static readonly HashSet<string> SkipDirs = [".git", "bin", "obj", "node_modules", ".vs", ".idea"];

    /// <summary>All files below <paramref name="root"/>, sorted, skipping build output, VCS and IDE folders and other dot-folders.</summary>
    public static List<string> WorkspaceFiles(string? root)
    {
        var result = new List<string>();
        if (string.IsNullOrEmpty(root))
        {
            return result;
        }
        var stack = new Stack<string>();
        stack.Push(root);
        while (stack.Count > 0 && result.Count < MaxFiles)
        {
            var directory = stack.Pop();
            List<FileSystemInfo> entries;
            try
            {
                entries = [.. new DirectoryInfo(directory).EnumerateFileSystemInfos()];
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException or System.Security.SecurityException or ArgumentException)
            {
                continue;
            }
            foreach (var entry in entries)
            {
                var path = PyPath.Join(directory, entry.Name);
                try
                {
                    if (entry is DirectoryInfo)
                    {
                        if (entry.LinkTarget == null && !SkipDirs.Contains(entry.Name) && !entry.Name.StartsWith('.'))
                        {
                            stack.Push(path);
                        }
                    }
                    else if (File.Exists(path))
                    {
                        result.Add(path);
                    }
                }
                catch (Exception e) when (e is IOException or UnauthorizedAccessException)
                {
                }
            }
        }
        result.Sort(PyPath.Order);
        return result;
    }

    // ───────────── .NET projects ─────────────

    public const string DefaultDotnetVersion = "10.0";

    /// <summary>The `.csproj` files below <paramref name="root"/> (up to 4 folders deep), shallowest first.</summary>
    public static List<string> DotnetProjects(string root)
    {
        var output = new List<string>();
        void Collect(string directory, int depth)
        {
            if (depth > 4)
            {
                return;
            }
            List<FileSystemInfo> entries;
            try
            {
                entries = [.. new DirectoryInfo(directory).EnumerateFileSystemInfos()];
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException or System.Security.SecurityException or ArgumentException)
            {
                return;
            }
            foreach (var entry in entries)
            {
                var path = PyPath.Join(directory, entry.Name);
                if (Directory.Exists(path))
                {
                    if (entry.Name is not ("bin" or "obj" or ".git" or "node_modules" or "target"))
                    {
                        Collect(path, depth + 1);
                    }
                }
                else if (entry.Name.EndsWith(".csproj", StringComparison.Ordinal))
                {
                    output.Add(path);
                }
            }
        }
        Collect(root, 0);
        return [.. output.OrderBy(p => PyPath.Parts(p).Parts.Count).ThenBy(p => p, PyPath.Order)];
    }

    public static string ReadText(string path) => File.ReadAllText(path, new UTF8Encoding(false)).Replace("\r\n", "\n").Replace('\r', '\n');

    /// <summary>Whether the project builds an app (console, ASP.NET Core, worker) rather than a library.</summary>
    public static bool IsApp(string project)
    {
        string lower;
        try
        {
            lower = ReadText(project).ToLowerInvariant();
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return false;
        }
        string[] markers = ["<outputtype>exe</outputtype>", "<outputtype>winexe</outputtype>", "microsoft.net.sdk.web", "microsoft.net.sdk.worker"];
        return markers.Any(lower.Contains);
    }

    /// <summary>The text of the first `&lt;name&gt;…&lt;/name&gt;` element (null when missing or empty).</summary>
    public static string? XmlValue(string text, string name)
    {
        var open = $"<{name}>";
        var start = text.IndexOf(open, StringComparison.Ordinal);
        if (start < 0)
        {
            return null;
        }
        start += open.Length;
        var end = text.IndexOf($"</{name}>", start, StringComparison.Ordinal);
        if (end < 0)
        {
            return null;
        }
        var value = text[start..end].Trim();
        return value.Length > 0 ? value : null;
    }

    /// <summary>Values of <paramref name="attribute"/> on every `&lt;element …&gt;` tag.</summary>
    public static List<string> AttributeValues(string text, string element, string attribute)
    {
        var output = new List<string>();
        var open = $"<{element}";
        var rest = text;
        while (true)
        {
            var at = rest.IndexOf(open, StringComparison.Ordinal);
            if (at < 0)
            {
                break;
            }
            rest = rest[(at + open.Length)..];
            var close = rest.IndexOf('>');
            var tag = rest[..(close >= 0 ? close : rest.Length)];
            var key = $"{attribute}=";
            var k = tag.IndexOf(key, StringComparison.Ordinal);
            if (k >= 0)
            {
                var value = tag[(k + key.Length)..];
                if (value.Length > 0 && value[0] is '"' or '\'')
                {
                    var end = value.IndexOf(value[0], 1);
                    if (end >= 0)
                    {
                        output.Add(value[1..end]);
                    }
                }
            }
        }
        return output;
    }

    // A `.dockerignore` for .NET solutions (keeps build output, IDE files and secrets out of the context).
    public const string DotnetDockerignore = """
        **/.classpath
        **/.dockerignore
        **/.env
        **/.git
        **/.gitignore
        **/.project
        **/.settings
        **/.toolstarget
        **/.vs
        **/.vscode
        **/.idea
        **/*.*proj.user
        **/*.dbmdl
        **/*.jfm
        **/azds.yaml
        **/bin
        **/charts
        **/docker-compose*
        **/compose.y*ml
        **/Dockerfile*
        **/node_modules
        **/npm-debug.log
        **/obj
        **/secrets.dev.yaml
        **/values.dev.yaml
        LICENSE
        README.md
        !**/.gitignore
        !.git/HEAD
        !.git/config
        !.git/packed-refs
        !.git/refs/heads/**

        """;

    static string DirSlash(string d) => d.Length == 0 ? "./" : $"{d}/";

    /// <summary>
    /// A multi-stage Dockerfile for <paramref name="project"/>, built with <paramref name="context"/> as the build
    /// context (Visual Studio's layout: restore with only the project files first so that layer is cached, then
    /// build and publish).
    /// </summary>
    public static string DotnetDockerfile(DotnetProject project, string context)
    {
        var v = project.Version;
        var projectRel = PyPath.Relative(project.Path, context);
        var projectDir = PyPath.PosixDirname(projectRel);
        var projectFile = PyPath.PosixBasename(projectRel);
        var s = new StringBuilder();
        s.Append("# Build:  docker build -f ").Append(DirSlash(projectDir)).Append("Dockerfile -t ").Append(ImageName(project.Assembly));
        s.Append(" .\n# (run from the build context: the folder that contains ").Append(projectDir.Length == 0 ? "this file" : "the solution").Append(")\n\n");
        if (project.Web)
        {
            s.Append($"FROM mcr.microsoft.com/dotnet/aspnet:{v} AS base\nUSER $APP_UID\nWORKDIR /app\nEXPOSE 8080\nEXPOSE 8081\n\n");
        }
        else
        {
            s.Append($"FROM mcr.microsoft.com/dotnet/runtime:{v} AS base\nUSER $APP_UID\nWORKDIR /app\n\n");
        }
        s.Append($"FROM mcr.microsoft.com/dotnet/sdk:{v} AS build\nARG BUILD_CONFIGURATION=Release\nWORKDIR /src\n");
        // Restore inputs first, so the restore layer is reused until a project or package version changes.
        foreach (var shared in new[] { "Directory.Build.props", "Directory.Build.targets", "Directory.Packages.props", "NuGet.config", "nuget.config", "global.json" })
        {
            if (File.Exists(PyPath.Join(context, shared)))
            {
                s.Append($"COPY [\"{shared}\", \".\"]\n");
            }
        }
        foreach (var path in project.References().Where(r => PyPath.StartsWith(r, context)).Prepend(project.Path))
        {
            var rel = PyPath.Relative(path, context);
            s.Append($"COPY [\"{rel}\", \"{DirSlash(PyPath.PosixDirname(rel))}\"]\n");
        }
        s.Append($"RUN dotnet restore \"./{projectRel}\"\nCOPY . .\nWORKDIR \"/src/{projectDir}\"\n");
        s.Append($"RUN dotnet build \"./{projectFile}\" -c $BUILD_CONFIGURATION -o /app/build\n\n");
        s.Append("FROM build AS publish\nARG BUILD_CONFIGURATION=Release\n");
        s.Append($"RUN dotnet publish \"./{projectFile}\" -c $BUILD_CONFIGURATION -o /app/publish /p:UseAppHost=false\n\n");
        s.Append($"FROM base AS final\nWORKDIR /app\nCOPY --from=publish /app/publish .\nENTRYPOINT [\"dotnet\", \"{project.Assembly}.dll\"]\n");
        return s.ToString().Replace("WORKDIR \"/src/\"", "WORKDIR \"/src\"");
    }

    /// <summary>A Compose service: its name, Dockerfile, build context and whether it's a web app.</summary>
    public sealed record Service(string Name, string Dockerfile, string Context, bool Web);

    /// <summary>A Compose service per Dockerfile, with unique names, its build context and whether it's a web app.</summary>
    public static List<Service> ComposeServices(IEnumerable<string> dockerfiles, string root)
    {
        var names = new HashSet<string>();
        var services = new List<Service>();
        foreach (var dockerfile in dockerfiles)
        {
            string text;
            try
            {
                text = ReadText(dockerfile);
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                text = "";
            }
            var @base = ImageNameFor(dockerfile);
            var name = @base;
            for (var n = 2; names.Contains(name); n++)
            {
                name = $"{@base}-{n}";
            }
            names.Add(name);
            services.Add(new Service(name, dockerfile, BuildContext(dockerfile, root), text.Contains("EXPOSE") || text.Contains("dotnet/aspnet")));
        }
        return services;
    }

    /// <summary>A `compose.yaml` (in <paramref name="directory"/>) that builds and runs the services. Web services get host ports 8080, 8090, …</summary>
    public static string ComposeFile(IEnumerable<Service> services, string directory)
    {
        var s = new StringBuilder("# Start: docker compose up --build    Stop: docker compose down\nservices:\n");
        var port = 8080;
        foreach (var service in services)
        {
            var context = PyPath.Relative(service.Context, directory);
            if (context.Length == 0)
            {
                context = ".";
            }
            else if (!context.StartsWith('.') && !context.StartsWith('/'))
            {
                context = $"./{context}";
            }
            var dockerfile = PyPath.Relative(service.Dockerfile, service.Context);
            s.Append($"  {service.Name}:\n    image: {service.Name}\n    build:\n      context: {context}\n      dockerfile: {dockerfile}\n");
            if (service.Web)
            {
                s.Append($"    ports:\n      - \"{port}:8080\"\n    environment:\n      - ASPNETCORE_ENVIRONMENT=Development\n");
                port += 10;
            }
        }
        return s.ToString();
    }
}

/// <summary>What a Dockerfile for a .NET project needs to know about it.</summary>
public sealed partial record DotnetProject(string Path, string Assembly, string Version, bool Web)
{
    [GeneratedRegex("^net\\+?([0-9]+)\\.\\+?([0-9]+)$")]
    private static partial Regex Framework();

    /// <summary>Reads a project file (throws IOException / UnauthorizedAccessException).</summary>
    public static DotnetProject Read(string path)
    {
        var text = DockerData.ReadText(path);
        var stem = PyPath.Stem(PyPath.Basename(path));
        if (stem.Length == 0)
        {
            stem = "App";
        }
        var assembly = DockerData.XmlValue(text, "AssemblyName");
        if (assembly == null || assembly.Contains('$'))
        {
            assembly = stem;
        }
        var frameworks = DockerData.XmlValue(text, "TargetFramework") ?? DockerData.XmlValue(text, "TargetFrameworks") ?? "";
        // The highest modern `netX.Y` (OS-specific ones like net9.0-windows can't run in Linux containers).
        (BigInteger Major, BigInteger Minor)? best = null;
        foreach (var framework in frameworks.Split(';'))
        {
            var m = Framework().Match(framework.Trim());
            if (!m.Success)
            {
                continue;
            }
            var major = BigInteger.Parse(m.Groups[1].Value);
            var minor = BigInteger.Parse(m.Groups[2].Value);
            if (major >= 5 && (best is not { } top || major > top.Major || (major == top.Major && minor > top.Minor)))
            {
                best = (major, minor);
            }
        }
        var lower = text.ToLowerInvariant();
        var web = lower.Contains("microsoft.net.sdk.web") || lower.Contains("microsoft.net.sdk.razor") || lower.Contains("microsoft.net.sdk.blazorwebassembly");
        return new DotnetProject(path, assembly, best is { } b ? $"{b.Major}.{b.Minor}" : DockerData.DefaultDotnetVersion, web);
    }

    /// <summary>The projects this one references, transitively (`&lt;ProjectReference Include="…"&gt;`), excluding itself.</summary>
    public List<string> References()
    {
        var seen = new List<string> { PyPath.Normpath(Path) };
        var queue = new List<string> { Path };
        var output = new List<string>();
        while (queue.Count > 0)
        {
            var project = queue[^1];
            queue.RemoveAt(queue.Count - 1);
            string text;
            try
            {
                text = DockerData.ReadText(project);
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
                continue;
            }
            var directory = PyPath.Dirname(project);
            if (directory.Length == 0)
            {
                directory = ".";
            }
            foreach (var include in DockerData.AttributeValues(text, "ProjectReference", "Include"))
            {
                var path = PyPath.Normpath(PyPath.Join(directory, include.Replace('\\', '/')));
                if (!seen.Contains(path))
                {
                    seen.Add(path);
                    output.Add(path);
                    queue.Add(path);
                }
            }
        }
        output.Sort(PyPath.Order);
        return output;
    }
}
