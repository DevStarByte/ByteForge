//! The settings screen (Ctrl+,): every setting ByteForge reads, grouped and searchable, with a switch,
//! a choice or a value to edit, and Reset for the ones the file sets. Changes go to settings.json (only
//! that key's text changes) and apply live like hand edits. A column on the left lists the groups and
//! scrolls to one; the settings the turned-on modules declare in `module.json` come last, under Extensions.

use bf_core::plugins::SettingContribution;
use gpui::{Div, ScrollHandle, point};
use serde_json::{Value, json};

use super::*;

/// How a setting is edited.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Kind {
    Bool,
    /// One of these values (shown as buttons).
    Choice(Vec<String>),
    Number,
    Text,
    /// A list of strings, edited comma-separated.
    List,
    /// Edited elsewhere (a picker, or settings.json).
    Special,
}

#[derive(Clone)]
pub(super) struct SettingInfo {
    pub key: String,
    pub title: String,
    pub description: String,
    pub category: String,
    /// The module that declares it (its name), for settings under Extensions.
    pub module: Option<String>,
    pub kind: Kind,
    pub default: Value,
}

/// How wide the settings list gets, and the column its controls sit in (beside the text they belong to).
const WIDTH: f32 = 780.;
const CONTROL: f32 = 260.;

/// The category module settings are listed under, after the built-in ones.
pub(super) const EXTENSIONS: &str = "Extensions";

macro_rules! s {
    ($cat:expr, $key:expr, $title:expr, $kind:expr, $default:expr, $desc:expr) => {
        SettingInfo {
            key: $key.into(),
            title: $title.into(),
            description: $desc.into(),
            category: $cat.into(),
            module: None,
            kind: $kind,
            default: json!($default),
        }
    };
}

fn choice(choices: &[&str]) -> Kind {
    Kind::Choice(choices.iter().map(|c| (*c).to_owned()).collect())
}

