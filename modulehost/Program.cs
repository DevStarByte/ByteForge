// ByteForge's .NET module host: one process that runs every .NET module, each in its own collectible
// AssemblyLoadContext, so a module can be loaded, unloaded and replaced (update, uninstall) while the others run.
//
// Lines on stdin and stdout are "<module id>\t<JSON message>". The messages are the module protocol
// (docs/modules.md), passed through unchanged; an empty module id is the host itself:
//
//   IDE -> host   \t{"host":"load","module":"id","dir":"<module folder>","assembly":"bin/X.dll"}
//                 \t{"host":"unload","module":"id"}
//   host -> IDE   \t{"host":"loaded","module":"id"}
//                 \t{"host":"unloaded","module":"id"}
//                 \t{"host":"exited","module":"id","reason":"…"}   (the module ended or failed)
//
// A module is a class library with a public static method
//
//   Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel)
//
// that reads the IDE's messages from `input` (null at the end) and writes its own to `output`, one per line.
// The module SDK (ByteForge.Modules, on the packages branch) provides one; the host needs nothing else from a
// module, so modules built against any SDK version run here. Assemblies are read into memory, never left
// open, so an uninstall or update can replace a module's files while it's loaded.

using System.Collections.Concurrent;
using System.Reflection;
using System.Runtime.Loader;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Channels;

namespace ByteForge.ModuleHost;

public static class Program
{
    static readonly object outLock = new();
    static readonly StreamWriter stdout = new(Console.OpenStandardOutput(), new UTF8Encoding(false)) { AutoFlush = false, NewLine = "\n" };
    static readonly ConcurrentDictionary<string, LoadedModule> modules = new();

    public static int Main()
    {
        using var stdin = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false));
        string? line;
        while ((line = stdin.ReadLine()) != null)
        {
            var tab = line.IndexOf('\t');
            if (tab < 0)
            {
                continue;
            }
            var id = line[..tab];
            var payload = line[(tab + 1)..];
            if (id.Length == 0)
            {
                Control(payload);
            }
            else if (modules.TryGetValue(id, out var module))
            {
                module.Input.Writer.TryWrite(payload);
            }
        }
        // The IDE is gone: end every module (briefly), then the process.
        Parallel.ForEach(modules.Keys.ToArray(), id => Unload(id, reply: false));
        return 0;
    }

    /// Writes one line for `module` ("" for the host) to the IDE.
    internal static void Emit(string module, string json)
    {
        lock (outLock)
        {
            stdout.Write(module);
            stdout.Write('\t');
            stdout.Write(json);
            stdout.Write('\n');
            stdout.Flush();
        }
    }

    static void HostMessage(string kind, string module, string? reason = null)
    {
        var message = new JsonObject { ["host"] = kind, ["module"] = module };
        if (reason != null)
        {
            message["reason"] = reason;
        }
        Emit("", message.ToJsonString());
    }

    static void Control(string payload)
    {
        JsonNode? message;
        try
        {
            message = JsonNode.Parse(payload);
        }
        catch (JsonException)
        {
            return;
        }
        var kind = message?["host"]?.GetValue<string>();
        var id = message?["module"]?.GetValue<string>();
        if (string.IsNullOrEmpty(id))
        {
            return;
        }
        switch (kind)
        {
            case "load":
                Load(id, message!["dir"]?.GetValue<string>() ?? "", message["assembly"]?.GetValue<string>() ?? "");
                break;
            case "unload":
                // Off the reading thread: other modules' messages keep flowing while this one winds down.
                Task.Run(() => Unload(id, reply: true));
                break;
        }
    }

    static void Load(string id, string dir, string assembly)
    {
        if (modules.ContainsKey(id))
        {
            Unload(id, reply: false);
        }
        var path = Path.GetFullPath(Path.Combine(dir, assembly));
        ModuleContext? context = null;
        try
        {
            if (!File.Exists(path))
            {
                throw new FileNotFoundException($"{assembly} is missing from the module's folder");
            }
            context = new ModuleContext(id, path);
            var asm = context.LoadFile(path);
            var entry = FindEntry(asm) ?? throw new MissingMethodException(
                $"{Path.GetFileName(path)} has no public static Task ByteForgeMain(TextReader, TextWriter, string, CancellationToken)");
            var module = new LoadedModule(id, context);
            modules[id] = module;
            var reader = new ChannelLineReader(module.Input.Reader);
            var writer = new ModuleWriter(id);
            // A thread of its own: a module may block reading its input.
            module.Run = Task.Factory.StartNew(() =>
            {
                var task = (Task?)entry.Invoke(null, [reader, writer, Path.GetFullPath(dir), module.Cancel.Token]) ?? Task.CompletedTask;
                return task;
            }, CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default).Unwrap();
            module.Run.ContinueWith(t => Ended(module, t), TaskScheduler.Default);
            HostMessage("loaded", id);
        }
        catch (Exception e)
        {
            modules.TryRemove(id, out _);
            context?.Unload();
            var error = e is TargetInvocationException { InnerException: { } inner } ? inner : e;
            HostMessage("exited", id, $"could not load: {error.Message}");
        }
    }

    /// The module's `ByteForgeMain`, on any exported type.
    static MethodInfo? FindEntry(Assembly asm)
    {
        Type[] signature = [typeof(TextReader), typeof(TextWriter), typeof(string), typeof(CancellationToken)];
        foreach (var type in asm.GetExportedTypes())
        {
            var method = type.GetMethod("ByteForgeMain", BindingFlags.Public | BindingFlags.Static, signature);
            if (method != null && typeof(Task).IsAssignableFrom(method.ReturnType))
            {
                return method;
            }
        }
        return null;
    }

    /// The module's main task ended: by itself (or a failure), or because it's being unloaded.
    static void Ended(LoadedModule module, Task run)
    {
        if (module.Unloading)
        {
            return;
        }
        modules.TryRemove(module.Id, out _);
        var reason = run.Exception?.GetBaseException() is { } e ? $"failed: {e.GetType().Name}: {e.Message}" : "ended";
        if (run.Exception?.GetBaseException() is { } failure)
        {
            Emit(module.Id, new JsonObject { ["type"] = "log", ["text"] = failure.ToString() }.ToJsonString());
        }
        module.Context.Unload();
        HostMessage("exited", module.Id, reason);
    }

    static void Unload(string id, bool reply)
    {
        if (modules.TryRemove(id, out var module))
        {
            module.Unloading = true;
            // Input ends (ReadLine returns null) and the token is cancelled; a module that doesn't stop in a few
            // seconds is left to the garbage collector (its files aren't held open either way).
            module.Input.Writer.TryComplete();
            module.Cancel.Cancel();
            try
            {
                module.Run?.Wait(TimeSpan.FromSeconds(3));
            }
            catch (AggregateException)
            {
            }
            module.Context.Unload();
        }
        if (reply)
        {
            HostMessage("unloaded", id);
        }
    }
}

