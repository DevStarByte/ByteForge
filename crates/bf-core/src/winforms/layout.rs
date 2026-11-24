//! Where the controls of a form are: Location and Size, docking (in Windows Forms' order, from the back of
//! a container to the front), flow and table panels, tab pages and split containers, all in pixels of the
//! form's client area.

use std::collections::HashMap;

use super::{Designer, parse_enum, parse_int, short_type};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn inset(&self, left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(self.x + left, self.y + top, (self.w - left - right).max(0), (self.h - top - bottom).max(0))
    }

    /// The part of `self` inside `other`.
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = (self.x + self.w).min(other.x + other.w);
        let b = (self.y + self.h).min(other.y + other.h);
        Rect::new(x, y, (r - x).max(0), (b - y).max(0))
    }
}

/// A control as laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub name: String,
    /// Its bounds, in the form's client coordinates.
    pub rect: Rect,
    /// What of it shows (its container clips it).
    pub visible: Rect,
    /// The container it is in (`None`: the form itself).
    pub parent: Option<String>,
    /// The part of its container (`Panel1`), or empty.
    pub slot: String,
    pub depth: usize,
    /// Docked, or placed by a flow or table panel: it can't be dragged to a position.
    pub arranged: bool,
}

/// A container's area for its controls, in the form's client coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub owner: String,
    pub slot: String,
    /// Where its controls' Location (0, 0) is.
    pub origin: (i32, i32),
    /// Where its controls show (and docked ones go).
    pub bounds: Rect,
    pub depth: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// Back to front: containers before what is in them.
    pub controls: Vec<Placed>,
    /// Every place controls can go: the form's client area first, then containers (and their parts).
    pub areas: Vec<Area>,
}

impl Layout {
    pub fn get(&self, name: &str) -> Option<&Placed> {
        self.controls.iter().find(|p| p.name == name)
    }

    /// The frontmost control at a point.
    pub fn hit(&self, x: i32, y: i32) -> Option<&Placed> {
        self.controls.iter().rev().find(|p| p.visible.contains(x, y))
    }

    /// The innermost place at a point a control could be dropped, leaving out `moving` and what is in them.
    pub fn area_at(&self, d: &Designer, x: i32, y: i32, moving: &[String]) -> Option<&Area> {
        self.areas
            .iter()
            .filter(|a| a.bounds.contains(x, y) && !moving.iter().any(|m| d.is_within(&a.owner, m)))
            // A container hidden under another doesn't take the drop.
            .filter(|a| d.is_root(&a.owner) || self.hit_ignoring(x, y, moving).is_some_and(|h| d.is_within(&h.name, &a.owner)))
            .max_by_key(|a| a.depth)
    }

    fn hit_ignoring(&self, x: i32, y: i32, moving: &[String]) -> Option<&Placed> {
        self.controls.iter().rev().find(|p| p.visible.contains(x, y) && !moving.iter().any(|m| p.name == *m || p.parent.as_deref() == Some(m.as_str())))
    }

    pub fn area(&self, owner: &str, slot: &str) -> Option<&Area> {
        self.areas.iter().find(|a| a.owner == owner && a.slot == slot)
    }
}

/// Lays out the form. `selected_tabs` says which page each TabControl shows (else its SelectedIndex).
pub fn layout(d: &Designer, selected_tabs: &HashMap<String, usize>) -> Layout {
    let mut out = Layout::default();
    let root = &d.root().name;
    let (w, h) = d.size(root);
    let client = Rect::new(0, 0, w, h);
    out.areas.push(Area { owner: root.clone(), slot: String::new(), origin: (0, 0), bounds: client, depth: 0 });
    place_children(d, root, "", (0, 0), client, client, 0, selected_tabs, &mut out);
    out
}

