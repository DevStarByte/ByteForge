//! Out-of-process modules. A module is a folder in `modules/` containing `module.json`:
//!
//! ```json
//! { "id": "me.hello", "name": "Hello", "command": "python", "args": ["hello.py"] }
//! ```
//!
//! The host starts the command in the module folder and talks newline-delimited JSON over stdio.
//! Module → host: `registerCommand {id,title,category?,keybinding?}`, `status {id,text,tooltip?,command?,alignment?}`,
//! `log {text}`, `openFile {path,line?,column?}`, `showView {id}`.
//! Host → module: `activate {workspace}`, `executeCommand {id}`, `event {name, path?}` (folderOpened, fileSaved, themeChanged).
//! Modules can be written in any language and cannot crash the IDE.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize)]
pub struct ModuleManifest {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Clone, Debug)]
pub enum PluginMessage {
    RegisterCommand { module: String, id: String, title: String, category: Option<String>, keybinding: Option<String> },
    Status { module: String, id: String, text: String, tooltip: Option<String>, command: Option<String>, right: bool },
    Log { module: String, text: String },
    OpenFile { path: PathBuf, line: Option<usize>, column: Option<usize> },
    ShowView { id: String },
    Exited { module: String, reason: String },
}

pub fn discover(dirs: &[PathBuf]) -> Vec<ModuleManifest> {
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let manifest = entry.path().join("module.json");
            let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
            match json5::from_str::<ModuleManifest>(&text) {
                Ok(mut m) => {
                    m.dir = entry.path();
                    found.push(m);
                }
                Err(e) => log::warn!("invalid module manifest {}: {e}", manifest.display()),
            }
        }
    }
    found
}

/// A running module process.
#[derive(Clone)]
pub struct PluginHandle {
    pub manifest: ModuleManifest,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl PluginHandle {
    pub fn start(manifest: ModuleManifest, events: flume::Sender<PluginMessage>) -> anyhow::Result<Self> {
        let mut command = Command::new(&manifest.command);
        command.args(&manifest.args).current_dir(&manifest.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();
        let module = manifest.id.clone();

        {
            let module = module.clone();
            let events = events.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(message) = parse(&module, &line) {
                        let _ = events.send(message);
                    }
                }
                let _ = events.send(PluginMessage::Exited { module, reason: "process ended".into() });
            });
        }
        if let Some(stderr) = stderr {
            let module = module.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = events.send(PluginMessage::Log { module: module.clone(), text: line });
                }
            });
        }
        Ok(Self { manifest, stdin: Arc::new(Mutex::new(stdin)), child: Arc::new(Mutex::new(Some(child))) })
    }

    pub fn send(&self, message: Value) {
        if let Some(stdin) = self.stdin.lock().as_mut() {
            let _ = writeln!(stdin, "{message}");
            let _ = stdin.flush();
        }
    }

    pub fn activate(&self, workspace: Option<&Path>) {
        self.send(json!({ "type": "activate", "workspace": workspace }));
    }

    pub fn execute(&self, command: &str) {
        self.send(json!({ "type": "executeCommand", "id": command }));
    }

    pub fn event(&self, name: &str, path: Option<&Path>) {
        self.send(json!({ "type": "event", "name": name, "path": path }));
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
        }
    }
}

fn parse(module: &str, line: &str) -> Option<PluginMessage> {
    let v: Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let module = module.to_owned();
    Some(match v.get("type")?.as_str()? {
        "registerCommand" => PluginMessage::RegisterCommand { module, id: s("id")?, title: s("title")?, category: s("category"), keybinding: s("keybinding") },
        "status" => PluginMessage::Status {
            module,
            id: s("id")?,
            text: s("text").unwrap_or_default(),
            tooltip: s("tooltip"),
            command: s("command"),
            right: s("alignment").is_some_and(|a| a == "right"),
        },
        "log" => PluginMessage::Log { module, text: s("text").unwrap_or_default() },
        "openFile" => PluginMessage::OpenFile {
            path: PathBuf::from(s("path")?),
            line: v.get("line").and_then(Value::as_u64).map(|n| n as usize),
            column: v.get("column").and_then(Value::as_u64).map(|n| n as usize),
        },
        "showView" => PluginMessage::ShowView { id: s("id")? },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_protocol_messages() {
        let m = parse("m", r#"{"type":"registerCommand","id":"a.b","title":"Hi","keybinding":"ctrl-alt-h"}"#).unwrap();
        assert!(matches!(m, PluginMessage::RegisterCommand { ref id, ref keybinding, .. } if id == "a.b" && keybinding.as_deref() == Some("ctrl-alt-h")));
        assert!(parse("m", "not json").is_none());
        assert!(matches!(parse("m", r#"{"type":"status","id":"s","text":"x","alignment":"right"}"#), Some(PluginMessage::Status { right: true, .. })));
    }
}
