//! Avalonia's layout, done the way Avalonia does it (measure, then arrange) for the panels and controls the
//! designer knows, with the sizes of the Fluent theme. Text is measured by the caller (the UI knows fonts).

use std::collections::HashMap;

use super::{Doc, Drop, ElPath, Element, GridLen, Role, ATTACHED_NAMES, format_num, parse_bool, parse_color, parse_len, parse_num, parse_thickness};

const INF: f32 = f32::INFINITY;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Inside a thickness ([left, top, right, bottom]).
    pub fn deflate(&self, t: [f32; 4]) -> Rect {
        Rect::new(self.x + t[0], self.y + t[1], (self.w - t[0] - t[2]).max(0.0), (self.h - t[1] - t[3]).max(0.0))
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }
}

/// The text properties an element has (its own, else inherited).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    /// Foreground (RGBA), when it or a parent sets one.
    pub fg: Option<u32>,
}

impl Default for Font {
    fn default() -> Self {
        Font { size: 14.0, bold: false, italic: false, fg: None }
    }
}

/// An element as laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub path: ElPath,
    pub rect: Rect,
    /// The part of it that shows (its clipping parents cut the rest).
    pub clip: Rect,
    pub font: Font,
}

/// How a container takes a dropped control.
#[derive(Clone, Debug, PartialEq)]
pub enum AreaKind {
    Canvas,
    /// The edges of the columns and rows (each from its start to the end of the last).
    Grid { cols: Vec<f32>, rows: Vec<f32> },
    Stack { vertical: bool },
    Wrap { horizontal: bool },
    Dock { fill: bool },
    /// Children lie over each other (Panel).
    Overlay,
    /// One child; whether something (an element or text) is in it.
    Single { occupied: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub path: ElPath,
    /// Where its children go.
    pub rect: Rect,
    pub kind: AreaKind,
    /// Where each child is (`None`: it isn't shown).
    pub children: Vec<Option<Rect>>,
}

/// Where a drop goes and how the designer shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct DropPlan {
    pub drop: Drop,
    /// The container's area.
    pub area: Rect,
    /// The line between children it goes in at (stacks, wraps, items).
    pub line: Option<Rect>,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Back to front (parents before their children); the root first.
    pub items: Vec<Placed>,
    /// Containers (inner ones before the ones they are in).
    pub areas: Vec<Area>,
    pub size: (f32, f32),
}

/// The text measurer: (text, font size, bold) → width.
pub type Measure<'a> = &'a dyn Fn(&str, f32, bool) -> f32;

/// The size a root gets: its Width/Height, else the design size, else a window's default.
pub fn root_size(doc: &Doc) -> (Option<f32>, Option<f32>) {
    let r = &doc.root;
    let w = r.attr("Width").and_then(parse_len).or_else(|| r.attr("d:DesignWidth").and_then(parse_len));
    let h = r.attr("Height").and_then(parse_len).or_else(|| r.attr("d:DesignHeight").and_then(parse_len));
    (w, h)
}

impl Layout {
    /// Lays out a view. `tabs`: the page each TabControl shows (else its SelectedIndex).
    pub fn of(doc: &Doc, measure: Measure, tabs: &HashMap<ElPath, usize>) -> Layout {
        let mut e = Engine { measure, tabs, desired: HashMap::new(), fonts: HashMap::new(), out: Layout::default() };
        e.fonts_of(&doc.root, &mut Vec::new(), Font::default());
        let (w, h) = root_size(doc);
        let window = doc.root.local() == "Window";
        let avail = (w.unwrap_or(if window { 800.0 } else { INF }), h.unwrap_or(if window { 450.0 } else { INF }));
        let root_path: ElPath = Vec::new();
        // The root fills its size; a user control without one is as big as its content.
        let chrome = e.chrome(&doc.root);
        let content = e.measure_core(&doc.root, &root_path, (avail.0 - chrome[0] - chrome[2], avail.1 - chrome[1] - chrome[3]));
        let size = (w.unwrap_or(if window { 800.0 } else { (content.0 + chrome[0] + chrome[2]).max(40.0) }), h.unwrap_or(if window { 450.0 } else { (content.1 + chrome[1] + chrome[3]).max(40.0) }));
        let rect = Rect::new(0.0, 0.0, size.0, size.1);
        e.out.size = size;
        let font = e.font(&root_path);
        e.out.items.push(Placed { path: root_path.clone(), rect, clip: rect, font });
        e.arrange_core(&doc.root, &root_path, rect, rect);
        e.out
    }

    pub fn get(&self, path: &[usize]) -> Option<&Placed> {
        self.items.iter().find(|p| p.path == path)
    }

    /// The front-most element under a point (the root when nothing else is).
    pub fn hit(&self, x: f32, y: f32) -> Option<&Placed> {
        self.items.iter().rev().find(|p| p.rect.contains(x, y) && p.clip.contains(x, y))
    }

    pub fn area(&self, path: &[usize]) -> Option<&Area> {
        self.areas.iter().find(|a| a.path == path)
    }

