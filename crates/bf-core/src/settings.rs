use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::watch::DebouncedWatcher;

const DEFAULT_SETTINGS: &str = r#"{
  // ByteForge settings. Saved changes apply immediately.
  "workbench.theme": "ByteForge Dark",
  "editor.tabSize": 4,
  "editor.insertSpaces": true,
  "editor.lineNumbers": true,
  "editor.smoothScrolling": true,
  "editor.cursorBlinking": true,
  "editor.autoClosingBrackets": true,
  "editor.indentGuides": true,
  "files.exclude": ["bin", "obj", ".git", ".vs", ".idea", "node_modules", "target"],
  "csharp.languageServer": "",
  "dotnet.project": "",
  "keybindings": {},
  "modules.disabled": []
}
"#;

/// User settings (JSON with comments). Thread-safe; reloads when the file changes on disk.
#[derive(Clone)]
pub struct Settings {
    path: PathBuf,
    values: Arc<RwLock<Map<String, Value>>>,
    _watcher: Arc<Option<DebouncedWatcher>>,
}

impl Settings {
    /// Loads settings (creating the default file) and calls `on_change` from a background thread
    /// whenever the file is modified.
    pub fn load(path: PathBuf, on_change: impl Fn() + Send + 'static) -> Self {
        if !path.exists() {
            let _ = std::fs::write(&path, DEFAULT_SETTINGS);
        }
        let values = Arc::new(RwLock::new(read(&path)));
        let watcher = {
            let values = values.clone();
            let file = path.clone();
            path.parent().and_then(|dir| {
                DebouncedWatcher::new(dir, false, Duration::from_millis(150), move |changed| {
                    if changed.iter().any(|p| p.file_name() == file.file_name()) {
                        *values.write() = read(&file);
                        on_change();
                    }
                })
                .ok()
            })
        };
        Self { path, values, _watcher: Arc::new(watcher) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str, fallback: T) -> T {
        self.values.read().get(key).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or(fallback)
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.values.read().get(key).and_then(|v| v.as_str().map(str::to_owned)).filter(|s| !s.is_empty())
    }

    /// Writes a value (the file is re-serialized; comments are not preserved).
    pub fn set(&self, key: &str, value: Value) {
        let text = {
            let mut values = self.values.write();
            values.insert(key.to_owned(), value);
            serde_json::to_string_pretty(&*values).unwrap_or_default()
        };
        let _ = std::fs::write(&self.path, text);
    }
}

fn read(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| json5::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Small machine-written key/value state (last folder, window size, …).
#[derive(Clone)]
pub struct State {
    path: PathBuf,
    values: Arc<RwLock<Map<String, Value>>>,
}

impl State {
    pub fn load(path: PathBuf) -> Self {
        let values = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Self { path, values: Arc::new(RwLock::new(values)) }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.values.read().get(key).and_then(|v| v.as_str().map(str::to_owned))
    }

    pub fn set(&self, key: &str, value: Option<&str>) {
        let text = {
            let mut values = self.values.write();
            match value {
                Some(v) => values.insert(key.to_owned(), Value::String(v.to_owned())),
                None => values.remove(key),
            };
            serde_json::to_string(&*values).unwrap_or_default()
        };
        let _ = std::fs::write(&self.path, text);
    }
}
