//! Items of strips, menus and grids on the designer's form: where each item and column header is drawn,
//! the menus a selected menu item opens, and the "Type Here" slots that add items. Clicks on them select
//! the item (its properties go to the grid) or add one.

use bf_core::winforms::{self as wf, Designer, Layout, Rect};

/// What a click on an item of a strip, menu or grid hits.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ItemHit {
    /// An item (a menu item, a strip's button, a grid's column).
    Item(String),
    /// The slot that adds an item to this strip, menu item or grid.
    Add(String),
}

/// Height of a row of an open menu.
pub(super) const MENU_ROW: i32 = 22;
/// Height of a separator in an open menu.
pub(super) const MENU_SEPARATOR: i32 = 8;
/// Width of the "Type Here" slot on a strip.
pub(super) const ADD_WIDTH: i32 = 76;
/// The row headers' column of a grid.
pub(super) const ROW_HEADERS: i32 = 41;
/// Height of a grid's column headers.
pub(super) const HEADER_HEIGHT: i32 = 23;

/// A menu's text as shown: `&File` → `File` (`&&` is an ampersand).
pub(super) fn display_text(text: &str) -> String {
    text.replace("&&", "\u{0}").replace('&', "").replace('\u{0}', "&")
}

/// Width of text as drawn (estimated like the tab headers').
fn text_width(text: &str) -> i32 {
    text.chars().count() as i32 * 7
}

pub(super) fn is_separator(doc: &Designer, name: &str) -> bool {
    doc.get(name).is_some_and(|c| c.short_type() == "ToolStripSeparator")
}

/// What an item shows: its text (a column's header text), else its name.
pub(super) fn label(doc: &Designer, name: &str) -> String {
    let prop = if doc.get(name).is_some_and(|c| c.short_type().starts_with("DataGridView")) { "HeaderText" } else { "Text" };
    let text = doc.prop(name, prop).and_then(wf::parse_string).map(|t| display_text(&t)).unwrap_or_default();
    if text.is_empty() { name.to_owned() } else { text }
}

/// A menu item's ShortcutKeys as a menu shows them: `Keys.Control | Keys.S` → `Ctrl+S`.
pub(super) fn shortcut(doc: &Designer, name: &str) -> Option<String> {
    let keys = wf::parse_flags(doc.prop(name, "ShortcutKeys")?, "Keys");
    let mut mods = Vec::new();
    let mut key = String::new();
    for k in keys {
        match k.as_str() {
            "Control" => mods.push("Ctrl"),
            "Shift" => mods.push("Shift"),
            "Alt" => mods.push("Alt"),
            "None" => {}
            // Digit keys are D0 … D9.
            other => key = other.strip_prefix('D').filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit())).unwrap_or(other).to_owned(),
        }
    }
    (!key.is_empty()).then(|| mods.into_iter().map(str::to_owned).chain([key]).collect::<Vec<_>>().join("+"))
}

/// The items of a strip, left to right.
pub(super) fn strip_items(doc: &Designer, strip: &str, rect: Rect) -> Vec<(String, Rect)> {
    let (y, h) = (rect.y + 2, (rect.h - 4).max(1));
    let mut x = rect.x + 4;
    doc.items(strip)
        .into_iter()
        .map(|n| {
            let w = if is_separator(doc, &n) { 6 } else { text_width(&label(doc, &n)) + 16 };
            let r = Rect::new(x, y, w, h);
            x += w;
            (n, r)
        })
        .collect()
}

/// Where a strip's "Type Here" slot goes: after its last item.
pub(super) fn strip_add(doc: &Designer, strip: &str, rect: Rect) -> Rect {
    let x = strip_items(doc, strip, rect).last().map_or(rect.x + 4, |(_, r)| r.x + r.w + 2);
    Rect::new(x, rect.y + 2, ADD_WIDTH, (rect.h - 4).max(1))
}

