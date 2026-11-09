//! The in-window question dialog behind `window.prompt(…)`: GPUI's fallback renderer is a plain white box,
//! so ByteForge installs its own builder that draws the prompt in the current theme, like the palette.
//!
//! The first button is the default (Enter); Escape and clicking outside pick "Cancel" when there is one.
//! Tab / ←→ move between the buttons.

use gpui::{
    App, Context, FocusHandle, Focusable, Hsla, KeyDownEvent, MouseButton, PromptButton, PromptHandle, PromptLevel, PromptResponse,
    RenderablePromptHandle, SharedString, Window, div, prelude::*, px,
};

use crate::assets;
use crate::theme::Theme;

pub fn install(cx: &mut App) {
    cx.set_prompt_builder(build);
}

fn build(
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    buttons: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let view = cx.new(|cx| PromptView {
        level,
        message: message.to_string().into(),
        detail: detail.filter(|d| !d.trim().is_empty()).map(|d| d.to_string().into()),
        buttons: buttons.iter().map(|b| b.label().clone()).collect(),
        cancel: buttons.iter().position(|b| matches!(b, PromptButton::Cancel(_)) || b.label().eq_ignore_ascii_case("cancel")),
        selected: 0,
        focus: cx.focus_handle(),
    });
    handle.with_view(view, window, cx)
}

struct PromptView {
    level: PromptLevel,
    message: SharedString,
    detail: Option<SharedString>,
    buttons: Vec<SharedString>,
    cancel: Option<usize>,
    selected: usize,
    focus: FocusHandle,
}

impl PromptView {
    fn answer(&mut self, ix: usize, cx: &mut Context<Self>) {
        cx.emit(PromptResponse(ix));
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.buttons.len();
        if count == 0 {
            return;
        }
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "enter" | "space" => self.answer(self.selected, cx),
            "escape" => {
                if let Some(ix) = self.cancel {
                    self.answer(ix, cx)
                }
            }
            "tab" if keystroke.modifiers.shift => self.selected = (self.selected + count - 1) % count,
            "tab" | "right" => self.selected = (self.selected + 1) % count,
            "left" => self.selected = (self.selected + count - 1) % count,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl gpui::EventEmitter<PromptResponse> for PromptView {}

impl Focusable for PromptView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PromptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let (icon, icon_color) = match self.level {
            PromptLevel::Info => ("info", t.c_or("editorInfo.foreground", 0x3794ffff)),
            PromptLevel::Warning => ("warning", t.c_or("editorWarning.foreground", 0xcca700ff)),
            PromptLevel::Critical => ("error", t.c_or("errorForeground", 0xf85149ff)),
        };
        let focus_ring = t.c_or("focusBorder", 0x0078d4ff);
        let button_radius = t.radius("button.radius", 4.0);
        let padding = t.edges("button.padding", 12.0);
        let border_width = px(f32::from(t.edges("button.borderThickness", 1.0).top).max(1.0));

        let buttons = self.buttons.iter().enumerate().map(|(ix, label)| {
            let primary = ix == 0;
            let selected = ix == self.selected;
            let (bg, fg, hover) = if primary {
                (t.bg("button.background"), t.c("button.foreground"), t.c("button.hoverBackground"))
            } else {
                let fg = if t.has("button.secondaryForeground") { t.c("button.secondaryForeground") } else { t.c("quickInput.foreground") };
                let hover = if t.has("button.secondaryHoverBackground") { t.c("button.secondaryHoverBackground") } else { t.c("list.hoverBackground") };
                (t.bg("button.secondaryBackground"), fg, hover)
            };
            let border: Hsla = if selected { focus_ring } else if t.has("button.border") { t.c("button.border") } else { gpui::transparent_black() };
            div()
                .id(ix)
                .min_w(px(76.))
                .px(padding.left)
                .py(padding.top)
                .flex()
                .justify_center()
                .rounded(button_radius)
                .border(border_width)
                .border_color(border)
                .bg(bg)
                .text_color(fg)
                .when(primary, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| this.answer(ix, cx)))
                .child(label.clone())
        });

        let card = div()
            .id("prompt-card")
            .w(px(440.))
            .max_w_full()
            .bg(t.bg("quickInput.background"))
            .text_color(t.c("quickInput.foreground"))
            .border(px(f32::from(t.edges("quickInput.borderThickness", 1.0).top)))
            .border_color(t.c("quickInput.border"))
            .rounded(t.radius("palette.radius", 8.0))
            .shadow(t.shadow("palette.shadow"))
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().pt(px(1.)).child(assets::icon(icon, px(20.), icon_color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(self.message.clone()))
                            .when_some(self.detail.clone(), |d, detail| d.child(div().text_size(px(12.)).text_color(muted).child(detail))),
                    ),
            )
            .child(div().flex().justify_end().gap_2().children(buttons));

        let cancel = self.cancel;
        div()
            .id("prompt-backdrop")
            .size_full()
            .occlude()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(ix) = cancel {
                        this.answer(ix, cx)
                    }
                }),
            )
            .bg(gpui::hsla(0., 0., 0., 0.35))
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .font_family(t.ui_font.clone())
            .text_size(t.m("ui.size", 13.0))
            .child(card)
    }
}
