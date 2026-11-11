//! What modules add to the workbench beyond commands: side bar views they fill with trees (`views` in
//! `module.json`, content via `setTree`), terminals they run commands in, pick lists, questions, file
//! icons and languages.

use std::collections::HashSet;

use bf_core::plugins::{PickItem, PromptRequest, TreeAction, TreeContent, TreeItem};

use super::*;

/// A side bar view declared by a module.
pub(super) struct ModuleView {
    pub module: String,
    pub id: String,
    pub title: String,
    pub icon: String,
    pub items: Vec<TreeItem>,
    pub message: Option<String>,
    pub error: Option<String>,
    pub badge: u64,
    pub actions: Vec<TreeAction>,
    /// Items the user collapsed (or expanded, when they start collapsed), by id.
    pub toggled: HashSet<String>,
}

/// The context menu of a row: the row's buttons, then its extra `menu` entries.
pub(super) struct ModuleMenu {
    module: String,
    view: String,
    item: String,
    entries: Vec<TreeAction>,
    position: gpui::Point<gpui::Pixels>,
}

/// A tree row to draw: the item, its depth and whether it's expanded.
struct Row<'a> {
    item: &'a TreeItem,
    depth: usize,
    expanded: bool,
}

impl ModuleView {
    fn expanded(&self, item: &TreeItem) -> bool {
        item.expanded.unwrap_or(true) != self.toggled.contains(&item.id)
    }