    /// Where a control dropped at (x, y) goes. `moving`: the control being moved (and where in it the mouse
    /// holds it), else a new one of `tag`.
    pub fn drop_at(&self, doc: &Doc, x: f32, y: f32, moving: Option<(&[usize], (f32, f32))>, tag: &str) -> Option<DropPlan> {
        let moving_path = moving.map(|(p, _)| p);
        let tab_item = super::local_name(tag) == "TabItem";
        // The innermost container under the mouse that takes it.
        let mut areas: Vec<&Area> = self.areas.iter().collect();
        areas.sort_by_key(|a| std::cmp::Reverse(a.path.len()));
        for area in areas {
            if !area.rect.contains(x, y) || moving_path.is_some_and(|m| Doc::is_within(&area.path, m)) {
                continue;
            }
            let el = doc.get(&area.path)?;
            // A TabControl takes tab items (other controls go into the page shown).
            if el.local() == "TabControl" && !tab_item {
                continue;
            }
            let current = moving_path.and_then(|m| m.split_last()).filter(|(_, parent)| *parent == area.path.as_slice()).map(|(i, _)| *i);
            let others: Vec<(usize, Rect)> = area.children.iter().enumerate().filter(|(i, _)| Some(*i) != current).filter_map(|(i, r)| r.map(|r| (i, r))).collect();
            let count = area.children.len();
            let mut attrs: Vec<(String, Option<String>)> = Vec::new();
            let keep = |attrs: &mut Vec<(String, Option<String>)>, own: &[&str]| {
                if let Some(m) = moving_path.and_then(|m| doc.get(m)) {
                    for a in ATTACHED_NAMES.iter().filter(|a| !own.contains(a)) {
                        if m.attr(a).is_some() {
                            attrs.push(((*a).to_owned(), None));
                        }
                    }
                }
            };
            let line_at = |vertical: bool, index: usize| -> Option<Rect> {
                let before = others.iter().filter(|(i, _)| *i < index).map(|(_, r)| *r).next_back();
                let after = others.iter().find(|(i, _)| *i >= index).map(|(_, r)| *r);
                let pos = match (before, after) {
                    (Some(b), Some(a)) => if vertical { (b.bottom() + a.y) / 2.0 } else { (b.right() + a.x) / 2.0 },
                    (Some(b), None) => if vertical { b.bottom() + 1.0 } else { b.right() + 1.0 },
                    (None, Some(a)) => if vertical { a.y - 1.0 } else { a.x - 1.0 },
                    (None, None) => if vertical { area.rect.y + 1.0 } else { area.rect.x + 1.0 },
                };
                Some(if vertical { Rect::new(area.rect.x, pos - 1.0, area.rect.w, 2.0) } else { Rect::new(pos - 1.0, area.rect.y, 2.0, area.rect.h) })
            };
            let plan = |index: usize, attrs: Vec<(String, Option<String>)>, in_place: bool, line: Option<Rect>| {
                Some(DropPlan { drop: Drop { parent: area.path.clone(), index, attrs, in_place }, area: area.rect, line })
            };
            match &area.kind {
                AreaKind::Single { occupied } => {
                    if *occupied && current.is_none() {
                        continue;
                    }
                    keep(&mut attrs, &[]);
                    return plan(0, attrs, current.is_some(), None);
                }
                AreaKind::Canvas => {
                    let (gx, gy) = moving.map_or((0.0, 0.0), |(_, g)| g);
                    keep(&mut attrs, &["Canvas.Left", "Canvas.Top"]);
                    attrs.push(("Canvas.Left".into(), Some(format_num((x - gx - area.rect.x).round()))));
                    attrs.push(("Canvas.Top".into(), Some(format_num((y - gy - area.rect.y).round()))));
                    return plan(current.unwrap_or(count), attrs, current.is_some(), None);
                }
                AreaKind::Grid { cols, rows } => {
                    let cell = |edges: &[f32], v: f32| edges.windows(2).position(|w| v < w[1]).unwrap_or(edges.len().saturating_sub(2));
                    let (c, r) = (cell(cols, x), cell(rows, y));
                    keep(&mut attrs, &["Grid.Row", "Grid.Column", "Grid.RowSpan", "Grid.ColumnSpan"]);
                    attrs.push(("Grid.Column".into(), (c > 0).then(|| c.to_string())));
                    attrs.push(("Grid.Row".into(), (r > 0).then(|| r.to_string())));
                    // Without a value there's nothing to take out.
                    if let Some(m) = moving_path.and_then(|m| doc.get(m)) {
                        attrs.retain(|(n, v)| v.is_some() || m.attr(n).is_some());
                    } else {
                        attrs.retain(|(_, v)| v.is_some());
                    }
                    return plan(current.unwrap_or(count), attrs, current.is_some(), None);
                }
                AreaKind::Stack { vertical } => {
                    let index = others.iter().filter(|(_, r)| if *vertical { r.y + r.h / 2.0 < y } else { r.x + r.w / 2.0 < x }).map(|(i, _)| i + 1).max().unwrap_or(0);
                    keep(&mut attrs, &[]);
                    return plan(index, attrs, false, line_at(*vertical, index));
                }
                AreaKind::Wrap { horizontal } => {
                    let before = |r: &Rect| if *horizontal { r.bottom() < y || (r.y <= y && r.x + r.w / 2.0 < x) } else { r.right() < x || (r.x <= x && r.y + r.h / 2.0 < y) };
                    let index = others.iter().filter(|(_, r)| before(r)).map(|(i, _)| i + 1).max().unwrap_or(0);
                    keep(&mut attrs, &[]);
                    return plan(index, attrs, false, line_at(!*horizontal, index));
                }
                AreaKind::Dock { fill } => {
                    let r = area.rect;
                    let d = [("Left", x - r.x), ("Top", y - r.y), ("Right", r.right() - x), ("Bottom", r.bottom() - y)];
                    let side = d.iter().min_by(|a, b| a.1.total_cmp(&b.1)).map_or("Left", |s| s.0);
                    keep(&mut attrs, &["DockPanel.Dock"]);
                    attrs.push(("DockPanel.Dock".into(), (side != "Left").then(|| side.to_owned())));
                    if let Some(m) = moving_path.and_then(|m| doc.get(m)) {
                        attrs.retain(|(n, v)| v.is_some() || m.attr(n).is_some());
                    } else {
                        attrs.retain(|(_, v)| v.is_some());
                    }
                    // The filling child stays last.
                    let n = count - usize::from(current.is_some_and(|c| c < count));
                    let index = if *fill && n > 0 && current != Some(count - 1) { count - 1 } else { count };
                    let line = match side {
                        "Left" => Rect::new(r.x, r.y, 3.0, r.h),
                        "Top" => Rect::new(r.x, r.y, r.w, 3.0),
                        "Right" => Rect::new(r.right() - 3.0, r.y, 3.0, r.h),
                        _ => Rect::new(r.x, r.bottom() - 3.0, r.w, 3.0),
                    };
                    return plan(index, attrs, false, Some(line));
                }
                AreaKind::Overlay => {
                    keep(&mut attrs, &[]);
                    return plan(current.unwrap_or(count), attrs, current.is_some(), None);
                }
            }
        }
        None
    }
}

/// A Grid child's column, column span, row, row span and desired size.
type Cell = (usize, usize, usize, usize, (f32, f32));

struct Engine<'a> {
    measure: Measure<'a>,
    tabs: &'a HashMap<ElPath, usize>,
    /// Desired sizes, margins included.
    desired: HashMap<ElPath, (f32, f32)>,
    fonts: HashMap<ElPath, Font>,
    out: Layout,
}

fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    v.min(hi).max(lo)
}

fn visible(el: &Element) -> bool {
    el.attr("IsVisible").and_then(parse_bool) != Some(false)
}

fn thickness(el: &Element, name: &str) -> Option<[f32; 4]> {
    el.attr(name).and_then(parse_thickness)
}

fn child_path(path: &[usize], i: usize) -> ElPath {
    let mut p = path.to_vec();
    p.push(i);
    p
}

/// The text a control shows for its content property (`Content`, `Text`, …), bindings as written.
pub fn content_text(el: &Element) -> Option<String> {
    let prop = el.info().map_or("Content", |c| c.content_prop);
    if prop.is_empty() {
        return None;
    }
    el.value(prop).map(|v| super::display_text(&v))
}

