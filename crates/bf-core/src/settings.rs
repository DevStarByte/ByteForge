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

    /// All settings as one JSON object (what modules receive).
    pub fn all(&self) -> Value {
        Value::Object(self.values.read().clone())
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.values.read().get(key).and_then(|v| v.as_str().map(str::to_owned)).filter(|s| !s.is_empty())
    }

    /// Writes a value. Only that key's text changes in the file, so comments, order and formatting stay.
    pub fn set(&self, key: &str, value: Value) {
        self.write(key, Some(value));
    }

    /// Removes a key from the file (the default applies again).
    pub fn remove(&self, key: &str) {
        self.write(key, None);
    }

    /// Whether the file sets `key`.
    pub fn is_set(&self, key: &str) -> bool {
        self.values.read().contains_key(key)
    }

    fn write(&self, key: &str, value: Option<Value>) {
        let old = std::fs::read_to_string(&self.path).unwrap_or_else(|_| "{\n}\n".to_owned());
        let text = match set_in_text(&old, key, value.as_ref()) {
            Some(text) => text,
            // Not an object we can edit (broken by hand): write the values cleanly.
            None => {
                let mut values = self.values.read().clone();
                match &value {
                    Some(v) => values.insert(key.to_owned(), v.clone()),
                    None => values.remove(key),
                };
                serde_json::to_string_pretty(&values).unwrap_or_default() + "\n"
            }
        };
        {
            let mut values = self.values.write();
            match value {
                Some(v) => values.insert(key.to_owned(), v),
                None => values.remove(key),
            };
        }
        let _ = std::fs::write(&self.path, text);
    }
}

/// A top-level member of a JSON-with-comments object: byte ranges of the whole member (key to value),
/// of its value, and where the comma after it ends (if there is one).
struct Member {
    key: String,
    start: usize,
    value: (usize, usize),
    comma_end: Option<usize>,
}

