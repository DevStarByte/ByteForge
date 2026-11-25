using System.Diagnostics;
using System.Text;

namespace ByteForge.Modules;

/// <summary>Finding and running programs.</summary>
public static class Programs
{
    /// <summary>
    /// The full path of a program on PATH, or null. Only PATH is searched, never the current folder (Windows'
    /// own lookup searches that first, and with PATHEXT a module's own files could be "found"). On Windows a
    /// name without an extension is tried with .exe, .cmd, .bat and .com.
    /// </summary>
    public static string? Which(string name)
    {
        if (Path.IsPathRooted(name))
        {
            return File.Exists(name) ? name : null;
        }
        string[] names = OperatingSystem.IsWindows() && !Path.HasExtension(name) ? [name + ".exe", name + ".cmd", name + ".bat", name + ".com"] : [name];
        foreach (var dir in (Environment.GetEnvironmentVariable("PATH") ?? "").Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            foreach (var candidate in names)
            {
                string full;
                try
                {
                    full = Path.Combine(dir.Trim('"'), candidate);
                }
                catch (ArgumentException)
                {
                    continue;
                }
                if (File.Exists(full) && (OperatingSystem.IsWindows() || IsExecutable(full)))
                {
                    return full;
                }
            }
        }
        return null;
    }

    static bool IsExecutable(string path)
    {
        if (OperatingSystem.IsWindows())
        {
            return true;
        }
        try
        {
            return (File.GetUnixFileMode(path) & (UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute)) != 0;
        }
        catch (IOException)
        {
            return false;
        }
    }

    /// <summary>The user's home folder.</summary>
    public static string Home => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);

    /// <summary>
    /// Runs a program to its end: its arguments as given (no shell), <paramref name="input"/> on its stdin, its
    /// output as UTF-8. A program that runs past <paramref name="timeout"/> (default one minute) is killed and
    /// <see cref="TimeoutException"/> thrown; one that can't start throws <see cref="ModuleException"/>.
    /// </summary>
    public static async Task<ProcessResult> RunAsync(string program, IEnumerable<string> arguments, string? input = null, string? workingDirectory = null,
        TimeSpan? timeout = null, IDictionary<string, string?>? environment = null, CancellationToken cancel = default)
    {
        var start = new ProcessStartInfo(program)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardOutputEncoding = new UTF8Encoding(false),
            StandardErrorEncoding = new UTF8Encoding(false),
            StandardInputEncoding = new UTF8Encoding(false),
        };
        if (workingDirectory != null)
        {
            start.WorkingDirectory = workingDirectory;
        }
        foreach (var argument in arguments)
        {
            start.ArgumentList.Add(argument);
        }
        if (environment != null)
        {
            foreach (var (key, value) in environment)
            {
                start.Environment[key] = value;
            }
        }
        Process process;
        try
        {
            process = Process.Start(start) ?? throw new ModuleException($"{program} didn't start");
        }
        catch (System.ComponentModel.Win32Exception e)
        {
            throw new ModuleException($"{program} couldn't be started: {e.Message}");
        }
        using (process)
        {
            using var limit = CancellationTokenSource.CreateLinkedTokenSource(cancel);
            limit.CancelAfter(timeout ?? TimeSpan.FromMinutes(1));
            var stdout = process.StandardOutput.ReadToEndAsync(limit.Token);
            var stderr = process.StandardError.ReadToEndAsync(limit.Token);
            try
            {
                if (input != null)
                {
                    await process.StandardInput.WriteAsync(input.AsMemory(), limit.Token);
                }
                process.StandardInput.Close();
                await process.WaitForExitAsync(limit.Token);
                return new ProcessResult(process.ExitCode, await stdout, await stderr);
            }
            catch (OperationCanceledException)
            {
                try
                {
                    process.Kill(entireProcessTree: true);
                }
                catch (InvalidOperationException)
                {
                }
                cancel.ThrowIfCancellationRequested();
                throw new TimeoutException($"{Path.GetFileName(program)} didn't finish in time");
            }
        }
    }
}

/// <summary>How a program ended and what it printed.</summary>
public sealed record ProcessResult(int ExitCode, string Output, string Error)
{
    public bool Success => ExitCode == 0;
}