/// Every built-in setting the screen shows, in screen order.
pub(super) fn all_settings() -> Vec<SettingInfo> {
    use Kind::*;
    vec![
        s!("Appearance", "workbench.theme", "Color theme", Special, "ByteForge Dark", "The theme of the whole window. Preferences → Color Theme shows them with a live preview."),
        s!("Editor", "editor.tabSize", "Tab size", Number, 4, "Columns a tab takes, and the size of one indent."),
        s!("Editor", "editor.insertSpaces", "Insert spaces", Bool, true, "Tab inserts spaces instead of a tab character."),
        s!("Editor", "editor.wordWrap", "Word wrap", choice(&["off", "on"]), "off", "Long lines continue on the next rows (Alt+Z)."),
        s!("Editor", "editor.autoClosingBrackets", "Close brackets and quotes", Bool, true, "Typing ( [ { \" ' adds the closing one."),
        s!("Editor", "editor.lineNumbers", "Line numbers", Bool, true, "Numbers in the gutter."),
        s!("Editor", "editor.indentGuides", "Indent guides", Bool, true, "Vertical lines at each indent level."),
        s!("Editor", "editor.folding", "Folding", Bool, true, "Fold arrows in the gutter."),
        s!("Editor", "editor.minimap", "Minimap", Bool, true, "An overview of the file at the right edge."),
        s!("Editor", "editor.stickyScroll.enabled", "Sticky scroll", Bool, true, "The first lines of the namespace, class and method the top of the view is in stay pinned at the top; click one to go there."),
        s!("Editor", "editor.stickyScroll.maxLineCount", "Sticky scroll lines", Number, 5, "At most this many pinned lines (and no more than a third of the editor)."),
        s!("Editor", "editor.inlayHints", "Inlay hints", Bool, true, "Inferred types and parameter names inside the code (C#)."),
        s!("Editor", "editor.codeLens", "Code Lens", Bool, true, "Reference counts and Run Test / Debug Test above declarations (C#); click one to list the references or run the test."),
        s!("Editor", "editor.linkedEditing", "Linked editing", Bool, true, "Renaming an XML, HTML or Razor tag renames its closing tag too."),
        s!("Editor", "editor.smoothScrolling", "Smooth scrolling", Bool, true, "Scrolling glides instead of jumping."),
        s!("Editor", "editor.cursorBlinking", "Blinking cursor", Bool, true, "The caret blinks."),
        s!("Editor", "editor.formatOnSave", "Format on save", Bool, false, "Ctrl+S formats the file first."),
        s!("Editor", "editor.defaultFormatter", "Default formatter", Text, "", "resharper, csharp-ls or csharpier (empty: the first that handles the file)."),
        s!("Files", "files.autoSave", "Auto save", choice(&["off", "afterDelay", "onFocusChange", "onWindowChange"]), "off", "Save by yourself, after a pause in typing, when switching tabs, or also when leaving the window."),
        s!("Files", "files.autoSaveDelay", "Auto save delay", Number, 1000, "Milliseconds of quiet before \"afterDelay\" saves."),
        s!("Files", "files.exclude", "Excluded folders", List, ["bin", "obj", ".git", "node_modules", "target"], "Folder and file names left out of the Explorer, search and Go to File."),
        s!("Files", "explorer.autoReveal", "Reveal the current file", Bool, true, "The Explorer expands and scrolls to the file you switch to."),
        s!("Files", "explorer.nativeIcons", "Finder icons", Bool, true, "macOS: the Explorer uses Finder's file and folder icons."),
        s!("C# and .NET", "csharp.server", "C# language server", choice(&["csharp-ls", "roslyn"]), "csharp-ls", "csharp-ls, or Microsoft's Roslyn server (the one VS Code uses: more fixes and analyzers; \"C#: Install or Update Roslyn Language Server\" installs it)."),
        s!("C# and .NET", "csharp.languageServer", "csharp-ls path", Text, "", "Path to csharp-ls (empty: found on PATH or in ~/.dotnet/tools)."),
        s!("C# and .NET", "metrics.statusBar", "App metrics in the status bar", Bool, true, "While the app runs, dotnet-counters (when installed) reports its CPU and memory to the status bar and the Metrics tab."),
        s!("C# and .NET", "dotnet.project", "Startup project", Text, "", "The project F5 and Run start, relative to the folder (empty: found by itself)."),
        s!("C# and .NET", "dotnet.launchProfile", "Launch profile", Text, "", "The launchSettings.json profile Run and F5 use (Select Launch Profile… sets it per project)."),
        s!("Refactoring", "refactor.previewMultiFileChanges", "Ask before changing several files", Bool, true, "A rename or refactoring that changes several files lists them first."),
        s!("Refactoring", "refactor.renameGenerated", "Rename what a refactoring made up", Bool, true, "After Extract Method and the like, the new name goes straight to Rename."),
        s!("Debugging", "debug.justMyCode", "Just my code", Bool, true, "Step over framework and library code."),
        s!("Debugging", "debug.stopAtEntry", "Stop at entry", Bool, false, "Pause on the first line of Main."),
        s!("Debugging", "debug.openBrowser", "Open the browser", Bool, true, "F5 on a web app opens its page once it listens."),
        s!("Debugging", "debug.breakOnExceptions", "Break on exceptions", choice(&["user-unhandled", "all", "never"]), ["user-unhandled"], "Which exceptions pause the program."),
        s!("Debugging", "debug.netcoredbgPath", "netcoredbg path", Text, "", "Path to the debugger (empty: found by itself)."),
        s!("Debugging", "debug.browser", "Browser for Blazor WebAssembly", Text, "", "Edge, Chrome or Chromium to debug WebAssembly apps in (empty: found by itself)."),
        s!("Git", "git.blame.currentLine", "Blame of the current line", Bool, true, "Who last changed the caret's line, shown after it."),
        s!("AI Assistant", "ai.provider", "Provider", choice(&["anthropic", "local"]), "anthropic", "anthropic: Claude through the Anthropic API. local: a model on this machine through Ollama, LM Studio or another OpenAI-compatible server."),
        s!("AI Assistant", "ai.model", "Claude model", Text, bf_core::ai::DEFAULT_CLAUDE_MODEL, "The Claude model to use (with the anthropic provider)."),
        s!("AI Assistant", "ai.effort", "Effort", choice(&["low", "medium", "high", "xhigh", "max"]), "high", "How hard Claude thinks before it answers: higher is more thorough, slower and uses more tokens."),
        s!("AI Assistant", "ai.apiKey", "Anthropic API key", Text, "", "Used when the ANTHROPIC_API_KEY environment variable isn't set. Stored as plain text in settings.json: prefer the environment variable."),
        s!("AI Assistant", "ai.local.url", "Local server", Text, bf_core::ai::DEFAULT_LOCAL_URL, "The OpenAI-compatible endpoint of the local model server (Ollama: http://localhost:11434/v1, LM Studio: http://localhost:1234/v1)."),
        s!("AI Assistant", "ai.local.model", "Local model", Text, bf_core::ai::DEFAULT_LOCAL_MODEL, "The local model's name, as the server knows it (ollama list). Tool use needs a model that supports it (Qwen 3.5, Llama 3.1 and later)."),
        s!("AI Assistant", "ai.local.apiKey", "Local server key", Text, "", "Only if the local server asks for one (sent as a bearer token)."),
        s!("Terminal", "terminal.shell", "Shell", Text, "", "The terminal's program (empty: your login shell)."),
        s!("Terminal", "terminal.args", "Shell arguments", List, Vec::<String>::new(), "Arguments for the shell, comma-separated."),
        s!("Updates", "update.checkAutomatically", "Check for updates", Bool, true, "Look for a new version every time ByteForge starts (Windows and macOS)."),
        s!("Updates", "update.includePrereleases", "Include betas", Bool, false, "Offer beta versions too."),
        s!("Keyboard", "keybindings", "Key bindings", Special, json!({}), "Each command's keys: change them on the Keyboard Shortcuts page (Preferences), or in settings.json, e.g. \"view.quickOpen\": \"ctrl-e\" (\"\" unbinds)."),
    ]
}