/// Fluent's padding of a control (its own Padding, else the theme's).
pub fn padding(el: &Element) -> [f32; 4] {
    if let Some(p) = thickness(el, "Padding") {
        return p;
    }
    match el.local() {
        "Button" | "ToggleButton" | "RepeatButton" | "HyperlinkButton" => [8.0, 5.0, 8.0, 6.0],
        "TextBox" | "MaskedTextBox" | "NumericUpDown" => [10.0, 6.0, 6.0, 5.0],
        "ComboBox" => [12.0, 7.0, 0.0, 7.0],
        "Label" => [4.0; 4],
        "ListBoxItem" => [12.0, 9.0, 12.0, 12.0],
        "ComboBoxItem" => [11.0, 5.0, 11.0, 7.0],
        "MenuItem" => [12.0, 6.0, 12.0, 6.0],
        "Expander" => [16.0; 4],
        "TabItem" => [12.0, 0.0, 12.0, 0.0],
        _ => [0.0; 4],
    }
}

/// Fluent's border thickness of a control.
pub fn border(el: &Element) -> [f32; 4] {
    if let Some(b) = thickness(el, "BorderThickness") {
        return b;
    }
    match el.local() {
        "Button" | "ToggleButton" | "RepeatButton" | "TextBox" | "MaskedTextBox" | "ComboBox" | "NumericUpDown" | "DatePicker" | "TimePicker" | "Expander" | "Calendar" => [1.0; 4],
        _ => [0.0; 4],
    }
}

/// The control's own margin (a Separator's comes from the theme).
fn margin(el: &Element) -> [f32; 4] {
    thickness(el, "Margin").unwrap_or(if el.local() == "Separator" { [0.0, 4.0, 0.0, 4.0] } else { [0.0; 4] })
}

/// The alignment a control has: its own, else Fluent's for its kind.
pub fn alignment(el: &Element, horizontal: bool) -> String {
    let attr = if horizontal { "HorizontalAlignment" } else { "VerticalAlignment" };
    el.attr(attr).map(str::to_owned).unwrap_or_else(|| el.info().map_or("Stretch", |c| if horizontal { c.h_align } else { c.v_align }).to_owned())
}

