//! The Keyboard Shortcuts page (Preferences → Keyboard Shortcuts): every command with its keys, searchable by
//! name or key. Clicking a command's keys records new ones (press them, Enter saves, Escape cancels; two
//! presses make a chord like "Ctrl+K Ctrl+S"); Remove takes its keys away, Reset brings the default back. Keys
//! used by another command too are pointed out. Changes go to "keybindings" in settings.json (only that entry
//! is rewritten) and take effect at once.

use gpui::{Div, ScrollHandle, Subscription, point};
use serde_json::{Map, Value};

use super::*;
use crate::actions::{Commands, pretty_keys};

#[derive(Default)]
pub(super) struct ShortcutsState {
    pub search: Option<Entity<TextInput>>,
    pub scroll: ScrollHandle,
    /// Only the commands whose keys the user changed.
    pub changed_only: bool,
    pub recording: Option<Recording>,
}

/// Keys being recorded for a command; the interceptor takes every keystroke until Enter or Escape.
pub(super) struct Recording {
    pub id: String,
    pub keys: Vec<String>,
    _intercept: Subscription,
}

/// A command as the page lists it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ShortcutRow {
    pub id: String,
    /// "Category: Title"
    pub title: String,
    /// The keys it runs with (the user's, else the default).
    pub key: Option<String>,
    pub default: Option<String>,
    /// The user changed (or removed) its keys.
    pub changed: bool,
}

/// Keys written the same way: lower case, single spaces between the presses of a chord.
pub(super) fn normalize(keys: &str) -> String {
    keys.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join(" ")
}

/// Every command once, sorted by title.
pub(super) fn rows(commands: &Commands) -> Vec<ShortcutRow> {
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<ShortcutRow> = commands
        .list
        .iter()
        .filter(|c| seen.insert(c.id.clone()))
        .map(|c| ShortcutRow {
            id: c.id.clone(),
            title: c.display_title(),
            key: commands.key(&c.id),
            default: c.default_key.clone(),
            changed: commands.overrides.contains_key(&c.id),
        })
        .collect();
    rows.sort_by_key(|r| r.title.to_lowercase());
    rows
}

/// The other commands that run with the same keys as `row`.
pub(super) fn conflicts<'a>(rows: &'a [ShortcutRow], row: &ShortcutRow) -> Vec<&'a ShortcutRow> {
    let Some(key) = row.key.as_deref().map(normalize) else { return Vec::new() };
    rows.iter().filter(|r| r.id != row.id && r.key.as_deref().map(normalize).as_deref() == Some(key.as_str())).collect()
}

/// Whether a row matches the search: words of its title, its id, or its keys (typed "ctrl+s" or "ctrl-s").
pub(super) fn matches(row: &ShortcutRow, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let keys = row.key.as_deref().map(|k| (normalize(k), pretty_keys(k).to_lowercase())).unwrap_or_default();
    let as_keys = query.replace('+', "-");
    query.split_whitespace().all(|word| row.title.to_lowercase().contains(word) || row.id.to_lowercase().contains(word))
        || (!keys.0.is_empty() && (keys.0.contains(&as_keys) || keys.1.contains(&query)))
}

/// The "keybindings" entry after giving `id` the keys `new` (None: none). Keys equal to the default leave no
/// entry (as if never changed); removing a command's only keys writes "".
pub(super) fn with_keys(mut overrides: Map<String, Value>, id: &str, new: Option<&str>, default: Option<&str>) -> Map<String, Value> {
    let new = new.map(normalize).filter(|k| !k.is_empty());
    let default = default.map(normalize);
    match new {
        Some(keys) if Some(&keys) == default.as_ref() => {
            overrides.remove(id);
        }
        Some(keys) => {
            overrides.insert(id.to_owned(), Value::String(keys));
        }
        None if default.is_some() => {
            overrides.insert(id.to_owned(), Value::String(String::new()));
        }
        None => {
            overrides.remove(id);
        }
    }
    overrides
}

