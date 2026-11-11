//! Debugging .NET apps inside Docker containers: the running containers and their processes, putting
//! netcoredbg into a container (`docker cp`, no tools needed in the image), and mapping source paths
//! between the workspace and the container (where the app was built, e.g. `/src/App/Program.cs`).
//!
//! netcoredbg binds a breakpoint whose path is relative by matching it against the end of the paths in
//! the program's symbols, so breakpoints are sent as the shortest unique workspace-relative path
//! (`App/Program.cs`) and need no knowledge of the container's layout. Stack frames come back with
//! container paths and are mapped to the workspace file whose path ends the same way.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context as _;

/// Where netcoredbg goes inside the container.
pub const DEBUGGER_DIR: &str = "/tmp/byteforge-netcoredbg";

/// A running container (`docker ps`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
}

/// A process inside a container (its own PID there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerProcess {
    pub pid: u32,
    pub command: String,
}

fn docker() -> Command {
    let mut command = Command::new("docker");
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

fn run(args: &[&str]) -> anyhow::Result<String> {
    let output = docker().args(args).output().context("could not run docker (is it installed and on PATH?)")?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim())
    }
}

/// The running containers.
pub fn containers() -> anyhow::Result<Vec<Container>> {
    Ok(parse_containers(&run(&["ps", "--format", "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}"])?))
}

pub fn parse_containers(text: &str) -> Vec<Container> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            Some(Container { id: f.next()?.to_owned(), name: f.next()?.to_owned(), image: f.next()?.to_owned(), status: f.next().unwrap_or_default().to_owned() })
        })
        .collect()
}

/// The processes in a container: with `ps` when the image has it, else read from `/proc` with the shell,
/// else (images without a shell) just PID 1, the entry point.
pub fn processes(container: &str) -> Vec<ContainerProcess> {
    if let Ok(text) = run(&["exec", container, "ps", "-eo", "pid=,args="]) {
        let found = parse_processes(&text);
        if !found.is_empty() {
            return found;
        }
    }
    let script = r#"for d in /proc/[0-9]*; do c=$(tr '\000' ' ' < "$d/cmdline" 2>/dev/null); [ -n "$c" ] && echo "${d#/proc/} $c"; done"#;
    if let Ok(text) = run(&["exec", container, "sh", "-c", script]) {
        let found = parse_processes(&text);
        if !found.is_empty() {
            return found;
        }
    }
    vec![ContainerProcess { pid: 1, command: "(entry point)".into() }]
}

/// Parses `<pid> <command line>` lines.
pub fn parse_processes(text: &str) -> Vec<ContainerProcess> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, command) = line.split_once(' ').unwrap_or((line, ""));
            Some(ContainerProcess { pid: pid.parse().ok()?, command: command.trim().to_owned() })
        })
        .collect()
}

/// The processes that can be .NET apps: everything but shells, `ps` itself, the debugger and the usual
/// keep-alive commands. `dotnet …` processes first.
pub fn app_candidates(processes: &[ContainerProcess]) -> Vec<ContainerProcess> {
    let helper = |c: &str| {
        let exe = c.split_whitespace().next().unwrap_or_default();
        let name = exe.rsplit('/').next().unwrap_or(exe);
        matches!(name, "sh" | "bash" | "ash" | "dash" | "ps" | "tr" | "sleep" | "tail" | "cat" | "tini" | "dumb-init" | "netcoredbg" | "vsdbg") || c.contains("byteforge-netcoredbg")
    };
    let mut out: Vec<ContainerProcess> = processes.iter().filter(|p| !helper(&p.command)).cloned().collect();
    out.sort_by_key(|p| (!p.command.split_whitespace().next().unwrap_or_default().ends_with("dotnet"), p.pid));
    out
}

/// The CPU architecture of the container's image, as Docker names it (`amd64`, `arm64`).
pub fn architecture(container: &str) -> Option<String> {
    let image = run(&["inspect", "--format", "{{.Image}}", container]).ok()?;
    let arch = run(&["image", "inspect", "--format", "{{.Architecture}}", image.trim()]).ok()?;
    Some(arch.trim().to_owned()).filter(|a| !a.is_empty())
}

/// Whether netcoredbg is in the container and runs there.
pub fn debugger_ready(container: &str) -> bool {
    run(&["exec", container, &format!("{DEBUGGER_DIR}/netcoredbg"), "--version"]).is_ok()
}

/// The host's netcoredbg folder, when it can run in the container: the host is Linux on the same
/// architecture, and `netcoredbg` is the release's self-contained folder (it has its libraries next to it).
pub fn host_debugger_dir(host_netcoredbg: &str, arch: &str) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") || docker_arch(std::env::consts::ARCH) != arch {
        return None;
    }
    let exe = std::fs::canonicalize(host_netcoredbg).ok()?;
    let dir = exe.parent()?.to_path_buf();
    dir.join("libdbgshim.so").exists().then_some(dir)
}

