//! Markdown preview tabs: Ctrl+Shift+V shows the preview of the current Markdown file in its place,
//! "Open Preview to the Side" (and the eye button on the tab strip) next to it. The preview follows the
//! open editor as you type; a file that isn't open is read from disk and reread when it changes.

use std::rc::Rc;
use std::time::SystemTime;

use crate::markdown_preview;

use super::*;

pub(super) fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown" | "mkd"))
}

/// The last disk read of a previewed file that isn't open in an editor.
pub(super) type DiskCache = HashMap<PathBuf, (Option<SystemTime>, Option<String>)>;

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

    pub(super) fn render_markdown_page(&mut self, path: &Path, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let page = div()
            .id(SharedString::from(format!("md-preview-{}", path.display())))
            .size_full()
            .overflow_y_scroll()
            .bg(t.c("editor.background"))
            .text_color(t.c("editor.foreground"))
            .flex()
            .justify_center();
        let Some(text) = self.markdown_source(path, cx) else {
            return page.child(div().p_8().text_size(px(13.)).text_color(t.c("foreground.muted")).child(format!("{name} can't be read. It may have been moved or deleted.")));
        };

        let weak = cx.entity().downgrade();
        let on_link: markdown_preview::LinkHandler = Rc::new(move |target: &str, window: &mut Window, cx: &mut App| {
            if target.starts_with('#') {
                return; // Jumping to headings isn't supported yet.
            }
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
        });
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let blocks = markdown_preview::parse(&text);
        let body = markdown_preview::render(&blocks, &base, on_link, cx);
        page.child(div().w_full().max_w(px(860.)).px(px(40.)).pt(px(28.)).pb(px(64.)).text_size(px(14.)).child(body))
    }
}
