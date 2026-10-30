use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;

/// Finds the build target: an explicit setting, else a .sln/.slnx in the root, else the shallowest .csproj.
/// For `run`, prefers an executable project.
pub fn find_target(root: &Path, configured: Option<&str>, runnable: bool) -> Option<PathBuf> {
    if let Some(configured) = configured.filter(|c| !c.is_empty()) {
        let path = root.join(configured);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut projects = Vec::new();
    collect(root, 0, &mut projects);
    projects.sort_by_key(|p| p.components().count());
    if runnable {
        return projects.iter().find(|p| is_executable(p)).or(projects.first()).cloned();
    }
    let solution = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).find(|p| {
        matches!(p.extension().and_then(|e| e.to_str()), Some("sln" | "slnx"))
    });
    solution.or_else(|| projects.first().cloned())
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

pub enum RunEvent {
    Line(String),
    Exited { code: Option<i32>, seconds: f32 },
}

/// A running `dotnet` process whose output streams to a channel.
#[derive(Clone)]
pub struct DotnetProcess {
    child: Arc<Mutex<Option<Child>>>,
}

impl DotnetProcess {
    pub fn start(root: &Path, args: Vec<String>) -> anyhow::Result<(Self, flume::Receiver<RunEvent>)> {
        let mut command = Command::new("dotnet");
        command
            .args(&args)
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
        let mut child = command.spawn()?;
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
        Ok((Self { child }, rx))
    }

    pub fn is_running(&self) -> bool {
        self.child.lock().is_some()
    }

    pub fn kill(&self) {
        if let Some(child) = self.child.lock().as_mut() {
            #[cfg(windows)]
            {
                // Kill the whole tree (dotnet run spawns the app as a child).
                let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).output();
            }
            let _ = child.kill();
        }
    }
}
