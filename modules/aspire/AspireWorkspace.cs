using System.Text;

namespace ByteForge.Aspire;

/// <summary>The workspace's AppHosts, their names and paths, and command lines for a terminal.</summary>
public static class AspireWorkspace
{
    public const int MaxFiles = 100_000;

    static readonly HashSet<string> SkipDirs = [".git", "bin", "obj", "node_modules", ".vs", ".idea", "target", ".aspire"];

    /// <summary>
    /// AppHosts that aren't projects: a file of one of these names (TypeScript, JavaScript, Go, Rust, Java and
    /// file-based C# AppHosts).
    /// </summary>
    public static readonly HashSet<string> FileApphosts =
        ["apphost.cs", "apphost.ts", "apphost.mts", "apphost.cts", "apphost.js", "apphost.mjs", "apphost.cjs", "apphost.go", "apphost.rs", "apphost.java"];

    const string SafeChars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./:=@%+,";

    /// <summary>Whether a .csproj is an Aspire AppHost (the AppHost SDK, or the older IsAspireHost property).</summary>
    public static bool IsApphostProject(string text) =>
        text.Contains("Aspire.AppHost.Sdk", StringComparison.Ordinal) || text.Replace(" ", "").Contains("<IsAspireHost>true</IsAspireHost>", StringComparison.Ordinal);

    /// <summary>Whether a file is an AppHost on its own (apphost.ts, a file-based apphost.cs, …).</summary>
    public static bool IsFileApphost(string name, string text, bool inProjectDir)
    {
        var lower = name.ToLowerInvariant();
        if (!FileApphosts.Contains(lower) && name != "AppHost.java")
        {
            return false;
        }
        if (lower == "apphost.cs")
        {
            // In a project folder AppHost.cs is the project's source; a file-based one names the SDK itself.
            return !inProjectDir && text.Contains("Aspire.AppHost.Sdk", StringComparison.Ordinal);
        }
        return text.ToLowerInvariant().Contains("aspire", StringComparison.Ordinal);
    }

    /// <summary>The AppHosts under <paramref name="root"/>, as paths (project files, or the AppHost file itself).</summary>
    public static List<string> FindApphosts(string? root)
    {
        var hosts = new List<string>();
        if (string.IsNullOrEmpty(root) || !Directory.Exists(root))
        {
            return hosts;
        }
        var seen = 0;
        Walk(root, hosts, ref seen);
        return hosts;
    }

    /// <summary>One folder, then its subfolders in name order; false once too many files were seen.</summary>
    static bool Walk(string directory, List<string> hosts, ref int seen)
    {
        List<string> files = [], dirs = [];
        try
        {
            foreach (var entry in new DirectoryInfo(directory).EnumerateFileSystemInfos())
            {
                if (entry is DirectoryInfo dir)
                {
                    // Linked folders aren't followed (loops, and other trees).
                    if (dir.LinkTarget == null && !dir.Attributes.HasFlag(FileAttributes.ReparsePoint) && !SkipDirs.Contains(dir.Name) && !dir.Name.StartsWith('.'))
                    {
                        dirs.Add(dir.Name);
                    }
                }
                else
                {
                    files.Add(entry.Name);
                }
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or System.Security.SecurityException)
        {
            return true;
        }
        seen += files.Count;
        if (seen > MaxFiles)
        {
            return false;
        }
        var projects = files.Any(f => f.EndsWith(".csproj", StringComparison.Ordinal));
        files.Sort(StringComparer.Ordinal);
        foreach (var name in files)
        {
            var path = Path.Combine(directory, name);
            try
            {
                if (name.EndsWith(".csproj", StringComparison.Ordinal))
                {
                    if (IsApphostProject(ReadText(path)))
                    {
                        hosts.Add(path);
                    }
                }
                else if (FileApphosts.Contains(name.ToLowerInvariant()) || name == "AppHost.java")
                {
                    if (IsFileApphost(name, ReadText(path), projects))
                    {
                        hosts.Add(path);
                    }
                }
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException)
            {
            }
        }
        dirs.Sort(StringComparer.Ordinal);
        foreach (var dir in dirs)
        {
            if (!Walk(Path.Combine(directory, dir), hosts, ref seen))
            {
                return false;
            }
        }
        return true;
    }

    /// <summary>The start of a file (200,000 characters), invalid UTF-8 replaced.</summary>
    static string ReadText(string path)
    {
        using var reader = new StreamReader(path, new UTF8Encoding(false, false), detectEncodingFromByteOrderMarks: false);
        var buffer = new char[200_000];
        var count = reader.ReadBlock(buffer, 0, buffer.Length);
        return new string(buffer, 0, count);
    }

    /// <summary>A path as a key: full, and on Windows in lower case.</summary>
    public static string PathKey(string path)
    {
        var full = Path.GetFullPath(path.Length == 0 ? "." : path);
        return OperatingSystem.IsWindows() ? full.ToLowerInvariant() : full;
    }

    /// <summary>What an AppHost is called in the view: the project name, or the folder of an AppHost file.</summary>
    public static string ApphostLabel(string path)
    {
        var name = Path.GetFileName(path);
        if (name.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase))
        {
            return name[..^".csproj".Length];
        }
        return Path.GetFileName(Dirname(path)) is { Length: > 0 } folder ? folder : name;
    }