/// A grid's column headers, left to right (each as wide as its Width).
pub(super) fn column_headers(doc: &Designer, grid: &str, rect: Rect) -> Vec<(String, Rect)> {
    let mut x = rect.x + 1 + ROW_HEADERS;
    doc.items(grid)
        .into_iter()
        .filter(|c| doc.prop(c, "Visible").and_then(wf::parse_bool) != Some(false))
        .map(|c| {
            let w = doc.prop(&c, "Width").and_then(wf::parse_int).map_or(100, |w| w.max(5) as i32);
            let r = Rect::new(x, rect.y + 1, w, HEADER_HEIGHT);
            x += w;
            (c, r)
        })
        .collect()
}

/// Whether `name` is `owner` or one of its items (or theirs).
pub(super) fn belongs_to(doc: &Designer, name: &str, owner: &str) -> bool {
    let mut at = name.to_owned();
    for _ in 0..32 {
        if at == owner {
            return true;
        }
        match doc.item_owner(&at) {
            Some(o) => at = o,
            None => return false,
        }
    }
    false
}

/// An open menu: the menu item it belongs to, where it is, and its rows (items and the "Type Here" row).
pub(super) struct Menu {
    pub owner: String,
    pub rect: Rect,
    pub rows: Vec<(ItemHit, Rect)>,
}

/// The menus open for the selection: a selected menu item opens its own menu and those of the menu items
/// it is in (the top one under its strip, the others to the right of the row that opens them).
pub(super) fn open_menus(doc: &Designer, layout: &Layout, primary: &str) -> Vec<Menu> {
    let mut chain = Vec::new();
    let mut at = primary.to_owned();
    let strip = loop {
        if doc.get(&at).is_some_and(|c| c.short_type() == "ToolStripMenuItem") {
            chain.push(at.clone());
        }
        match doc.item_owner(&at) {
            Some(o) => at = o,
            None => break at,
        }
    };
    chain.reverse();
    let Some(strip_rect) = layout.get(&strip).map(|p| p.rect) else { return Vec::new() };
    let mut menus: Vec<Menu> = Vec::new();
    for item in chain {
        let origin = match menus.last() {
            None => match strip_items(doc, &strip, strip_rect).into_iter().find(|(n, _)| *n == item) {
                Some((_, r)) => (r.x, strip_rect.y + strip_rect.h),
                None => break,
            },
            Some(m) => match m.rows.iter().find(|(h, _)| *h == ItemHit::Item(item.clone())) {
                Some((_, r)) => (m.rect.x + m.rect.w - 3, r.y - 3),
                None => break,
            },
        };
        let items = doc.items(&item);
        let widest = items.iter().map(|n| text_width(&label(doc, n)) + shortcut(doc, n).map_or(0, |s| text_width(&s) + 24)).max().unwrap_or(0);
        let w = (widest + 28 + 34).max(150);
        let mut y = origin.1 + 3;
        let mut rows = Vec::new();
        for n in items {
            let h = if is_separator(doc, &n) { MENU_SEPARATOR } else { MENU_ROW };
            rows.push((ItemHit::Item(n), Rect::new(origin.0 + 3, y, w - 6, h)));
            y += h;
        }
        rows.push((ItemHit::Add(item.clone()), Rect::new(origin.0 + 3, y, w - 6, MENU_ROW)));
        y += MENU_ROW;
        menus.push(Menu { owner: item, rect: Rect::new(origin.0, origin.1, w, y + 3 - origin.1), rows });
    }
    menus
}