fn border(d: &Designer, name: &str) -> i32 {
    match d.prop(name, "BorderStyle").and_then(parse_enum).as_deref() {
        Some("FixedSingle") => 1,
        Some("Fixed3D") => 2,
        _ => 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn place_children(d: &Designer, owner: &str, slot: &str, origin: (i32, i32), area: Rect, clip: Rect, depth: usize, tabs: &HashMap<String, usize>, out: &mut Layout) {
    let Some(c) = d.get(owner) else { return };
    let owner_type = c.short_type();
    let children: Vec<&super::Child> = c.children.iter().filter(|ch| ch.slot == slot).collect();
    let mut rects: Vec<(usize, Rect, bool)> = Vec::new();
    match owner_type {
        "FlowLayoutPanel" => {
            let down = matches!(d.prop(owner, "FlowDirection").and_then(parse_enum).as_deref(), Some("TopDown") | Some("BottomUp"));
            let wrap = d.prop(owner, "WrapContents").is_none_or(|v| v.trim() != "false");
            let (mut x, mut y, mut line) = (area.x, area.y, 0);
            // Flow panels lay out in collection order: the first control comes first.
            for (i, ch) in children.iter().enumerate() {
                let (w, h) = d.size(&ch.name);
                let (mw, mh) = (w + 6, h + 6);
                if down {
                    if wrap && y > area.y && y + mh > area.y + area.h {
                        y = area.y;
                        x += line;
                        line = 0;
                    }
                    rects.push((i, Rect::new(x + 3, y + 3, w, h), true));
                    y += mh;
                    line = line.max(mw);
                } else {
                    if wrap && x > area.x && x + mw > area.x + area.w {
                        x = area.x;
                        y += line;
                        line = 0;
                    }
                    rects.push((i, Rect::new(x + 3, y + 3, w, h), true));
                    x += mw;
                    line = line.max(mh);
                }
            }
        }
        "TableLayoutPanel" => {
            let cols = d.prop(owner, "ColumnCount").and_then(parse_int).unwrap_or(1).max(1) as i32;
            let rows = d.prop(owner, "RowCount").and_then(parse_int).unwrap_or(1).max(1) as i32;
            let (cw, rh) = (area.w / cols, area.h / rows);
            let mut next = 0;
            for (i, ch) in children.iter().enumerate() {
                let cell = super::lex::split_args(ch.args.trim_start_matches(',')).iter().map(|a| parse_int(a).map(|n| n as i32)).collect::<Option<Vec<_>>>();
                let (col, row) = match cell.as_deref() {
                    Some([c, r]) => (*c, *r),
                    _ => {
                        let at = next;
                        next += 1;
                        (at % cols, at / cols)
                    }
                };
                let (w, h) = d.size(&ch.name);
                let full = d.dock(&ch.name) == "Fill";
                let (w, h) = if full { (cw - 6, rh - 6) } else { (w.min(cw - 6), h.min(rh - 6)) };
                rects.push((i, Rect::new(area.x + col * cw + 3, area.y + row * rh + 3, w, h), true));
            }
        }
        "TabControl" => {
            let selected = tabs.get(owner).copied().or_else(|| d.prop(owner, "SelectedIndex").and_then(parse_int).map(|n| n.max(0) as usize)).unwrap_or(0);
            let page = area.inset(4, 24, 4, 4);
            if let Some((i, _)) = children.iter().enumerate().nth(selected.min(children.len().saturating_sub(1))) {
                rects.push((i, page, true));
            }
        }
        _ => {
            // Docking goes from the back of the collection to the front.
            let mut free = area;
            for (i, ch) in children.iter().enumerate().rev() {
                let (w, h) = d.size(&ch.name);
                let rect = match d.dock(&ch.name).as_str() {
                    "Top" => {
                        let r = Rect::new(free.x, free.y, free.w, h);
                        free = free.inset(0, h, 0, 0);
                        r
                    }
                    "Bottom" => {
                        let r = Rect::new(free.x, free.y + free.h - h, free.w, h);
                        free = free.inset(0, 0, 0, h);
                        r
                    }
                    "Left" => {
                        let r = Rect::new(free.x, free.y, w, free.h);
                        free = free.inset(w, 0, 0, 0);
                        r
                    }
                    "Right" => {
                        let r = Rect::new(free.x + free.w - w, free.y, w, free.h);
                        free = free.inset(0, 0, w, 0);
                        r
                    }
                    "Fill" => free,
                    _ => {
                        let (x, y) = d.location(&ch.name);
                        rects.push((i, Rect::new(origin.0 + x, origin.1 + y, w, h), false));
                        continue;
                    }
                };
                rects.push((i, rect, true));
            }
        }
    }
    // Back to front: the last control in the collection is drawn first.
    rects.sort_by_key(|(i, _, _)| std::cmp::Reverse(*i));
    for (i, rect, arranged) in rects {
        let ch = children[i];
        let visible = rect.intersect(&clip);
        out.controls.push(Placed { name: ch.name.clone(), rect, visible, parent: Some(owner.to_owned()).filter(|o| !d.is_root(o)), slot: slot.to_owned(), depth: depth + 1, arranged });
        let Some(child) = d.get(&ch.name) else { continue };
        let ty = short_type(&child.type_name);
        let is_container = super::info(&child.type_name).is_some_and(|i| i.container) || !child.children.is_empty();
        if !is_container {
            continue;
        }
        if ty == "SplitContainer" {
            let horizontal = d.prop(&ch.name, "Orientation").and_then(parse_enum).as_deref() == Some("Horizontal");
            let size = if horizontal { rect.h } else { rect.w };
            let dist = d.prop(&ch.name, "SplitterDistance").and_then(parse_int).map_or(size / 2, |n| n as i32).clamp(0, size);
            let (one, two) = if horizontal {
                (Rect::new(rect.x, rect.y, rect.w, dist), Rect::new(rect.x, rect.y + dist + 4, rect.w, (rect.h - dist - 4).max(0)))
            } else {
                (Rect::new(rect.x, rect.y, dist, rect.h), Rect::new(rect.x + dist + 4, rect.y, (rect.w - dist - 4).max(0), rect.h))
            };
            for (part, r) in [("Panel1", one), ("Panel2", two)] {
                let clip = r.intersect(&visible);
                out.areas.push(Area { owner: ch.name.clone(), slot: part.into(), origin: (r.x, r.y), bounds: clip, depth: depth + 1 });
                place_children(d, &ch.name, part, (r.x, r.y), r, clip, depth + 1, tabs, out);
            }
            continue;
        }
        let b = border(d, &ch.name);
        let inner = rect.inset(b, b, b, b);
        // A group box's caption takes the top of it; its controls are placed from its top left corner.
        let (origin, dock_area) = match ty {
            "GroupBox" => ((rect.x, rect.y), rect.inset(3, 19, 3, 3)),
            "TabControl" => ((rect.x, rect.y), rect),
            _ => ((inner.x, inner.y), inner),
        };
        let clip = inner.intersect(&visible);
        if ty != "TabControl" {
            out.areas.push(Area { owner: ch.name.clone(), slot: String::new(), origin, bounds: clip, depth: depth + 1 });
        }
        place_children(d, &ch.name, "", origin, dock_area, clip, depth + 1, tabs, out);
    }
}

/// A snap line drawn while dragging: from (x0, y0) to (x1, y1) in the form's client coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// The recommended space between controls or to the container's edge, not an alignment.
    pub margin: bool,
}

/// Which edges of a rectangle are being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Edges {
    pub const ALL: Edges = Edges { left: true, top: true, right: true, bottom: true };
}

