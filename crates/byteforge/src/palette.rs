//! VS Code-style quick input: commands (">"), files, themes (live preview), go to line (":") and prompts.

use std::path::PathBuf;

use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, Subscription, UniformListScrollHandle, Window,
    div, prelude::*, px, uniform_list,
};

use crate::text_input::{InputEvent, TextInput};
use crate::theme::{Theme, parse_background};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Commands,
    Files,
    Themes,
    GoToLine,
    Prompt,
    Branches,
}

pub enum PaletteAction {
    Command(Box<dyn Action>),
    OpenFile(PathBuf),
    Theme(String),
    GoToLine(usize),
    Prompt(String),
    Branch(bf_core::git::Branch),
}

impl Clone for PaletteAction {
    fn clone(&self) -> Self {
        match self {
            Self::Command(a) => Self::Command(a.boxed_clone()),
            Self::OpenFile(p) => Self::OpenFile(p.clone()),
            Self::Theme(t) => Self::Theme(t.clone()),
            Self::GoToLine(l) => Self::GoToLine(*l),
            Self::Prompt(s) => Self::Prompt(s.clone()),
            Self::Branch(b) => Self::Branch(b.clone()),
        }
    }
}

#[derive(Clone)]
pub struct PaletteItem {
    pub label: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub search: Option<String>,
    pub swatches: Vec<String>,
    pub action: PaletteAction,
}

impl PaletteItem {
    pub fn new(label: impl Into<String>, action: PaletteAction) -> Self {
        Self { label: label.into(), detail: None, hint: None, search: None, swatches: Vec::new(), action }
    }
}

pub enum PaletteEvent {
    Accept(PaletteAction),
    Preview(PaletteAction),
    /// Dismissed without accepting (theme previews should be reverted).
    Dismissed,
    /// The text changed to a different mode prefix (">" or ":"); the owner refills items.
    SwitchMode(Mode),
}

pub struct Palette {
    pub mode: Mode,
    input: Entity<TextInput>,
    items: Vec<PaletteItem>,
    filtered: Vec<usize>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _sub: Subscription,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Focusable for Palette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Palette {
    pub fn new(mode: Mode, items: Vec<PaletteItem>, placeholder: &str, initial: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut i = TextInput::new(placeholder.to_owned(), cx);
            i.set_text(initial, cx);
            i
        });
        let sub = cx.subscribe_in(&input, window, |this, _, event, _, cx| match event {
            InputEvent::Changed => this.on_text_changed(cx),
            InputEvent::Confirmed => this.accept(cx),
            InputEvent::Cancelled => cx.emit(PaletteEvent::Dismissed),
            InputEvent::Up => this.move_by(-1, cx),
            InputEvent::Down => this.move_by(1, cx),
            InputEvent::PageUp => this.move_by(-10, cx),
            InputEvent::PageDown => this.move_by(10, cx),
        });
        let mut palette = Self { mode, input, items, filtered: Vec::new(), selected: 0, scroll: UniformListScrollHandle::new(), _sub: sub };
        palette.filter(cx);
        palette
    }

    pub fn set_items(&mut self, mode: Mode, items: Vec<PaletteItem>, cx: &mut Context<Self>) {
        self.mode = mode;
        self.items = items;
        self.filter(cx);
    }

    pub fn select_hint(&mut self, hint: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self.filtered.iter().position(|&i| self.items[i].hint.as_deref() == Some(hint)) {
            self.selected = pos;
            self.scroll.scroll_to_item(pos, gpui::ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub fn focus_input(&self, window: &mut Window, cx: &App) {
        window.focus(&self.input.focus_handle(cx));
    }

    fn query(&self, cx: &App) -> String {
        let text = self.input.read(cx).text().to_owned();
        match self.mode {
            Mode::Commands => text.trim_start_matches('>').trim().to_owned(),
            Mode::GoToLine => text.trim_start_matches(':').trim().to_owned(),
            _ => text.trim().to_owned(),
        }
    }

    fn on_text_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().to_owned();
        let wanted = match self.mode {
            Mode::Files if text.starts_with('>') => Some(Mode::Commands),
            Mode::Files if text.starts_with(':') => Some(Mode::GoToLine),
            Mode::Commands | Mode::GoToLine if text.is_empty() => Some(Mode::Files),
            _ => None,
        };
        if let Some(mode) = wanted {
            cx.emit(PaletteEvent::SwitchMode(mode));
        }
        self.filter(cx);
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        if self.mode == Mode::GoToLine {
            self.items = match query.parse::<usize>() {
                Ok(n) if n > 0 => vec![PaletteItem::new(format!("Go to line {n}"), PaletteAction::GoToLine(n - 1))],
                _ => vec![PaletteItem::new("Type a line number", PaletteAction::GoToLine(usize::MAX))],
            };
        }
        if self.mode == Mode::Prompt {
            self.items = vec![PaletteItem::new(if query.is_empty() { "Type a name and press Enter".into() } else { format!("Confirm: {query}") }, PaletteAction::Prompt(query.clone()))];
        }
        let mut scored: Vec<(usize, i32)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                if query.is_empty() || matches!(self.mode, Mode::GoToLine | Mode::Prompt) {
                    return Some((i, 0));
                }
                let label = fuzzy(&item.label, &query).map(|s| s * 2);
                let search = item.search.as_deref().and_then(|s| fuzzy(s, &query));
                label.max(search).map(|s| (i, s))
            })
            .collect();
        if !query.is_empty() {
            scored.sort_by(|a, b| b.1.cmp(&a.1).then(self.items[a.0].label.len().cmp(&self.items[b.0].label.len())));
        }
        self.filtered = scored.into_iter().take(500).map(|(i, _)| i).collect();
        self.selected = 0;
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, self.filtered.len() as isize - 1) as usize;
        self.scroll.scroll_to_item(self.selected, gpui::ScrollStrategy::Center);
        if self.mode == Mode::Themes {
            let action = self.items[self.filtered[self.selected]].action.clone();
            cx.emit(PaletteEvent::Preview(action));
        }
        cx.notify();
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Prompt {
            cx.emit(PaletteEvent::Accept(PaletteAction::Prompt(self.query(cx))));
            return;
        }
        if let Some(&i) = self.filtered.get(self.selected) {
            let action = self.items[i].action.clone();
            if matches!(action, PaletteAction::GoToLine(usize::MAX)) {
                return;
            }
            cx.emit(PaletteEvent::Accept(action));
        }
    }
}

