using ByteForge.Modules;

namespace ByteForge.Docker;

/// <summary>
/// What the module asks of the machine: running programs (docker, sg), finding programs on PATH (sg, systemctl,
/// the package managers, a custom compose command) and the user's docker group membership. Tests override it.
/// </summary>
public class DockerSystem
{
    /// <summary>Whether this is Linux (systemctl, the docker group).</summary>
    public virtual bool IsLinux => OperatingSystem.IsLinux();

    /// <summary>A program on PATH, or null.</summary>
    public virtual string? Which(string name) => Programs.Which(name);

    /// <summary>
    /// Runs <paramref name="argv"/> (program first; no shell, no input). Throws <see cref="ModuleException"/> when it
    /// can't be started and <see cref="TimeoutException"/> when it runs past <paramref name="timeout"/>.
    /// </summary>
    public virtual Task<ProcessResult> RunAsync(IReadOnlyList<string> argv, TimeSpan timeout, CancellationToken cancel)
    {
        var program = Which(argv[0]) ?? throw new ModuleException($"{argv[0]} was not found on PATH");
        return Programs.RunAsync(program, argv.Skip(1), timeout: timeout, cancel: cancel);
    }

    /// <summary>Linux: (listed, active) for the `docker` group, or null when there's no such group (or we're root).</summary>
    public virtual (bool Listed, bool Active)? DockerGroup()
    {
        if (!OperatingSystem.IsLinux())
        {
            return null;
        }
        try
        {
            return DockerData.DockerGroup(File.ReadAllText("/proc/self/status"), File.ReadAllText("/etc/group"), File.ReadAllText("/etc/passwd"));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return null;
        }
    }
}
