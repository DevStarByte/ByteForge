//! The .NET module host: one `dotnet ByteForge.ModuleHost.dll` process (see `modulehost/Program.cs`) runs every
//! module whose `module.json` says `"runtime": "dotnet"`, each in its own load context. Lines both ways are
//! `<module id>\t<message>`; the messages are the usual module protocol, an empty id is the host itself
//! (`load`, `unload` from here; `loaded`, `unloaded`, `exited` back). The host starts with the first .NET
//! module and starts again with the next one after it ends.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};

use crate::plugins::{ModuleManifest, PluginMessage, parse};

/// Where `ByteForge.ModuleHost.dll` is (set at start-up; see [`crate::AppPaths::module_host`]).
static HOST_DLL: RwLock<Option<PathBuf>> = RwLock::new(None);
/// The running host, if any.
static HOST: Mutex<Option<Arc<HostProcess>>> = Mutex::new(None);

pub fn set_host_dll(path: PathBuf) {
    *HOST_DLL.write() = Some(path);
}

pub struct HostProcess {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    alive: AtomicBool,
    /// Where each loaded module's messages go.
    routes: Mutex<HashMap<String, flume::Sender<PluginMessage>>>,
    /// Modules being unloaded, told when the host has let them go.
    unloads: Mutex<HashMap<String, flume::Sender<()>>>,
}

impl HostProcess {
    /// The running host, started if needed.
    fn get() -> anyhow::Result<Arc<Self>> {
        let mut slot = HOST.lock();
        if let Some(host) = slot.as_ref().filter(|h| h.alive.load(Ordering::SeqCst)) {
            return Ok(host.clone());
        }
        let dll = HOST_DLL.read().clone().ok_or_else(|| anyhow::anyhow!("the .NET module host isn't set up"))?;
        anyhow::ensure!(dll.is_file(), "the .NET module host is missing ({})", dll.display());
        let host = Arc::new(Self::start(&dll)?);
        *slot = Some(host.clone());
        Ok(host)
    }

    fn start(dll: &Path) -> anyhow::Result<Self> {
        let mut command = crate::process::command("dotnet");
        command
            .arg(dll)
            .current_dir(dll.parent().unwrap_or(Path::new(".")))
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // See `PluginHandle::start`: programs a module looks up come from PATH, not the current folder.
        #[cfg(windows)]
        command.env("NoDefaultCurrentDirectoryInExePath", "1");
        let mut child = command.spawn().map_err(|e| anyhow::anyhow!("could not start the .NET module host (is .NET 10 installed?): {e}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;
        let stderr = child.stderr.take();
        let host = Self { stdin: Mutex::new(stdin), child: Mutex::new(child), alive: AtomicBool::new(true), routes: Mutex::default(), unloads: Mutex::default() };
        host.listen(stdout, stderr);
        Ok(host)
    }

    /// Reads the host's output on background threads. The threads find the host through [`HOST`] (they start
    /// before it's stored there, so they wait for it on the first line).
    fn listen(&self, stdout: std::process::ChildStdout, stderr: Option<std::process::ChildStderr>) {
        let pid = self.child.lock().id();
        std::thread::spawn(move || {
            let host = || HOST.lock().clone().filter(|h| h.child.lock().id() == pid);
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some((module, message)) = line.split_once('\t') else { continue };
                let Some(host) = host() else { continue };
                if module.is_empty() {
                    host.on_control(message);
                } else if let Some(tx) = host.routes.lock().get(module)
                    && let Some(m) = parse(module, message)
                {
                    let _ = tx.send(m);
                }
            }
            // The host ended: every module it ran has stopped.
            if let Some(host) = host() {
                host.alive.store(false, Ordering::SeqCst);
                for (module, tx) in host.routes.lock().drain() {
                    let _ = tx.send(PluginMessage::Exited { module, reason: "the .NET module host ended".into() });
                }
                for (_, done) in host.unloads.lock().drain() {
                    let _ = done.send(());
                }
                let _ = host.child.lock().wait();
            }
        });
        if let Some(stderr) = stderr {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::warn!("module host: {line}");
                }
            });
        }
    }

    fn on_control(&self, message: &str) {
        let Ok(v) = serde_json::from_str::<Value>(message) else { return };
        let Some(module) = v.get("module").and_then(Value::as_str) else { return };
        match v.get("host").and_then(Value::as_str) {
            Some("unloaded") => {
                if let Some(done) = self.unloads.lock().remove(module) {
                    let _ = done.send(());
                }
            }
            Some("exited") => {
                let reason = v.get("reason").and_then(Value::as_str).unwrap_or("ended").to_owned();
                if let Some(tx) = self.routes.lock().remove(module) {
                    let _ = tx.send(PluginMessage::Exited { module: module.to_owned(), reason });
                }
            }
            _ => {}
        }
    }

    fn write(&self, module: &str, message: &str) {
        let mut stdin = self.stdin.lock();
        let _ = writeln!(stdin, "{module}\t{message}");
        let _ = stdin.flush();
    }
}

/// A .NET module loaded in the host.
pub struct HostedModule {
    id: String,
    host: Arc<HostProcess>,
}

impl HostedModule {
    pub fn load(manifest: &ModuleManifest, events: flume::Sender<PluginMessage>) -> anyhow::Result<Self> {
        let assembly = manifest.assembly.clone().filter(|a| !a.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("module.json names no \"assembly\""))?;
        let host = HostProcess::get()?;
        host.routes.lock().insert(manifest.id.clone(), events);
        host.write("", &json!({ "host": "load", "module": manifest.id, "dir": manifest.dir, "assembly": assembly }).to_string());
        Ok(Self { id: manifest.id.clone(), host })
    }

    pub fn send(&self, message: &Value) {
        self.host.write(&self.id, &message.to_string());
    }

    /// Unloads the module and waits (a few seconds at most) until the host has let it go, so its folder can be
    /// replaced or removed next.
    pub fn stop(&self) {
        self.host.routes.lock().remove(&self.id);
        if !self.host.alive.load(Ordering::SeqCst) {
            return;
        }
        let (tx, rx) = flume::bounded(1);
        self.host.unloads.lock().insert(self.id.clone(), tx);
        self.host.write("", &json!({ "host": "unload", "module": self.id }).to_string());
        let _ = rx.recv_timeout(Duration::from_secs(5));
        self.host.unloads.lock().remove(&self.id);
    }
}

/// Ends the host (at exit; modules get no further messages).
pub fn shutdown() {
    if let Some(host) = HOST.lock().take() {
        host.alive.store(false, Ordering::SeqCst);
        let _ = host.child.lock().kill();
    }
}
