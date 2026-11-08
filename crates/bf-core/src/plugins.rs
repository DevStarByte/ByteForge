//! Out-of-process modules. A module is a folder in `modules/` containing `module.json`:
//!
//! ```json
//! { "id": "me.hello", "name": "Hello", "command": "python", "args": ["hello.py"] }
//! ```
//!
//! The host starts the command in the module folder and talks newline-delimited JSON over stdio.
//! Module → host: `registerCommand {id,title,category?,keybinding?}`, `status {id,text,tooltip?,command?,alignment?}`,
//! `log {text}`, `openFile {path,line?,column?}`, `showView {id}`, `insertText {text}` (at the caret(s) of the active editor), `setTheme {name}`,
//! `registerFormatter {id,name,extensions}`, `formatResult {requestId,text?|error?}`.
//! Host → module: `activate {workspace}`, `executeCommand {id}`, `event {name, path?}` (folderOpened, fileSaved, themeChanged, editorChanged),
//! `format {requestId,path,text}`.
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
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
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
    /// Insert text at the caret(s) of the active editor.
    InsertText { text: String },
    /// Switch to the color theme with this name (e.g. one the module wrote to the user themes folder).
    SetTheme { name: String },
    /// The module formats files with these extensions (without the dot).
    RegisterFormatter { module: String, id: String, name: String, extensions: Vec<String> },
    /// The answer to a `format` request: the formatted text, or why it couldn't be formatted.
    FormatResult { module: String, request: u64, result: Result<String, String> },
    Exited { module: String, reason: String },
}

/// Finds the installed modules (folders with a `module.json`) in `dir`, sorted by name. Preinstalled
/// modules were copied there on first start (see [`crate::registry::preinstall`]).
pub fn discover(dir: &Path) -> Vec<ModuleManifest> {
    let mut found: Vec<ModuleManifest> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue; // staging folders of installs in progress
        }
        let manifest = entry.path().join("module.json");
        let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
        match json5::from_str::<ModuleManifest>(&text) {
            Ok(mut m) => {
                m.dir = entry.path();
                if !found.iter().any(|f| f.id == m.id) {
                    found.push(m);
                }
            }
            Err(e) => log::warn!("invalid module manifest {}: {e}", manifest.display()),
        }
    }
    found.sort_by_key(|m| m.name.to_lowercase());
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

    /// Asks a formatter module to format `text` (the content of `path`); answered by `formatResult`.
    pub fn format(&self, request: u64, path: &Path, text: &str) {
        self.send(json!({ "type": "format", "requestId": request, "path": path, "text": text }));
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
        "insertText" => PluginMessage::InsertText { text: s("text")? },
        "setTheme" => PluginMessage::SetTheme { name: s("name")? },
        "registerFormatter" => PluginMessage::RegisterFormatter {
            id: s("id")?,
            name: s("name").unwrap_or_else(|| module.clone()),
            extensions: v
                .get("extensions")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|e| e.trim_start_matches('.').to_ascii_lowercase()).collect())
                .unwrap_or_default(),
            module,
        },
        "formatResult" => PluginMessage::FormatResult {
            request: v.get("requestId")?.as_u64()?,
            result: match (s("text"), s("error")) {
                (_, Some(error)) => Err(error),
                (Some(text), None) => Ok(text),
                (None, None) => Err("the formatter returned nothing".into()),
            },
            module,
        },
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
        let f = parse("m", r#"{"type":"registerFormatter","id":"csharpier","name":"CSharpier","extensions":[".cs","CSPROJ"]}"#).unwrap();
        assert!(matches!(f, PluginMessage::RegisterFormatter { ref extensions, .. } if extensions == &["cs", "csproj"]));
        let ok = parse("m", r#"{"type":"formatResult","requestId":7,"text":"x"}"#).unwrap();
        assert!(matches!(ok, PluginMessage::FormatResult { request: 7, result: Ok(ref t), .. } if t == "x"));
        let err = parse("m", r#"{"type":"formatResult","requestId":8,"error":"syntax"}"#).unwrap();
        assert!(matches!(err, PluginMessage::FormatResult { request: 8, result: Err(_), .. }));
    }
}