sealed class LoadedModule(string id, ModuleContext context)
{
    public string Id { get; } = id;
    public ModuleContext Context { get; } = context;
    public Channel<string> Input { get; } = Channel.CreateUnbounded<string>(new UnboundedChannelOptions { SingleReader = true });
    public CancellationTokenSource Cancel { get; } = new();
    public Task? Run { get; set; }
    public volatile bool Unloading;
}

/// A module's assemblies: its own folder first (through its .deps.json when there is one), the shared framework
/// for the rest. Files are read into memory so none stays open.
sealed class ModuleContext : AssemblyLoadContext
{
    readonly string dir;
    readonly AssemblyDependencyResolver? resolver;

    public ModuleContext(string name, string mainAssembly) : base(name, isCollectible: true)
    {
        dir = Path.GetDirectoryName(mainAssembly)!;
        try
        {
            resolver = new AssemblyDependencyResolver(mainAssembly);
        }
        catch (InvalidOperationException)
        {
            resolver = null;
        }
    }

    public Assembly LoadFile(string path)
    {
        using var image = new MemoryStream(File.ReadAllBytes(path));
        var pdbPath = Path.ChangeExtension(path, ".pdb");
        using var pdb = File.Exists(pdbPath) ? new MemoryStream(File.ReadAllBytes(pdbPath)) : null;
        return LoadFromStream(image, pdb);
    }

    protected override Assembly? Load(AssemblyName name)
    {
        var path = resolver?.ResolveAssemblyToPath(name);
        if (path == null && name.Name != null)
        {
            var beside = Path.Combine(dir, name.Name + ".dll");
            path = File.Exists(beside) ? beside : null;
        }
        // Not the module's: the shared framework (default context).
        return path != null && File.Exists(path) ? LoadFile(path) : null;
    }

    protected override IntPtr LoadUnmanagedDll(string unmanagedDllName)
    {
        var path = resolver?.ResolveUnmanagedDllToPath(unmanagedDllName);
        return path != null ? LoadUnmanagedDllFromPath(path) : IntPtr.Zero;
    }
}

/// The IDE's messages for one module, one line at a time; null once the module is being unloaded.
sealed class ChannelLineReader(ChannelReader<string> lines) : TextReader
{
    string? current;
    int position;

    public override string? ReadLine()
    {
        if (current != null)
        {
            var rest = current[position..];
            current = null;
            return rest;
        }
        return Next();
    }

    public override async ValueTask<string?> ReadLineAsync(CancellationToken cancellationToken)
    {
        if (current != null)
        {
            return ReadLine();
        }
        try
        {
            return await lines.ReadAsync(cancellationToken);
        }
        catch (ChannelClosedException)
        {
            return null;
        }
    }

    public override Task<string?> ReadLineAsync() => ReadLineAsync(CancellationToken.None).AsTask();

    public override int Peek()
    {
        if (!Fill())
        {
            return -1;
        }
        return position < current!.Length ? current[position] : '\n';
    }

    public override int Read()
    {
        if (!Fill())
        {
            return -1;
        }
        if (position < current!.Length)
        {
            return current[position++];
        }
        current = null;
        return '\n';
    }

    bool Fill()
    {
        if (current != null)
        {
            return true;
        }
        current = Next();
        position = 0;
        return current != null;
    }

    string? Next()
    {
        try
        {
            return lines.ReadAsync().AsTask().GetAwaiter().GetResult();
        }
        catch (ChannelClosedException)
        {
            return null;
        }
    }
}

/// What a module writes: each line goes to the IDE with the module's id in front.
sealed class ModuleWriter(string module) : TextWriter
{
    readonly StringBuilder pending = new();

    public override Encoding Encoding => Encoding.UTF8;

    public override void Write(char value)
    {
        lock (pending)
        {
            if (value == '\n')
            {
                FlushLine();
            }
            else if (value != '\r')
            {
                pending.Append(value);
            }
        }
    }

    public override void Write(string? value)
    {
        if (value == null)
        {
            return;
        }
        lock (pending)
        {
            foreach (var c in value)
            {
                if (c == '\n')
                {
                    FlushLine();
                }
                else if (c != '\r')
                {
                    pending.Append(c);
                }
            }
        }
    }

    public override void WriteLine(string? value)
    {
        lock (pending)
        {
            Write(value);
            FlushLine();
        }
    }

    public override void WriteLine() => Write('\n');

    void FlushLine()
    {
        if (pending.Length > 0)
        {
            Program.Emit(module, pending.ToString());
            pending.Clear();
        }
    }
}
