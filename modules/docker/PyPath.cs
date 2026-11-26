namespace ByteForge.Docker;

/// <summary>
/// Path helpers with the semantics the module has always had (Python's os.path): paths are kept as given (no
/// separator rewriting by the file system APIs), so item ids, tooltips and generated files stay the same.
/// On Windows both separators count, elsewhere only '/'.
/// </summary>
public static class PyPath
{
    static bool Windows => OperatingSystem.IsWindows();

    static bool IsSep(char c) => c == '/' || (Windows && c == '\\');

    /// <summary>The drive ("C:" or "\\server\share") of a Windows path; "" elsewhere.</summary>
    static (string Drive, string Tail) SplitDrive(string p)
    {
        if (!Windows || p.Length < 2)
        {
            return ("", p);
        }
        if (IsSep(p[0]) && IsSep(p[1]) && (p.Length < 3 || !IsSep(p[2])))
        {
            var server = IndexOfSep(p, 2);
            if (server < 0)
            {
                return (p, "");
            }
            var share = IndexOfSep(p, server + 1);
            return share < 0 ? (p, "") : (p[..share], p[share..]);
        }
        return p[1] == ':' ? (p[..2], p[2..]) : ("", p);
    }

    static int IndexOfSep(string p, int from)
    {
        for (var i = from; i < p.Length; i++)
        {
            if (IsSep(p[i]))
            {
                return i;
            }
        }
        return -1;
    }

    static int LastSep(string p)
    {
        for (var i = p.Length - 1; i >= 0; i--)
        {
            if (IsSep(p[i]))
            {
                return i;
            }
        }
        return -1;
    }

    /// <summary>os.path.basename.</summary>
    public static string Basename(string p)
    {
        var (_, rest) = SplitDrive(p);
        return rest[(LastSep(rest) + 1)..];
    }

    /// <summary>os.path.dirname.</summary>
    public static string Dirname(string p)
    {
        var (drive, rest) = SplitDrive(p);
        var head = rest[..(LastSep(rest) + 1)];
        if (head.Length > 0 && !head.All(IsSep))
        {
            head = head.TrimEnd(Windows ? ['/', '\\'] : ['/']);
        }
        return drive + head;
    }

    /// <summary>os.path.join for two parts.</summary>
    public static string Join(string a, string b) => Path.Combine(a, b);

    /// <summary>os.path.splitext: (root, extension with its dot); a name's leading dots don't start an extension.</summary>
    public static (string Root, string Ext) SplitExt(string name)
    {
        var dot = name.LastIndexOf('.');
        var sep = LastSep(name);
        if (dot > sep)
        {
            for (var i = sep + 1; i < dot; i++)
            {
                if (name[i] != '.')
                {
                    return (name[..dot], name[dot..]);
                }
            }
        }
        return (name, "");
    }

    /// <summary>The extension without the dot ("" for none).</summary>
    public static string Extension(string name) => SplitExt(name).Ext is { Length: > 0 } ext ? ext[1..] : "";

    /// <summary>The name without its extension.</summary>
    public static string Stem(string name) => SplitExt(name).Root;

    /// <summary>os.path.normpath: resolves "." and ".." without touching the file system.</summary>
    public static string Normpath(string p)
    {
        if (Windows)
        {
            p = p.Replace('/', '\\');
            var (drive, path) = SplitDrive(p);
            var root = "";
            if (path.StartsWith('\\'))
            {
                root = "\\";
                path = path.TrimStart('\\');
            }
            var parts = Resolve(path.Split('\\'), root.Length > 0);
            if (drive.Length == 0 && root.Length == 0 && parts.Count == 0)
            {
                return ".";
            }
            return drive + root + string.Join('\\', parts);
        }
        if (p.Length == 0)
        {
            return ".";
        }
        var initial = p.StartsWith('/') ? (p.StartsWith("//") && !p.StartsWith("///") ? 2 : 1) : 0;
        var comps = Resolve(p.Split('/'), initial > 0);
        var result = new string('/', initial) + string.Join('/', comps);
        return result.Length == 0 ? "." : result;
    }

    static List<string> Resolve(string[] comps, bool rooted)
    {
        var result = new List<string>();
        foreach (var comp in comps)
        {
            if (comp is "" or ".")
            {
                continue;
            }
            if (comp != "..")
            {
                result.Add(comp);
            }
            else if (result.Count > 0 && result[^1] != "..")
            {
                result.RemoveAt(result.Count - 1);
            }
            else if (!rooted)
            {
                result.Add(comp);
            }
        }
        return result;
    }

    /// <summary>The components of a path (normalized, "/" and "\" both separate) and whether it starts with a separator.</summary>
    public static (List<string> Parts, bool Absolute) Parts(string path) =>
        ([.. Normpath(path).Replace('\\', '/').Split('/').Where(p => p is not ("" or "."))], path.StartsWith('/') || path.StartsWith('\\'));

    /// <summary>Component-wise prefix test (like Rust's Path::starts_with).</summary>
    public static bool StartsWith(string path, string @base)
    {
        var (p, pAbs) = Parts(path);
        var (b, bAbs) = Parts(@base);
        return pAbs == bAbs && p.Count >= b.Count && p.Take(b.Count).SequenceEqual(b, StringComparer.Ordinal);
    }

    /// <summary>Whether two paths are the same once normalized.</summary>
    public static bool Same(string a, string b) => Normpath(a) == Normpath(b);

    /// <summary><paramref name="path"/> relative to <paramref name="base"/> with "/" ("" when equal), or the path itself (with "/") when it isn't below it.</summary>
    public static string Relative(string path, string @base)
    {
        if (StartsWith(path, @base))
        {
            return string.Join('/', Parts(path).Parts.Skip(Parts(@base).Parts.Count));
        }
        return path.Replace('\\', '/');
    }

    /// <summary>Sorts paths component by component, like Rust's PathBuf ordering.</summary>
    public static readonly IComparer<string> Order = Comparer<string>.Create((a, b) => CompareParts(Parts(a).Parts, Parts(b).Parts));

    public static int CompareParts(List<string> a, List<string> b)
    {
        for (var i = 0; i < Math.Min(a.Count, b.Count); i++)
        {
            var c = string.CompareOrdinal(a[i], b[i]);
            if (c != 0)
            {
                return c;
            }
        }
        return a.Count.CompareTo(b.Count);
    }

    /// <summary>posixpath.dirname (for the "/" paths of generated files).</summary>
    public static string PosixDirname(string p)
    {
        var i = p.LastIndexOf('/') + 1;
        var head = p[..i];
        return head.Length > 0 && head.Any(c => c != '/') ? head.TrimEnd('/') : head;
    }

    /// <summary>posixpath.basename.</summary>
    public static string PosixBasename(string p) => p[(p.LastIndexOf('/') + 1)..];
}