impl Engine<'_> {
    fn fonts_of(&mut self, el: &Element, path: &mut ElPath, inherited: Font) {
        let mut f = inherited;
        if let Some(s) = el.attr("FontSize").and_then(parse_num) {
            f.size = s.max(1.0);
        }
        if let Some(w) = el.attr("FontWeight") {
            f.bold = matches!(w, "SemiBold" | "Bold" | "ExtraBold" | "Black" | "Heavy" | "DemiBold" | "UltraBold") || w.parse::<u32>().is_ok_and(|n| n >= 600);
        }
        if let Some(s) = el.attr("FontStyle") {
            f.italic = s != "Normal";
        }
        if let Some(c) = el.attr("Foreground").and_then(parse_color) {
            f.fg = Some(c);
        }
        self.fonts.insert(path.clone(), f);
        // A tab header's font isn't inherited by the page.
        for (i, c) in el.children.iter().enumerate() {
            path.push(i);
            self.fonts_of(c, path, f);
            path.pop();
        }
    }

    fn font(&self, path: &[usize]) -> Font {
        self.fonts.get(path).copied().unwrap_or_default()
    }

    fn text_size(&self, text: &str, font: Font, wrap_width: Option<f32>) -> (f32, f32) {
        let line_h = (font.size * 1.33).round();
        let mut w: f32 = 0.0;
        let mut lines = 0.0;
        for line in text.split('\n') {
            let lw = (self.measure)(line, font.size, font.bold);
            match wrap_width.filter(|ww| ww.is_finite() && *ww > 0.0 && lw > *ww) {
                Some(ww) => {
                    w = w.max(ww);
                    lines += (lw / ww).ceil();
                }
                None => {
                    w = w.max(lw);
                    lines += 1.0;
                }
            }
        }
        (w.ceil(), line_h * lines)
    }

    /// Padding and border together.
    fn chrome(&self, el: &Element) -> [f32; 4] {
        let p = padding(el);
        let b = border(el);
        [p[0] + b[0], p[1] + b[1], p[2] + b[2], p[3] + b[3]]
    }

    /// Measures an element in `avail` (margins included); returns its desired size.
    fn measure(&mut self, el: &Element, path: &[usize], avail: (f32, f32)) -> (f32, f32) {
        if !visible(el) {
            self.desired.insert(path.to_vec(), (0.0, 0.0));
            return (0.0, 0.0);
        }
        let m = margin(el);
        let width = el.attr("Width").and_then(parse_len);
        let height = el.attr("Height").and_then(parse_len);
        let (min_w, max_w) = (el.attr("MinWidth").and_then(parse_len).unwrap_or(0.0), el.attr("MaxWidth").and_then(parse_len).unwrap_or(INF));
        let (min_h, max_h) = (el.attr("MinHeight").and_then(parse_len).unwrap_or(0.0), el.attr("MaxHeight").and_then(parse_len).unwrap_or(INF));
        let aw = width.unwrap_or((avail.0 - m[0] - m[2]).max(0.0));
        let ah = height.unwrap_or((avail.1 - m[1] - m[3]).max(0.0));
        let inner = (clampf(aw, min_w, max_w), clampf(ah, min_h, max_h));
        let core = self.measure_core(el, path, inner);
        let w = clampf(width.unwrap_or(core.0), min_w, max_w);
        let h = clampf(height.unwrap_or(core.1), min_h, max_h);
        let d = (w + m[0] + m[2], h + m[1] + m[3]);
        self.desired.insert(path.to_vec(), d);
        d
    }

    fn desired(&self, path: &[usize]) -> (f32, f32) {
        self.desired.get(path).copied().unwrap_or_default()
    }

    /// Places an element in its slot (margins and alignment applied) and lays out what is inside it.
    fn arrange(&mut self, el: &Element, path: &[usize], slot: Rect, clip: Rect) -> Option<Rect> {
        if !visible(el) {
            return None;
        }
        let s = slot.deflate(margin(el));
        let d = self.desired(path);
        let m = margin(el);
        let (dw, dh) = ((d.0 - m[0] - m[2]).max(0.0), (d.1 - m[1] - m[3]).max(0.0));
        let width = el.attr("Width").and_then(parse_len);
        let height = el.attr("Height").and_then(parse_len);
        let (min_w, max_w) = (el.attr("MinWidth").and_then(parse_len).unwrap_or(0.0), el.attr("MaxWidth").and_then(parse_len).unwrap_or(INF));
        let (min_h, max_h) = (el.attr("MinHeight").and_then(parse_len).unwrap_or(0.0), el.attr("MaxHeight").and_then(parse_len).unwrap_or(INF));
        let ha = alignment(el, true);
        let va = alignment(el, false);
        let w = match width {
            Some(w) => clampf(w, min_w, max_w),
            None if ha == "Stretch" => clampf(s.w, min_w, max_w),
            None => dw.min(s.w).max(min_w),
        };
        let h = match height {
            Some(h) => clampf(h, min_h, max_h),
            None if va == "Stretch" => clampf(s.h, min_h, max_h),
            None => dh.min(s.h).max(min_h),
        };
        let x = match ha.as_str() {
            "Left" => s.x,
            "Right" => s.right() - w,
            _ => s.x + (s.w - w) / 2.0,
        };
        let y = match va.as_str() {
            "Top" => s.y,
            "Bottom" => s.bottom() - h,
            _ => s.y + (s.h - h) / 2.0,
        };
        let rect = Rect::new(x, y, w, h);
        let font = self.font(path);
        self.out.items.push(Placed { path: path.to_vec(), rect, clip, font });
        let clips = matches!(el.local(), "ScrollViewer" | "ListBox" | "TabControl" | "Expander" | "TextBox") || el.attr("ClipToBounds").and_then(parse_bool) == Some(true);
        let inner_clip = if clips { clip.intersect(&rect) } else { clip };
        self.arrange_core(el, path, rect, inner_clip);
        Some(rect)
    }

    /// The size of an element's content and chrome in `avail` (no margin).
    fn measure_core(&mut self, el: &Element, path: &[usize], avail: (f32, f32)) -> (f32, f32) {
        let font = self.font(path);
        let chrome = self.chrome(el);
        let (cw, ch) = (chrome[0] + chrome[2], chrome[1] + chrome[3]);
        let inner = ((avail.0 - cw).max(0.0), (avail.1 - ch).max(0.0));
        let text = || content_text(el).unwrap_or_default();
        let line_h = (font.size * 1.33).round();
        let tag = el.local();
        match tag {
            "TextBlock" | "SelectableTextBlock" => {
                let wrap = el.attr("TextWrapping").is_some_and(|w| w != "NoWrap");
                let (w, h) = self.text_size(&text(), font, wrap.then_some(inner.0));
                return (w + cw, h + ch);
            }
            "TextBox" | "MaskedTextBox" => {
                let t = el.value("Text").or_else(|| el.attr("Watermark").map(str::to_owned)).unwrap_or_default();
                let (w, h) = self.text_size(&super::display_text(&t), font, None);
                return ((w + cw).max(64.0), (h + ch).max(32.0));
            }
            "NumericUpDown" => {
                let t = el.attr("Value").unwrap_or("0");
                let (w, h) = self.text_size(t, font, None);
                return ((w + cw + 64.0).max(100.0), (h + ch).max(32.0));
            }
            "ComboBox" => {
                let widest = el.children.iter().map(|c| content_text(c).unwrap_or_default()).chain(el.attr("PlaceholderText").map(str::to_owned)).map(|t| self.text_size(&t, font, None).0).fold(0.0, f32::max);
                for (i, c) in el.children.iter().enumerate() {
                    self.measure(c, &child_path(path, i), (INF, INF));
                }
                return ((widest + cw + 32.0).max(64.0), (line_h + ch).max(32.0));
            }
            "CheckBox" | "RadioButton" => {
                let (w, h) = self.content_size(el, path, (inner.0 - 28.0, inner.1), font);
                return (w + 28.0 + if w > 0.0 { 8.0 } else { 0.0 }, h.max(32.0));
            }
            "ToggleSwitch" => {
                let header = self.content_size(el, path, inner, font);
                let state = if el.attr("IsChecked").and_then(parse_bool) == Some(true) { el.attr("OnContent").unwrap_or("On") } else { el.attr("OffContent").unwrap_or("Off") };
                let sw = 40.0 + 12.0 + self.text_size(state, font, None).0;
                return (header.0.max(sw), header.1 + if header.1 > 0.0 { 4.0 } else { 0.0 } + 32.0);
            }
            "Slider" => {
                let vertical = el.attr("Orientation") == Some("Vertical");
                return if vertical { (32.0, 64.0) } else { (64.0, 32.0) };
            }
            "ProgressBar" => return if el.attr("Orientation") == Some("Vertical") { (4.0, 100.0) } else { (100.0, 4.0) },
            "Separator" => return (0.0, 1.0),
            "Image" => return (64.0, 64.0),
            "Rectangle" | "Ellipse" => return (0.0, 0.0),
            "DatePicker" => return (296.0, 32.0),
            "TimePicker" => return (242.0, 32.0),
            "Calendar" => return (300.0, 320.0),
            _ => {}
        }
        match el.role() {
            Role::Leaf => {
                let t = text();
                if t.is_empty() { (cw, ch) } else {
                    let (w, h) = self.text_size(&t, font, None);
                    (w + cw, h + ch)
                }
            }
            Role::Content | Role::Decorator => match tag {
                "ScrollViewer" => {
                    let horizontal = el.attr("HorizontalScrollBarVisibility").is_some_and(|v| v != "Disabled");
                    let vertical = el.attr("VerticalScrollBarVisibility") != Some("Disabled");
                    let (w, h) = self.content_size(el, path, (if horizontal { INF } else { inner.0 }, if vertical { INF } else { inner.1 }), font);
                    ((w + cw).min(avail.0.max(cw)), (h + ch).min(avail.1.max(ch)))
                }
                "Expander" => {
                    let header = self.text_size(&super::display_text(el.attr("Header").unwrap_or_default()), font, None);
                    let expanded = el.attr("IsExpanded").and_then(parse_bool) == Some(true);
                    let content = if expanded { self.content_size(el, path, (inner.0, (inner.1 - 48.0).max(0.0)), font) } else {
                        self.content_size(el, path, (0.0, 0.0), font);
                        (0.0, 0.0)
                    };
                    let b = border(el);
                    let hw = header.0 + 16.0 + 48.0;
                    let w = hw.max(content.0 + cw);
                    let h = 48.0 + if expanded { content.1 + ch } else { b[1] + b[3] };
                    (w, h)
                }
                "Button" | "ToggleButton" | "RepeatButton" | "HyperlinkButton" => {
                    let (w, h) = self.content_size(el, path, inner, font);
                    (w + cw, (h + ch).max(if tag == "HyperlinkButton" { 0.0 } else { 32.0 }))
                }
                "TabItem" => {
                    let header = self.text_size(&super::display_text(el.attr("Header").unwrap_or_default()), Font { size: 20.0, ..font }, None);
                    (header.0 + cw, header.1.max(48.0))
                }
                _ => {
                    let (w, h) = self.content_size(el, path, inner, font);
                    (w + cw, h + ch)
                }
            },
            Role::Items => self.measure_items(el, path, inner, chrome, font),
            Role::Panel => {
                let (w, h) = self.measure_panel(el, path, inner);
                (w + cw, h + ch)
            }
        }
    }

    /// The size of a content control's content: its child element, else its text.
    fn content_size(&mut self, el: &Element, path: &[usize], avail: (f32, f32), font: Font) -> (f32, f32) {
        if let Some(c) = el.children.first() {
            let mut size = self.measure(c, &child_path(path, 0), avail);
            // More than one child (an unknown control): they lie over each other.
            for (i, c) in el.children.iter().enumerate().skip(1) {
                let d = self.measure(c, &child_path(path, i), avail);
                size = (size.0.max(d.0), size.1.max(d.1));
            }
            return size;
        }
        match content_text(el).filter(|t| !t.is_empty()) {
            Some(t) => self.text_size(&t, font, None),
            None => (0.0, 0.0),
        }
    }

    fn measure_items(&mut self, el: &Element, path: &[usize], inner: (f32, f32), chrome: [f32; 4], font: Font) -> (f32, f32) {
        let (cw, ch) = (chrome[0] + chrome[2], chrome[1] + chrome[3]);
        match el.local() {
            "TabControl" => {
                let shown = self.shown_tab(el, path);
                let (mut hw, mut hh): (f32, f32) = (0.0, 48.0);
                let mut page = (0.0f32, 0.0f32);
                for (i, c) in el.children.iter().enumerate() {
                    let p = child_path(path, i);
                    let d = self.measure(c, &p, (INF, INF));
                    hw += d.0;
                    hh = hh.max(d.1);
                    if i == shown && c.local() == "TabItem" {
                        let ip = padding_of_page();
                        page = self.content_size(c, &p, ((inner.0 - ip[0] - ip[2]).max(0.0), (inner.1 - hh - ip[1] - ip[3]).max(0.0)), font);
                        page = (page.0 + ip[0] + ip[2], page.1 + ip[1] + ip[3]);
                    }
                }
                (hw.max(page.0) + cw, hh + page.1 + ch)
            }
            "Menu" | "MenuItem" => {
                let (mut w, mut h): (f32, f32) = (0.0, 0.0);
                for (i, c) in el.children.iter().enumerate() {
                    let d = self.measure(c, &child_path(path, i), (INF, INF));
                    if el.local() == "Menu" {
                        w += d.0;
                        h = h.max(d.1);
                    }
                }
                if el.local() == "MenuItem" {
                    let t = self.text_size(&super::display_text(el.attr("Header").unwrap_or_default()), font, None);
                    return (t.0 + cw, t.1 + ch);
                }
                (w + cw, h.max(32.0) + ch)
            }
            _ => {
                // A list: its items one under the other (an item that isn't a ListBoxItem gets one's padding).
                let (mut w, mut h): (f32, f32) = (0.0, 0.0);
                for (i, c) in el.children.iter().enumerate() {
                    let pad = item_padding(el, c);
                    let d = self.measure(c, &child_path(path, i), ((inner.0 - pad[0] - pad[2]).max(0.0), INF));
                    w = w.max(d.0 + pad[0] + pad[2]);
                    h += d.1 + pad[1] + pad[3];
                }
                (w + cw, h + ch)
            }
        }
    }

    fn shown_tab(&self, el: &Element, path: &[usize]) -> usize {
        let selected = el.children.iter().position(|c| c.attr("IsSelected").and_then(parse_bool) == Some(true));
        self.tabs.get(path).copied().or(selected).or_else(|| el.attr("SelectedIndex").and_then(parse_num).map(|n| n.max(0.0) as usize)).unwrap_or(0)
    }

    fn measure_panel(&mut self, el: &Element, path: &[usize], avail: (f32, f32)) -> (f32, f32) {
        let n = el.children.len();
        match el.local() {
            "StackPanel" => {
                let vertical = el.attr("Orientation") != Some("Horizontal");
                let spacing = el.attr("Spacing").and_then(parse_num).unwrap_or(0.0);
                let (mut w, mut h): (f32, f32) = (0.0, 0.0);
                let mut shown = 0usize;
                for (i, c) in el.children.iter().enumerate() {
                    let d = self.measure(c, &child_path(path, i), if vertical { (avail.0, INF) } else { (INF, avail.1) });
                    if visible(c) {
                        shown += 1;
                    }
                    if vertical {
                        w = w.max(d.0);
                        h += d.1;
                    } else {
                        w += d.0;
                        h = h.max(d.1);
                    }
                }
                let gaps = spacing * shown.saturating_sub(1) as f32;
                if vertical { (w, h + gaps) } else { (w + gaps, h) }
            }
            "WrapPanel" => {
                let horizontal = el.attr("Orientation") != Some("Vertical");
                let iw = el.attr("ItemWidth").and_then(parse_len);
                let ih = el.attr("ItemHeight").and_then(parse_len);
                let limit = if horizontal { avail.0 } else { avail.1 };
                let (mut line_u, mut line_v, mut total_u, mut total_v): (f32, f32, f32, f32) = (0.0, 0.0, 0.0, 0.0);
                for (i, c) in el.children.iter().enumerate() {
                    let d = self.measure(c, &child_path(path, i), (iw.unwrap_or(avail.0), ih.unwrap_or(avail.1)));
                    let d = (iw.unwrap_or(d.0), ih.unwrap_or(d.1));
                    let (u, v) = if horizontal { d } else { (d.1, d.0) };
                    if line_u + u > limit && line_u > 0.0 {
                        total_u = total_u.max(line_u);
                        total_v += line_v;
                        line_u = 0.0;
                        line_v = 0.0;
                    }
                    line_u += u;
                    line_v = line_v.max(v);
                }
                total_u = total_u.max(line_u);
                total_v += line_v;
                if horizontal { (total_u, total_v) } else { (total_v, total_u) }
            }
            "DockPanel" => {
                let fill = el.attr("LastChildFill").and_then(parse_bool) != Some(false);
                let (mut used_w, mut used_h, mut max_w, mut max_h): (f32, f32, f32, f32) = (0.0, 0.0, 0.0, 0.0);
                for (i, c) in el.children.iter().enumerate() {
                    let d = self.measure(c, &child_path(path, i), ((avail.0 - used_w).max(0.0), (avail.1 - used_h).max(0.0)));
                    let last = fill && i + 1 == n;
                    match c.attr("DockPanel.Dock").unwrap_or("Left") {
                        _ if last => {
                            max_w = max_w.max(used_w + d.0);
                            max_h = max_h.max(used_h + d.1);
                        }
                        "Top" | "Bottom" => {
                            max_w = max_w.max(used_w + d.0);
                            used_h += d.1;
                        }
                        _ => {
                            max_h = max_h.max(used_h + d.1);
                            used_w += d.0;
                        }
                    }
                }
                (max_w.max(used_w), max_h.max(used_h))
            }
            "UniformGrid" => {
                let (cols, rows) = uniform_dims(el);
                let cell_avail = (avail.0 / cols as f32, avail.1 / rows as f32);
                let mut cell: (f32, f32) = (0.0, 0.0);
                for (i, c) in el.children.iter().enumerate() {
                    let d = self.measure(c, &child_path(path, i), cell_avail);
                    cell = (cell.0.max(d.0), cell.1.max(d.1));
                }
                (cell.0 * cols as f32, cell.1 * rows as f32)
            }
            "Grid" => {
                let (cols, rows) = self.grid_tracks(el, path, avail);
                (cols.iter().sum(), rows.iter().sum())
            }
            "Canvas" => {
                for (i, c) in el.children.iter().enumerate() {
                    self.measure(c, &child_path(path, i), (INF, INF));
                }
                (0.0, 0.0)
            }
            _ => {
                let mut size: (f32, f32) = (0.0, 0.0);
                for (i, c) in el.children.iter().enumerate() {
                    let d = self.measure(c, &child_path(path, i), avail);
                    size = (size.0.max(d.0), size.1.max(d.1));
                }
                size
            }
        }
    }

    /// The widths of a Grid's columns and the heights of its rows in `avail` (its children measured).
    fn grid_tracks(&mut self, el: &Element, path: &[usize], avail: (f32, f32)) -> (Vec<f32>, Vec<f32>) {
        let mut col_defs = el.definitions(false);
        let mut row_defs = el.definitions(true);
        if col_defs.is_empty() {
            col_defs.push(GridLen::Star(1.0));
        }
        if row_defs.is_empty() {
            row_defs.push(GridLen::Star(1.0));
        }
        let cell = |c: &Element, defs: &[GridLen], col: bool| -> (usize, usize) {
            let (at, span) = if col { ("Grid.Column", "Grid.ColumnSpan") } else { ("Grid.Row", "Grid.RowSpan") };
            let i = c.attr(at).and_then(parse_num).map_or(0, |n| n.max(0.0) as usize).min(defs.len() - 1);
            let s = c.attr(span).and_then(parse_num).map_or(1, |n| n.max(1.0) as usize).min(defs.len() - i);
            (i, s)
        };
        // Pass 1: children measured in their pixel cells (else unbounded) for the Auto tracks.
        let fixed = |defs: &[GridLen], from: usize, span: usize| -> f32 {
            let mut sum = 0.0;
            for d in &defs[from..from + span] {
                match d {
                    GridLen::Px(p) => sum += p,
                    _ => return INF,
                }
            }
            sum
        };
        let mut sizes = Vec::new();
        for (i, c) in el.children.iter().enumerate() {
            let (ci, cs) = cell(c, &col_defs, true);
            let (ri, rs) = cell(c, &row_defs, false);
            let d = self.measure(c, &child_path(path, i), (fixed(&col_defs, ci, cs), fixed(&row_defs, ri, rs)));
            sizes.push((ci, cs, ri, rs, d));
        }
        let tracks = |defs: &[GridLen], col: bool, total: f32, sizes: &[Cell]| -> Vec<f32> {
            let mut out: Vec<f32> = defs.iter().map(|d| if let GridLen::Px(p) = d { *p } else { 0.0 }).collect();
            // Auto (and, without room to share, star) tracks take their single-span children's size.
            for &(ci, cs, ri, rs, d) in sizes {
                let (i, s, v) = if col { (ci, cs, d.0) } else { (ri, rs, d.1) };
                if s == 1 && (defs[i] == GridLen::Auto || (matches!(defs[i], GridLen::Star(_)) && !total.is_finite())) {
                    out[i] = out[i].max(v);
                }
            }
            if total.is_finite() {
                let used: f32 = defs.iter().zip(&out).filter(|(d, _)| !matches!(d, GridLen::Star(_))).map(|(_, s)| s).sum();
                let weights: f32 = defs.iter().map(|d| if let GridLen::Star(w) = d { *w } else { 0.0 }).sum();
                let left = (total - used).max(0.0);
                for (d, s) in defs.iter().zip(out.iter_mut()) {
                    if let GridLen::Star(w) = d {
                        *s = if weights > 0.0 { left * w / weights } else { 0.0 };
                    }
                }
            } else {
                // Unbounded: stars share in proportion to the largest one's need.
                let unit = defs.iter().zip(&out).filter_map(|(d, s)| if let GridLen::Star(w) = d { Some(s / w.max(0.01)) } else { None }).fold(0.0, f32::max);
                for (d, s) in defs.iter().zip(out.iter_mut()) {
                    if let GridLen::Star(w) = d {
                        *s = unit * w;
                    }
                }
            }
            out
        };
        let cols = tracks(&col_defs, true, avail.0, &sizes);
        let rows = tracks(&row_defs, false, avail.1, &sizes);
        // Pass 2: children measured in their final cells (wrapping text gets its height).
        for (i, c) in el.children.iter().enumerate() {
            let (ci, cs, ri, rs, _) = sizes[i];
            let w: f32 = cols[ci..ci + cs].iter().sum();
            let h: f32 = rows[ri..ri + rs].iter().sum();
            self.measure(c, &child_path(path, i), (w, if row_defs[ri] == GridLen::Auto { INF } else { h }));
        }
        (cols, rows)
    }

    /// Lays out what is inside an element placed at `rect`.
    fn arrange_core(&mut self, el: &Element, path: &[usize], rect: Rect, clip: Rect) {
        let chrome = self.chrome(el);
        let inner = rect.deflate(chrome);
        let tag = el.local();
        let mut children: Vec<Option<Rect>> = Vec::new();
        let kind = match el.role() {
            Role::Leaf => {
                // A ComboBox's items stay in its closed drop-down.
                return;
            }
            Role::Content | Role::Decorator => {
                let area = match tag {
                    "CheckBox" | "RadioButton" => Rect::new(inner.x + 28.0, inner.y, (inner.w - 28.0).max(0.0), inner.h),
                    "Expander" => {
                        if el.attr("IsExpanded").and_then(parse_bool) != Some(true) {
                            return;
                        }
                        Rect::new(rect.x, rect.y + 48.0, rect.w, (rect.h - 48.0).max(0.0)).deflate(chrome)
                    }
                    // A tab item's content shows in its TabControl's page.
                    "TabItem" => return,
                    "ScrollViewer" => {
                        // Content as big as it wants (in the directions that scroll), at the top left.
                        let d = el.children.first().map(|_| self.desired(&child_path(path, 0))).unwrap_or_default();
                        let horizontal = el.attr("HorizontalScrollBarVisibility").is_some_and(|v| v != "Disabled");
                        let vertical = el.attr("VerticalScrollBarVisibility") != Some("Disabled");
                        Rect::new(inner.x, inner.y, if horizontal { inner.w.max(d.0) } else { inner.w }, if vertical { inner.h.max(d.1) } else { inner.h })
                    }
                    _ => inner,
                };
                for (i, c) in el.children.iter().enumerate() {
                    children.push(self.arrange_content(el, c, &child_path(path, i), area, clip));
                }
                let occupied = !el.children.is_empty() || content_text(el).is_some_and(|t| !t.is_empty()) && el.attr(el.info().map_or("Content", |c| c.content_prop)).is_none();
                self.out.areas.push(Area { path: path.to_vec(), rect: area, kind: AreaKind::Single { occupied }, children });
                return;
            }
            Role::Items => {
                self.arrange_items(el, path, rect, inner, clip);
                return;
            }
            Role::Panel => match tag {
                "StackPanel" => {
                    let vertical = el.attr("Orientation") != Some("Horizontal");
                    let spacing = el.attr("Spacing").and_then(parse_num).unwrap_or(0.0);
                    let mut at = if vertical { inner.y } else { inner.x };
                    for (i, c) in el.children.iter().enumerate() {
                        let p = child_path(path, i);
                        let d = self.desired(&p);
                        let slot = if vertical { Rect::new(inner.x, at, inner.w, d.1) } else { Rect::new(at, inner.y, d.0, inner.h) };
                        let r = self.arrange(c, &p, slot, clip);
                        if r.is_some() {
                            at += (if vertical { d.1 } else { d.0 }) + spacing;
                        }
                        children.push(r);
                    }
                    AreaKind::Stack { vertical }
                }
                "WrapPanel" => {
                    let horizontal = el.attr("Orientation") != Some("Vertical");
                    let iw = el.attr("ItemWidth").and_then(parse_len);
                    let ih = el.attr("ItemHeight").and_then(parse_len);
                    let (mut u, mut v, mut line_v) = (0.0f32, 0.0f32, 0.0f32);
                    let limit = if horizontal { inner.w } else { inner.h };
                    for (i, c) in el.children.iter().enumerate() {
                        let p = child_path(path, i);
                        let d = self.desired(&p);
                        let d = (iw.unwrap_or(d.0), ih.unwrap_or(d.1));
                        let (du, dv) = if horizontal { d } else { (d.1, d.0) };
                        if u + du > limit && u > 0.0 {
                            v += line_v;
                            u = 0.0;
                            line_v = 0.0;
                        }
                        let slot = if horizontal { Rect::new(inner.x + u, inner.y + v, du, dv) } else { Rect::new(inner.x + v, inner.y + u, dv, du) };
                        children.push(self.arrange(c, &p, slot, clip));
                        u += du;
                        line_v = line_v.max(dv);
                    }
                    AreaKind::Wrap { horizontal }
                }
                "DockPanel" => {
                    let fill = el.attr("LastChildFill").and_then(parse_bool) != Some(false);
                    let mut left = inner;
                    let n = el.children.len();
                    for (i, c) in el.children.iter().enumerate() {
                        let p = child_path(path, i);
                        let d = self.desired(&p);
                        let slot = if fill && i + 1 == n {
                            left
                        } else {
                            match c.attr("DockPanel.Dock").unwrap_or("Left") {
                                "Top" => {
                                    let s = Rect::new(left.x, left.y, left.w, d.1.min(left.h));
                                    left = Rect::new(left.x, left.y + s.h, left.w, left.h - s.h);
                                    s
                                }
                                "Bottom" => {
                                    let s = Rect::new(left.x, left.bottom() - d.1.min(left.h), left.w, d.1.min(left.h));
                                    left.h -= s.h;
                                    s
                                }
                                "Right" => {
                                    let s = Rect::new(left.right() - d.0.min(left.w), left.y, d.0.min(left.w), left.h);
                                    left.w -= s.w;
                                    s
                                }
                                _ => {
                                    let s = Rect::new(left.x, left.y, d.0.min(left.w), left.h);
                                    left = Rect::new(left.x + s.w, left.y, left.w - s.w, left.h);
                                    s
                                }
                            }
                        };
                        children.push(self.arrange(c, &p, slot, clip));
                    }
                    AreaKind::Dock { fill }
                }
                "UniformGrid" => {
                    let (cols, rows) = uniform_dims(el);
                    let (cw, ch) = (inner.w / cols as f32, inner.h / rows as f32);
                    let first = el.attr("FirstColumn").and_then(parse_num).map_or(0, |n| n.max(0.0) as usize);
                    for (i, c) in el.children.iter().enumerate() {
                        let k = i + first;
                        let slot = Rect::new(inner.x + (k % cols) as f32 * cw, inner.y + (k / cols) as f32 * ch, cw, ch);
                        children.push(self.arrange(c, &child_path(path, i), slot, clip));
                    }
                    AreaKind::Overlay
                }
                "Grid" => {
                    let (cols, rows) = self.grid_tracks(el, path, (inner.w, inner.h));
                    let edges = |start: f32, tracks: &[f32]| -> Vec<f32> {
                        let mut e = vec![start];
                        for t in tracks {
                            e.push(e.last().copied().unwrap_or(start) + t);
                        }
                        e
                    };
                    let (cx, ry) = (edges(inner.x, &cols), edges(inner.y, &rows));
                    for (i, c) in el.children.iter().enumerate() {
                        let span = |at: &str, sp: &str, n: usize| {
                            let i = c.attr(at).and_then(parse_num).map_or(0, |v| v.max(0.0) as usize).min(n - 1);
                            let s = c.attr(sp).and_then(parse_num).map_or(1, |v| v.max(1.0) as usize).min(n - i);
                            (i, s)
                        };
                        let (ci, cs) = span("Grid.Column", "Grid.ColumnSpan", cols.len());
                        let (ri, rs) = span("Grid.Row", "Grid.RowSpan", rows.len());
                        let slot = Rect::new(cx[ci], ry[ri], cx[ci + cs] - cx[ci], ry[ri + rs] - ry[ri]);
                        children.push(self.arrange(c, &child_path(path, i), slot, clip));
                    }
                    AreaKind::Grid { cols: cx, rows: ry }
                }
                "Canvas" => {
                    for (i, c) in el.children.iter().enumerate() {
                        let p = child_path(path, i);
                        let d = self.desired(&p);
                        let num = |a: &str| c.attr(a).and_then(parse_num);
                        let x = num("Canvas.Left").map(|l| inner.x + l).or_else(|| num("Canvas.Right").map(|r| inner.right() - r - d.0)).unwrap_or(inner.x);
                        let y = num("Canvas.Top").map(|t| inner.y + t).or_else(|| num("Canvas.Bottom").map(|b| inner.bottom() - b - d.1)).unwrap_or(inner.y);
                        children.push(self.arrange(c, &p, Rect::new(x, y, d.0, d.1), clip));
                    }
                    AreaKind::Canvas
                }
                _ => {
                    for (i, c) in el.children.iter().enumerate() {
                        children.push(self.arrange(c, &child_path(path, i), inner, clip));
                    }
                    AreaKind::Overlay
                }
            },
        };
        self.out.areas.push(Area { path: path.to_vec(), rect: inner, kind, children });
    }

    /// A content control's child in its content area, aligned as HorizontalContentAlignment says.
    fn arrange_content(&mut self, parent: &Element, c: &Element, path: &[usize], area: Rect, clip: Rect) -> Option<Rect> {
        let centered = matches!(parent.local(), "Button" | "ToggleButton" | "RepeatButton");
        let hca = parent.attr("HorizontalContentAlignment").unwrap_or(if centered { "Center" } else { "Stretch" });
        let vca = parent.attr("VerticalContentAlignment").unwrap_or(if centered || matches!(parent.local(), "CheckBox" | "RadioButton") { "Center" } else { "Stretch" });
        let d = self.desired(path);
        let w = if hca == "Stretch" { area.w } else { d.0.min(area.w) };
        let h = if vca == "Stretch" { area.h } else { d.1.min(area.h) };
        let x = match hca {
            "Left" | "Stretch" => area.x,
            "Right" => area.right() - w,
            _ => area.x + (area.w - w) / 2.0,
        };
        let y = match vca {
            "Top" | "Stretch" => area.y,
            "Bottom" => area.bottom() - h,
            _ => area.y + (area.h - h) / 2.0,
        };
        self.arrange(c, path, Rect::new(x, y, w, h), clip)
    }

    fn arrange_items(&mut self, el: &Element, path: &[usize], rect: Rect, inner: Rect, clip: Rect) {
        let mut children = Vec::new();
        match el.local() {
            "TabControl" => {
                let shown = self.shown_tab(el, path);
                let header_h = el.children.iter().enumerate().map(|(i, _)| self.desired(&child_path(path, i)).1).fold(48.0, f32::max);
                let mut x = inner.x;
                for (i, c) in el.children.iter().enumerate() {
                    let p = child_path(path, i);
                    let d = self.desired(&p);
                    let r = self.arrange(c, &p, Rect::new(x, inner.y, d.0, header_h), clip);
                    x += d.0;
                    children.push(r);
                }
                let strip = Rect::new(inner.x, inner.y, inner.w, header_h);
                self.out.areas.push(Area { path: path.to_vec(), rect: strip, kind: AreaKind::Stack { vertical: false }, children });
                // The page shown: its tab item's content, which takes drops for that tab item.
                let page = Rect::new(inner.x, inner.y + header_h, inner.w, (inner.h - header_h).max(0.0)).deflate(padding_of_page());
                if let Some(item) = el.children.get(shown).filter(|c| c.local() == "TabItem" && visible(c)) {
                    let ip = child_path(path, shown);
                    let mut page_children = Vec::new();
                    for (j, c) in item.children.iter().enumerate() {
                        page_children.push(self.arrange(c, &child_path(&ip, j), page, clip));
                    }
                    let occupied = !item.children.is_empty() || item.text.is_some();
                    self.out.areas.push(Area { path: ip, rect: page, kind: AreaKind::Single { occupied }, children: page_children });
                }
                let _ = rect;
            }
            "ComboBox" => {
                // Closed: its items aren't shown.
                self.out.areas.push(Area { path: path.to_vec(), rect: inner, kind: AreaKind::Stack { vertical: true }, children: el.children.iter().map(|_| None).collect() });
            }
            "Menu" => {
                let mut x = inner.x;
                for (i, c) in el.children.iter().enumerate() {
                    let p = child_path(path, i);
                    let d = self.desired(&p);
                    children.push(self.arrange(c, &p, Rect::new(x, inner.y, d.0, inner.h), clip));
                    x += d.0;
                }
                self.out.areas.push(Area { path: path.to_vec(), rect: inner, kind: AreaKind::Stack { vertical: false }, children });
            }
            "MenuItem" => {
                // A submenu stays closed.
            }
            _ => {
                let mut y = inner.y;
                for (i, c) in el.children.iter().enumerate() {
                    let p = child_path(path, i);
                    let d = self.desired(&p);
                    let pad = item_padding(el, c);
                    let slot = Rect::new(inner.x + pad[0], y + pad[1], (inner.w - pad[0] - pad[2]).max(0.0), d.1);
                    let r = self.arrange(c, &p, slot, clip);
                    if r.is_some() {
                        y += d.1 + pad[1] + pad[3];
                    }
                    children.push(r);
                }
                self.out.areas.push(Area { path: path.to_vec(), rect: inner, kind: AreaKind::Stack { vertical: true }, children });
            }
        }
    }
}

