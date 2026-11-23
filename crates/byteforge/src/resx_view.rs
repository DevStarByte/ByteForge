//! The resource editor: a `.resx` resource in all its languages (`Strings.resx`, `Strings.de.resx`, …) as
//! one table, a row per key and a column per language plus the comment. Values are edited in place;
//! keys are added and removed in every language at once; missing translations are marked and can be
//! filtered. Saving writes each file that changed, touching only the `<data>` entries that changed
//! (see `bf_core::resx`).

use std::path::{Path, PathBuf};

use bf_core::resx::{self, Entry};
use gpui::{Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};

use crate::assets::icon;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

pub enum ResxEvent {
    /// Something was edited (the tab shows it's modified) or saved.
    Changed,
    /// Files were written: the Problems list may change.
    Saved,
    /// "Open XML": the file as text.
    OpenFile(PathBuf),
}

struct File {
    culture: Option<String>,
    path: PathBuf,
    bom: bool,
    /// The text on disk, and the text with the edits.
    saved: String,
    text: String,
    entries: Vec<Entry>,
}

impl File {
    fn load(culture: Option<String>, path: PathBuf) -> anyhow::Result<Self> {
        let bytes = std::fs::read(&path)?;
        let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8_lossy(if bom { &bytes[3..] } else { &bytes[..] }).into_owned();
        let entries = resx::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Ok(Self { culture, path, bom, saved: text.clone(), text, entries })
    }

    fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == key)
    }

    fn label(&self) -> String {
        self.culture.clone().unwrap_or_else(|| "Default".to_owned())
    }
}

/// A cell: the value in file `n`, or the comment (kept in the first file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Col {
    Value(usize),
    Comment,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Adding {
    Key,
    Language,
}

pub struct ResxView {
    /// The file the editor was opened with (any language of the resource).
    opened: PathBuf,
    name: String,
    files: Vec<File>,
    missing_only: bool,
    editing: Option<(String, Col)>,
    input: Entity<TextInput>,
    adding: Option<Adding>,
    add_input: Entity<TextInput>,
    error: Option<String>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ResxEvent> for ResxView {}

impl Focusable for ResxView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ResxView {
    pub fn new(path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("", cx));
        let add_input = cx.new(|cx| TextInput::new("", cx));
        let subs = vec![
            cx.subscribe_in(&input, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Confirmed => this.commit(window, cx),
                InputEvent::Cancelled => {
                    this.editing = None;
                    this.focus.focus(window);
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe_in(&add_input, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Confirmed => this.confirm_add(window, cx),
                InputEvent::Cancelled => {
                    this.adding = None;
                    this.focus.focus(window);
                    cx.notify();
                }
                _ => {}
            }),
        ];
        let mut view = Self {
            opened: path.to_path_buf(),
            name: String::new(),
            files: Vec::new(),
            missing_only: false,
            editing: None,
            input,
            adding: None,
            add_input,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        };
        view.load();
        view
    }

    /// Reads the resource's files from disk (dropping edits).
    fn load(&mut self) {
        self.error = None;
        let Some(family) = resx::family(&self.opened) else {
            self.error = Some(format!("{} isn't in a folder that can be read.", self.opened.display()));
            return;
        };
        self.name = family.name.clone();
        self.files.clear();
        for (culture, path) in family.files() {
            match File::load(culture, path) {
                Ok(f) => self.files.push(f),
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
    }

    /// The resource's files changed on disk: read them again, unless there are edits to keep.
    pub fn reload_if_unchanged(&mut self, cx: &mut Context<Self>) {
        if self.is_dirty() {
            return;
        }
        let before: Vec<(PathBuf, String)> = self.files.iter().map(|f| (f.path.clone(), f.saved.clone())).collect();
        self.load();
        let after: Vec<(PathBuf, String)> = self.files.iter().map(|f| (f.path.clone(), f.saved.clone())).collect();
        if before != after {
            cx.notify();
        }
    }

    /// The files of this resource (to tell whether a path belongs to it).
    pub fn contains(&self, path: &Path) -> bool {
        self.files.iter().any(|f| bf_core::paths::same_path(&f.path, path)) || bf_core::paths::same_path(&self.opened, path)
    }

    pub fn title(&self) -> String {
        format!("{}.resx", self.name)
    }

    pub fn is_dirty(&self) -> bool {
        self.files.iter().any(|f| f.text != f.saved)
    }

    /// The keys of all files: the first file's order, then keys only other files have.
    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for f in &self.files {
            for e in &f.entries {
                if !keys.contains(&e.name) {
                    keys.push(e.name.clone());
                }
            }
        }
        keys
    }

    /// Whether a key lacks a value in some language.
    fn row_missing(&self, key: &str) -> bool {
        self.files.iter().any(|f| f.entry(key).is_none_or(|e| e.value.is_empty()))
    }

    /// Applies `edit` to file `n`'s text and parses it again.
    fn edit_file(&mut self, n: usize, edit: impl FnOnce(&str) -> anyhow::Result<String>) {
        let Some(f) = self.files.get_mut(n) else { return };
        match edit(&f.text).and_then(|text| Ok((resx::parse(&text)?, text))) {
            Ok((entries, text)) => {
                f.text = text;
                f.entries = entries;
            }
            Err(e) => self.error = Some(format!("{}: {e:#}", f.path.display())),
        }
    }

    fn cell_text(&self, key: &str, col: Col) -> String {
        match col {
            Col::Value(n) => self.files.get(n).and_then(|f| f.entry(key)).map(|e| e.value.clone()).unwrap_or_default(),
            Col::Comment => self.files.first().and_then(|f| f.entry(key)).and_then(|e| e.comment.clone()).unwrap_or_default(),
        }
    }

    fn start_edit(&mut self, key: String, col: Col, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.commit(window, cx);
        }
        let text = self.cell_text(&key, col);
        self.input.update(cx, |i, cx| {
            i.set_text(text, cx);
            i.select_all_text(cx);
        });
        self.input.read(cx).focus_handle(cx).focus(window);
        self.editing = Some((key, col));
        cx.notify();
    }