    fn rows(&self) -> Vec<Row<'_>> {
        fn walk<'a>(view: &'a ModuleView, items: &'a [TreeItem], depth: usize, out: &mut Vec<Row<'a>>) {
            for item in items {
                let expanded = view.expanded(item);
                out.push(Row { item, depth, expanded });
                if expanded {
                    walk(view, &item.children, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self, &self.items, 0, &mut out);
        out
    }
}

impl Workbench {
    /// Updates the icons, languages and views to the installed modules that are turned on.
    pub(super) fn refresh_module_contributions(&mut self, cx: &mut Context<Self>) {
        let disabled: Vec<String> = self.settings.get("modules.disabled", Vec::new());
        let on: Vec<ModuleManifest> = self.modules.iter().filter(|m| !disabled.contains(&m.id)).cloned().collect();
        crate::explorer::set_module_file_icons(&on);

        let languages = on.iter().flat_map(|m| m.languages.iter()).map(module_language).collect();
        bf_syntax::module_lang::set_module_languages(languages);

        let mut views = Vec::new();
        for m in &on {
            for v in &m.views {
                let existing = self.module_views.iter().position(|e| e.module == m.id && e.id == v.id).map(|i| self.module_views.remove(i));
                views.push(match existing {
                    Some(mut e) => {
                        e.title = v.title.clone();
                        e.icon = v.icon.clone();
                        e
                    }
                    None => ModuleView {
                        module: m.id.clone(),
                        id: v.id.clone(),
                        title: v.title.clone(),
                        icon: v.icon.clone(),
                        items: Vec::new(),
                        message: Some("Starting…".into()),
                        error: None,
                        badge: 0,
                        actions: Vec::new(),
                        toggled: HashSet::new(),
                    },
                });
            }
        }
        self.module_views = views;
        if self.side == SideView::Module && !self.module_views.iter().any(|v| Some(&v.id) == self.module_view.as_ref()) {
            self.side = SideView::Explorer;
            self.module_view = None;
        }
        // Open editors pick up languages that appeared or went away.
        for tab in &self.tabs {
            tab.editor.update(cx, |e, cx| e.redetect_language(cx));
        }
        cx.refresh_windows();
    }

    pub(super) fn module_view_active(&self, id: &str) -> bool {
        self.side_visible && self.side == SideView::Module && self.module_view.as_deref() == Some(id)
    }

    pub(super) fn show_module_view(&mut self, id: &str, cx: &mut Context<Self>) {
        self.module_view = Some(id.to_owned());
        self.show_side(SideView::Module, cx);
    }

    pub(super) fn toggle_module_view(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.module_view_active(id) {
            self.side_visible = false;
            cx.notify();
        } else {
            self.show_module_view(id, cx);
        }
    }

    /// Tells modules when their view becomes visible or hidden (so they only poll while shown).
    pub(super) fn sync_module_view_visibility(&mut self) {
        let shown = self.module_views.iter().find(|v| self.module_view_active(&v.id)).map(|v| (v.module.clone(), v.id.clone()));
        if shown == self.module_view_shown {
            return;
        }
        if let Some((module, view)) = self.module_view_shown.take()
            && let Some(p) = self.plugins.iter().find(|p| p.manifest.id == module)
        {
            p.view_event(&view, false);
        }
        if let Some((module, view)) = &shown
            && let Some(p) = self.plugins.iter().find(|p| &p.manifest.id == module)
        {
            p.view_event(view, true);
        }
        self.module_view_shown = shown;
    }

    pub(super) fn set_module_tree(&mut self, module: String, tree: TreeContent) {
        if let Some(v) = self.module_views.iter_mut().find(|v| v.module == module && v.id == tree.view) {
            v.items = tree.items;
            v.message = tree.message;
            v.error = tree.error;
            v.badge = tree.badge;
            v.actions = tree.actions;
        }
    }

    /// A module stopped: its views say so instead of showing stale content.
    pub(super) fn module_views_stopped(&mut self, module: &str) {
        for v in self.module_views.iter_mut().filter(|v| v.module == module) {
            v.items.clear();
            v.badge = 0;
            v.actions.clear();
            v.message = None;
            v.error = Some("The module stopped. Details are in Output.".into());
        }
    }

    fn send_tree_action(&self, module: &str, view: &str, item: Option<&str>, action: &str) {
        if let Some(p) = self.plugins.iter().find(|p| p.manifest.id == module) {
            p.tree_action(view, item, action);
        }
    }

    /// Opens a new terminal named `title` and runs `command` in the user's shell (in `cwd` if given).
    pub(super) fn run_in_terminal(&mut self, title: String, command: String, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.terminals.len();
        self.new_terminal(window, cx);
        if self.terminals.len() == before {
            return;
        }
        let command = match cwd {
            Some(dir) => format!("cd {} && {command}", shell_quote(&dir.to_string_lossy())),
            None => command,
        };
        if let Some((terminal, _)) = self.terminals.last() {
            terminal.update(cx, |t, cx| {
                t.label = Some(title);
                t.run_command(&command, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn open_pick_palette(&mut self, items: Vec<PaletteItem>, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, placeholder, "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    pub(super) fn module_quick_pick(&mut self, module: String, request: u64, placeholder: String, items: Vec<PickItem>, window: &mut Window, cx: &mut Context<Self>) {
        let items = items
            .into_iter()
            .map(|i| {
                let action = ModulePickChoice { module: module.clone(), request, value: i.value };
                let mut item = PaletteItem::new(i.label, PaletteAction::Command(Box::new(action)));
                item.detail = i.detail;
                item.hint = i.hint;
                item
            })
            .collect();
        self.open_pick_palette(items, &placeholder, window, cx);
    }

    pub(super) fn module_pick_choice(&mut self, choice: &ModulePickChoice) {
        if let Some(p) = self.plugins.iter().find(|p| p.manifest.id == choice.module) {
            p.quick_pick_result(choice.request, &choice.value);
        }
    }

    pub(super) fn module_prompt(&mut self, module: String, prompt: PromptRequest, window: &mut Window, cx: &mut Context<Self>) {
        let request = prompt.request;
        let buttons = if prompt.buttons.is_empty() { vec!["OK".to_owned()] } else { prompt.buttons };
        let labels: Vec<&str> = buttons.iter().map(String::as_str).collect();
        let rx = window.prompt(PromptLevel::Warning, &prompt.message, prompt.detail.as_deref(), &labels, cx);
        cx.spawn(async move |this, cx| {
            let index = rx.await.ok();
            let _ = this.update(cx, |wb, _| {
                if let Some(p) = wb.plugins.iter().find(|p| p.manifest.id == module) {
                    p.prompt_result(request, index);
                }
            });
        })
        .detach();
    }

    /// The content of a module's side bar view: its status line and buttons, then the tree.
    pub(super) fn render_module_view(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let Some(view) = self.module_views.iter().find(|v| Some(&v.id) == self.module_view.as_ref()) else {
            return div().p_4().child("This view's module isn't turned on.").into_any_element();
        };
        let muted = t.c("foreground.muted");
        let row_h = t.n("list.itemHeight", 22.0);
        let (module, view_id) = (view.module.clone(), view.id.clone());

        let small = |id: SharedString, name: &str, tip: String| {
            div()
                .id(id)
                .p(px(2.))
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip))
                .child(icon(name, px(14.), t.c("icon.foreground")))
        };

        let mut top = div().px_3().flex().items_center().gap_1().min_h(px(22.));
        top = top.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(muted).child(view.message.clone().unwrap_or_default()));
        for a in &view.actions {
            let (m, v, action) = (module.clone(), view_id.clone(), a.id.clone());
            top = top.child(small(SharedString::from(format!("mv-top-{}", a.id)), &a.icon, a.tooltip.clone()).on_click(cx.listener(move |wb, _, _, _| wb.send_tree_action(&m, &v, None, &action))));
        }

        let mut list = div().id(SharedString::from(format!("mv-{view_id}"))).flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        for (ix, row) in view.rows().into_iter().enumerate() {
            let item = row.item;
            let has_children = !item.children.is_empty();
            let (m, v, item_id) = (module.clone(), view_id.clone(), item.id.clone());
            let mut el = div()
                .id(SharedString::from(format!("mv-row-{ix}-{}", item.id)))
                .group("mv-row")
                .w_full()
                .flex_none()
                .h(px(row_h))
                .pl(px(8. + row.depth as f32 * 14.))
                .pr_3()
                .flex()
                .items_center()
                .gap(t.m("explorer.itemSpacing", 6.0))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("list.hoverBackground")))
                .when_some(item.tooltip.clone(), |d, tip| d.tooltip(tooltip(tip)));
            if item.header {
                el = el
                    .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                    .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                    .text_color(t.c("sideBarSectionHeader.foreground"));
            } else {
                el = el.text_size(t.m("list.fontSize", 13.0));
            }
            if has_children {
                el = el.child(icon(if row.expanded { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground")));
            }
            if let Some(name) = item.icon.as_deref().filter(|n| !n.is_empty()) {
                let (icon_name, color) = match name.strip_prefix("file:") {
                    Some(file) => {
                        let (n, c) = crate::explorer::file_icon(file, false, false);
                        (n.into_owned(), crate::explorer::file_icon_color(&t, &c))
                    }
                    None => (name.to_owned(), item.icon_color.as_deref().map_or_else(|| t.c("icon.foreground"), |c| color_value(&t, c))),
                };
                let color = match (name.starts_with("file:"), item.icon_color.as_deref()) {
                    (true, Some(c)) => color_value(&t, c),
                    _ => color,
                };
                el = el.child(icon(&icon_name, if item.header { px(14.) } else { t.m("explorer.iconSize", 15.0) }, color));
            }
            el = el.child(div().whitespace_nowrap().when(item.header, |d| d.flex_1()).child(item.label.clone()));
            if !item.header {
                el = el.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(item.description.clone().unwrap_or_default()));
            }
            if let Some(count) = item.count {
                el = el.child(div().px(px(6.)).rounded(px(9.)).text_size(px(11.)).bg(t.bg("badge.background")).text_color(t.c("badge.foreground")).child(count.to_string()));
            }
            if !item.actions.is_empty() {
                let mut buttons = div().flex().gap_1().invisible().group_hover("mv-row", |s| s.visible());
                for a in &item.actions {
                    let (m, v, i, action) = (module.clone(), view_id.clone(), item.id.clone(), a.id.clone());
                    buttons = buttons.child(small(SharedString::from(format!("mv-act-{}-{}", item.id, a.id)), &a.icon, a.tooltip.clone()).on_click(cx.listener(move |wb, _, _, cx| {
                        cx.stop_propagation();
                        wb.send_tree_action(&m, &v, Some(&i), &action);
                    })));
                }
                el = el.child(buttons);
            }
            let mut entries = item.actions.clone();
            if !item.menu.is_empty() {
                if !entries.is_empty() && !item.menu[0].is_separator() {
                    entries.push(TreeAction { id: "-".into(), ..Default::default() });
                }
                entries.extend(item.menu.iter().cloned());
            }
            if entries.iter().any(|e| !e.is_separator()) {
                let (m, v, i) = (module.clone(), view_id.clone(), item.id.clone());
                el = el.on_mouse_down(MouseButton::Right, cx.listener(move |wb, ev: &gpui::MouseDownEvent, _, cx| {
                    wb.module_menu = Some(ModuleMenu { module: m.clone(), view: v.clone(), item: i.clone(), entries: entries.clone(), position: ev.position });
                    cx.stop_propagation();
                    cx.notify();
                }));
            }
            el = el.on_click(cx.listener(move |wb, _, _, cx| {
                if has_children {
                    if let Some(view) = wb.module_views.iter_mut().find(|x| x.module == m && x.id == v)
                        && !view.toggled.remove(&item_id)
                    {
                        view.toggled.insert(item_id.clone());
                    }
                    cx.notify();
                } else {
                    wb.send_tree_action(&m, &v, Some(&item_id), "click");
                }
            }));
            list = list.child(el);
        }

        let menu = self.module_menu.as_ref().filter(|m| m.module == module && m.view == view_id).map(|m| {
            let mut panel = crate::menu::panel(&t, 180.).on_mouse_down_out(cx.listener(|wb, _, _, cx| {
                wb.module_menu = None;
                cx.notify();
            }));
            let mut last_separator = true;
            for (ix, entry) in m.entries.iter().enumerate() {
                if entry.is_separator() {
                    if !last_separator {
                        panel = panel.child(crate::menu::separator(&t));
                    }
                    last_separator = true;
                    continue;
                }
                last_separator = false;
                let (md, v, i, action) = (m.module.clone(), m.view.clone(), m.item.clone(), entry.id.clone());
                panel = panel.child(crate::menu::item(&t, SharedString::from(format!("mv-menu-{ix}")), entry.menu_label().to_owned(), None).on_click(cx.listener(
                    move |wb, _, _, cx| {
                        wb.module_menu = None;
                        wb.send_tree_action(&md, &v, Some(&i), &action);
                        cx.notify();
                    },
                )));
            }
            gpui::deferred(gpui::anchored().position(m.position).snap_to_window().child(panel)).with_priority(1)
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .children(menu)
            .child(top)
            .when_some(view.error.clone(), |d, e| d.child(div().px_3().text_size(px(11.)).text_color(t.c("errorForeground")).child(e)))
            .child(list)
            .into_any_element()
    }
}

/// A module's language, compiled for the highlighter (invalid rules are skipped and logged).
pub(crate) fn module_language(l: &bf_core::plugins::LanguageContribution) -> bf_syntax::module_lang::ModuleLanguage {
    use bf_syntax::module_lang::{ModuleLanguage, RegexRule, intern};
    let files = l.clone();
    ModuleLanguage {
        id: intern(&l.id),
        name: intern(if l.name.is_empty() { &l.id } else { &l.name }),
        line_comment: l.line_comment.as_deref().map(intern),
        matches: Box::new(move |name: &str| files.matches(name)),
        rules: l.rules.iter().filter_map(|r| RegexRule::new(&r.pattern, r.group, &r.token)).collect(),
    }
}

/// A theme color key (`icon.docker`) or a literal color (`#3fb950`).
fn color_value(t: &Theme, value: &str) -> gpui::Hsla {
    if value.starts_with('#') {
        crate::theme::parse_color(value).unwrap_or_else(|| t.c("icon.foreground"))
    } else if t.has(value) {
        t.c(value)
    } else {
        t.c("icon.foreground")
    }
}

/// Quotes an argument for POSIX shells and fish (`'…'`, with `'` written as `'\''`).
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bf_syntax::{Highlighter, Language};

    /// Module languages are global: tests that install some take turns.
    static LANGUAGES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The bundled SQL module's rules all compile (Rust regex syntax) and highlight a query.
    #[test]
    fn sql_module_highlights_queries() {
        let _turn = LANGUAGES.lock().unwrap_or_else(|e| e.into_inner());
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
        let module = bf_core::plugins::discover(&dir).into_iter().find(|m| m.id == "byteforge.sql").expect("modules/sql");
        let lang = module.languages.iter().find(|l| l.id == "sql").expect("sql language");
        let compiled = super::module_language(lang);
        assert_eq!(compiled.rules.len(), lang.rules.len(), "every rule compiles");
        bf_syntax::module_lang::set_module_languages(vec![compiled]);
        assert!(module.file_icons.iter().any(|r| r.matches("schema.sql")) && module.file_icons.iter().any(|r| r.matches("app.db")));
        let language = Language::from_path(Path::new("queries/report.sql")).expect("sql files");
        assert_eq!(language.line_comment(), Some("--"));

        let text = "-- top customers\nSELECT c.name, count(*) AS n FROM \"Customers\" c\nWHERE c.email LIKE '%@example.com' AND c.id > 10 AND c.kind = @kind\nCREATE TABLE t (id INT, name NVARCHAR(50)); /* note */\n";
        let spans = Highlighter::new(language).highlight(text, 0..text.len());
        let kind = |needle: &str, nth: usize| {
            let start = text.match_indices(needle).nth(nth).map(|(i, _)| i).unwrap();
            spans.iter().find(|s| s.start <= start && start < s.end).map(|s| s.kind)
        };
        assert_eq!(kind("-- top", 0), Some("comment"));
        assert_eq!(kind("SELECT", 0), Some("keyword"));
        assert_eq!(kind("count", 0), Some("method name"));
        assert_eq!(kind("\"Customers\"", 0), Some("field name"));
        assert_eq!(kind("'%@example.com'", 0), Some("string"));
        assert_eq!(kind("10", 0), Some("number"));
        assert_eq!(kind("@kind", 0), Some("local name"));
        assert_eq!(kind("INT", 0), Some("class name"));
        assert_eq!(kind("NVARCHAR", 0), Some("class name"));
        assert_eq!(kind("50", 0), Some("number"));
        assert_eq!(kind("/* note */", 0), Some("comment"));
        assert_eq!(kind(">", 0), Some("operator"));
        bf_syntax::module_lang::set_module_languages(Vec::new());
    }
}