/// The space around a tab page's content.
fn padding_of_page() -> [f32; 4] {
    [12.0, 12.0, 12.0, 12.0]
}

/// The padding an item gets from the container Avalonia puts it in (none when it is that container).
fn item_padding(list: &Element, item: &Element) -> [f32; 4] {
    match (list.local(), item.local()) {
        ("ListBox", "ListBoxItem") | ("ComboBox", "ComboBoxItem") => [0.0; 4],
        ("ListBox", _) => [12.0, 9.0, 12.0, 12.0],
        _ => [0.0; 4],
    }
}

fn uniform_dims(el: &Element) -> (usize, usize) {
    let n = el.children.iter().filter(|c| visible(c)).count().max(1);
    let first = el.attr("FirstColumn").and_then(parse_num).map_or(0, |v| v.max(0.0) as usize);
    let rows = el.attr("Rows").and_then(parse_num).map(|v| v.max(0.0) as usize).filter(|v| *v > 0);
    let cols = el.attr("Columns").and_then(parse_num).map(|v| v.max(0.0) as usize).filter(|v| *v > 0);
    match (cols, rows) {
        (Some(c), Some(r)) => (c, r),
        (Some(c), None) => (c, (n + first).div_ceil(c)),
        (None, Some(r)) => ((n + first).div_ceil(r), r),
        (None, None) => {
            let c = ((n + first) as f32).sqrt().ceil() as usize;
            (c.max(1), (n + first).div_ceil(c.max(1)))
        }
    }
}