/// A module's declared setting as the screen shows it.
pub(super) fn module_setting(module: &str, c: &SettingContribution) -> SettingInfo {
    let kind = match c.kind.as_str() {
        _ if !c.choices.is_empty() => Kind::Choice(c.choices.clone()),
        "boolean" | "bool" => Kind::Bool,
        "number" | "integer" => Kind::Number,
        "string" | "text" => Kind::Text,
        "list" | "array" => Kind::List,
        _ => Kind::Special,
    };
    SettingInfo {
        key: c.key.clone(),
        title: if c.title.is_empty() { c.key.clone() } else { c.title.clone() },
        description: c.description.clone(),
        category: EXTENSIONS.into(),
        module: Some(module.to_owned()),
        kind,
        default: c.default.clone(),
    }
}

/// The built-in settings, then those of the modules that are turned on, by module name. A key ByteForge
/// already lists, or one listed twice, is left out.
pub(super) fn settings_with_modules(modules: &[ModuleManifest], disabled: &[String]) -> Vec<SettingInfo> {
    let mut all = all_settings();
    let mut on: Vec<&ModuleManifest> = modules.iter().filter(|m| !disabled.contains(&m.id) && !m.settings.is_empty()).collect();
    on.sort_by_key(|m| m.name.to_lowercase());
    for m in on {
        for c in &m.settings {
            if !c.key.is_empty() && !all.iter().any(|s| s.key == c.key) {
                all.push(module_setting(&m.name, c));
            }
        }
    }
    all
}

/// A run of settings with the same category and module: one section of the list and one entry of the
/// column on the left.
pub(super) struct Section {
    pub category: String,
    pub module: Option<String>,
    pub settings: Vec<SettingInfo>,
}

pub(super) fn sections(settings: Vec<SettingInfo>) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for s in settings {
        match out.last_mut() {
            Some(last) if last.category == s.category && last.module == s.module => last.settings.push(s),
            _ => out.push(Section { category: s.category.clone(), module: s.module.clone(), settings: vec![s] }),
        }
    }
    out
}

/// Whether a setting matches the search text (title, key, description, category or module).
pub(super) fn matches_search(s: &SettingInfo, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || [s.title.as_str(), &s.key, &s.description, &s.category, s.module.as_deref().unwrap_or("")].iter().any(|t| t.to_lowercase().contains(&q))
}

