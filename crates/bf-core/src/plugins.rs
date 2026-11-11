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
//!
//! A module can also contribute file icons declaratively (`"fileIcons"` in `module.json`); a module with
//! only icons leaves out `"command"` and isn't started.

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
    /// The program to start; empty for a module that only declares things like file icons.
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    /// The author's page; without it the author links to `https://github.com/<author>`.
    #[serde(default, rename = "authorUrl")]
    pub author_url: Option<String>,
    /// Side bar views the module fills with `setTree` (each gets an activity bar item).
    #[serde(default)]
    pub views: Vec<ViewContribution>,
    /// Languages (file matching, line comment, regex highlighting) the module adds.
    #[serde(default)]
    pub languages: Vec<LanguageContribution>,
    /// File icons the module adds while it's installed and turned on.
    #[serde(default, rename = "fileIcons")]
    pub file_icons: Vec<FileIconRule>,
    #[serde(skip)]
    pub dir: PathBuf,
}

/// A side bar view declared in `module.json`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ViewContribution {
    pub id: String,
    pub title: String,
    /// A built-in icon name or SVG path data (24×24 view box, even-odd fill).
    #[serde(default)]
    pub icon: String,
}

/// A language declared in `module.json`: which files it's for, its line comment and highlighting rules.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct LanguageContribution {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default, rename = "lineComment")]
    pub line_comment: Option<String>,
    #[serde(default)]
    pub rules: Vec<HighlightRule>,
}

impl LanguageContribution {
    pub fn matches(&self, file_name: &str) -> bool {
        FileIconRule { names: self.names.clone(), patterns: self.patterns.clone(), extensions: self.extensions.clone(), ..Default::default() }.matches(file_name)
    }
}

/// A regex highlighting rule: matches (or capture group `group` of them) get the theme token `token`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HighlightRule {
    #[serde(rename = "match")]
    pub pattern: String,
    pub token: String,
    #[serde(default)]
    pub group: usize,
}

/// A row of a module's tree view (`setTree`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TreeItem {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tooltip: Option<String>,
    /// Built-in icon name, SVG path data, or `file:<name>` for that file's icon.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, rename = "iconColor")]
    pub icon_color: Option<String>,
    #[serde(default)]
    pub header: bool,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub expanded: Option<bool>,
    #[serde(default)]
    pub children: Vec<TreeItem>,
    #[serde(default)]
    pub actions: Vec<TreeAction>,
    /// More entries for the row's context menu, after its `actions` (`{"id":"-"}` is a separator).
    #[serde(default)]
    pub menu: Vec<TreeAction>,
}

/// The content of a module's view (`setTree`).
#[derive(Clone, Debug, Default)]
pub struct TreeContent {
    pub view: String,
    pub items: Vec<TreeItem>,
    /// Text in the view's top row.
    pub message: Option<String>,
    pub error: Option<String>,
    /// Number on the activity bar item (0 = none).
    pub badge: u64,
    /// Buttons in the view's top row.
    pub actions: Vec<TreeAction>,
}

/// A question with buttons (`prompt`).
#[derive(Clone, Debug, Default)]
pub struct PromptRequest {
    pub request: u64,
    pub message: String,
    pub detail: Option<String>,
    pub buttons: Vec<String>,
}

/// A button on a tree row or in a view's top row.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TreeAction {
    pub id: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub tooltip: String,
    /// The entry's name in the context menu; the tooltip when left out.
    #[serde(default)]
    pub label: Option<String>,
}

impl TreeAction {
    /// The name to show in a menu.
    pub fn menu_label(&self) -> &str {
        self.label.as_deref().filter(|l| !l.is_empty()).unwrap_or(&self.tooltip)
    }

    pub fn is_separator(&self) -> bool {
        self.id == "-"
    }
}

/// An entry of a module's pick list (`quickPick`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PickItem {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub value: String,
}