/// Everything clickable of the form's strips, menus and grids, in drawing order (the last one on top):
/// their items, and the "Type Here" / add-column slots of what the selection is in.
pub(super) fn hits(doc: &Designer, layout: &Layout, primary: Option<&str>, menus: &[Menu]) -> Vec<(ItemHit, Rect)> {
    let mut out = Vec::new();
    for p in &layout.controls {
        let Some(c) = doc.get(&p.name) else { continue };
        let active = primary.is_some_and(|s| belongs_to(doc, s, &p.name));
        match c.short_type() {
            "MenuStrip" | "ToolStrip" | "StatusStrip" => {
                out.extend(strip_items(doc, &p.name, p.rect).into_iter().map(|(n, r)| (ItemHit::Item(n), r)));
                if active {
                    out.push((ItemHit::Add(p.name.clone()), strip_add(doc, &p.name, p.rect)));
                }
            }
            "DataGridView" => {
                let headers = column_headers(doc, &p.name, p.rect);
                let x = headers.last().map_or(p.rect.x + 1 + ROW_HEADERS, |(_, r)| r.x + r.w);
                out.extend(headers.into_iter().map(|(n, r)| (ItemHit::Item(n), r)));
                if active {
                    out.push((ItemHit::Add(p.name.clone()), Rect::new(x, p.rect.y + 1, 24, HEADER_HEIGHT)));
                }
            }
            _ => {}
        }
    }
    for m in menus {
        out.extend(m.rows.iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_and_shortcuts() {
        assert_eq!(display_text("&File"), "File");
        assert_eq!(display_text("Save && Close"), "Save & Close");
        let code = "namespace A;\npartial class Form1\n{\n    private void InitializeComponent()\n    {\n        menuStrip1 = new MenuStrip();\n        fileToolStripMenuItem = new ToolStripMenuItem();\n        saveToolStripMenuItem = new ToolStripMenuItem();\n        // \n        // menuStrip1\n        // \n        menuStrip1.Items.AddRange(new ToolStripItem[] { fileToolStripMenuItem });\n        menuStrip1.Location = new Point(0, 0);\n        menuStrip1.Name = \"menuStrip1\";\n        menuStrip1.Size = new Size(400, 24);\n        // \n        // fileToolStripMenuItem\n        // \n        fileToolStripMenuItem.DropDownItems.AddRange(new ToolStripItem[] { saveToolStripMenuItem });\n        fileToolStripMenuItem.Text = \"&File\";\n        // \n        // saveToolStripMenuItem\n        // \n        saveToolStripMenuItem.ShortcutKeys = Keys.Control | Keys.S;\n        saveToolStripMenuItem.Text = \"&Save\";\n        // \n        // Form1\n        // \n        ClientSize = new Size(400, 300);\n        Controls.Add(menuStrip1);\n    }\n\n    private MenuStrip menuStrip1;\n    private ToolStripMenuItem fileToolStripMenuItem;\n    private ToolStripMenuItem saveToolStripMenuItem;\n}\n";
        let doc = Designer::parse(code, "Form").unwrap();
        assert_eq!(shortcut(&doc, "saveToolStripMenuItem").as_deref(), Some("Ctrl+S"));
        assert_eq!(label(&doc, "fileToolStripMenuItem"), "File");
        let layout = wf::layout(&doc, &Default::default());
        let strip = layout.get("menuStrip1").unwrap().rect;
        let file = strip_items(&doc, "menuStrip1", strip);
        assert_eq!(file, [("fileToolStripMenuItem".to_owned(), Rect::new(4, 2, 44, 20))]);
        // Selecting Save opens File's menu under File, with Save and the "Type Here" row, and Save's own
        // (empty) submenu to its right, where its first item can be typed.
        let menus = open_menus(&doc, &layout, "saveToolStripMenuItem");
        assert_eq!(menus.len(), 2);
        assert_eq!(menus[1].rows.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>(), [ItemHit::Add("saveToolStripMenuItem".into())]);
        assert_eq!(menus[1].rect.x, menus[0].rect.x + menus[0].rect.w - 3);
        assert_eq!((menus[0].owner.as_str(), menus[0].rect.x, menus[0].rect.y), ("fileToolStripMenuItem", 4, 24));
        assert_eq!(menus[0].rows.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>(), [ItemHit::Item("saveToolStripMenuItem".into()), ItemHit::Add("fileToolStripMenuItem".into())]);
        // The strip's "Type Here" shows while something of it is selected.
        let all = hits(&doc, &layout, Some("saveToolStripMenuItem"), &menus);
        assert!(all.iter().any(|(h, _)| *h == ItemHit::Add("menuStrip1".into())));
        assert!(!hits(&doc, &layout, Some("Form1"), &[]).iter().any(|(h, _)| matches!(h, ItemHit::Add(_))));
    }
}