/// Rust's architecture name as Docker's.
fn docker_arch(rust: &str) -> &str {
    match rust {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Downloads the Linux netcoredbg release for `arch` (once, into `cache`) and returns its folder.
pub fn download_debugger(arch: &str, cache: &Path) -> anyhow::Result<PathBuf> {
    let dir = cache.join(format!("netcoredbg-linux-{arch}"));
    let exe = dir.join("netcoredbg").join("netcoredbg");
    if exe.exists() {
        return Ok(exe.parent().unwrap_or(&dir).to_path_buf());
    }
    let asset = match arch {
        "amd64" => "netcoredbg-linux-amd64.tar.gz",
        "arm64" => "netcoredbg-linux-arm64.tar.gz",
        other => anyhow::bail!("there is no netcoredbg release for {other} containers"),
    };
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(asset);
    let url = format!("https://github.com/Samsung/netcoredbg/releases/latest/download/{asset}");
    let out = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--location", "--max-time", "600", "--output"])
        .arg(&archive)
        .arg(&url)
        .stdin(Stdio::null())
        .output()
        .context("could not run curl")?;
    if !out.status.success() {
        anyhow::bail!("downloading {url} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    // tar ships with Linux, macOS and Windows 10+.
    let out = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&dir).stdin(Stdio::null()).output().context("could not run tar")?;
    let _ = std::fs::remove_file(&archive);
    if !out.status.success() || !exe.exists() {
        anyhow::bail!("unpacking {asset} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(exe.parent().unwrap_or(&dir).to_path_buf())
}

/// Copies the netcoredbg folder `dir` into the container and checks that it runs there.
pub fn install_debugger(container: &str, dir: &Path) -> anyhow::Result<()> {
    // "dir/." copies the folder's contents into DEBUGGER_DIR (created if missing).
    let source = format!("{}/.", dir.display());
    run(&["cp", "--follow-link", &source, &format!("{container}:{DEBUGGER_DIR}")]).context("copying netcoredbg into the container failed")?;
    if !debugger_ready(container) {
        anyhow::bail!("netcoredbg doesn't run in this container (images based on Alpine/musl aren't supported by netcoredbg's releases)");
    }
    Ok(())
}

/// How to start the adapter: `docker exec -i <container> …/netcoredbg --interpreter=vscode`.
pub fn adapter_args(container: &str) -> Vec<String> {
    vec!["exec".into(), "-i".into(), container.into(), format!("{DEBUGGER_DIR}/netcoredbg"), "--interpreter=vscode".into()]
}

/// Path components, for comparing paths from different systems.
fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect()
}

/// The path to send a breakpoint with: the shortest tail of `file`'s workspace-relative path that no other
/// workspace file shares (`Program.cs`, or `App/Program.cs` when several projects have one).
pub fn breakpoint_path(file: &Path, root: &Path, files: &[PathBuf]) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file).to_string_lossy().replace('\\', "/");
    let parts = components(&rel);
    for n in 1..=parts.len() {
        let tail = &parts[parts.len() - n..];
        let shared = files.iter().filter(|f| f.as_path() != file).any(|f| {
            let other = f.to_string_lossy();
            let other = components(&other);
            other.len() >= n && other[other.len() - n..] == *tail
        });
        if !shared {
            return tail.join("/");
        }
    }
    parts.join("/")
}

/// The workspace file a container path stands for: the one sharing the longest tail of components with
/// it (at least the file name). Ties go to the shallower file.
pub fn host_path(container_path: &str, files: &[PathBuf]) -> Option<PathBuf> {
    let wanted = components(container_path);
    let common = |f: &PathBuf| {
        let text = f.to_string_lossy();
        let have = components(&text);
        wanted.iter().rev().zip(have.iter().rev()).take_while(|(a, b)| a == b).count()
    };
    files.iter().filter(|f| common(f) > 0).max_by_key(|f| (common(f), std::cmp::Reverse(f.components().count()))).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<PathBuf> {
        ["/w/App/Program.cs", "/w/Api/Program.cs", "/w/App/Models/Order.cs", "/w/README.md"].iter().map(PathBuf::from).collect()
    }

    #[test]
    fn breakpoints_use_the_shortest_unique_tail() {
        let root = Path::new("/w");
        assert_eq!(breakpoint_path(Path::new("/w/App/Program.cs"), root, &files()), "App/Program.cs");
        assert_eq!(breakpoint_path(Path::new("/w/App/Models/Order.cs"), root, &files()), "Order.cs");
    }

    #[test]
    fn container_paths_map_to_workspace_files() {
        assert_eq!(host_path("/src/App/Program.cs", &files()), Some(PathBuf::from("/w/App/Program.cs")));
        assert_eq!(host_path("/src/Api/Program.cs", &files()), Some(PathBuf::from("/w/Api/Program.cs")));
        assert_eq!(host_path("/src/Models/Order.cs", &files()), Some(PathBuf::from("/w/App/Models/Order.cs")));
        assert_eq!(host_path("/usr/share/dotnet/Something.cs", &files()), None);
        assert_eq!(host_path("C:\\src\\App\\Program.cs", &files()), Some(PathBuf::from("/w/App/Program.cs")));
    }

    #[test]
    fn parses_docker_output() {
        let c = parse_containers("abc123\tshop-web-1\tshop-web:dev\tUp 5 minutes\n");
        assert_eq!((c[0].name.as_str(), c[0].image.as_str()), ("shop-web-1", "shop-web:dev"));
        let p = parse_processes("    1 dotnet App.dll\n   27 sh -c ps\n   33 ps -eo pid=,args=\n");
        assert_eq!(p.len(), 3);
        let apps = app_candidates(&p);
        assert_eq!(apps, [ContainerProcess { pid: 1, command: "dotnet App.dll".into() }]);
        // An apphost (no "dotnet") still counts; dotnet processes come first.
        let p = parse_processes("1 /app/Shop\n7 dotnet Worker.dll\n9 /bin/bash\n");
        assert_eq!(app_candidates(&p).iter().map(|p| p.pid).collect::<Vec<_>>(), [7, 1]);
    }
}