/// Subsequence match with bonuses for word starts and consecutive characters.
pub fn fuzzy(candidate: &str, query: &str) -> Option<i32> {
    let mut score = 0;
    let mut q = query.chars().map(|c| c.to_ascii_lowercase()).peekable();
    let mut last: Option<usize> = None;
    let chars: Vec<char> = candidate.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let Some(&qc) = q.peek() else { break };
        if c.to_ascii_lowercase() != qc {
            continue;
        }
        let mut bonus = 1;
        if last.is_some_and(|l| l + 1 == i) {
            bonus += 5;
        }
        if i == 0 || !chars[i - 1].is_alphanumeric() || (c.is_uppercase() && chars[i - 1].is_lowercase()) {
            bonus += 8;
        }
        score += bonus;
        last = Some(i);
        q.next();
    }
    q.peek().is_none().then_some(score)
}

impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let row_h = t.n("list.itemHeight", 22.0).max(t.n("ui.size", 13.0) * 1.9);
        let count = self.filtered.len();
        let max_h = t.n("palette.maxHeight", 440.0);
        let list_h = (count as f32 * row_h).min(max_h);
        let selected = self.selected;
        let entity = cx.entity();
        let view = entity.clone();
        let list = uniform_list("palette-list", count, move |range, _, cx| {
            let palette = view.read(cx);
            let t = Theme::global(cx);
            range
                .map(|ix| {
                    let item = &palette.items[palette.filtered[ix]];
                    let active = ix == selected;
                    let entity = view.clone();
                    div()
                        .id(ix)
                        .w_full()
                        .h(px(row_h))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(t.radius("list.radius", 0.0))
                        .cursor_pointer()
                        .when(active, |d| d.bg(t.bg("list.activeSelectionBackground")).text_color(t.c("list.activeSelectionForeground")))
                        .when(!active, |d| d.hover(|s| s.bg(t.c("list.hoverBackground"))))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            entity.update(cx, |p, cx| {
                                p.selected = ix;
                                p.accept(cx);
                            })
                        })
                        .when(!item.swatches.is_empty(), |d| {
                            d.child(div().flex().gap(px(3.)).w(px(117.)).children(item.swatches.iter().take(8).filter_map(|s| {
                                parse_background(s).map(|(bg, _)| div().size(px(12.)).rounded(px(3.)).bg(bg).border_1().border_color(gpui::hsla(0., 0., 0.5, 0.5)))
                            })))
                        })
                        .child(div().whitespace_nowrap().child(item.label.clone()))
                        .when_some(item.detail.clone(), |d, detail| {
                            d.child(div().overflow_hidden().whitespace_nowrap().text_color(t.c("foreground.muted")).text_size(px(12.)).child(detail))
                        })
                        .child(div().flex_1())
                        .when_some(item.hint.clone(), |d, hint| d.child(div().text_color(t.c("foreground.muted")).text_size(px(12.)).child(hint)))
                })
                .collect()
        })
        .track_scroll(self.scroll.clone())
        .h(px(list_h));

        div()
            .w(t.m("palette.width", 620.0))
            .bg(t.bg("quickInput.background"))
            .text_color(t.c("quickInput.foreground"))
            .border_color(t.c("quickInput.border"))
            .border(px(f32::from(t.edges("quickInput.borderThickness", 1.0).top)))
            .rounded(t.radius("palette.radius", 8.0))
            .shadow(t.shadow("palette.shadow"))
            .p(t.edges("palette.padding", 6.0).top)
            .flex()
            .flex_col()
            .gap_1()
            .font_family(t.ui_font.clone())
            .text_size(t.m("ui.size", 13.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.input.clone())
            .when(count > 0, |d| d.child(list))
    }
}
