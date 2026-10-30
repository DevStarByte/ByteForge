use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde_json::Value;

use crate::watch::DebouncedWatcher;

pub const DEFAULT_DARK: &str = "ByteForge Dark";
pub const DEFAULT_LIGHT: &str = "ByteForge Light";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenStyle {
    pub foreground: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

pub type Section = HashMap<String, String>;

/// A theme as written in JSON: every value is kept as its raw string and interpreted by the UI,
/// so themes stay forward-compatible. Themes inherit and only override what they change.
#[derive(Clone, Debug, Default)]
pub struct ThemeData {
    pub name: String,
    pub kind: String,
    pub inherits: Option<String>,
    pub description: Option<String>,
    pub file_path: Option<PathBuf>,
    pub colors: Section,
    pub token_colors: HashMap<String, TokenStyle>,
    pub fonts: Section,
    pub metrics: Section,
    pub layout: Section,
    pub editor: Section,
    pub effects: Section,
    pub icons: Section,
    pub avalonia: Section,
    /// Unknown top-level objects, readable by modules.
    pub sections: HashMap<String, Section>,
}

const KNOWN: &[&str] = &[
    "name", "type", "inherits", "description", "colors", "tokenColors", "fonts", "metrics", "layout", "editor", "effects",
    "icons", "avalonia",
];

impl ThemeData {
    pub fn is_light(&self) -> bool {
        self.kind.eq_ignore_ascii_case("light")
    }

    pub fn parse(text: &str, fallback_name: &str) -> anyhow::Result<Self> {
        let root: Value = json5::from_str(text)?;
        let obj = root.as_object().ok_or_else(|| anyhow::anyhow!("theme root must be an object"))?;
        let s = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_owned);
        let mut theme = ThemeData {
            name: s("name").filter(|n| !n.is_empty()).unwrap_or_else(|| fallback_name.to_owned()),
            kind: s("type").unwrap_or_else(|| "dark".into()),
            inherits: s("inherits"),
            description: s("description"),
            ..Default::default()
        };
        let section = |k: &str| obj.get(k).map(read_section).unwrap_or_default();
        theme.colors = section("colors");
        theme.fonts = section("fonts");
        theme.metrics = section("metrics");
        theme.layout = section("layout");
        theme.editor = section("editor");
        theme.effects = section("effects");
        theme.icons = section("icons");
        theme.avalonia = section("avalonia");
        if let Some(Value::Object(tokens)) = obj.get("tokenColors") {
            for (k, v) in tokens {
                let style = match v {
                    Value::String(c) => TokenStyle { foreground: Some(c.clone()), ..Default::default() },
                    Value::Object(o) => {
                        let font_style = o.get("fontStyle").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
                        TokenStyle {
                            foreground: o.get("foreground").and_then(Value::as_str).map(str::to_owned),
                            bold: font_style.contains("bold"),
                            italic: font_style.contains("italic"),
                            underline: font_style.contains("underline"),
                        }
                    }
                    _ => continue,
                };
                theme.token_colors.insert(k.clone(), style);
            }
        }
        for (k, v) in obj {
            if !KNOWN.contains(&k.as_str()) && v.is_object() {
                theme.sections.insert(k.clone(), read_section(v));
            }
        }
        Ok(theme)
    }

    pub fn merge_from(&mut self, other: &ThemeData) {
        self.name = other.name.clone();
        self.kind = other.kind.clone();
        self.file_path = other.file_path.clone();
        if other.description.is_some() {
            self.description = other.description.clone();
        }
        for (dst, src) in [
            (&mut self.colors, &other.colors),
            (&mut self.fonts, &other.fonts),
            (&mut self.metrics, &other.metrics),
            (&mut self.layout, &other.layout),
            (&mut self.editor, &other.editor),
            (&mut self.effects, &other.effects),
            (&mut self.icons, &other.icons),
            (&mut self.avalonia, &other.avalonia),
        ] {
            dst.extend(src.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        self.token_colors.extend(other.token_colors.iter().map(|(k, v)| (k.clone(), v.clone())));
        for (name, values) in &other.sections {
            self.sections.entry(name.clone()).or_default().extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }

    pub fn number(&self, section: &Section, key: &str, fallback: f32) -> f32 {
        section.get(key).and_then(|v| v.trim().parse().ok()).unwrap_or(fallback)
    }

    pub fn flag(&self, section: &Section, key: &str, fallback: bool) -> bool {
        section.get(key).map_or(fallback, |v| !v.eq_ignore_ascii_case("false"))
    }
}

fn read_section(value: &Value) -> Section {
    let Value::Object(map) = value else { return Section::new() };
    map.iter()
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                other => other.to_string(),
            };
            (k.clone(), text)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct ThemeInfo {
    pub name: String,
    pub kind: String,
    pub path: PathBuf,
    pub user: bool,
    pub description: Option<String>,
    /// Raw color values for previews.
    pub swatches: Vec<String>,
}

/// Discovers theme files, resolves inheritance and hot-reloads the active theme.
#[derive(Clone)]
pub struct ThemeService {
    dirs: Vec<(PathBuf, bool)>,
    current: Arc<RwLock<Arc<ThemeData>>>,
    current_name: Arc<RwLock<String>>,
    _watchers: Arc<Vec<DebouncedWatcher>>,
}

impl ThemeService {
    /// `on_reload` is called from a background thread when a theme file changes.
    pub fn new(builtin: PathBuf, user: PathBuf, on_reload: impl Fn() + Send + Sync + 'static) -> Self {
        let on_reload = Arc::new(on_reload);
        let watchers = [&builtin, &user]
            .iter()
            .filter(|d| d.is_dir())
            .filter_map(|dir| {
                let on_reload = on_reload.clone();
                DebouncedWatcher::new(dir, false, Duration::from_millis(120), move |_| on_reload()).ok()
            })
            .collect();
        Self {
            dirs: vec![(builtin, false), (user, true)],
            current: Arc::new(RwLock::new(Arc::new(ThemeData::default()))),
            current_name: Arc::new(RwLock::new(DEFAULT_DARK.into())),
            _watchers: Arc::new(watchers),
        }
    }

    pub fn current(&self) -> Arc<ThemeData> {
        self.current.read().clone()
    }

    pub fn available(&self) -> Vec<ThemeInfo> {
        let mut found: HashMap<String, ThemeInfo> = HashMap::new();
        for (dir, user) in &self.dirs {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                let Ok(theme) = ThemeData::parse(&text, &stem(&path)) else { continue };
                let swatches = ["editor.background", "sideBar.background", "accent", "statusBar.background"]
                    .iter()
                    .filter_map(|k| theme.colors.get(*k).cloned())
                    .chain(
                        ["keyword", "string", "class name", "method name"]
                            .iter()
                            .filter_map(|k| theme.token_colors.get(*k).and_then(|t| t.foreground.clone())),
                    )
                    .collect();
                found.insert(theme.name.to_lowercase(), ThemeInfo {
                    name: theme.name.clone(),
                    kind: theme.kind.clone(),
                    path,
                    user: *user,
                    description: theme.description.clone(),
                    swatches,
                });
            }
        }
        let mut list: Vec<ThemeInfo> = found.into_values().collect();
        list.sort_by_key(|t| t.name.to_lowercase());
        list
    }

    /// Resolves and activates a theme by name (falls back to the default dark theme).
    pub fn apply(&self, name: &str) -> anyhow::Result<Arc<ThemeData>> {
        let theme = Arc::new(self.resolve(name, 0)?);
        *self.current_name.write() = theme.name.clone();
        *self.current.write() = theme.clone();
        Ok(theme)
    }

    pub fn reload(&self) -> anyhow::Result<Arc<ThemeData>> {
        let name = self.current_name.read().clone();
        self.apply(&name)
    }

    fn resolve(&self, name: &str, depth: usize) -> anyhow::Result<ThemeData> {
        anyhow::ensure!(depth < 8, "theme inheritance is too deep");
        let themes = self.available();
        let info = themes
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .or_else(|| themes.iter().find(|t| stem(&t.path).eq_ignore_ascii_case(name)));
        let Some(info) = info else {
            if depth == 0 && !name.eq_ignore_ascii_case(DEFAULT_DARK) {
                log::warn!("theme '{name}' not found, using {DEFAULT_DARK}");
                return self.resolve(DEFAULT_DARK, depth + 1);
            }
            anyhow::bail!("theme '{name}' not found");
        };
        let mut raw = ThemeData::parse(&std::fs::read_to_string(&info.path)?, &info.name)?;
        raw.file_path = Some(info.path.clone());
        let is_default = raw.name.eq_ignore_ascii_case(DEFAULT_DARK) || raw.name.eq_ignore_ascii_case(DEFAULT_LIGHT);
        let parent = raw.inherits.clone().or_else(|| {
            (!is_default).then(|| if raw.is_light() { DEFAULT_LIGHT.to_owned() } else { DEFAULT_DARK.to_owned() })
        });
        match parent {
            None => Ok(raw),
            Some(parent) => {
                let mut merged = self.resolve(&parent, depth + 1)?;
                merged.merge_from(&raw);
                Ok(merged)
            }
        }
    }
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_shipped_themes_parse_and_resolve() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes");
        let service = ThemeService::new(dir.clone(), dir.join("__none__"), || {});
        let themes = service.available();
        assert!(themes.len() >= 9, "expected the built-in themes, found {}", themes.len());
        for info in themes {
            let theme = service.apply(&info.name).unwrap_or_else(|e| panic!("{}: {e}", info.name));
            assert!(theme.colors.contains_key("editor.background"), "{} lacks inherited colors", info.name);
            assert!(theme.metrics.contains_key("tab.height"), "{} lacks inherited metrics", info.name);
        }
    }

    #[test]
    fn parses_comments_sections_and_token_styles() {
        let theme = ThemeData::parse(
            r##"{
              // comment
              "name": "T", "type": "light",
              "colors": { "accent": "#fff", },
              "tokenColors": { "keyword": "#00f", "comment": { "foreground": "#0f0", "fontStyle": "italic underline" } },
              "metrics": { "tab.height": 30 },
              "editor": { "smoothCaretAnimation": false },
              "myModule": { "x": "1" }
            }"##,
            "fallback",
        )
        .unwrap();
        assert_eq!(theme.name, "T");
        assert!(theme.is_light());
        assert_eq!(theme.metrics["tab.height"], "30");
        assert_eq!(theme.editor["smoothCaretAnimation"], "false");
        assert!(theme.token_colors["comment"].italic && theme.token_colors["comment"].underline);
        assert_eq!(theme.sections["myModule"]["x"], "1");
    }
}