    /// <summary>The file to open for an AppHost: AppHost.cs (or Program.cs) next to a project, else the file itself.</summary>
    public static string SourceFile(string path)
    {
        if (!path.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase))
        {
            return path;
        }
        var directory = Dirname(path);
        foreach (var name in new[] { "AppHost.cs", "apphost.cs", "Program.cs" })
        {
            var candidate = Path.Combine(directory, name);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }
        return path;
    }

    /// <summary><paramref name="path"/> relative to <paramref name="baseDir"/> when it's inside it, else as given.</summary>
    public static string Relative(string path, string? baseDir)
    {
        if (!string.IsNullOrEmpty(baseDir))
        {
            try
            {
                var full = Path.GetFullPath(path);
                var fullBase = Path.GetFullPath(baseDir);
                var comparison = OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal;
                if (string.Equals(Path.GetPathRoot(full), Path.GetPathRoot(fullBase), comparison))
                {
                    var rel = Path.GetRelativePath(fullBase, full);
                    if (!rel.StartsWith("..", StringComparison.Ordinal))
                    {
                        return rel;
                    }
                }
            }
            catch (Exception e) when (e is ArgumentException or NotSupportedException or PathTooLongException)
            {
            }
        }
        return path;
    }

    /// <summary>The folder part of a path as given (separators untouched, unlike Path.GetDirectoryName on Windows).</summary>
    public static string Dirname(string path)
    {
        var drive = OperatingSystem.IsWindows() && path.Length >= 2 && path[1] == ':' ? path[..2] : "";
        var rest = path[drive.Length..];
        var i = rest.Length;
        while (i > 0 && !IsSeparator(rest[i - 1]))
        {
            i--;
        }
        var head = rest[..i];
        var trimmed = OperatingSystem.IsWindows() ? head.TrimEnd('/', '\\') : head.TrimEnd('/');
        return drive + (trimmed.Length > 0 ? trimmed : head);
    }

    static bool IsSeparator(char c) => c == '/' || OperatingSystem.IsWindows() && c == '\\';

    /// <summary>An argument for the terminal's shell: as is when it's plain, else quoted (Windows: "…", else '…').</summary>
    public static string ShellQuote(string arg, bool windows)
    {
        if (arg.Length > 0 && arg.All(c => SafeChars.Contains(c)))
        {
            return arg;
        }
        return windows ? "\"" + arg.Replace("\"", "\\\"") + "\"" : "'" + arg.Replace("'", "'\\''") + "'";
    }

    public static string CommandLine(IEnumerable<string> args) => CommandLine(args, OperatingSystem.IsWindows());

    public static string CommandLine(IEnumerable<string> args, bool windows) => string.Join(" ", args.Select(a => ShellQuote(a, windows)));
}
