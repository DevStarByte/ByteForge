//! Markdown preview tabs: Ctrl+Shift+V shows the preview of the current Markdown file in its place,
//! "Open Preview to the Side" (and the eye button on the tab strip) next to it. The preview follows the
//! open editor as you type; a file that isn't open is read from disk and reread when it changes.
//!
//! The preview is a page to read and use: it scrolls (wheel, arrow keys, Page Up/Down, Home/End, Space),
//! its text can be selected with the mouse and copied (Ctrl+C, Ctrl+A), links open (in the browser, in
//! ByteForge, or jump to a heading), and task list checkboxes tick and clear the source.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use gpui::{ClipboardItem, FocusHandle, Focusable, KeyDownEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle};

use crate::markdown_preview::{self, Pos, Rendered, TaskHandler};

use super::*;

pub(super) fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown" | "mkd"))
}

/// The last disk read of a previewed file that isn't open in an editor.
pub(super) type DiskCache = HashMap<PathBuf, (Option<SystemTime>, Option<String>)>;

/// Opens a link of the preview: a URL in the browser, a local file in ByteForge.
pub(super) type OpenLink = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// The preview of one Markdown file.
pub(super) struct MarkdownView {
    path: PathBuf,
    /// The previewed text (None: the file can't be read) and its parse.
    text: Option<String>,
    blocks: Vec<markdown_preview::Block>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    rendered: Rc<RefCell<Rendered>>,
    /// The selection: where the mouse went down and where it is now.
    anchor: Option<Pos>,
    head: Option<Pos>,
    selecting: bool,
    /// A link under the mouse when it went down (opened if it comes up on it without dragging).
    pressed_link: Option<String>,
    pressed_at: Option<Point<Pixels>>,
    hover_link: bool,
    open_link: OpenLink,
    on_task: TaskHandler,
}