impl Workbench {
    pub(super) fn open_shortcuts_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| matches!(t.page, Some(Page::Shortcuts))) {
            return self.activate(ix, window, cx);
        }
        let input = cx.new(|cx| TextInput::new("Search by command or keys (ctrl+s)", cx));
        let sub = cx.subscribe_in(&input, window, |wb: &mut Self, _, _: &InputEvent, _, cx| {
            wb.shortcuts.scroll.set_offset(point(px(0.), px(0.)));
            cx.notify()
        });
        self.shortcuts.search = Some(input.clone());
        self.shortcuts.scroll = ScrollHandle::new();
        self.shortcuts.recording = None;
        let editor = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e
        });
        self.add_tab(editor, None, "Keyboard Shortcuts".into(), false, window, cx);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(Page::Shortcuts);
            tab._subs.push(sub);
        }
        window.focus(&input.focus_handle(cx));
        cx.notify();
    }

    /// Gives a command new keys (None: none), in settings.json and right away.
    pub(super) fn set_command_keys(&mut self, id: &str, keys: Option<&str>, cx: &mut Context<Self>) {
        let default = cx.global::<Commands>().get(id).and_then(|c| c.default_key.clone());
        let current = match self.settings.get::<Value>("keybindings", Value::Object(Map::new())) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let updated = with_keys(current, id, keys, default.as_deref());
        let overrides: HashMap<String, String> = updated.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect();
        if updated.is_empty() {
            self.settings.remove("keybindings");
        } else {
            self.settings.set("keybindings", Value::Object(updated));
        }
        crate::actions::bind_all(cx, &overrides);
        cx.notify();
    }

    /// Starts recording keys for a command: every keystroke goes here (not to its command) until Enter or Escape.
    fn record_keys(&mut self, id: String, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = event.keystroke.clone();
            if this.update(cx, |wb, cx| wb.on_recorded_keystroke(&keystroke, cx)).is_ok() {
                cx.stop_propagation();
            }
        });
        self.shortcuts.recording = Some(Recording { id, keys: Vec::new(), _intercept: intercept });
        cx.notify();
    }

    fn on_recorded_keystroke(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) {
        let plain = !keystroke.modifiers.modified();
        match keystroke.key.as_str() {
            "escape" if plain => self.shortcuts.recording = None,
            "enter" if plain => {
                if let Some(recording) = self.shortcuts.recording.take()
                    && !recording.keys.is_empty()
                {
                    let keys = recording.keys.join(" ");
                    self.set_command_keys(&recording.id, Some(&keys), cx);
                }
            }
            _ => {
                if let Some(recording) = self.shortcuts.recording.as_mut() {
                    // A chord has two presses at most; a third starts over.
                    if recording.keys.len() == 2 {
                        recording.keys.clear();
                    }
                    recording.keys.push(keystroke.unparse());
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_shortcuts_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.shortcuts.search.as_ref().map(|i| i.read(cx).text().to_owned()).unwrap_or_default();
        let all = rows(cx.global::<Commands>());
        let shown: Vec<ShortcutRow> = all.iter().filter(|r| matches(r, &query) && (!self.shortcuts.changed_only || r.changed)).cloned().collect();
        let t = Theme::global(cx);
        let (fg, muted, rule) = (t.c("editor.foreground"), t.c("foreground.muted"), t.c("editorWidget.border"));
        let hover = t.c("toolbar.hoverBackground");
        let accent = t.c_or("button.background", 0x0e639cff);
        let on_accent = t.c_or("button.foreground", 0xffffffff);
        let changed_count = all.iter().filter(|r| r.changed).count();

        let button = |id: SharedString, label: &str| {
            div().id(id).px(px(10.)).py(px(4.)).rounded(px(5.)).border_1().border_color(rule).cursor_pointer().hover(move |s| s.bg(hover)).text_size(px(12.)).child(label.to_owned())
        };
        let changed_only = self.shortcuts.changed_only;
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_size(px(22.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Keyboard Shortcuts"))
            .child(div().flex_1())
            .child(
                button("shortcuts-changed".into(), &format!("Changed only ({changed_count})"))
                    .when(changed_only, |d| d.bg(accent).text_color(on_accent))
                    .on_click(cx.listener(|wb, _, _, cx| {
                        wb.shortcuts.changed_only = !wb.shortcuts.changed_only;
                        cx.notify();
                    })),
            )
            .child(button("shortcuts-json".into(), "Open settings.json").on_click(cx.listener(|wb, _, w, cx| {
                let path = wb.paths.settings_file();
                wb.open_file(path, None, w, cx);
            })));

        let mut list = div()
            .id("shortcuts-list")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.shortcuts.scroll)
            .flex()
            .flex_col()
            .child(
                div()
                    .max_w(px(960.))
                    .px_8()
                    .pt_8()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(header)
                    .child(div().text_size(px(12.)).text_color(muted).child(
                        "Click a command's keys to change them: press the new keys (two presses for a chord), Enter saves, Escape cancels.",
                    ))
                    .children(self.shortcuts.search.clone()),
            );
        let mut body = div().max_w(px(960.)).px_8().pb_8().flex().flex_col();
        for (i, row) in shown.iter().enumerate() {
            body = body.child(self.render_shortcut_row(i, row, &all, cx));
        }
        if shown.is_empty() {
            body = body.child(div().mt_4().text_color(muted).child(format!("No command matches “{query}”.")));
        }
        list = list.child(body);
        div().id("shortcuts-page").size_full().bg(t.c("editor.background")).text_color(fg).text_size(px(13.)).child(list)
    }

    fn render_shortcut_row(&mut self, i: usize, row: &ShortcutRow, all: &[ShortcutRow], cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let t = Theme::global(cx);
        let (muted, rule) = (t.c("foreground.muted"), t.c("editorWidget.border"));
        let hover = t.c("toolbar.hoverBackground");
        let warning = t.c_or("editorWarning.foreground", 0xcca700ff);
        let accent = t.c_or("focusBorder", 0x007fd4ff);
        let recording = self.shortcuts.recording.as_ref().filter(|r| r.id == row.id).map(|r| r.keys.clone());

        let keycap = |text: String| {
            div().px(px(6.)).py(px(1.)).rounded(px(4.)).border_1().border_color(rule).bg(t.c_or("keybindingLabel.background", 0x8080802b)).text_size(px(12.)).child(text)
        };
        let keys: gpui::AnyElement = match &recording {
            Some(keys) if keys.is_empty() => div().text_color(accent).child("Press the keys…").into_any_element(),
            Some(keys) => div()
                .flex()
                .items_center()
                .gap_2()
                .child(keycap(pretty_keys(&keys.join(" "))))
                .child(div().text_size(px(11.)).text_color(muted).child("Enter saves · Escape cancels"))
                .into_any_element(),
            None => match &row.key {
                Some(key) => keycap(pretty_keys(key)).into_any_element(),
                None => div().text_color(muted).child("—").into_any_element(),
            },
        };
        let id = row.id.clone();
        let keys_cell = div()
            .id(("shortcut-keys", i))
            .w(px(260.))
            .flex_none()
            .px_2()
            .py(px(3.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(recording.is_some(), |d| d.border_1().border_color(accent))
            .when(recording.is_none(), |d| d.hover(move |s| s.bg(hover)))
            .child(keys)
            .on_click(cx.listener(move |wb, _, _, cx| {
                if wb.shortcuts.recording.as_ref().is_none_or(|r| r.id != id) {
                    wb.record_keys(id.clone(), cx);
                }
            }));

        let small = |name: &str, label: &str| {
            div()
                .id(SharedString::from(format!("shortcut-{name}-{i}")))
                .px_2()
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(muted)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(label.to_owned())
        };
        let mut actions = div().flex().items_center().gap_1().flex_none();
        if row.key.is_some() && recording.is_none() {
            let id = row.id.clone();
            actions = actions.child(small("remove", "Remove").on_click(cx.listener(move |wb, _, _, cx| wb.set_command_keys(&id, None, cx))));
        }
        if row.changed {
            let (id, default) = (row.id.clone(), row.default.clone());
            actions = actions.child(small("reset", "Reset").on_click(cx.listener(move |wb, _, _, cx| wb.set_command_keys(&id, default.as_deref(), cx))));
        }

        let clashes = conflicts(all, row);
        let (category, title) = match row.title.split_once(": ") {
            Some((c, t)) => (Some(c.to_owned()), t.to_owned()),
            None => (None, row.title.clone()),
        };
        div()
            .id(("shortcut-row", i))
            .py(px(5.))
            .border_b_1()
            .border_color(rule)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .children(category.map(|c| div().text_color(muted).child(format!("{c}:"))))
                            .child(title)
                            .child(div().text_size(px(11.)).text_color(muted).child(row.id.clone()))
                            .when(row.changed, |d| d.child(div().text_size(px(11.)).text_color(accent).child("changed"))),
                    )
                    .child(keys_cell)
                    .child(actions.w(px(120.))),
            )
            .when(!clashes.is_empty(), |d| {
                let names: Vec<String> = clashes.iter().map(|r| r.title.clone()).collect();
                d.child(div().text_size(px(12.)).text_color(warning).child(format!("Also the keys of: {}", names.join(", "))))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(id: &str, key: Option<&str>) -> ShortcutRow {
        ShortcutRow { id: id.into(), title: format!("T: {id}"), key: key.map(Into::into), default: None, changed: false }
    }

    #[test]
    fn keys_equal_to_the_default_leave_no_entry() {
        let map = |v: Value| v.as_object().unwrap().clone();
        let changed = with_keys(Map::new(), "view.quickOpen", Some("Ctrl-E"), Some("ctrl-p"));
        assert_eq!(Value::Object(changed.clone()), json!({ "view.quickOpen": "ctrl-e" }));
        // Back to the default: the entry goes (Reset).
        assert!(with_keys(changed, "view.quickOpen", Some("ctrl-p"), Some("ctrl-p")).is_empty());
        // Removing a command's default keys writes ""; removing keys a command never had leaves nothing.
        assert_eq!(Value::Object(with_keys(Map::new(), "file.save", None, Some("ctrl-s"))), json!({ "file.save": "" }));
        assert!(with_keys(map(json!({ "x.y": "ctrl-1" })), "x.y", None, None).is_empty());
        // Chords keep their two presses.
        assert_eq!(Value::Object(with_keys(Map::new(), "a.b", Some("ctrl-k  ctrl-s"), None)), json!({ "a.b": "ctrl-k ctrl-s" }));
    }

    #[test]
    fn conflicts_and_search() {
        let rows = vec![row("file.save", Some("ctrl-s")), row("my.thing", Some("Ctrl-S")), row("other", Some("ctrl-k ctrl-s")), row("none", None)];
        let clashes: Vec<&str> = conflicts(&rows, &rows[0]).iter().map(|r| r.id.as_str()).collect();
        assert_eq!(clashes, ["my.thing"], "same keys written differently, but not a chord ending in them");
        assert!(conflicts(&rows, &rows[3]).is_empty());
        assert!(matches(&rows[0], "save") && matches(&rows[0], "file.") && matches(&rows[0], "ctrl+s") && matches(&rows[0], "CTRL-S"));
        assert!(!matches(&rows[0], "open"));
        assert!(matches(&rows[2], "ctrl+k"));
        assert!(matches(&rows[3], ""));
    }
}

#[cfg(test)]
mod view_tests {
    use crate::actions::PressKeys;
    use crate::workbench::settings_page::view_tests::workbench;

    /// Toggle Side Bar gets new keys by pressing them on the page: they work at once, the old ones stop; then
    /// Remove and Reset.
    #[gpui::test]
    fn keys_are_recorded_saved_and_take_effect(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "shortcuts");
        wb.update_in(cx, |wb, window, cx| {
            wb.open_shortcuts_page(window, cx);
            assert!(matches!(wb.tabs.last().and_then(|t| t.page.clone()), Some(super::Page::Shortcuts)));
            wb.record_keys("view.toggleSideBar".into(), cx);
        });
        cx.run_until_parked();
        cx.press("ctrl-alt-b");
        cx.press("enter");
        cx.run_until_parked();
        let settings = std::fs::read_to_string(guard.dir.join("config/settings.json")).unwrap();
        let expected = if cfg!(target_os = "macos") { "cmd-alt-b" } else { "ctrl-alt-b" };
        assert!(settings.contains(&format!("\"view.toggleSideBar\": \"{expected}\"")), "{settings}");

        let side = |cx: &mut gpui::VisualTestContext| wb.read_with(cx, |wb, _| wb.side_visible);
        let before = side(cx);
        cx.press("ctrl-alt-b");
        cx.run_until_parked();
        assert_ne!(side(cx), before, "the new keys run the command");
        cx.press("ctrl-b");
        cx.run_until_parked();
        assert_ne!(side(cx), before, "the old keys don't any more");

        // Escape while recording changes nothing (and isn't taken for the command's keys).
        wb.update(cx, |wb, cx| wb.record_keys("view.toggleSideBar".into(), cx));
        cx.press("ctrl-q");
        cx.press("escape");
        cx.run_until_parked();
        wb.update(cx, |wb, cx| {
            assert!(wb.shortcuts.recording.is_none());
            assert_eq!(cx.global::<crate::actions::Commands>().key("view.toggleSideBar").as_deref(), Some(expected));
            // Remove: no keys; Reset: the default again, and no entry left.
            wb.set_command_keys("view.toggleSideBar", None, cx);
            assert_eq!(cx.global::<crate::actions::Commands>().key("view.toggleSideBar"), None);
            let default = cx.global::<crate::actions::Commands>().get("view.toggleSideBar").and_then(|c| c.default_key.clone());
            wb.set_command_keys("view.toggleSideBar", default.as_deref(), cx);
            assert!(!wb.settings.is_set("keybindings"), "back to the defaults: nothing left in settings.json");
        });
        let before = side(cx);
        cx.press("ctrl-b");
        cx.run_until_parked();
        assert_ne!(side(cx), before, "the default keys work again");
        drop(guard);
    }
}