/// The space Visual Studio's snap lines keep between two controls (their Margin of 3 each).
pub const SNAP_SPACING: i32 = 6;

fn overlaps(a0: i32, a1: i32, b0: i32, b1: i32) -> bool {
    a0 < b1 && b0 < a1
}

/// The smallest move (within `threshold`) that puts one of `moving` on one of `targets`.
fn nearest(moving: &[i32], targets: &[i32], threshold: i32) -> i32 {
    let mut best: Option<i32> = None;
    for &m in moving {
        for &t in targets {
            let d = t - m;
            if d.abs() <= threshold && best.is_none_or(|b| d.abs() < b.abs()) {
                best = Some(d);
            }
        }
    }
    best.unwrap_or(0)
}

/// Where `rect`'s dragged edges snap to: the same edge of another control, the recommended space next to
/// it, or the recommended distance to the container's edge (`padding`), whichever is nearest within
/// `threshold` on each axis. Returns how far to move them and the snap lines of the result.
pub fn snap(rect: Rect, edges: Edges, others: &[Rect], area: Rect, padding: i32, threshold: i32) -> (i32, i32, Vec<Guide>) {
    let pick = |on: [(bool, i32); 2]| on.iter().filter(|(on, _)| *on).map(|(_, v)| *v).collect::<Vec<i32>>();
    let xs_moving = pick([(edges.left, rect.x), (edges.right, rect.x + rect.w)]);
    let ys_moving = pick([(edges.top, rect.y), (edges.bottom, rect.y + rect.h)]);
    let mut xs = vec![area.x + padding, area.x + area.w - padding];
    let mut ys = vec![area.y + padding, area.y + area.h - padding];
    for o in others {
        xs.extend([o.x, o.x + o.w]);
        ys.extend([o.y, o.y + o.h]);
        if overlaps(rect.y, rect.y + rect.h, o.y, o.y + o.h) {
            xs.extend([o.x + o.w + SNAP_SPACING, o.x - SNAP_SPACING]);
        }
        if overlaps(rect.x, rect.x + rect.w, o.x, o.x + o.w) {
            ys.extend([o.y + o.h + SNAP_SPACING, o.y - SNAP_SPACING]);
        }
    }
    let dx = nearest(&xs_moving, &xs, threshold);
    let dy = nearest(&ys_moving, &ys, threshold);
    // The lines of where it ends up.
    let l = rect.x + if edges.left { dx } else { 0 };
    let t = rect.y + if edges.top { dy } else { 0 };
    let rt = rect.x + rect.w + if edges.right { dx } else { 0 };
    let b = rect.y + rect.h + if edges.bottom { dy } else { 0 };
    let mut guides: Vec<Guide> = Vec::new();
    let mut push = |g: Guide| {
        if !guides.contains(&g) {
            guides.push(g);
        }
    };
    for o in others {
        let (ol, ot, or, ob) = (o.x, o.y, o.x + o.w, o.y + o.h);
        for (x, on) in [(l, edges.left), (rt, edges.right)] {
            if on && (x == ol || x == or) {
                push(Guide { x0: x, y0: t.min(ot), x1: x, y1: b.max(ob), margin: false });
            }
        }
        for (y, on) in [(t, edges.top), (b, edges.bottom)] {
            if on && (y == ot || y == ob) {
                push(Guide { x0: l.min(ol), y0: y, x1: rt.max(or), y1: y, margin: false });
            }
        }
        if overlaps(t, b, ot, ob) {
            let y = (t.max(ot) + b.min(ob)) / 2;
            if edges.left && l == or + SNAP_SPACING {
                push(Guide { x0: or, y0: y, x1: l, y1: y, margin: true });
            }
            if edges.right && rt == ol - SNAP_SPACING {
                push(Guide { x0: rt, y0: y, x1: ol, y1: y, margin: true });
            }
        }
        if overlaps(l, rt, ol, or) {
            let x = (l.max(ol) + rt.min(or)) / 2;
            if edges.top && t == ob + SNAP_SPACING {
                push(Guide { x0: x, y0: ob, x1: x, y1: t, margin: true });
            }
            if edges.bottom && b == ot - SNAP_SPACING {
                push(Guide { x0: x, y0: b, x1: x, y1: ot, margin: true });
            }
        }
    }
    let (my, mx) = ((t + b) / 2, (l + rt) / 2);
    if edges.left && l == area.x + padding {
        push(Guide { x0: area.x, y0: my, x1: l, y1: my, margin: true });
    }
    if edges.right && rt == area.x + area.w - padding {
        push(Guide { x0: rt, y0: my, x1: area.x + area.w, y1: my, margin: true });
    }
    if edges.top && t == area.y + padding {
        push(Guide { x0: mx, y0: area.y, x1: mx, y1: t, margin: true });
    }
    if edges.bottom && b == area.y + area.h - padding {
        push(Guide { x0: mx, y0: b, x1: mx, y1: area.y + area.h, margin: true });
    }
    (dx, dy, guides)
}

/// How to line controls up with the primary one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Lefts,
    Centers,
    Rights,
    Tops,
    Middles,
    Bottoms,
    SameWidth,
    SameHeight,
    SameSize,
}

/// `r` lined up with `primary`.
pub fn aligned(primary: Rect, r: Rect, how: Align) -> Rect {
    let mut out = r;
    match how {
        Align::Lefts => out.x = primary.x,
        Align::Centers => out.x = primary.x + primary.w / 2 - r.w / 2,
        Align::Rights => out.x = primary.x + primary.w - r.w,
        Align::Tops => out.y = primary.y,
        Align::Middles => out.y = primary.y + primary.h / 2 - r.h / 2,
        Align::Bottoms => out.y = primary.y + primary.h - r.h,
        Align::SameWidth => out.w = primary.w,
        Align::SameHeight => out.h = primary.h,
        Align::SameSize => {
            out.w = primary.w;
            out.h = primary.h;
        }
    }
    out
}

/// The distance Visual Studio keeps from a container's edge: 12 on a form, 3 elsewhere.
pub fn snap_padding(d: &Designer, owner: &str) -> i32 {
    if d.is_root(owner) { 12 } else { 3 }
}
