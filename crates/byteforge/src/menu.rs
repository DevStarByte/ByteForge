//! The one look for every menu in the workbench: the menu bar's dropdowns, the Explorer's context menu and the
//! context menus of module views all build on these pieces, so they share the `menu.*` theme keys.

use gpui::{Div, ElementId, Hsla, SharedString, Stateful, div, prelude::*, px};

use crate::theme::Theme;

/// The menu's box: background, border, radius, shadow and padding. Add items with `.child(item(…))`.
pub fn panel(t: &Theme, min_width: f32) -> Div {
    div()
        .min_w(px(min_width))
        .bg(t.bg("menu.background"))
        .text_color(t.c("menu.foreground"))
        .border_1()
        .border_color(t.c("menu.border"))
        .rounded(t.radius("widget.radius", 6.0))
        .shadow(t.shadow("widget.shadow"))
        .p_1()
        .text_size(t.m("ui.size", 13.0))
}

/// One entry; `hint` (a key binding, for example) is drawn muted on the right. Attach `.on_click`.
pub fn item(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, hint: Option<SharedString>) -> Stateful<Div> {
    let hover: Hsla = t.c("menu.selectionBackground");
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_6()
        .px_3()
        .py_1()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(div().flex_1().whitespace_nowrap().child(label.into()))
        .when_some(hint.filter(|h| !h.is_empty()), |d, h| d.child(div().text_color(t.c("foreground.muted")).text_size(px(12.)).child(h)))
}

/// A line between groups of entries.
pub fn separator(t: &Theme) -> Div {
    div().h(px(1.)).my_1().bg(t.c("menu.border"))
}