/// The text a value is edited as (a list comma-separated, a string bare).
pub(super) fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(|i| i.as_str().map(str::to_owned).unwrap_or_else(|| i.to_string())).collect::<Vec<_>>().join(", "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The value typed for a setting of `kind`; an error message when it doesn't fit.
pub(super) fn parse_value(kind: &Kind, text: &str) -> Result<Value, String> {
    let t = text.trim();
    match kind {
        Kind::Number => match (t.parse::<i64>(), t.parse::<f64>()) {
            (Ok(n), _) => Ok(json!(n)),
            (_, Ok(n)) if n.is_finite() => Ok(json!(n)),
            _ => Err(format!("\"{t}\" isn't a number")),
        },
        Kind::List => Ok(json!(t.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>())),
        _ => Ok(json!(t)),
    }
}

/// The AI settings of the provider not in use: Claude's under "local", the local server's under "anthropic".
fn hidden_for_provider(key: &str, local: bool) -> bool {
    if local { matches!(key, "ai.model" | "ai.effort" | "ai.apiKey") } else { key.starts_with("ai.local.") }
}

/// A choice's value as stored: debug.breakOnExceptions keeps a list of filters.
fn choice_value(key: &str, choice: &str) -> Value {
    match (key, choice) {
        ("debug.breakOnExceptions", "never") => json!([]),
        ("debug.breakOnExceptions", c) => json!([c]),
        (_, c) => json!(c),
    }
}

fn current_choice(key: &str, v: &Value) -> String {
    match (key, v) {
        ("debug.breakOnExceptions", Value::Array(a)) if a.is_empty() => "never".into(),
        ("debug.breakOnExceptions", Value::Array(a)) => a.first().and_then(Value::as_str).unwrap_or("user-unhandled").to_owned(),
        (_, v) => value_text(v),
    }
}

impl Workbench {
    /// Every setting the screen lists: the built-in ones and those of the turned-on modules.
    pub(super) fn listed_settings(&self) -> Vec<SettingInfo> {
        let disabled: Vec<String> = self.settings.get("modules.disabled", Vec::new());
        let local = self.settings.get_string("ai.provider").as_deref() == Some("local");
        settings_with_modules(&self.modules, &disabled).into_iter().filter(|s| !hidden_for_provider(&s.key, local)).collect()
    }

    /// Ctrl+,: the settings screen in a tab (reused when it is open).
    pub(super) fn open_settings_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| matches!(t.page, Some(Page::Settings))) {
            return self.activate(ix, window, cx);
        }
        let input = cx.new(|cx| TextInput::new("Search settings", cx));
        let sub = cx.subscribe_in(&input, window, |wb: &mut Self, _, _: &InputEvent, _, cx| {
            // a new search starts at the top of what it finds
            wb.settings_scroll.set_offset(point(px(0.), px(0.)));
            wb.settings_nav_clicked = None;
            cx.notify()
        });
        self.settings_search = Some(input.clone());
        self.settings_scroll = ScrollHandle::new();
        self.settings_nav_clicked = None;
        let editor = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e
        });
        self.add_tab(editor, None, "Settings".into(), false, window, cx);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(Page::Settings);
            tab._subs.push(sub);
        }
        window.focus(&input.focus_handle(cx));
        cx.notify();
    }

    /// A value typed in the prompt for setting `key`.
    pub(super) fn set_setting_text(&mut self, key: &str, text: &str, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let Some(info) = self.listed_settings().into_iter().find(|s| s.key == key) else { return Ok(()) };
        let value = parse_value(&info.kind, text).map_err(|e| anyhow::anyhow!(e))?;
        self.settings.set(key, value);
        cx.notify();
        Ok(())
    }

    /// The section the column on the left marks: the one at the top of the list, or the one last picked
    /// there when the list can't scroll that far (the last sections can be too short to reach the top).
    fn current_settings_section(&self, count: usize) -> usize {
        let handle = &self.settings_scroll;
        // section i is child i of the list
        let top = handle.top_item().min(count.saturating_sub(1));
        let max = handle.max_offset().height;
        let at_end = max <= px(0.) || -handle.offset().y >= max - px(1.);
        match self.settings_nav_clicked {
            Some(picked) if picked < count && (picked == top || (at_end && picked > top)) => picked,
            _ => top,
        }
    }

    /// The column on the left: the categories, then Extensions with one entry per module.
    fn render_settings_nav(&mut self, sections: &[Section], cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let t = Theme::global(cx);
        let (fg, muted) = (t.c("editor.foreground"), t.c("foreground.muted"));
        let hover = t.c("toolbar.hoverBackground");
        let selected_bg = t.c_or("list.activeSelectionBackground", 0x04395eff);
        let accent = t.c_or("button.background", 0x0e639cff);
        let current = self.current_settings_section(sections.len());
        // How many settings of each section differ from the default: where your configuration is.
        let changed: Vec<usize> = sections.iter().map(|s| s.settings.iter().filter(|i| self.settings.is_set(&i.key)).count()).collect();
        let modified = if t.has("settings.modifiedIndicator") { t.c("settings.modifiedIndicator") } else { t.c("editorGutter.modifiedBackground") };
        let entry = |id: SharedString, label: String, indent: bool, active: bool, ix: usize, count: usize, cx: &mut Context<Self>| {
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .gap_2()
                .py(px(4.))
                .pr_2()
                .pl(px(if indent { 22. } else { 10. }))
                .rounded(px(5.))
                .cursor_pointer()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(if active { fg } else { muted })
                .when(active, |d| d.bg(selected_bg).child(div().absolute().left_0().top(px(6.)).w(px(3.)).h(px(14.)).rounded(px(2.)).bg(accent)))
                .when(!active, |d| d.hover(move |s| s.bg(hover).text_color(fg)))
                .child(div().flex_1().min_w_0().overflow_hidden().child(label))
                .when(count > 0, |d| {
                    d.tooltip(tooltip(if count == 1 { "1 setting changed from the default".to_owned() } else { format!("{count} settings changed from the default") }))
                        .child(div().flex_none().flex().items_center().gap(px(4.)).text_size(px(11.)).child(div().size(px(6.)).rounded_full().bg(modified)).child(count.to_string()))
                })
                .on_click(cx.listener(move |wb, _, _, cx| {
                    wb.settings_scroll.scroll_to_top_of_item(ix);
                    wb.settings_nav_clicked = Some(ix);
                    cx.notify();
                }))
        };
        let mut nav = div()
            .id("settings-nav")
            .w(px(200.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .pt_8()
            .pb_4()
            .px_2()
            .flex()
            .flex_col()
            .gap(px(1.))
            .border_r_1()
            .border_color(t.c("editorWidget.border"))
            .text_size(px(13.));
        let mut extensions_listed = false;
        for (ix, section) in sections.iter().enumerate() {
            let id: SharedString = format!("settings-nav-{ix}").into();
            match &section.module {
                None => nav = nav.child(entry(id, section.category.clone(), false, current == ix, ix, changed[ix], cx)),
                Some(module) => {
                    if !extensions_listed {
                        extensions_listed = true;
                        let inside = sections.get(current).is_some_and(|s| s.module.is_some());
                        nav = nav.child(
                            entry("settings-nav-extensions".into(), EXTENSIONS.to_owned(), false, false, ix, 0, cx)
                                .mt_2()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .when(inside, |d| d.text_color(fg)),
                        );
                    }
                    nav = nav.child(entry(id, module.clone(), true, current == ix, ix, changed[ix], cx));
                }
            }
        }
        nav
    }

    pub(super) fn render_settings_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.settings_search.as_ref().map(|i| i.read(cx).text().to_owned()).unwrap_or_default();
        let sections = sections(self.listed_settings().into_iter().filter(|s| matches_search(s, &query)).collect());
        let nav = self.render_settings_nav(&sections, cx);
        let t = Theme::global(cx);
        let (fg, muted, rule) = (t.c("editor.foreground"), t.c("foreground.muted"), t.c("editorWidget.border"));
        let hover = t.c("toolbar.hoverBackground");
        let (background, open_json_bg) = (t.c("editor.background"), hover);

        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_size(px(22.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Settings"))
            .child(div().flex_1())
            .child(
                div()
                    .id("settings-open-json")
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(rule)
                    .cursor_pointer()
                    .hover(move |s| s.bg(open_json_bg))
                    .text_size(px(12.))
                    .child("Open settings.json")
                    .on_click(cx.listener(|wb, _, w, cx| {
                        let path = wb.paths.settings_file();
                        wb.open_file(path, None, w, cx);
                    })),
            );
        // The title and the search stay above the list while it scrolls.
        let top = div()
            .flex_none()
            .pt_6()
            .pb_3()
            .border_b_1()
            .border_color(rule)
            // the same width and padding as the sections below, so their edges line up
            .child(div().max_w(px(WIDTH)).px_8().flex().flex_col().gap_3().child(header).children(self.settings_search.clone()));
        // The list's children are the sections, one each; the column on the left scrolls to them by index.
        let mut list = div().id("settings-list").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.settings_scroll).flex().flex_col();
        let count = sections.len();
        let mut row = 0usize;
        let mut extensions_shown = false;
        for (sx, section) in sections.into_iter().enumerate() {
            let mut body = div().max_w(px(WIDTH)).px_8().when(sx == 0, |d| d.pt_2()).when(sx + 1 == count, |d| d.pb_8()).flex().flex_col();
            if section.module.is_some() && !extensions_shown {
                extensions_shown = true;
                body = body
                    .child(div().mt_6().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child(EXTENSIONS))
                    .child(div().text_size(px(12.)).text_color(muted).child("What the installed modules that are turned on let you set."));
            }
            body = match &section.module {
                None => body.child(div().mt_4().mb_1().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child(section.category.clone())),
                Some(module) => body.child(div().mt_3().mb_1().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(muted).child(module.clone())),
            };
            for info in section.settings {
                row += 1;
                body = body.child(self.render_setting_row(row, info, cx));
            }
            list = list.child(body);
        }
        if count == 0 {
            list = list.child(div().max_w(px(WIDTH)).px_8().mt_4().text_color(muted).child(format!("No settings match “{query}”. Try a word from its description, or its key.")));
        }
        div()
            .id("settings-page")
            .size_full()
            .flex()
            .bg(background)
            .text_color(fg)
            .text_size(px(13.))
            .child(nav)
            .child(div().flex_1().min_w_0().h_full().flex().flex_col().child(top).child(list))
    }

    /// Under the model settings, the models to pick from: Claude's for "ai.model", the ones the local server
    /// has for "ai.local.model". A click sets it.
    fn render_model_suggestions(&mut self, key: &str, value: &Value, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = Theme::global(cx);
        let (muted, rule, hover) = (t.c("foreground.muted"), t.c("editorWidget.border"), t.c("toolbar.hoverBackground"));
        let accent = t.c_or("button.background", 0x0e639cff);
        let on_accent = t.c_or("button.foreground", 0xffffffff);
        let current = value.as_str().unwrap_or("").to_owned();
        let chip = |id: SharedString, label: String, tip: Option<String>, picked: bool| {
            div()
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .px_2()
                .py(px(2.))
                .rounded(px(5.))
                .border_1()
                .border_color(if picked { accent } else { rule })
                .text_size(px(12.))
                .cursor_pointer()
                .when(picked, |d| d.bg(accent).text_color(on_accent))
                .when(!picked, |d| d.hover(move |s| s.bg(hover)))
                .when_some(tip, |d, tip| d.tooltip(tooltip(tip)))
                .child(label)
        };
        let models: Vec<(String, String, Option<String>)> = match key {
            "ai.model" => bf_core::ai::CLAUDE_MODELS.iter().map(|(id, note)| ((*id).to_owned(), bf_core::ai::display_name(id), Some(format!("{id}\n{note}")))).collect(),
            "ai.local.model" => {
                let url = self.ai_local_url();
                self.refresh_local_models(false, cx);
                match self.assistant.local_models(&url).cloned() {
                    Some(Ok(list)) if !list.is_empty() => list.into_iter().map(|m| (m.clone(), m, None)).collect(),
                    Some(Ok(_)) => return Some(div().text_size(px(12.)).text_color(muted).child(format!("The server at {url} has no models yet (ollama pull qwen3.5).")).into_any_element()),
                    Some(Err(e)) => {
                        return Some(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .text_size(px(12.))
                                .text_color(muted)
                                .child(format!("No model list from {url}: {e}. Is the server running?"))
                                .child(
                                    div()
                                        .id("setting-models-refresh")
                                        .debug_selector(|| "setting-models-refresh".into())
                                        .text_color(t.c_or("textLink.foreground", 0x3794ffff))
                                        .cursor_pointer()
                                        .child("Try again")
                                        .on_click(cx.listener(|wb, _, _, cx| wb.refresh_local_models(true, cx))),
                                )
                                .into_any_element(),
                        );
                    }
                    None => return Some(div().text_size(px(12.)).text_color(muted).child(format!("Asking {url} for its models…")).into_any_element()),
                }
            }
            _ => return None,
        };
        let key_owned = key.to_owned();
        let mut row = div().mt(px(4.)).flex().flex_wrap().items_center().gap(px(6.));
        for (n, (id, label, tip)) in models.into_iter().enumerate() {
            let picked = id == current;
            let (k, v) = (key_owned.clone(), id.clone());
            row = row.child(chip(SharedString::from(format!("setting-model-{key_owned}-{n}")), label, tip, picked).on_click(cx.listener(move |wb, _, _, cx| {
                wb.settings.set(&k, json!(v));
                cx.notify();
            })));
        }
        if key == "ai.local.model" {
            row = row.child(
                div()
                    .id("setting-models-refresh")
                    .debug_selector(|| "setting-models-refresh".into())
                    .text_size(px(12.))
                    .text_color(t.c_or("textLink.foreground", 0x3794ffff))
                    .cursor_pointer()
                    .tooltip(tooltip("Ask the server again (after ollama pull)"))
                    .child("Refresh")
                    .on_click(cx.listener(|wb, _, _, cx| wb.refresh_local_models(true, cx))),
            );
        }
        Some(row.into_any_element())
    }

    /// One setting: its title, description and key, the control that changes it, and Reset when set.
    fn render_setting_row(&mut self, i: usize, info: SettingInfo, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let default_text = match value_text(&info.default) {
            d if d.is_empty() => "empty".to_owned(),
            d => d,
        };
        let t = Theme::global(cx);
        let (fg, muted, rule) = (t.c("editor.foreground"), t.c("foreground.muted"), t.c("editorWidget.border"));
        let hover = t.c("toolbar.hoverBackground");
        let accent = t.c_or("button.background", 0x0e639cff);
        let on_accent = t.c_or("button.foreground", 0xffffffff);
        let key: SharedString = info.key.clone().into();
        let value: Value = self.settings.get(&key, info.default.clone());
        let set = self.settings.is_set(&key);
        let control: gpui::AnyElement = match &info.kind {
            Kind::Bool => {
                let on = value.as_bool().unwrap_or(false);
                let key = key.clone();
                div()
                    .id(("setting-bool", i))
                    .w(px(36.))
                    .h(px(20.))
                    .rounded(px(10.))
                    .p(px(2.))
                    .flex()
                    .when(on, |d| d.justify_end().bg(accent))
                    .when(!on, |d| d.justify_start().bg(t.c_or("input.background", 0x3c3c3cff)).border_1().border_color(rule))
                    .cursor_pointer()
                    .child(div().w(px(16.)).h(px(16.)).rounded(px(8.)).bg(if on { on_accent } else { muted }))
                    .on_click(cx.listener(move |wb, _, _, cx| {
                        wb.settings.set(&key, json!(!on));
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Kind::Choice(choices) => {
                let current = current_choice(&key, &value);
                div()
                    .flex()
                    .border_1()
                    .border_color(rule)
                    .rounded(px(5.))
                    .overflow_hidden()
                    .children(choices.iter().enumerate().map(|(ci, choice)| {
                        let selected = current == *choice;
                        let (key, choice) = (key.clone(), choice.clone());
                        div()
                            .id(SharedString::from(format!("setting-choice-{i}-{ci}")))
                            .px_2()
                            .py(px(3.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .when(ci > 0, |d| d.border_l_1().border_color(rule))
                            .when(selected, |d| d.bg(accent).text_color(on_accent))
                            .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                            .child(choice.clone())
                            .on_click(cx.listener(move |wb, _, _, cx| {
                                wb.settings.set(&key, choice_value(&key, &choice));
                                cx.notify();
                            }))
                    }))
                    .into_any_element()
            }
            Kind::Number | Kind::Text | Kind::List => {
                let text = value_text(&value);
                let shown_text = if text.is_empty() { "(not set)".to_owned() } else { text.clone() };
                let key = key.clone();
                div()
                    .id(("setting-edit", i))
                    .max_w(px(CONTROL - 24.))
                    .px_2()
                    .py(px(3.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(rule)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(t.editor_font.clone())
                    .text_size(px(12.))
                    .text_color(if text.is_empty() { muted } else { fg })
                    .child(shown_text)
                    .on_click(cx.listener(move |wb, _, w, cx| wb.open_prompt(PromptKind::SettingValue { key: key.to_string() }, &text, w, cx)))
                    .into_any_element()
            }
            Kind::Special if key == "workbench.theme" => div()
                .id(("setting-theme", i))
                .px_2()
                .py(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(rule)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .text_size(px(12.))
                .child(format!("{} …", value_text(&value)))
                .on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(SelectTheme), cx)))
                .into_any_element(),
            Kind::Special if key == "keybindings" => div()
                .id(("setting-shortcuts", i))
                .px_2()
                .py(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(rule)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .text_size(px(12.))
                .child("Keyboard Shortcuts …")
                .on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(crate::actions::OpenKeyboardShortcuts), cx)))
                .into_any_element(),
            Kind::Special => div()
                .id(("setting-json", i))
                .px_2()
                .py(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(rule)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .text_size(px(12.))
                .child("Edit in settings.json")
                .on_click(cx.listener(|wb, _, w, cx| {
                    let path = wb.paths.settings_file();
                    wb.open_file(path, None, w, cx);
                }))
                .into_any_element(),
        };
        let special = matches!(info.kind, Kind::Special);
        // A changed setting is marked like a changed line in the editor: a bar at its edge.
        let modified = if t.has("settings.modifiedIndicator") { t.c("settings.modifiedIndicator") } else { t.c("editorGutter.modifiedBackground") };
        let row_rule = if t.has("settings.rowBorder") { t.c("settings.rowBorder") } else { rule.opacity(0.45) };
        let reset = (set && !special).then(|| {
            let key = key.clone();
            div()
                .id(("setting-reset", i))
                .debug_selector(move || format!("setting-reset-{i}"))
                .p(px(3.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(tooltip(format!("Reset to the default ({default_text})")))
                .child(icon("discard", px(14.), muted))
                .on_click(cx.listener(move |wb, _, _, cx| {
                    wb.settings.remove(&key);
                    cx.notify();
                }))
        });
        div()
            .id(("setting", i))
            .relative()
            .flex()
            .items_start()
            .gap_6()
            .py(px(10.))
            .pl(px(14.))
            .pr_2()
            .border_b_1()
            .border_color(row_rule)
            .hover(move |s| s.bg(hover.opacity(0.5)))
            .when(set, |d| d.child(div().absolute().left_0().top(px(10.)).bottom(px(10.)).w(px(2.)).rounded(px(1.)).bg(modified)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_baseline()
                            .gap_x_2()
                            .child(div().font_weight(gpui::FontWeight::MEDIUM).child(info.title))
                            .child(div().text_size(px(11.)).text_color(muted).font_family(t.editor_font.clone()).child(key.clone())),
                    )
                    .when(!info.description.is_empty(), |d| d.child(div().max_w(px(520.)).text_size(px(12.)).text_color(muted).child(info.description)))
                    .children(self.render_model_suggestions(&key, &value, cx)),
            )
            .child(div().flex_none().w(px(CONTROL)).flex().items_center().justify_end().gap_1().child(control).child(div().w(px(20.)).flex_none().children(reset)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_well_formed() {
        let all = all_settings();
        let mut keys: Vec<&str> = all.iter().map(|s| s.key.as_str()).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "a setting is listed twice");
        for s in &all {
            if let Kind::Choice(choices) = &s.kind {
                let default = current_choice(&s.key, &s.default);
                assert!(choices.contains(&default), "{}: default {default} isn't a choice", s.key);
            }
        }
    }

    #[test]
    fn values_are_typed_and_shown() {
        assert_eq!(parse_value(&Kind::Number, " 8 ").unwrap(), json!(8));
        assert_eq!(parse_value(&Kind::Number, "1.5").unwrap(), json!(1.5));
        assert!(parse_value(&Kind::Number, "eight").is_err());
        assert_eq!(parse_value(&Kind::List, "bin, obj,, dist").unwrap(), json!(["bin", "obj", "dist"]));
        assert_eq!(value_text(&json!(["bin", "obj"])), "bin, obj");
        assert_eq!(current_choice("debug.breakOnExceptions", &json!([])), "never");
        assert_eq!(choice_value("debug.breakOnExceptions", "all"), json!(["all"]));
        let wrap = all_settings().into_iter().find(|s| s.key == "editor.wordWrap").unwrap();
        assert!(matches_search(&wrap, "WRAP") && matches_search(&wrap, "editor") && !matches_search(&wrap, "netcoredbg"));
    }

    #[test]
    fn module_settings_come_last_under_extensions() {
        let manifest = |id: &str, name: &str, settings: &str| -> ModuleManifest {
            serde_json::from_str(&format!(r#"{{ "id": "{id}", "name": "{name}", "settings": {settings} }}"#)).unwrap()
        };
        let modules = vec![
            manifest(
                "z.sql",
                "SQL",
                r#"[{ "key": "sql.discover", "title": "Discover", "type": "boolean", "default": true },
                    { "key": "sql.format.keywordCase", "type": "string", "choices": ["upper", "lower"], "default": "upper" },
                    { "key": "sql.connections", "type": "json", "default": [] },
                    { "key": "editor.tabSize", "type": "number" }]"#,
            ),
            manifest("a.aspire", "Aspire", r#"[{ "key": "aspire.cliPath", "title": "CLI path", "type": "string", "default": "" }]"#),
            manifest("off.mod", "Off", r#"[{ "key": "off.x", "type": "boolean" }]"#),
        ];
        let all = settings_with_modules(&modules, &["off.mod".to_owned()]);
        let builtin = all_settings().len();
        let added: Vec<(&str, Option<&str>)> = all[builtin..].iter().map(|s| (s.key.as_str(), s.module.as_deref())).collect();
        // sorted by module name; a built-in key isn't taken over; a turned-off module adds nothing
        assert_eq!(
            added,
            [("aspire.cliPath", Some("Aspire")), ("sql.discover", Some("SQL")), ("sql.format.keywordCase", Some("SQL")), ("sql.connections", Some("SQL"))]
        );
        assert!(all[builtin..].iter().all(|s| s.category == EXTENSIONS));
        assert_eq!(all[builtin].title, "CLI path");
        assert_eq!(all[builtin + 1].kind, Kind::Bool);
        assert_eq!(all[builtin + 2].kind, Kind::Choice(vec!["upper".into(), "lower".into()]));
        assert_eq!(all[builtin + 3].kind, Kind::Special);
        assert!(matches_search(&all[builtin + 1], "sql"), "a module's name finds its settings");

        let secs = sections(all);
        let modules_last: Vec<Option<&str>> = secs.iter().rev().take(2).map(|s| s.module.as_deref()).collect();
        assert_eq!(modules_last, [Some("SQL"), Some("Aspire")]);
        assert_eq!(secs.iter().take_while(|s| s.module.is_none()).count(), secs.len() - 2);
    }
}

#[cfg(test)]
pub(crate) mod view_tests {
    use super::*;

    /// A workbench in a test window with its own config folder (removed on drop) and no folder open.
    pub(crate) struct TestWorkbench {
        pub dir: PathBuf,
    }

    impl Drop for TestWorkbench {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    pub(crate) fn workbench<'a>(cx: &'a mut gpui::TestAppContext, name: &str) -> (TestWorkbench, Entity<Workbench>, &'a mut gpui::VisualTestContext) {
        let dir = std::env::temp_dir().join(format!("bf-wb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The long form of the path: Windows' TEMP can be an 8.3 short name (C:\Users\ABCDEF~1\…), while git
        // and language servers report the long one, so the paths a test compares wouldn't match.
        let dir = std::fs::canonicalize(&dir).map(|p| PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\"))).unwrap_or(dir);
        let paths = AppPaths { config_dir: dir.join("config"), install_dir: dir.join("install"), cache_override: Some(dir.join("cache")) };
        paths.ensure_created();
        let settings = Settings::load(paths.settings_file(), || {});
        let themes = ThemeService::new(paths.builtin_themes(), paths.user_themes(), None, || {});
        let (tx, rx) = flume::unbounded();
        cx.update(|cx| {
            let data = themes.apply("ByteForge Dark").unwrap_or_default();
            Theme::build(data, 1, cx).install(cx);
            crate::actions::install(cx, &HashMap::new());
        });
        let (view, vcx) = cx.add_window_view(|window, cx| Workbench::new(paths, settings, themes, rx, tx, Vec::new(), true, window, cx));
        (TestWorkbench { dir }, view, vcx)
    }

    #[gpui::test]
    fn the_settings_screen_draws_and_changes_settings(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "settings");
        let file = guard.dir.join("config").join("settings.json");
        std::fs::write(&file, "{\n  // keep me\n  \"editor.tabSize\": 4\n}\n").unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.settings = Settings::load(wb.paths.settings_file(), || {});
            // a module's declared setting is listed and typed like a built-in one
            wb.modules.push(serde_json::from_str(r#"{ "id": "t.mod", "name": "Test", "settings": [{ "key": "test.limit", "type": "number", "default": 5 }] }"#).unwrap());
            wb.open_settings_page(window, cx);
            assert!(matches!(wb.tabs.last().and_then(|t| t.page.clone()), Some(Page::Settings)));
        });
        cx.run_until_parked();
        wb.update_in(cx, |wb, _, cx| {
            // What the switch and the value prompt do.
            wb.settings.set("editor.wordWrap", json!("on"));
            wb.set_setting_text("editor.tabSize", "2", cx).unwrap();
            assert!(wb.set_setting_text("editor.tabSize", "two", cx).is_err());
            wb.set_setting_text("test.limit", "7", cx).unwrap();
            wb.settings.remove("editor.wordWrap");
            // The column on the left jumps to a section.
            wb.settings_scroll.scroll_to_top_of_item(3);
            wb.settings_nav_clicked = Some(2);
            // Searching narrows the list; the page draws with it.
            let search = wb.settings_search.clone().unwrap();
            search.update(cx, |s, cx| s.set_text("wrap".to_owned(), cx));
            cx.notify();
        });
        cx.run_until_parked();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("// keep me") && text.contains("\"editor.tabSize\": 2") && text.contains("\"test.limit\": 7") && !text.contains("wordWrap"), "{text}");
        drop(guard);
    }
}
