use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;

/// Finds the build target: an explicit setting, else a .sln/.slnx in the root, else the shallowest .csproj.
/// For `run` and debugging, prefers an executable project, and an app over an Aspire AppHost (an AppHost
/// starts the apps as separate processes, which the debugger doesn't follow).
pub fn find_target(root: &Path, configured: Option<&str>, runnable: bool) -> Option<PathBuf> {
    if let Some(configured) = configured.filter(|c| !c.is_empty()) {
        let path = root.join(configured);
        if path.is_file() {
            return Some(path);
        }
    }
    let projects = projects(root);
    if runnable {
        let apps = projects.iter().filter(|p| is_executable(p));
        return apps.clone().find(|p| !is_aspire_host(p)).or_else(|| apps.clone().next()).or(projects.first()).cloned();
    }
    let solution = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).find(|p| {
        matches!(p.extension().and_then(|e| e.to_str()), Some("sln" | "slnx"))
    });
    solution.or_else(|| projects.first().cloned())
}

/// The `.csproj` files below `root` (up to 4 folders deep), shallowest first.
pub fn projects(root: &Path) -> Vec<PathBuf> {
    let mut projects = Vec::new();
    collect(root, 0, &mut projects);
    projects.sort_by_key(|p| (p.components().count(), p.clone()));
    projects
}

/// Whether the project builds an app (console, ASP.NET Core, worker) rather than a library.
pub fn is_app(project: &Path) -> bool {
    is_executable(project)
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "bin" | "obj" | ".git" | "node_modules" | "target") {
                collect(&path, depth + 1, out);
            }
        } else if name.ends_with(".csproj") {
            out.push(path);
        }
    }
}

fn is_executable(project: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(project) else { return false };
    let lower = text.to_lowercase();
    lower.contains("<outputtype>exe</outputtype>") || lower.contains("<outputtype>winexe</outputtype>")
        || lower.contains("microsoft.net.sdk.web") || lower.contains("microsoft.net.sdk.worker")
}

fn is_aspire_host(project: &Path) -> bool {
    std::fs::read_to_string(project).is_ok_and(|text| text.to_lowercase().contains("aspire.apphost.sdk"))
}

pub enum RunEvent {
    Line(String),
    Exited { code: Option<i32>, seconds: f32 },
}

/// A running `dotnet` process whose output streams to a channel.
#[derive(Clone)]
pub struct DotnetProcess {
    child: Arc<Mutex<Option<Child>>>,
    /// The process id, which on Unix is also the id of the process group the process leads.
    pid: u32,
}

impl DotnetProcess {
    pub fn start(root: &Path, args: Vec<String>) -> anyhow::Result<(Self, flume::Receiver<RunEvent>)> {
        Self::start_program("dotnet", root, args, &[])
    }

    /// Like [`start`](Self::start) with extra environment variables (e.g. `VSTEST_HOST_DEBUG=1`).
    pub fn start_with_env(root: &Path, args: Vec<String>, env: &[(String, String)]) -> anyhow::Result<(Self, flume::Receiver<RunEvent>)> {
        Self::start_program("dotnet", root, args, env)
    }

    /// Starts any program the same way (`docker build`, …): output streamed, stoppable as a group.
    pub fn start_program(program: &str, root: &Path, args: Vec<String>, env: &[(String, String)]) -> anyhow::Result<(Self, flume::Receiver<RunEvent>)> {
        let mut command = Command::new(program);
        command
            .args(&args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(root)
            .env("DOTNET_NOLOGO", "1")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        // Lead a new process group, so stopping also ends the app `dotnet run` starts as a child.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;
        let pid = child.id();
        let (tx, rx) = flume::unbounded();
        for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as _)]
            .into_iter()
            .flatten()
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let _ = tx.send(RunEvent::Line(line));
                }
            });
        }
        let child = Arc::new(Mutex::new(Some(child)));
        let watcher = child.clone();
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let mut guard = watcher.lock();
                let Some(process) = guard.as_mut() else { break };
                if let Ok(Some(status)) = process.try_wait() {
                    // Give the reader threads a moment to flush the last lines.
                    drop(guard);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = tx.send(RunEvent::Exited { code: status.code(), seconds: started.elapsed().as_secs_f32() });
                    *watcher.lock() = None;
                    break;
                }
            }
        });
        Ok((Self { child, pid }, rx))
    }

    pub fn is_running(&self) -> bool {
        self.child.lock().is_some()
    }

    /// Stops the process and everything it started (`dotnet run` starts the app as a child process).
    /// On Unix the group first gets SIGTERM, so ASP.NET Core can shut down cleanly, then SIGKILL after 3 s.
    pub fn kill(&self) {
        #[cfg(unix)]
        {
            let group = format!("-{}", self.pid);
            let _ = Command::new("kill").args(["-TERM", "--", &group]).stderr(Stdio::null()).status();
            let child = self.child.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let _ = Command::new("kill").args(["-KILL", "--", &group]).stderr(Stdio::null()).status();
                if let Some(child) = child.lock().as_mut() {
                    let _ = child.kill();
                }
            });
        }
        #[cfg(windows)]
        if let Some(child) = self.child.lock().as_mut() {
            // Kill the whole tree.
            let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).output();
            let _ = child.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn kill_stops_child_processes() {
        let marker = format!("bf-kill-test-{}", std::process::id());
        // Like `dotnet run`: a parent that waits on a child doing the real work.
        let script = format!("sh -c 'exec -a {marker} sleep 120' & wait");
        let (process, events) = DotnetProcess::start_program("sh", Path::new("."), vec!["-c".into(), script], &[]).unwrap();
        let running = || Command::new("pgrep").args(["-f", &marker]).output().is_ok_and(|o| o.status.success());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !running() {
            assert!(std::time::Instant::now() < deadline, "child never started");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        process.kill();
        let exited = events.iter().any(|e| matches!(e, RunEvent::Exited { .. }));
        assert!(exited);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!running(), "the child process survived");
        assert!(!process.is_running());
    }

    #[test]
    fn runnable_target_prefers_apps_over_aspire_hosts() {
        let dir = std::env::temp_dir().join(format!("bf-dotnet-target-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |rel: &str, text: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("A.AppHost/A.AppHost.csproj", r#"<Project Sdk="Aspire.AppHost.Sdk/13.0.0"><PropertyGroup><OutputType>Exe</OutputType></PropertyGroup></Project>"#);
        write("B.Lib/B.Lib.csproj", r#"<Project Sdk="Microsoft.NET.Sdk"></Project>"#);
        write("C.Api/C.Api.csproj", r#"<Project Sdk="Microsoft.NET.Sdk.Web"></Project>"#);
        write("App.slnx", "<Solution />");
        let run = find_target(&dir, None, true).unwrap();
        assert!(run.ends_with("C.Api/C.Api.csproj"), "{}", run.display());
        assert!(find_target(&dir, None, false).unwrap().ends_with("App.slnx"));
        assert!(find_target(&dir, Some("A.AppHost/A.AppHost.csproj"), true).unwrap().ends_with("A.AppHost.csproj"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