/// A file icon from a module: SVG path data (24×24 view box, even-odd fill) for the files it matches.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FileIconRule {
    pub icon: String,
    /// A theme color key (`icon.docker`) or a color (`#2496ed`); the theme's file icon color when missing.
    #[serde(default)]
    pub color: Option<String>,
    /// Exact file names, ignoring case (`Dockerfile`).
    #[serde(default)]
    pub names: Vec<String>,
    /// File name patterns with `*` wildcards, ignoring case (`Dockerfile.*`, `docker-compose*.yml`).
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Extensions without the dot (`dockerfile`).
    #[serde(default)]
    pub extensions: Vec<String>,
}

impl FileIconRule {
    pub fn matches(&self, file_name: &str) -> bool {
        let name = file_name.to_lowercase();
        self.names.iter().any(|n| n.to_lowercase() == name)
            || self.patterns.iter().any(|p| glob_match(&p.to_lowercase(), &name))
            || Path::new(&name).extension().and_then(|e| e.to_str()).is_some_and(|ext| self.extensions.iter().any(|e| e.trim_start_matches('.').to_lowercase() == ext))
    }
}

/// `*` matches any run of characters (including none); everything else matches itself.
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
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
    /// Replaces the content of one of the module's side bar views.
    SetTree { module: String, tree: TreeContent },
    /// Runs a command line in a new terminal tab.
    RunInTerminal { title: String, command: String, cwd: Option<PathBuf>, save_all: bool },
    /// Shows a pick list; the choice is answered with `quickPickResult`.
    QuickPick { module: String, request: u64, placeholder: String, items: Vec<PickItem> },
    /// Asks a question with buttons; answered with `promptResult`.
    Prompt { module: String, prompt: PromptRequest },
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

    pub fn activate(&self, workspace: Option<&Path>, settings: &Value) {
        self.send(json!({ "type": "activate", "workspace": workspace, "settings": settings }));
    }

    /// A button or row of one of the module's views was clicked (`item` is `None` for view actions).
    pub fn tree_action(&self, view: &str, item: Option<&str>, action: &str) {
        self.send(json!({ "type": "treeAction", "view": view, "item": item, "action": action }));
    }

    pub fn view_event(&self, view: &str, shown: bool) {
        self.send(json!({ "type": "event", "name": if shown { "viewShown" } else { "viewHidden" }, "view": view }));
    }

    pub fn quick_pick_result(&self, request: u64, value: &str) {
        self.send(json!({ "type": "quickPickResult", "requestId": request, "value": value }));
    }

    pub fn prompt_result(&self, request: u64, index: Option<usize>) {
        self.send(json!({ "type": "promptResult", "requestId": request, "index": index }));
    }

    pub fn settings_changed(&self, settings: &Value) {
        self.send(json!({ "type": "event", "name": "settingsChanged", "settings": settings }));
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
        "setTree" => PluginMessage::SetTree {
            tree: TreeContent {
                view: s("view")?,
                items: v.get("items").cloned().and_then(|i| serde_json::from_value(i).ok()).unwrap_or_default(),
                message: s("message"),
                error: s("error"),
                badge: v.get("badge").and_then(Value::as_u64).unwrap_or(0),
                actions: v.get("actions").cloned().and_then(|a| serde_json::from_value(a).ok()).unwrap_or_default(),
            },
            module,
        },
        "runInTerminal" => PluginMessage::RunInTerminal {
            title: s("title").unwrap_or_else(|| module.clone()),
            command: s("command")?,
            cwd: s("cwd").map(PathBuf::from),
            save_all: v.get("saveAll").and_then(Value::as_bool).unwrap_or(false),
        },
        "quickPick" => PluginMessage::QuickPick {
            request: v.get("requestId")?.as_u64()?,
            placeholder: s("placeholder").unwrap_or_default(),
            items: v.get("items").cloned().and_then(|i| serde_json::from_value(i).ok()).unwrap_or_default(),
            module,
        },
        "prompt" => PluginMessage::Prompt {
            prompt: PromptRequest {
                request: v.get("requestId")?.as_u64()?,
                message: s("message")?,
                detail: s("detail"),
                buttons: v.get("buttons").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
            },
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
    #[test]
    fn tree_items_carry_menu_entries_and_labels() {
        let item: super::TreeItem = serde_json::from_str(
            r#"{"id":"c1","actions":[{"id":"remove","icon":"trash","tooltip":"Remove the container","label":"Remove…"}],
                "menu":[{"id":"-"},{"id":"copy","tooltip":"Copy ID"}]}"#,
        )
        .unwrap();
        assert_eq!(item.actions[0].menu_label(), "Remove…");
        assert!(item.menu[0].is_separator());
        assert_eq!(item.menu[1].menu_label(), "Copy ID");
    }

    #[test]
    fn file_icon_rules_match_names_patterns_and_extensions() {
        let rule = super::FileIconRule {
            icon: "M0,0Z".into(),
            names: vec!["Dockerfile".into()],
            patterns: vec!["Dockerfile.*".into(), "docker-compose*.yml".into()],
            extensions: vec!["dockerfile".into()],
            ..Default::default()
        };
        for name in ["Dockerfile", "dockerfile", "Dockerfile.dev", "app.dockerfile", "docker-compose.yml", "docker-compose.override.yml"] {
            assert!(rule.matches(name), "{name}");
        }
        for name in ["Dockerfile2", "compose.yaml", "docker-compose.yaml", "README.md"] {
            assert!(!rule.matches(name), "{name}");
        }
        assert!(super::glob_match("a*b*c", "a-x-b-y-c") && !super::glob_match("a*b*c", "a-c") && super::glob_match("*", "x"));
    }

    #[test]
    fn parses_view_and_dialog_messages() {
        let tree = super::parse(
            "m",
            r#"{"type":"setTree","view":"docker","badge":2,"message":"2 running","items":[{"id":"section:files","label":"Files","header":true,"count":1,"children":[{"id":"file:/a","label":"Dockerfile","icon":"file:Dockerfile","actions":[{"id":"build","icon":"build","tooltip":"Build"}]}]}],"actions":[{"id":"refresh","icon":"refresh","tooltip":"Refresh"}]}"#,
        );
        let Some(super::PluginMessage::SetTree { tree, .. }) = tree else { panic!("setTree") };
        let items = &tree.items;
        assert_eq!((tree.view.as_str(), tree.badge, tree.message.as_deref(), tree.actions.len()), ("docker", 2, Some("2 running"), 1));
        assert!(items[0].header && items[0].children[0].actions[0].id == "build" && items[0].children[0].icon.as_deref() == Some("file:Dockerfile"));
        let pick = super::parse("m", r#"{"type":"quickPick","requestId":7,"placeholder":"p","items":[{"label":"a","value":"/a"}]}"#);
        assert!(matches!(pick, Some(super::PluginMessage::QuickPick { request: 7, ref items, .. }) if items[0].value == "/a"));
        let term = super::parse("m", r#"{"type":"runInTerminal","title":"t","command":"ls","saveAll":true}"#);
        assert!(matches!(term, Some(super::PluginMessage::RunInTerminal { save_all: true, .. })));
        let prompt = super::parse("m", r#"{"type":"prompt","requestId":8,"message":"Remove?","buttons":["Remove","Cancel"]}"#);
        assert!(matches!(prompt, Some(super::PluginMessage::Prompt { ref prompt, .. }) if prompt.request == 8 && prompt.buttons.len() == 2));
    }

    #[test]
    fn a_module_without_a_command_only_declares_things() {
        let m: super::ModuleManifest = json5::from_str(r#"{ id: "x.icons", name: "Icons", fileIcons: [{ icon: "M0,0Z", names: ["a"] }] }"#).unwrap();
        assert!(m.command.is_empty());
        assert_eq!(m.file_icons.len(), 1);
    }

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
