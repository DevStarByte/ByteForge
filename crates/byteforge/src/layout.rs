//! The workbench layout as described by the theme's "layout" section. Every container is an ordered
//! list of part names, so a theme can reorder, move or drop any part of the UI:
//!
//! ```jsonc
//! "workbench.order": ["titleBar", "main", "statusBar"],          // rows of the window
//! "main.order": ["activityBar", "sideBar", "editor"],              // columns ("panel" here = side panel)
//! "editor.order": ["tabs", "breadcrumbs", "content", "panel"],     // the editor column
//! "sideBar.order": ["header", "content"],                          // add "activityBar" for a horizontal one
//! "titleBar.left": ["logo", "menu"], "titleBar.center": ["commandCenter"], "titleBar.right": [...],
//! "statusBar.left": [...], "statusBar.center": [], "statusBar.right": [...],
//! "activityBar.items": ["explorer", "search", "scm", "modules", "spacer", "settings"],
//! ```
//!
//! Lists can be JSON arrays or comma-separated strings. Any bar also accepts `"command:<id>"` or
//! `"command:<id>:<icon>"` to place a button for any command, and `"spacer"` to push items apart.
//! The older keys (`sideBar.position`, `activityBar.visible`, `tabs.visible`, …) still work as defaults.

use bf_core::theme::Section;

#[derive(Clone, Debug)]
pub struct LayoutConfig {
    pub workbench: Vec<String>,
    pub main: Vec<String>,
    pub editor: Vec<String>,
    pub side_bar: Vec<String>,
    pub title_left: Vec<String>,
    pub title_center: Vec<String>,
    pub title_right: Vec<String>,
    pub status_left: Vec<String>,
    pub status_center: Vec<String>,
    pub status_right: Vec<String>,
    pub activity_items: Vec<String>,
    pub panel_tabs: Vec<String>,
    pub menus: Vec<String>,
    pub welcome: Vec<String>,
    pub welcome_commands: Vec<String>,
    pub welcome_title: String,
    /// "top" or "bottom": which edge of a tab carries the active indicator.
    pub tab_indicator_bottom: bool,
    pub side_visible: bool,
    pub panel_visible: bool,
    pub side_default: String,
    /// Status bar cursor position: {line}, {col} and {lines} are replaced.
    pub position_format: String,
}