impl Focusable for MarkdownView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MarkdownView {
    pub(super) fn new(path: PathBuf, open_link: OpenLink, on_task: TaskHandler, cx: &mut Context<Self>) -> Self {
        MarkdownView {
            path,
            text: None,
            blocks: Vec::new(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            rendered: Rc::default(),
            anchor: None,
            head: None,
            selecting: false,
            pressed_link: None,
            pressed_at: None,
            hover_link: false,
            open_link,
            on_task,
        }
    }

    /// The text to show; parsed again only when it changed.
    pub(super) fn set_text(&mut self, text: Option<String>) {
        if self.text != text {
            self.blocks = text.as_deref().map(markdown_preview::parse).unwrap_or_default();
            self.text = text;
            // Positions in the old text mean nothing in the new one.
            self.anchor = None;
            self.head = None;
        }
    }

    fn selection(&self) -> Option<(Pos, Pos)> {
        let (a, b) = (self.anchor?, self.head?);
        (a != b).then(|| if a <= b { (a, b) } else { (b, a) })
    }

    /// The selected text, blocks joined by line breaks.
    pub(super) fn selected_text(&self) -> String {
        let Some((a, b)) = self.selection() else { return String::new() };
        let rendered = self.rendered.borrow();
        let mut parts = Vec::new();
        for i in a.0..=b.0 {
            let Some(t) = rendered.texts.get(i) else { break };
            let start = if i == a.0 { a.1 } else { 0 }.min(t.text.len());
            let end = if i == b.0 { b.1 } else { t.text.len() }.min(t.text.len());
            if let Some(s) = t.text.get(start..end) {
                parts.push(s.replace('\u{a0}', " ").trim_matches(' ').to_owned());
            }
        }
        parts.join("\n")
    }

    /// The text position at `p` (window coordinates), or the nearest one.
    fn hit(&self, p: Point<Pixels>) -> Option<Pos> {
        let rendered = self.rendered.borrow();
        let texts = &rendered.texts;
        let bounds: Vec<gpui::Bounds<Pixels>> = texts.iter().map(|t| t.layout.bounds()).collect();
        // On a text block: the character there.
        if let Some(i) = bounds.iter().position(|b| b.contains(&p)) {
            let ix = texts[i].layout.index_for_position(p).unwrap_or_else(|closest| closest);
            return Some((i, ix.min(texts[i].text.len())));
        }
        // In the same rows as a block (beside it, e.g. between table cells): its nearest end.
        let beside = bounds.iter().enumerate().filter(|(_, b)| b.top() <= p.y && p.y <= b.bottom()).min_by(|(_, a), (_, b)| {
            let d = |r: &gpui::Bounds<Pixels>| if p.x < r.left() { r.left() - p.x } else { p.x - r.right() };
            d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some((i, b)) = beside {
            return Some((i, if p.x < b.left() { 0 } else { texts[i].text.len() }));
        }
        // Between blocks: the end of the last one above, or the start of the first.
        match bounds.iter().rposition(|b| b.bottom() < p.y) {
            Some(i) => Some((i, texts[i].text.len())),
            None => (!texts.is_empty()).then_some((0, 0)),
        }
    }

    /// The link at `p`, if the mouse is over one.
    fn link_at(&self, p: Point<Pixels>) -> Option<String> {
        let rendered = self.rendered.borrow();
        let t = rendered.texts.iter().find(|t| t.layout.bounds().contains(&p))?;
        let ix = t.layout.index_for_position(p).ok()?;
        t.links.iter().find(|(range, _)| range.contains(&ix)).map(|(_, target)| target.clone())
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.pressed_link = self.link_at(event.position);
        self.pressed_at = Some(event.position);
        let at = self.hit(event.position);
        if event.modifiers.shift && self.anchor.is_some() {
            self.head = at;
        } else {
            self.anchor = at;
            self.head = at;
        }
        // A double click selects the whole block.
        if event.click_count >= 2
            && let Some((i, _)) = at
        {
            let len = self.rendered.borrow().texts.get(i).map_or(0, |t| t.text.len());
            self.anchor = Some((i, 0));
            self.head = Some((i, len));
        }
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.pressed_button == Some(gpui::MouseButton::Left) {
            let moved = self.pressed_at.is_some_and(|p| (p.x - event.position.x).abs() + (p.y - event.position.y).abs() > px(3.));
            if moved {
                self.pressed_link = None;
                self.head = self.hit(event.position);
                cx.notify();
            }
            return;
        }
        let over = self.link_at(event.position).is_some();
        if over != self.hover_link {
            self.hover_link = over;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        if let Some(target) = self.pressed_link.take()
            && self.link_at(event.position).as_deref() == Some(target.as_str())
        {
            self.anchor = None;
            self.head = None;
            if let Some(anchor) = target.strip_prefix('#') {
                self.scroll_to_heading(anchor);
            } else {
                (self.open_link)(&target, window, cx);
            }
        }
        cx.notify();
    }

    /// Scrolls so the heading with `anchor` (its GitHub-style slug) is at the top.
    fn scroll_to_heading(&mut self, anchor: &str) {
        let wanted = markdown_preview::heading_slug(&anchor.replace('-', " ")).replace(' ', "-");
        let rendered = self.rendered.borrow();
        let Some(&(_, index)) = rendered.headings.iter().find(|(slug, _)| *slug == anchor.to_lowercase() || *slug == wanted) else { return };
        let Some(text) = rendered.texts.get(index) else { return };
        let top = text.layout.bounds().top();
        let viewport = self.scroll.bounds();
        let offset = self.scroll.offset();
        self.set_scroll(offset.y - (top - viewport.top()) + px(16.));
    }

    fn set_scroll(&self, y: Pixels) {
        let max = self.scroll.max_offset().height;
        let offset = self.scroll.offset();
        self.scroll.set_offset(gpui::point(offset.x, y.clamp(-max, px(0.))));
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let page = self.scroll.bounds().size.height * 0.9;
        let y = self.scroll.offset().y;
        let secondary = key.modifiers.secondary() && !key.modifiers.alt;
        match key.key.as_str() {
            "c" | "insert" if secondary => {
                let text = self.selected_text();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            "a" if secondary => {
                let rendered = self.rendered.borrow();
                if let Some(last) = rendered.texts.len().checked_sub(1) {
                    let len = rendered.texts[last].text.len();
                    drop(rendered);
                    self.anchor = Some((0, 0));
                    self.head = Some((last, len));
                }
            }
            "up" => self.set_scroll(y + px(40.)),
            "down" => self.set_scroll(y - px(40.)),
            "pageup" => self.set_scroll(y + page),
            "pagedown" => self.set_scroll(y - page),
            "space" if key.modifiers.shift => self.set_scroll(y + page),
            "space" => self.set_scroll(y - page),
            "home" => self.set_scroll(px(0.)),
            "end" => self.set_scroll(-self.scroll.max_offset().height),
            "escape" => {
                self.anchor = None;
                self.head = None;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for MarkdownView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let body: gpui::AnyElement = match &self.text {
            None => div().text_size(px(13.)).text_color(t.c("foreground.muted")).child(format!("{name} can't be read. It may have been moved or deleted.")).into_any_element(),
            Some(_) => {
                let base = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
                markdown_preview::render(&self.blocks, &base, self.selection(), self.on_task.clone(), self.rendered.clone(), cx)
            }
        };
        div()
            .id("md-preview")
            .debug_selector(|| "md-preview".into())
            .key_context("MarkdownPreview")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(t.c("editor.background"))
            .text_color(t.c("editor.foreground"))
            .cursor(if self.hover_link { gpui::CursorStyle::PointingHand } else { gpui::CursorStyle::IBeam })
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(div().w_full().flex().justify_center().child(div().w_full().max_w(px(860.)).px(px(40.)).pt(px(28.)).pb(px(64.)).text_size(px(14.)).child(body)))
    }
}

impl Workbench {
    /// Opens the preview of the active Markdown file, here or (`side`) in the other editor group. On a
    /// preview, goes back to its source.
    pub(super) fn open_markdown_preview(&mut self, side: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else {
            self.log("Markdown Preview: open a Markdown file first.");
            return;
        };
        if let Some(source) = self.tabs[ix].markdown_page().map(Path::to_path_buf) {
            self.open_file(source, None, window, cx);
            return;
        }
        let Some(path) = self.tabs[ix].path.clone().filter(|p| is_markdown(p)) else {
            self.log("Markdown Preview: the current file isn't a Markdown file (.md).");
            return;
        };
        let source_group = self.tabs[ix].group;
        let group = if side { 1 - source_group } else { source_group };

        if let Some(existing) = self.tabs.iter().position(|t| t.markdown_page().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
            if self.tabs[existing].group != group {
                self.tabs[existing].group = group;
                self.visible[group] = Some(existing);
                self.normalize_groups();
            }
            self.activate(existing, window, cx);
            if side {
                self.activate(ix, window, cx);
            }
            cx.notify();
            return;
        }

        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let editor = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e
        });
        self.focus_group = group;
        self.add_tab(editor, None, format!("Preview {name}"), false, window, cx);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(Page::Markdown(path));
        }
        self.normalize_groups();
        if side {
            // Keep typing in the source; the preview follows along next to it.
            self.activate(ix, window, cx);
        }
        cx.notify();
    }

    /// The text to preview: the open editor's (unsaved edits included), else the file on disk.
    fn markdown_source(&mut self, path: &Path, cx: &App) -> Option<String> {
        if let Some(tab) = self.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path))) {
            return Some(tab.editor.read(cx).save_path_text());
        }
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some((stamp, text)) = self.md_cache.get(path)
            && *stamp == modified
        {
            return text.clone();
        }
        let text = std::fs::read(path).ok().map(|b| String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&b)).replace("\r\n", "\n"));
        self.md_cache.insert(path.to_path_buf(), (modified, text.clone()));
        text
    }

    /// The preview of `path`: its view, made on first use, with the current text.
    pub(super) fn render_markdown_page(&mut self, path: &Path, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let text = self.markdown_source(path, cx);
        let view = match self.md_views.get(path) {
            Some(view) => view.clone(),
            None => {
                let weak = cx.entity().downgrade();
                let open_link: OpenLink = Rc::new({
                    let weak = weak.clone();
                    move |target: &str, window: &mut Window, cx: &mut App| {
                        if target.contains("://") || target.starts_with("mailto:") {
                            cx.open_url(target);
                            return;
                        }
                        let path = PathBuf::from(target);
                        let _ = weak.update(cx, |wb, cx| {
                            if path.exists() {
                                wb.open_file(path, None, window, cx);
                            } else {
                                wb.log(format!("Markdown Preview: {} doesn't exist.", path.display()));
                            }
                        });
                    }
                });
                let source = path.to_path_buf();
                let on_task: TaskHandler = Rc::new(move |index: usize, done: bool, _: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |wb, cx| wb.set_markdown_task(&source, index, done, cx));
                });
                let view = cx.new(|cx| MarkdownView::new(path.to_path_buf(), open_link, on_task, cx));
                self.md_views.insert(path.to_path_buf(), view.clone());
                view
            }
        };
        view.update(cx, |v, _| v.set_text(text));
        view
    }

    /// Ticks or clears the `index`th checkbox of `path`: in its open editor (an edit to undo or save
    /// like any other), else in the file on disk.
    pub(super) fn set_markdown_task(&mut self, path: &Path, index: usize, done: bool, cx: &mut Context<Self>) {
        let editor = self.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path))).map(|t| t.editor.clone());
        if let Some(editor) = editor {
            editor.update(cx, |e, cx| {
                let text = e.state.buffer.text();
                if let Some(new) = markdown_preview::set_task(&text, index, done) {
                    e.replace_text_minimal(&new, cx);
                }
            });
        } else {
            let Ok(bytes) = std::fs::read(path) else { return };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            match markdown_preview::set_task(&text, index, done) {
                Some(new) => {
                    if let Err(e) = std::fs::write(path, new) {
                        self.log(format!("Markdown Preview: could not write {}: {e}", path.display()));
                    }
                    self.md_cache.remove(path);
                }
                None => return,
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use crate::actions::PressKeys;
    use crate::workbench::settings_page::view_tests::workbench;
    use gpui::{Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, point};

    /// Where `needle` is drawn in the preview (window coordinates, a little inside its first character).
    fn position_of(view: &Entity<MarkdownView>, cx: &mut gpui::VisualTestContext, needle: &str) -> Point<Pixels> {
        view.read_with(cx, |v, _| {
            let rendered = v.rendered.borrow();
            let t = rendered.texts.iter().find(|t| t.text.contains(needle)).unwrap_or_else(|| panic!("{needle} isn't drawn"));
            let ix = t.text.find(needle).unwrap();
            let at = t.layout.position_for_index(ix).expect("laid out");
            point(at.x + px(2.), at.y + t.layout.line_height() / 2.)
        })
    }

    fn scroll_y(view: &Entity<MarkdownView>, cx: &mut gpui::VisualTestContext) -> Pixels {
        view.read_with(cx, |v, _| v.scroll.offset().y)
    }

    /// The preview scrolls (wheel and keys), its text is selected with the mouse and copied, a link
    /// opens its file, a `#heading` link scrolls to the heading, and a checkbox ticks the source.
    #[gpui::test]
    fn the_preview_scrolls_selects_copies_and_follows_links(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "md-preview");
        let other = guard.dir.join("Other.md");
        std::fs::write(&other, "# Other\n").unwrap();
        let mut md = String::from("# Title\n\nSee [the other file](Other.md) and [details](#details).\n\n- [ ] first task\n- [x] second task\n\n");
        for i in 1..=80 {
            md.push_str(&format!("Paragraph {i} with a few words in it.\n\n"));
        }
        md.push_str("## Details\n\nThe end.\n");
        let file = guard.dir.join("README.md");
        std::fs::write(&file, &md).unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_file(file.clone(), None, window, cx);
            wb.open_markdown_preview(false, window, cx);
        });
        cx.run_until_parked();
        let view = wb.update(cx, |wb, _| wb.md_views.get(&file).cloned()).expect("the preview's view");
        let page = cx.debug_bounds("md-preview").expect("the preview is drawn");

        // The mouse wheel scrolls.
        cx.simulate_event(ScrollWheelEvent { position: page.center(), delta: ScrollDelta::Pixels(point(px(0.), px(-400.))), ..Default::default() });
        cx.run_until_parked();
        assert!(scroll_y(&view, cx) < px(-100.), "the wheel scrolled ({:?})", scroll_y(&view, cx));

        // Keys scroll once the preview has focus (a click gives it).
        cx.simulate_click(page.center(), Modifiers::none());
        cx.press("home");
        cx.run_until_parked();
        assert_eq!(scroll_y(&view, cx), px(0.), "Home goes to the top");
        cx.press("pagedown");
        cx.run_until_parked();
        assert!(scroll_y(&view, cx) < px(-100.), "Page Down scrolls");
        cx.press("home");
        cx.run_until_parked();

        // Dragging over the title selects it; Ctrl+C copies it.
        let from = position_of(&view, cx, "Title");
        let to = point(from.x + px(400.), from.y);
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        cx.press("ctrl-c");
        cx.run_until_parked();
        let copied = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        assert!(copied.starts_with("itle") || copied.starts_with("Title"), "{copied:?}");

        // Ctrl+A selects everything.
        cx.press("ctrl-a");
        cx.press("ctrl-c");
        cx.run_until_parked();
        let all = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        assert!(all.contains("Title") && all.contains("The end."), "{} chars", all.len());

        // A checkbox ticks the task in the source.
        let checkbox = cx.debug_bounds("md-task-0").expect("the first checkbox").center();
        cx.simulate_click(checkbox, Modifiers::none());
        cx.run_until_parked();
        let source = wb.update(cx, |wb, cx| wb.tabs.iter().find(|t| t.is_document() && t.path.as_deref() == Some(file.as_path())).map(|t| t.editor.read(cx).state.buffer.text()));
        assert!(source.as_deref().is_some_and(|s| s.contains("- [x] first task")), "{source:?}");

        // A #heading link scrolls down to the heading.
        let details = position_of(&view, cx, "details");
        cx.simulate_click(details, Modifiers::none());
        cx.run_until_parked();
        assert!(scroll_y(&view, cx) < px(-500.), "the link scrolled to Details ({:?})", scroll_y(&view, cx));

        // A link to a file opens it.
        cx.press("home");
        cx.run_until_parked();
        let link = position_of(&view, cx, "the other file");
        cx.simulate_click(link, Modifiers::none());
        cx.run_until_parked();
        let active = wb.update(cx, |wb, _| wb.active.and_then(|i| wb.tabs.get(i)).and_then(|t| t.path.clone()));
        assert_eq!(active.as_deref(), Some(other.as_path()));
        drop(guard);
    }
}