/// The top-level members of `text` (an object, comments and trailing commas allowed) and the position of
/// its closing brace. `None` when it isn't an object.
fn members(text: &str) -> Option<(Vec<Member>, usize)> {
    let b = text.as_bytes();
    let mut i = 0;
    // Skips whitespace and comments.
    let skip = |mut i: usize| -> usize {
        loop {
            while i < b.len() && (b[i] as char).is_whitespace() {
                i += 1;
            }
            if b[i..].starts_with(b"//") {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            } else if b[i..].starts_with(b"/*") {
                i = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            } else {
                return i;
            }
        }
    };
    // The end of a string starting at `i` (just past its closing quote).
    let string_end = |mut i: usize| -> usize {
        let quote = b[i];
        i += 1;
        while i < b.len() && b[i] != quote {
            if b[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        (i + 1).min(b.len())
    };
    i = skip(i);
    if i >= b.len() || b[i] != b'{' {
        return None;
    }
    i += 1;
    let mut out = Vec::new();
    loop {
        i = skip(i);
        if i >= b.len() {
            return None;
        }
        if b[i] == b'}' {
            return Some((out, i));
        }
        let start = i;
        let key = if b[i] == b'"' || b[i] == b'\'' {
            let end = string_end(i);
            let k = text[i + 1..end - 1].to_owned();
            i = end;
            k
        } else {
            let s = i;
            while i < b.len() && ((b[i] as char).is_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] == b'.') {
                i += 1;
            }
            text[s..i].to_owned()
        };
        i = skip(i);
        if i >= b.len() || b[i] != b':' {
            return None;
        }
        i = skip(i + 1);
        // The value: up to the comma or closing brace at this level.
        let value_start = i;
        let mut depth = 0i32;
        let mut value_end = i;
        while i < b.len() {
            match b[i] {
                b'"' | b'\'' => {
                    i = string_end(i);
                    value_end = i;
                    continue;
                }
                b'/' if b[i..].starts_with(b"//") || b[i..].starts_with(b"/*") => {
                    i = skip(i);
                    continue;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' if depth == 0 => break,
                b'}' | b']' => depth -= 1,
                b',' if depth == 0 => break,
                c if (c as char).is_whitespace() => {
                    i += 1;
                    continue;
                }
                _ => {}
            }
            i += 1;
            value_end = i;
        }
        let comma_end = (i < b.len() && b[i] == b',').then(|| i + 1);
        out.push(Member { key, start, value: (value_start, value_end), comma_end });
        if let Some(e) = comma_end {
            i = e;
        }
    }
}

/// `text` with top-level `key` set to `value` (or removed, with `None`): only that member's text changes.
/// `None` when the text isn't an object.
pub fn set_in_text(text: &str, key: &str, value: Option<&Value>) -> Option<String> {
    let (list, close) = members(text)?;
    let rendered = value.map(|v| serde_json::to_string(v).unwrap_or_default());
    let found = list.iter().position(|m| m.key == key);
    let mut out = text.to_owned();
    match (found, rendered) {
        (Some(i), Some(v)) => {
            let m = &list[i];
            out.replace_range(m.value.0..m.value.1, &v);
        }
        (Some(i), None) => {
            let m = &list[i];
            // From the start of its line (when only whitespace precedes it) to after its comma.
            let line_start = text[..m.start].rfind('\n').map_or(0, |n| n + 1);
            let from = if text[line_start..m.start].trim().is_empty() { line_start } else { m.start };
            let mut to = m.comma_end.unwrap_or(m.value.1);
            if from == line_start && text[to..].starts_with('\n') {
                to += 1;
            }
            out.replace_range(from..to, "");
            // The removed member was the last with a comma before it: that comma goes too (strict JSON).
            if m.comma_end.is_none() && i > 0 && let Some(prev_comma) = list[i - 1].comma_end {
                let at = prev_comma - 1;
                if out.as_bytes().get(at) == Some(&b',') {
                    out.remove(at);
                }
            }
        }
        (None, Some(v)) => {
            let indent = list.first().map(|m| {
                let line_start = text[..m.start].rfind('\n').map_or(0, |n| n + 1);
                text[line_start..m.start].to_owned()
            });
            let indent = indent.filter(|i| i.trim().is_empty()).unwrap_or_else(|| "  ".to_owned());
            let entry = format!("{indent}{}: {v}", serde_json::to_string(key).unwrap_or_default());
            match list.last() {
                // After the last member (adding its missing comma).
                Some(last) => {
                    let (insert_at, needs_comma) = match last.comma_end {
                        Some(e) => (e, false),
                        None => (last.value.1, true),
                    };
                    let rest_on_line = text[insert_at..].find('\n').map_or(text.len(), |n| insert_at + n);
                    // Keep a comment that follows on the same line with its member.
                    let at = if text[insert_at..rest_on_line].trim_start().starts_with("//") { rest_on_line } else { insert_at };
                    let piece = format!("\n{entry}");
                    out.insert_str(at, &piece);
                    if needs_comma {
                        out.insert(insert_at, ',');
                    }
                }
                None => {
                    out.replace_range(close..close, &format!("\n{entry}\n"));
                }
            }
        }
        (None, None) => {}
    }
    Some(out)
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

#[cfg(test)]
mod edit_tests {
    use super::*;
    use serde_json::json;

    const FILE: &str = "{\n  // My theme\n  \"workbench.theme\": \"ByteForge Dark\",\n  \"editor.tabSize\": 4, // spaces\n  /* block */\n  \"files.exclude\": [\"bin\", \"obj\"]\n}\n";

    fn parse(text: &str) -> Value {
        json5::from_str(text).unwrap()
    }

    #[test]
    fn changes_one_value_and_keeps_comments() {
        let out = set_in_text(FILE, "editor.tabSize", Some(&json!(2))).unwrap();
        assert_eq!(out, FILE.replace("\"editor.tabSize\": 4,", "\"editor.tabSize\": 2,"));
        let out = set_in_text(FILE, "files.exclude", Some(&json!(["bin"]))).unwrap();
        assert!(out.contains("// My theme") && out.contains("/* block */") && out.contains("[\"bin\"]\n}"), "{out}");
        assert_eq!(parse(&out)["files.exclude"], json!(["bin"]));
    }

    #[test]
    fn adds_a_new_key_after_the_last() {
        let out = set_in_text(FILE, "editor.wordWrap", Some(&json!("on"))).unwrap();
        assert!(out.contains("[\"bin\", \"obj\"],\n  \"editor.wordWrap\": \"on\"\n}"), "{out}");
        assert_eq!(parse(&out)["editor.wordWrap"], json!("on"));
        assert_eq!(parse(&out)["editor.tabSize"], json!(4));
        // Into an empty object.
        let out = set_in_text("{}", "a", Some(&json!(true))).unwrap();
        assert_eq!(parse(&out), json!({ "a": true }));
    }

    #[test]
    fn removes_a_key_with_its_line() {
        let out = set_in_text(FILE, "editor.tabSize", None).unwrap();
        assert!(!out.contains("tabSize") && out.contains("// My theme"), "{out}");
        assert_eq!(parse(&out)["workbench.theme"], json!("ByteForge Dark"));
        // The last member: the comma before it goes too.
        let out = set_in_text("{\n  \"a\": 1,\n  \"b\": 2\n}", "b", None).unwrap();
        assert_eq!(out, "{\n  \"a\": 1\n}");
        assert!(serde_json::from_str::<Value>(&out).is_ok());
    }

    #[test]
    fn values_with_strings_and_nesting() {
        let text = "{ \"keybindings\": { \"a\": \"ctrl-}\" }, \"x\": \"a,b\" }";
        let out = set_in_text(text, "x", Some(&json!("c"))).unwrap();
        assert_eq!(parse(&out)["keybindings"]["a"], json!("ctrl-}"));
        assert_eq!(parse(&out)["x"], json!("c"));
        assert!(set_in_text("[1, 2]", "x", Some(&json!(1))).is_none());
    }
}