fn list(section: &Section, key: &str, default: &[&str]) -> Vec<String> {
    let Some(raw) = section.get(key) else { return default.iter().map(|s| (*s).to_owned()).collect() };
    let raw = raw.trim();
    if raw.starts_with('[') {
        if let Ok(items) = serde_json::from_str::<Vec<String>>(raw) {
            return items;
        }
    }
    raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

fn flag(section: &Section, key: &str, default: bool) -> bool {
    section.get(key).map_or(default, |v| !v.eq_ignore_ascii_case("false"))
}

fn is(section: &Section, key: &str, value: &str) -> bool {
    section.get(key).is_some_and(|v| v.eq_ignore_ascii_case(value))
}

impl LayoutConfig {
    pub fn parse(s: &Section) -> Self {
        // Defaults derived from the legacy keys, so older themes keep working.
        let side_right = is(s, "sideBar.position", "right");
        let activity_right = s.get("activityBar.position").map_or(side_right, |p| p.eq_ignore_ascii_case("right"));
        let activity_visible = flag(s, "activityBar.visible", true);
        let mut main: Vec<&str> = Vec::new();
        if activity_visible && !activity_right {
            main.push("activityBar");
        }
        if side_right {
            main.extend(["editor", "sideBar"]);
        } else {
            main.extend(["sideBar", "editor"]);
        }
        if activity_visible && activity_right {
            main.push("activityBar");
        }
        let mut editor = Vec::new();
        if flag(s, "tabs.visible", true) {
            editor.push("tabs");
        }
        if flag(s, "breadcrumbs.visible", true) {
            editor.push("breadcrumbs");
        }
        editor.extend(["content", "panel"]);
        let mut workbench = vec!["titleBar", "main"];
        if flag(s, "statusBar.visible", true) {
            workbench.push("statusBar");
        }
        let mut title_left = vec!["logo"];
        if flag(s, "menuBar.visible", true) {
            title_left.push("menu");
        }
        let title_center: &[&str] = if flag(s, "commandCenter.visible", true) { &["commandCenter"] } else { &[] };
        let mut title_right = Vec::new();
        if flag(s, "titleBar.layoutControls", true) {
            title_right.push("layoutControls");
        }
        title_right.push("windowControls");

        Self {
            workbench: list(s, "workbench.order", &workbench),
            main: list(s, "main.order", &main),
            editor: list(s, "editor.order", &editor),
            side_bar: list(s, "sideBar.order", &["header", "content"]),
            title_left: list(s, "titleBar.left", &title_left),
            title_center: list(s, "titleBar.center", title_center),
            title_right: list(s, "titleBar.right", &title_right),
            status_left: list(s, "statusBar.left", &["branch", "sync", "problems", "run", "modules"]),
            status_center: list(s, "statusBar.center", &[]),
            status_right: list(s, "statusBar.right", &["modules", "position", "indent", "encoding", "eol", "language", "lsp"]),
            activity_items: list(s, "activityBar.items", &["explorer", "search", "scm", "modules", "spacer", "settings"]),
            panel_tabs: list(s, "panel.tabs", &["problems", "output"]),
            menus: list(s, "menuBar.menus", &["File", "Edit", "View", "Go", "Git", "Run", "Preferences", "Help"]),
            welcome: list(s, "welcome.order", &["logo", "title", "shortcuts"]),
            welcome_commands: list(s, "welcome.shortcuts", &["file.openFolder", "view.quickOpen", "view.commandPalette", "theme.select", "dotnet.build"]),
            welcome_title: s.get("welcome.title").cloned().unwrap_or_else(|| "ByteForge".into()),
            tab_indicator_bottom: is(s, "tab.indicatorPosition", "bottom"),
            side_visible: flag(s, "sideBar.visible", true),
            panel_visible: flag(s, "panel.visible", false),
            side_default: s.get("sideBar.defaultView").cloned().unwrap_or_else(|| "explorer".into()),
            position_format: s.get("statusBar.positionFormat").cloned().unwrap_or_else(|| "Ln {line}, Col {col}".into()),
        }
    }

    fn index(items: &[String], name: &str) -> Option<usize> {
        items.iter().position(|i| i == name)
    }

    /// True when `part` comes after the editor in the main row (it then sits on the right).
    pub fn right_of_editor(&self, part: &str) -> bool {
        match (Self::index(&self.main, part), Self::index(&self.main, "editor")) {
            (Some(p), Some(e)) => p > e,
            _ => false,
        }
    }

    /// The panel is a column of the main row (left/right) instead of a row of the editor column.
    pub fn panel_in_main(&self) -> bool {
        Self::index(&self.main, "panel").is_some()
    }

    /// The panel comes before the editor content (sits above it).
    pub fn panel_above_content(&self) -> bool {
        match (Self::index(&self.editor, "panel"), Self::index(&self.editor, "content")) {
            (Some(p), Some(c)) => p < c,
            _ => false,
        }
    }

    /// The title bar is below the main area (menus open upwards).
    pub fn title_bar_at_bottom(&self) -> bool {
        match (Self::index(&self.workbench, "titleBar"), Self::index(&self.workbench, "main")) {
            (Some(t), Some(m)) => t > m,
            _ => false,
        }
    }

    /// Tabs come after the content (the tab strip is at the bottom).
    pub fn tabs_below_content(&self) -> bool {
        match (Self::index(&self.editor, "tabs"), Self::index(&self.editor, "content")) {
            (Some(t), Some(c)) => t > c,
            _ => false,
        }
    }

    pub fn contains_title_item(&self, name: &str) -> bool {
        [&self.title_left, &self.title_center, &self.title_right].iter().any(|l| l.iter().any(|i| i == name))
    }
}

/// Parses `"command:<id>"` / `"command:<id>:<icon>"`.
pub fn command_item(item: &str) -> Option<(&str, Option<&str>)> {
    let rest = item.strip_prefix("command:")?;
    Some(match rest.split_once(':') {
        Some((id, icon)) => (id, Some(icon)),
        None => (rest, None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(pairs: &[(&str, &str)]) -> Section {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn legacy_keys_become_defaults() {
        let l = LayoutConfig::parse(&section(&[("sideBar.position", "right"), ("tabs.visible", "false"), ("statusBar.visible", "false")]));
        // The activity bar follows the side bar unless activityBar.position says otherwise.
        assert_eq!(l.main, ["editor", "sideBar", "activityBar"].map(String::from));
        assert!(l.right_of_editor("sideBar") && l.right_of_editor("activityBar"));
        assert!(!l.editor.contains(&"tabs".to_owned()));
        assert!(!l.workbench.contains(&"statusBar".to_owned()));
    }

    #[test]
    fn lists_parse_from_json_arrays_and_csv() {
        let l = LayoutConfig::parse(&section(&[
            ("main.order", r#"["editor","panel","sideBar"]"#),
            ("editor.order", "content, tabs"),
            ("sideBar.order", r#"["activityBar","content"]"#),
        ]));
        assert!(l.panel_in_main() && l.right_of_editor("panel") && l.right_of_editor("sideBar"));
        assert!(l.tabs_below_content() && l.side_bar[0] == "activityBar");
        assert_eq!(command_item("command:dotnet.build:build"), Some(("dotnet.build", Some("build"))));
    }
}