    /// Writes the cell being edited into its file.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, col)) = self.editing.take() else { return };
        let text = self.input.read(cx).text().to_owned();
        self.set_cell(&key, col, &text, cx);
        self.focus.focus(window);
    }

    fn set_cell(&mut self, key: &str, col: Col, text: &str, cx: &mut Context<Self>) {
        if self.cell_text(key, col) == text {
            cx.notify();
            return;
        }
        match col {
            // An empty value for a language that doesn't have the key: nothing to write.
            Col::Value(n) if text.is_empty() && self.files.get(n).is_some_and(|f| f.entry(key).is_none()) => {}
            Col::Value(n) => self.edit_file(n, |t| resx::set_value(t, key, text)),
            Col::Comment => self.edit_file(0, |t| resx::set_comment(t, key, Some(text))),
        }
        cx.emit(ResxEvent::Changed);
        cx.notify();
    }

    fn remove_key(&mut self, key: &str, cx: &mut Context<Self>) {
        for n in 0..self.files.len() {
            self.edit_file(n, |t| resx::remove(t, key));
        }
        if self.editing.as_ref().is_some_and(|(k, _)| k == key) {
            self.editing = None;
        }
        cx.emit(ResxEvent::Changed);
        cx.notify();
    }

    fn start_adding(&mut self, what: Adding, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = Some(what);
        self.add_input.update(cx, |i, cx| i.set_text("", cx));
        self.add_input.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn confirm_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(what) = self.adding else { return };
        let text = self.add_input.read(cx).text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let result = match what {
            Adding::Key => self.add_key(&text, cx),
            Adding::Language => self.add_language(&text, cx),
        };
        match result {
            Ok(()) => {
                self.adding = None;
                self.error = None;
                self.focus.focus(window);
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }

    /// Adds a key to every language (empty, to be filled in).
    fn add_key(&mut self, key: &str, cx: &mut Context<Self>) -> Result<(), String> {
        if self.keys().iter().any(|k| k == key) {
            return Err(format!("There is a key {key} already."));
        }
        if self.files.is_empty() {
            return Err("There's no .resx file to add it to.".into());
        }
        for n in 0..self.files.len() {
            self.edit_file(n, |t| resx::add(t, key, "", None));
        }
        cx.emit(ResxEvent::Changed);
        Ok(())
    }

    /// Creates `<name>.<culture>.resx` (the first file's header, no entries) next to the others.
    fn add_language(&mut self, culture: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(first) = self.files.first() else { return Err("There's no .resx file to copy.".into()) };
        let dir = first.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let path = dir.join(format!("{}.{culture}.resx", self.name));
        if resx::family(&path).is_none_or(|f| f.name != self.name) || !culture.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
            return Err(format!("{culture} isn't a language code (de, fr-CA, zh-Hans, …)."));
        }
        if path.exists() {
            return Err(format!("{} exists already.", path.display()));
        }
        let text = resx::empty_like(&first.saved).map_err(|e| format!("{e:#}"))?;
        std::fs::write(&path, &text).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        let file = File::load(Some(culture.to_owned()), path).map_err(|e| format!("{e:#}"))?;
        let at = self.files.iter().position(|f| f.culture.as_deref().is_some_and(|c| c > culture)).unwrap_or(self.files.len());
        self.files.insert(at, file);
        // The columns moved: the cell being edited would point at the wrong file.
        self.editing = None;
        cx.emit(ResxEvent::Saved);
        cx.emit(ResxEvent::Changed);
        Ok(())
    }

    /// Writes each file that changed. Returns the files written, or the first error.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<Vec<PathBuf>> {
        self.commit(window, cx);
        let mut written = Vec::new();
        for f in &mut self.files {
            if f.text == f.saved {
                continue;
            }
            let mut bytes = Vec::with_capacity(f.text.len() + 3);
            if f.bom {
                bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            bytes.extend_from_slice(f.text.as_bytes());
            std::fs::write(&f.path, bytes).map_err(|e| anyhow::anyhow!("Could not write {}: {e}", f.path.display()))?;
            f.saved = f.text.clone();
            written.push(f.path.clone());
        }
        cx.emit(ResxEvent::Saved);
        cx.emit(ResxEvent::Changed);
        cx.notify();
        Ok(written)
    }
}

impl Render for ResxView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let (fg, muted, rule) = (t.c("editor.foreground"), t.c("foreground.muted"), t.c("editorWidget.border"));
        let hover = t.c("toolbar.hoverBackground");
        let warn = t.c_or("editorWarning.foreground", 0xcca700ff);
        let mono = t.editor_font.clone();
        let keys = self.keys();
        let missing_rows = keys.iter().filter(|k| self.row_missing(k)).count();
        let button = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px(px(10.))
                .py(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(rule)
                .cursor_pointer()
                .when(on, |d| d.bg(t.c("list.activeSelectionBackground")))
                .hover(move |s| s.bg(hover))
                .text_size(px(12.))
                .child(label)
        };
        let languages = self.files.iter().filter(|f| f.culture.is_some()).count();
        let opened = self.files.first().map(|f| f.path.clone()).unwrap_or_else(|| self.opened.clone());
        let toolbar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rule)
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(self.title()))
            .child(div().text_size(px(12.)).text_color(muted).child(format!("{} keys, {} language{}", keys.len(), languages, if languages == 1 { "" } else { "s" })))
            .child(div().flex_1())
            .child(button("resx-add-key", "Add Key".into(), self.adding == Some(Adding::Key)).on_click(cx.listener(|v, _, w, cx| v.start_adding(Adding::Key, w, cx))))
            .child(button("resx-add-language", "Add Language".into(), self.adding == Some(Adding::Language)).on_click(cx.listener(|v, _, w, cx| v.start_adding(Adding::Language, w, cx))))
            .child(
                button("resx-missing", format!("Missing translations ({missing_rows})"), self.missing_only).on_click(cx.listener(|v, _, _, cx| {
                    v.missing_only = !v.missing_only;
                    cx.notify();
                })),
            )
            .child(button("resx-xml", "Open XML".into(), false).on_click(cx.listener(move |_, _, _, cx| cx.emit(ResxEvent::OpenFile(opened.clone())))));
        let adding_row = self.adding.map(|what| {
            let prompt = match what {
                Adding::Key => "New key (Enter adds it to every language, Escape cancels):",
                Adding::Language => "Language code, e.g. de or fr-CA (Enter creates the file):",
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_4()
                .py_1()
                .border_b_1()
                .border_color(rule)
                .text_size(px(12.))
                .child(div().flex_none().text_color(muted).child(prompt))
                .child(div().w(px(260.)).child(self.add_input.clone()))
        });
        let error = self.error.clone().map(|e| div().px_4().py_1().text_size(px(12.)).text_color(t.c("errorForeground")).child(e));

        // The table: key, a column per file, comment.
        let key_w = px(220.);
        let header_cell = |text: String| div().flex_1().min_w(px(140.)).px_2().overflow_hidden().whitespace_nowrap().child(text);
        let mut header = div().flex().py_1().px_2().text_size(px(11.)).text_color(muted).border_b_1().border_color(rule).child(div().w(key_w).flex_none().px_2().child("KEY"));
        for f in &self.files {
            header = header.child(header_cell(f.label().to_uppercase()));
        }
        header = header.child(header_cell("COMMENT".into())).child(div().w(px(24.)).flex_none());

        let mut rows = div().flex().flex_col();
        let shown: Vec<&String> = keys.iter().filter(|k| !self.missing_only || self.row_missing(k)).collect();
        if shown.is_empty() {
            let text = if keys.is_empty() { "No string resources yet: Add Key creates one." } else { "Every key has a value in every language." };
            rows = rows.child(div().p_4().text_color(muted).child(text));
        }
        let cols: Vec<Col> = (0..self.files.len()).map(Col::Value).chain([Col::Comment]).collect();
        for (r, key) in shown.into_iter().enumerate() {
            let mut row = div()
                .id(SharedString::from(format!("resx-row-{r}")))
                .group("resx-row")
                .flex()
                .items_center()
                .px_2()
                .min_h(px(26.))
                .border_b_1()
                .border_color(rule)
                .hover(move |s| s.bg(hover))
                .child(div().w(key_w).flex_none().px_2().overflow_hidden().whitespace_nowrap().font_family(mono.clone()).child(key.clone()));
            for &col in &cols {
                let editing = self.editing.as_ref().is_some_and(|(k, c)| k == key && *c == col);
                let id = match col {
                    Col::Value(n) => format!("resx-cell-{key}-{}", self.files[n].label()),
                    Col::Comment => format!("resx-cell-{key}-comment"),
                };
                let cell = div().id(SharedString::from(id.clone())).debug_selector(move || id.clone()).flex_1().min_w(px(140.)).px_2().py(px(3.)).overflow_hidden();
                if editing {
                    row = row.child(cell.child(self.input.clone()));
                    continue;
                }
                let value = match col {
                    Col::Value(n) => self.files[n].entry(key).map(|e| e.value.clone()),
                    Col::Comment => self.files.first().and_then(|f| f.entry(key)).and_then(|e| e.comment.clone()),
                };
                let missing = matches!(col, Col::Value(_)) && value.as_deref().is_none_or(str::is_empty);
                let shown_text = value.unwrap_or_default().replace('\n', " ↵ ");
                let key_for_click = key.clone();
                row = row.child(
                    cell.cursor_text()
                        .whitespace_nowrap()
                        .when(missing, |d| d.text_color(warn).italic().child("missing"))
                        .when(!missing, |d| d.text_color(if matches!(col, Col::Comment) { muted } else { fg }).child(shown_text))
                        .on_click(cx.listener(move |v, _, w, cx| v.start_edit(key_for_click.clone(), col, w, cx))),
                );
            }
            let key_for_remove = key.clone();
            let remove_id = format!("resx-remove-{key}");
            row = row.child(
                div()
                    .id(SharedString::from(remove_id.clone()))
                    .debug_selector(move || remove_id.clone())
                    .w(px(24.))
                    .flex_none()
                    .p(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .invisible()
                    .group_hover("resx-row", |s| s.visible())
                    .hover(move |s| s.bg(hover))
                    .child(icon("trash", px(14.), muted))
                    .on_click(cx.listener(move |v, _, _, cx| v.remove_key(&key_for_remove, cx))),
            );
            rows = rows.child(row);
        }

        div()
            .id("resx-editor")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(t.c("editor.background"))
            .text_color(fg)
            .text_size(px(13.))
            .child(toolbar)
            .children(adding_row)
            .children(error)
            .child(header)
            .child(div().id("resx-rows").flex_1().min_h_0().overflow_y_scroll().child(rows))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const NEUTRAL: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root>\n  <data name=\"Hello\" xml:space=\"preserve\">\n    <value>Hello</value>\n    <comment>Greeting</comment>\n  </data>\n  <data name=\"Bye\" xml:space=\"preserve\">\n    <value>Bye</value>\n  </data>\n</root>\n";
    pub(crate) const GERMAN: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root>\n  <data name=\"Hello\" xml:space=\"preserve\">\n    <value>Hallo</value>\n  </data>\n</root>\n";

    pub(crate) fn setup(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bf-resx-view-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Strings.resx"), NEUTRAL).unwrap();
        std::fs::write(dir.join("Strings.de.resx"), GERMAN).unwrap();
        dir
    }

    /// A new language gets its own file with the header and no entries; all keys show as missing there.
    #[gpui::test]
    fn adds_a_language(cx: &mut gpui::TestAppContext) {
        let dir = setup("language");
        cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
        let (view, cx) = cx.add_window_view(|window, cx| ResxView::new(&dir.join("Strings.resx"), window, cx));
        view.update(cx, |v, cx| {
            assert!(v.add_language("Klingon", cx).is_err());
            v.add_language("fr", cx).unwrap();
            assert_eq!(v.files.iter().map(File::label).collect::<Vec<_>>(), ["Default", "de", "fr"]);
            assert!(v.row_missing("Hello"));
        });
        assert!(resx::parse(&std::fs::read_to_string(dir.join("Strings.fr.resx")).unwrap()).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
