//! Paints the editor: only visible lines are shaped (and cached), so frame cost is independent of file size.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use bf_text::{Point as TPoint, visual_col};
use gpui::{
    App, Bounds, Corners, ElementId, ElementInputHandler, Entity, FontStyle, FontWeight, GlobalElementId, Hsla, LayoutId,
    Pixels, ShapedLine, SharedString, Style, TextRun, UnderlineStyle, Window, fill, outline, point, prelude::*, px, quad,
    relative, size,
};

use super::{EditorView, Layout};
use crate::theme::{CaretStyle, LineHighlight, Theme, Whitespace};

pub struct EditorElement {
    view: Entity<EditorView>,
}

impl EditorElement {
    pub fn new(view: Entity<EditorView>) -> Self {
        Self { view }
    }
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

const MAX_SHAPED_COLS: usize = 4000;

fn is_ignored(kind: Option<&str>) -> bool {
    kind.is_some_and(|k| k.starts_with("string") || k.contains("comment") || k.starts_with("excluded"))
}

/// Bracket depth change across a line, skipping `//` comments and string literals (lexical heuristic).
fn bracket_delta(line: &str) -> i32 {
    let mut depth = 0;
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '/' if chars.peek() == Some(&'/') => break,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

impl EditorView {
    fn depth_at(&mut self, line: usize) -> usize {
        if self.depth_cache.len() > self.state.buffer.line_count() + 1 {
            self.depth_cache.truncate(self.state.buffer.line_count() + 1);
        }
        if self.depth_cache.is_empty() {
            self.depth_cache.push(0);
        }
        while self.depth_cache.len() <= line {
            let prev = self.depth_cache.len() - 1;
            let d = (self.depth_cache[prev] + bracket_delta(&self.state.buffer.line(prev))).max(0);
            self.depth_cache.push(d);
        }
        self.depth_cache[line] as usize
    }

    fn find_bracket_match(&self) -> Option<(TPoint, Option<TPoint>)> {
        let caret = self.state.caret();
        let line: Vec<char> = self.state.buffer.line(caret.line).chars().collect();
        let is_bracket = |c: char| matches!(c, '(' | ')' | '[' | ']' | '{' | '}');
        let col = if caret.col < line.len() && is_bracket(line[caret.col]) {
            caret.col
        } else if caret.col > 0 && caret.col <= line.len() && is_bracket(line[caret.col - 1]) {
            caret.col - 1
        } else {
            return None;
        };
        let open = line[col];
        let (close, dir): (char, isize) = match open {
            '(' => (')', 1),
            '[' => (']', 1),
            '{' => ('}', 1),
            ')' => ('(', -1),
            ']' => ('[', -1),
            _ => ('{', -1),
        };
        let mut depth = 0;
        let mut l = caret.line as isize;
        let mut c = col as isize;
        for _ in 0..5000 {
            let text: Vec<char> = self.state.buffer.line(l as usize).chars().collect();
            while c >= 0 && (c as usize) < text.len() {
                let ch = text[c as usize];
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((TPoint::new(caret.line, col), Some(TPoint::new(l as usize, c as usize))));
                    }
                }
                c += dir;
            }
            l += dir;
            if l < 0 || l as usize >= self.state.buffer.line_count() {
                break;
            }
            c = if dir > 0 { 0 } else { self.state.buffer.line_len(l as usize) as isize - 1 };
        }
        Some((TPoint::new(caret.line, col), None))
    }
}

pub struct Prepaint {
    lines: Vec<(usize, f32, ShapedLine)>,
    numbers: Vec<(f32, ShapedLine, f32)>,
    /// Char ranges of file references per visible line.
    links: Vec<(usize, Vec<(usize, usize)>)>,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let theme = Theme::global(cx);
        let style = theme.editor.clone();
        let editor_font = gpui::Font { weight: style.font_weight, ..gpui::font(theme.editor_font.clone()) };
        let font_size = px(style.font_size);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&editor_font);
        let char_width = text_system.advance(font_id, font_size, 'm').map(|s| f32::from(s.width)).unwrap_or(8.0).max(1.0);
        let line_height = (style.font_size * style.line_height).round().max(4.0);
        let number_size = if style.line_number_size > 0.0 { style.line_number_size } else { style.font_size };
        let foreground = theme.c("editor.foreground");

        self.view.update(cx, |view, cx| {
            let theme = Theme::global(cx);
            let digits = (view.state.buffer.line_count().max(1) as f32).log10().floor() as usize + 1;
            let glyph = if view.glyph_margin { theme.n("editorGutter.glyphMarginWidth", line_height).round() } else { 0.0 };
            let fold_width = if view.folding_enabled && view.show_line_numbers { (char_width * 1.6).round().max(12.0) } else { 0.0 };
            let gutter = glyph
                + fold_width
                + if view.show_line_numbers {
                    (digits.max(3) as f32 * char_width * number_size / style.font_size).ceil() + style.gutter_left + style.gutter_right
                } else {
                    style.gutter_left.max(4.0)
                };
            view.layout = Layout {
                left: f32::from(bounds.left()),
                top: f32::from(bounds.top()),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                gutter,
                text_left: f32::from(bounds.left()) + gutter + style.text_padding,
                char_width,
                line_height,
                glyph,
                // The chevrons sit between the line numbers and the git change markers.
                fold_left: f32::from(bounds.left()) + gutter - fold_width - style.gutter_right.min(6.0),
                fold_width,
                minimap: if view.minimap_enabled && f32::from(bounds.size.width) > 500.0 { theme.n("minimap.width", 80.0).round() } else { 0.0 },
            };
            if fold_width > 0.0 {
                // Cached per buffer version.
                view.fold_regions();
            }
            view.take_pending_reveal();
            view.scroll = view.clamp_scroll(view.scroll);
            view.scroll_target = view.clamp_scroll(view.scroll_target);
            view.refresh_minimap(&theme);

            // Smooth scrolling + gliding caret share one frame-driven animation step.
            let now = Instant::now();
            let dt = view.last_frame.map(|t| (now - t).as_secs_f32().clamp(0.001, 0.1)).unwrap_or(1.0 / 60.0);
            view.last_frame = Some(now);
            let k = 1.0 - (-dt / style.animation_speed.max(0.001)).exp();
            let mut animating = false;
            let (dx, dy) = (view.scroll_target.0 - view.scroll.0, view.scroll_target.1 - view.scroll.1);
            if dx.abs() > 0.5 || dy.abs() > 0.5 {
                view.scroll = (view.scroll.0 + dx * k, view.scroll.1 + dy * k);
                animating = true;
                view.on_viewport_changed(cx);
            } else {
                view.scroll = view.scroll_target;
            }
            let caret = view.state.caret();
            let caret_line = view.state.buffer.line(caret.line);
            let caret_visual = visual_col(&caret_line, caret.col, view.state.tab_size) + view.inlay_shift(caret.line, caret.col, false);
            let target = (caret_visual as f32 * char_width, view.folds.row_of(caret.line) as f32 * line_height);
            if style.smooth_caret {
                let drawn = view.caret_drawn.unwrap_or(target);
                let (cx_, cy_) = (target.0 - drawn.0, target.1 - drawn.1);
                if (cx_.abs() > 0.4 || cy_.abs() > 0.4) && cy_.abs() < line_height * 40.0 {
                    let kc = 1.0 - (-dt / (style.animation_speed * 0.6).max(0.001)).exp();
                    view.caret_drawn = Some((drawn.0 + cx_ * kc, drawn.1 + cy_ * kc));
                    animating = true;
                } else {
                    view.caret_drawn = Some(target);
                }
            } else {
                view.caret_drawn = Some(target);
            }
            if animating {
                window.request_animation_frame();
            } else {
                view.last_frame = None;
            }
            if (view.state.caret(), view.state.buffer.version()) != view.bracket_key {
                view.bracket_key = (view.state.caret(), view.state.buffer.version());
                view.bracket_match = if theme.editor.match_brackets { view.find_bracket_match() } else { None };
            }
        });

        // Shape visible lines (cached by content + colors).
        let (first, last) = self.view.read(cx).visible_lines();
        let mut lines = Vec::new();
        let mut numbers = Vec::new();
        let bracket_colors = theme.bracket_colors();
        let use_brackets = style.bracket_colors && !bracket_colors.is_empty();
        let unexpected = theme.c("editorBracketHighlight.unexpectedBracket.foreground");
        let number_color = theme.c("editorLineNumber.foreground");
        let active_number_color = theme.c("editorLineNumber.activeForeground");
        let theme_version = theme.version;

        // Collect token styles up front (avoid borrowing the theme inside the view update).
        let token_style = |kind: Option<&str>| -> (Hsla, bool, bool, bool) {
            match kind.and_then(|k| theme.token(k)) {
                Some(t) => (t.color, t.bold, t.italic, t.underline),
                None => (foreground, false, false, false),
            }
        };

        let inlay_color = theme.c_or("editorInlayHint.foreground", 0x969696ff);
        let inlay_font = gpui::Font { style: FontStyle::Italic, ..editor_font.clone() };
        let inlay_run = |len: usize| TextRun { len, font: inlay_font.clone(), color: inlay_color, background_color: None, underline: None, strikethrough: None };

        let mut shaped_inputs: Vec<(usize, String, Vec<TextRun>, u64)> = Vec::new();
        {
            let view = self.view.read(cx);
            let tab = view.state.tab_size;
            for line_ix in first..=last {
                if view.folds.is_hidden(line_ix) {
                    continue;
                }
                let text = view.state.buffer.line(line_ix);
                if text.is_empty() {
                    continue;
                }
                let kinds = view.line_kinds(line_ix, text.len());
                let mut display = String::with_capacity(text.len());
                let mut runs: Vec<TextRun> = Vec::new();
                let mut hasher = DefaultHasher::new();
                theme_version.hash(&mut hasher);
                let mut v = 0usize;
                let mut depth = 0usize; // filled below for brackets
                let _ = &mut depth;
                let inlays = view.inlays_on(line_ix);
                let mut next_inlay = 0;
                for (col, (byte, ch)) in text.char_indices().enumerate() {
                    if col >= MAX_SHAPED_COLS {
                        break;
                    }
                    // Inlay hints go before the character they are attached to.
                    while let Some(hint) = inlays.get(next_inlay).filter(|h| h.at.col <= col) {
                        display.push_str(&hint.text);
                        runs.push(inlay_run(hint.text.len()));
                        next_inlay += 1;
                    }
                    let kind = kinds.get(byte).copied().flatten();
                    let (mut color, bold, italic, underline) = token_style(kind);
                    if use_brackets && matches!(ch, '(' | ')' | '[' | ']' | '{' | '}') && !is_ignored(kind) {
                        color = Hsla::default(); // placeholder, resolved after depth scan
                    }
                    let piece_len;
                    if ch == '\t' {
                        let next = (v / tab + 1) * tab;
                        for _ in v..next {
                            display.push(' ');
                        }
                        piece_len = next - v;
                        v = next;
                    } else {
                        display.push(ch);
                        piece_len = ch.len_utf8();
                        v += 1;
                    }
                    let font = gpui::Font {
                        weight: if bold { FontWeight::BOLD } else { style.font_weight },
                        style: if italic { FontStyle::Italic } else { FontStyle::Normal },
                        ..editor_font.clone()
                    };
                    let underline = underline.then(|| UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false });
                    match runs.last_mut() {
                        Some(last) if last.color == color && last.font == font && last.underline == underline => last.len += piece_len,
                        _ => runs.push(TextRun { len: piece_len, font, color, background_color: None, underline, strikethrough: None }),
                    }
                }
                // Hints at the end of the line (after the last character).
                for hint in &inlays[next_inlay..] {
                    display.push_str(&hint.text);
                    runs.push(inlay_run(hint.text.len()));
                }
                text.hash(&mut hasher);
                kinds.hash(&mut hasher);
                inlays.hash(&mut hasher);
                shaped_inputs.push((line_ix, display, runs, hasher.finish()));
            }
        }

        // Resolve bracket colors (needs mutable depth cache).
        if use_brackets {
            self.view.update(cx, |view, _| {
                for (line_ix, display, runs, hash) in &mut shaped_inputs {
                    let mut depth = view.depth_at(*line_ix);
                    depth.hash(&mut DefaultHasher::new());
                    *hash ^= (depth as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                    // Walk runs and characters together, splitting runs at brackets.
                    let mut new_runs = Vec::with_capacity(runs.len());
                    let mut offset = 0;
                    for run in runs.drain(..) {
                        if run.color != Hsla::default() {
                            offset += run.len;
                            new_runs.push(run);
                            continue;
                        }
                        let piece = &display[offset..offset + run.len];
                        for ch in piece.chars() {
                            let color = match ch {
                                '(' | '[' | '{' => {
                                    let c = bracket_colors[depth % bracket_colors.len()];
                                    depth += 1;
                                    c
                                }
                                _ if depth == 0 => unexpected,
                                _ => {
                                    depth -= 1;
                                    bracket_colors[depth % bracket_colors.len()]
                                }
                            };
                            new_runs.push(TextRun { len: ch.len_utf8(), color, ..run.clone() });
                        }
                        offset += run.len;
                    }
                    *runs = new_runs;
                }
            });
        }

        let view_cache_len = self.view.read(cx).line_cache.len();
        for (line_ix, display, runs, hash) in shaped_inputs {
            let cached = self.view.read(cx).line_cache.get(&hash).cloned();
            let shaped = match cached {
                Some(s) => s,
                None => {
                    let s = text_system.shape_line(SharedString::from(display), font_size, &runs, None);
                    self.view.update(cx, |view, _| {
                        view.line_cache.insert(hash, s.clone());
                    });
                    s
                }
            };
            lines.push((line_ix, 0.0, shaped));
        }
        if view_cache_len > 4000 {
            self.view.update(cx, |view, _| view.line_cache.clear());
        }

        // File references to underline.
        let links: Vec<(usize, Vec<(usize, usize)>)> = self.view.update(cx, |view, _| {
            if !view.detect_links {
                return Vec::new();
            }
            (first..=last.min(view.state.buffer.line_count().saturating_sub(1)))
                .filter_map(|line| {
                    let spans: Vec<_> = view.links_in(line).iter().map(|l| (l.start, l.end)).collect();
                    (!spans.is_empty()).then_some((line, spans))
                })
                .collect()
        });

        // Line numbers.
        let (show_numbers, caret_line, hidden) = {
            let v = self.view.read(cx);
            (v.show_line_numbers, v.state.caret().line, v.folds.hidden_ranges().to_vec())
        };
        if show_numbers {
            let number_font = editor_font.clone();
            for line_ix in first..=last {
                if hidden.iter().any(|&(s, e)| s <= line_ix && line_ix <= e) {
                    continue;
                }
                let active = line_ix == caret_line;
                let label = (line_ix + 1).to_string();
                let run = TextRun {
                    len: label.len(),
                    font: number_font.clone(),
                    color: if active { active_number_color } else { number_color },
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = text_system.shape_line(SharedString::from(label), px(number_size), &[run], None);
                let width = f32::from(shaped.width);
                numbers.push((line_ix as f32, shaped, width));
            }
        }

        Prepaint { lines, numbers, links }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = Snap::take(&self.view, cx);
        let focus = view.focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        let focused = focus.is_focused(window);

        let theme = Theme::global(cx);
        let style = theme.editor.clone();
        let l = view.layout;
        let (sx, sy) = (view.scroll.0.round(), view.scroll.1.round());
        let lh = l.line_height;
        let cw = l.char_width;
        let tab = view.tab;
        let origin_x = l.text_left - sx;
        let gutter_right = l.left + l.gutter;
        let line_top = |line: usize| px(l.top + view.row_of(line) as f32 * lh - sy);
        // Where the character at `col` is drawn (after any inlay hint attached there), and where a range ending
        // at `col` ends (before such a hint).
        let x_at = |line: usize, col: usize| -> f32 { origin_x + (visual_col(view.line(line), col, tab) + view.inlay_shift(line, col, true)) as f32 * cw };
        let x_end = |line: usize, col: usize| -> f32 { origin_x + (visual_col(view.line(line), col, tab) + view.inlay_shift(line, col, false)) as f32 * cw };
        let (first, last) = (view.first, view.last);
        let width = bounds.size.width;

        // Each area is painted exactly once so translucent themes stay translucent (alphas don't stack).
        window.paint_quad(fill(Bounds::new(point(px(gutter_right), bounds.top()), size(width - px(l.gutter), bounds.size.height)), theme.bg("editor.background")));
        let text_clip = Bounds::new(point(px(gutter_right), bounds.top()), size(px(l.text_width() + (l.text_left - gutter_right)), bounds.size.height));

        window.with_content_mask(Some(gpui::ContentMask { bounds: text_clip }), |window| {
            let caret = view.caret;
            let has_selection = view.has_selection;
            let highlight = if has_selection { LineHighlight::None } else { style.line_highlight };
            if (first..=last).contains(&caret.line) {
                let row = Bounds::new(point(px(gutter_right), line_top(caret.line)), size(width - px(l.gutter), px(lh)));
                if matches!(highlight, LineHighlight::Fill | LineHighlight::All) {
                    window.paint_quad(fill(row, theme.bg("editor.lineHighlightBackground")));
                }
                if matches!(highlight, LineHighlight::Border | LineHighlight::All) && theme.has("editor.lineHighlightBorder") {
                    window.paint_quad(outline(row, theme.c("editor.lineHighlightBorder"), gpui::BorderStyle::Solid));
                }
            }

            // The line the debugger is paused on.
            if let Some((line, top)) = view.exec_line
                && (first..=last).contains(&line)
            {
                let color = if top {
                    theme.c_or("editor.stackFrameHighlightBackground", 0xffff0033)
                } else {
                    theme.c_or("editor.focusedStackFrameHighlightBackground", 0x7abd7a4d)
                };
                window.paint_quad(fill(Bounds::new(point(px(gutter_right), line_top(line)), size(width - px(l.gutter), px(lh))), color));
            }

            // Whole-line backgrounds (diff view).
            for &(line, key) in &view.line_backgrounds {
                window.paint_quad(fill(Bounds::new(point(px(gutter_right), line_top(line)), size(width - px(l.gutter), px(lh))), theme.bg(key)));
            }

            // Rulers.
            if theme.has("editorRuler.foreground") {
                for &col in &style.rulers {
                    let x = (origin_x + col as f32 * cw).round();
                    window.paint_quad(fill(Bounds::new(point(px(x), bounds.top()), size(px(1.), bounds.size.height)), theme.c("editorRuler.foreground")));
                }
            }

            // Ranges: find matches, selection, bracket match.
            let paint_range = |window: &mut Window, s: TPoint, e: TPoint, bg: gpui::Background, border: Option<Hsla>, extend: bool| {
                for line in s.line.max(first)..=e.line.min(last) {
                    if view.is_hidden(line) {
                        continue;
                    }
                    let len = view.line_len(line);
                    let c0 = if line == s.line { s.col } else { 0 };
                    let c1 = if line == e.line { e.col } else { len };
                    let x0 = x_at(line, c0);
                    let x1 = x_end(line, c1) + if extend && line != e.line { cw * 0.6 } else { 0.0 };
                    if x1 <= x0 {
                        continue;
                    }
                    let b = Bounds::new(point(px(x0), line_top(line)), size(px(x1 - x0), px(lh)));
                    let r = px(style.selection_radius);
                    window.paint_quad(quad(b, Corners::all(r), bg, if border.is_some() { 1.0 } else { 0.0 }, border.unwrap_or_default(), gpui::BorderStyle::Solid));
                }
            };
            // Other uses of the symbol under the caret (assignments stronger).
            for h in &view.word_highlights {
                let key = if h.write { "editor.wordHighlightStrongBackground" } else { "editor.wordHighlightBackground" };
                let fallback = if h.write { 0x004972b8 } else { 0x57575780 };
                let border = theme.has("editor.wordHighlightBorder").then(|| theme.c("editor.wordHighlightBorder"));
                let bg = if theme.has(key) { theme.bg(key) } else { theme.c_or(key, fallback).into() };
                paint_range(window, h.start, h.end, bg, border, false);
            }

            // Inlay hint backgrounds (the text is part of the shaped line).
            let inlay_bg = theme.c_or("editorInlayHint.background", 0x8080801f);
            for hint in &view.inlays {
                let x = x_end(hint.at.line, hint.at.col) + hint.pad_left as f32 * cw;
                let rect = Bounds::new(point(px(x - 1.0), line_top(hint.at.line) + px(lh * 0.1)), size(px(hint.label_len as f32 * cw + 2.0), px(lh * 0.8)));
                window.paint_quad(quad(rect, Corners::all(px(3.)), inlay_bg, 0.0, Hsla::default(), gpui::BorderStyle::Solid));
            }

            let find_border = theme.has("editor.findMatchBorder").then(|| theme.c("editor.findMatchBorder"));
            for &(s, e) in &view.find[..] {
                if e.line < first || s.line > last {
                    continue;
                }
                paint_range(window, s, e, theme.bg("editor.findMatchBackground"), find_border, false);
            }
            if has_selection {
                let (s, e) = view.selection;
                let key = if focused { "editor.selectionBackground" } else { "editor.inactiveSelectionBackground" };
                let border = theme.has("editor.selectionBorder").then(|| theme.c("editor.selectionBorder"));
                paint_range(window, s, e, theme.bg(key), border, true);
            }
            for &(_, (s, e)) in &view.extra_cursors {
                if s != e {
                    let key = if focused { "editor.selectionBackground" } else { "editor.inactiveSelectionBackground" };
                    let border = theme.has("editor.selectionBorder").then(|| theme.c("editor.selectionBorder"));
                    paint_range(window, s, e, theme.bg(key), border, true);
                }
            }
            if let Some((a, b)) = view.bracket_match {
                for p in [Some(a), b].into_iter().flatten() {
                    if p.line < first || p.line > last {
                        continue;
                    }
                    let rect = Bounds::new(point(px(x_at(p.line, p.col)), line_top(p.line)), size(px(cw), px(lh)));
                    let border = if b.is_some() { theme.c("editorBracketMatch.border") } else { theme.c("editorBracketHighlight.unexpectedBracket.foreground") };
                    window.paint_quad(quad(rect, Corners::all(px(2.)), theme.bg("editorBracketMatch.background"), 1.0, border, gpui::BorderStyle::Solid));
                }
            }

            // Indent guides and whitespace.
            let guide = theme.c("editorIndentGuide.background");
            let ws_color = theme.c("editorWhitespace.foreground");
            for line in first..=last {
                if view.is_hidden(line) {
                    continue;
                }
                let text = view.line(line);
                let top = line_top(line);
                if style.indent_guides && view.show_indent_guides {
                    let lead: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                    if !lead.is_empty() && lead.len() < text.len() {
                        let visual = visual_col(&lead, lead.chars().count(), tab);
                        let mut c = 0;
                        while c < visual {
                            let x = (origin_x + c as f32 * cw).round();
                            window.paint_quad(fill(Bounds::new(point(px(x), top), size(px(1.), px(lh))), guide));
                            c += tab;
                        }
                    }
                }
                if style.whitespace != Whitespace::None {
                    let mut v = 0;
                    for (col, ch) in text.chars().enumerate() {
                        let next = if ch == '\t' { (v / tab + 1) * tab } else { v + 1 };
                        let hints = view.inlay_shift(line, col, true) as f32 * cw;
                        if ch == ' ' {
                            let x = origin_x + hints + (v as f32 + 0.5) * cw - 1.0;
                            window.paint_quad(fill(Bounds::new(point(px(x), top + px(lh / 2.0 - 1.0)), size(px(2.), px(2.))), ws_color));
                        } else if ch == '\t' {
                            let x0 = origin_x + hints + (v as f32 + 0.2) * cw;
                            let x1 = origin_x + hints + (next as f32 - 0.2) * cw;
                            window.paint_quad(fill(Bounds::new(point(px(x0), top + px(lh / 2.0)), size(px(x1 - x0), px(1.))), ws_color));
                        } else if style.whitespace == Whitespace::Leading {
                            break;
                        }
                        v = next;
                    }
                }
            }

            // Text.
            for (line, _, shaped) in &prepaint.lines {
                let y = line_top(*line);
                let _ = shaped.paint(point(px(origin_x), y), px(lh), window, cx);
            }

            // Folded regions: a "⋯" marker after their first line.
            let fold_bg = theme.c_or("editor.foldBackground", 0x264f7840);
            let fold_fg = theme.c_or("editorFoldPlaceholder.foreground", 0x9d9d9dff);
            for &(start, _) in &view.folded {
                if start < first || start > last || view.is_hidden(start) {
                    continue;
                }
                let x = x_end(start, view.line_len(start)) + cw;
                let top = line_top(start);
                let rect = Bounds::new(point(px(x), top + px(lh * 0.15)), size(px(cw * 3.0), px(lh * 0.7)));
                window.paint_quad(quad(rect, Corners::all(px(3.)), fold_bg, 0.0, Hsla::default(), gpui::BorderStyle::Solid));
                let font = gpui::Font { weight: style.font_weight, ..gpui::font(theme.editor_font.clone()) };
                let dots = "⋯";
                let run = TextRun { len: dots.len(), font, color: fold_fg, background_color: None, underline: None, strikethrough: None };
                let glyph = window.text_system().shape_line(SharedString::from(dots), px(style.font_size), &[run], None);
                let gx = x + (cw * 3.0 - f32::from(glyph.width)) / 2.0;
                let _ = glyph.paint(point(px(gx), top), px(lh), window, cx);
            }

            // File references.
            let link_color = theme.c_or("textLink.foreground", 0x3794ffff);
            for (line, spans) in &prepaint.links {
                for &(start, end) in spans {
                    let (x0, x1) = (x_at(*line, start), x_end(*line, end));
                    let y = line_top(*line) + px(lh - 2.0);
                    window.paint_underline(point(px(x0), y), px(x1 - x0), &UnderlineStyle { color: Some(link_color), thickness: px(1.), wavy: false });
                }
            }

            // Diagnostics.
            for d in &view.diagnostics {
                if d.end.line < first || d.start.line > last {
                    continue;
                }
                let color = match d.severity {
                    bf_lsp::Severity::Error => theme.c("editorError.foreground"),
                    bf_lsp::Severity::Warning => theme.c("editorWarning.foreground"),
                    _ => theme.c("editorInfo.foreground"),
                };
                for line in d.start.line.max(first)..=d.end.line.min(last) {
                    if view.is_hidden(line) {
                        continue;
                    }
                    let len = view.line_len(line);
                    let c0 = if line == d.start.line { d.start.col.min(len) } else { 0 };
                    let c1 = if line == d.end.line { d.end.col.min(len) } else { len };
                    let x0 = x_at(line, c0);
                    let x1 = x_end(line, c1).max(x0 + cw);
                    let y = line_top(line) + px(lh - 3.0);
                    window.paint_underline(point(px(x0), y), px(x1 - x0), &UnderlineStyle { color: Some(color), thickness: px(1.), wavy: style.squiggle_wavy });
                }
            }

            // Caret.
            if focused {
                let opacity = view.opacity;
                // Extra cursors: drawn in place (only the primary caret glides).
                if opacity > 0.01 {
                    let mut color = theme.c("editorCursor.foreground");
                    color.a *= opacity;
                    let w = style.caret_width;
                    for &(c, _) in &view.extra_cursors {
                        if c.line < first || c.line > last || view.is_hidden(c.line) {
                            continue;
                        }
                        let (x, y) = (x_at(c.line, c.col).round(), line_top(c.line));
                        let rect = match style.caret_style {
                            CaretStyle::Line => Bounds::new(point(px(x), y), size(px(w), px(lh))),
                            CaretStyle::Block | CaretStyle::BlockOutline => Bounds::new(point(px(x), y), size(px(cw), px(lh))),
                            CaretStyle::Underline => Bounds::new(point(px(x), y + px(lh - w)), size(px(cw), px(w))),
                        };
                        if style.caret_style == CaretStyle::Line {
                            window.paint_quad(fill(rect, color));
                        } else {
                            window.paint_quad(outline(rect, color, gpui::BorderStyle::Solid));
                        }
                    }
                }
                if opacity > 0.01 {
                    let (cxp, cyp) = view.caret_drawn.unwrap_or((0.0, 0.0));
                    let x = (origin_x + cxp).round();
                    let y = px((l.top + cyp - sy).round());
                    let mut color = theme.c("editorCursor.foreground");
                    color.a *= opacity;
                    let w = style.caret_width;
                    let rect = match style.caret_style {
                        CaretStyle::Line => Bounds::new(point(px(x), y), size(px(w), px(lh))),
                        CaretStyle::Block | CaretStyle::BlockOutline => Bounds::new(point(px(x), y), size(px(cw), px(lh))),
                        CaretStyle::Underline => Bounds::new(point(px(x), y + px(lh - w)), size(px(cw), px(w))),
                    };
                    if style.caret_style == CaretStyle::BlockOutline {
                        window.paint_quad(outline(rect, color, gpui::BorderStyle::Solid));
                    } else {
                        window.paint_quad(fill(rect, color));
                    }
                    if style.caret_style == CaretStyle::Block && opacity > 0.5 {
                        let caret = view.caret;
                        let text = view.line(caret.line);
                        if let Some(ch) = text.chars().nth(caret.col).filter(|c| !c.is_whitespace()) {
                            let font = gpui::Font { weight: style.font_weight, ..gpui::font(theme.editor_font.clone()) };
                            let s = ch.to_string();
                            let color = if theme.has("editorCursor.background") { theme.c("editorCursor.background") } else { theme.c("editor.background") };
                            let run = TextRun { len: s.len(), font, color, background_color: None, underline: None, strikethrough: None };
                            let glyph = window.text_system().shape_line(SharedString::from(s), px(style.font_size), &[run], None);
                            let _ = glyph.paint(point(px(x), y), px(lh), window, cx);
                        }
                    }
                }
            }
        });

        // Gutter.
        let gutter_bounds = Bounds::new(bounds.origin, size(px(l.gutter), bounds.size.height));
        window.paint_quad(fill(gutter_bounds, theme.bg("editorGutter.background")));
        let caret_line = view.caret.line;
        if (first..=last).contains(&caret_line) && !view.has_selection && matches!(style.line_highlight, LineHighlight::Gutter | LineHighlight::All) {
            window.paint_quad(fill(Bounds::new(point(bounds.left(), line_top(caret_line)), size(px(l.gutter), px(lh))), theme.bg("editor.lineHighlightBackground")));
        }
        if theme.has("editorGutter.border") {
            window.paint_quad(fill(Bounds::new(point(px(gutter_right - 1.0), bounds.top()), size(px(1.), bounds.size.height)), theme.c("editorGutter.border")));
        }
        window.with_content_mask(Some(gpui::ContentMask { bounds: gutter_bounds }), |window| {
            // Git change markers along the right edge of the gutter.
            use bf_core::git::LineChangeKind;
            let mark_w = theme.n("editorGutter.changeWidth", 3.0);
            let mark_x = gutter_right - mark_w - theme.n("editorGutter.changeOffset", 2.0);
            for &(line, kind) in &view.git_marks {
                if view.is_hidden(line) {
                    continue;
                }
                let top = line_top(line);
                let (key, rect) = match kind {
                    LineChangeKind::Added => ("editorGutter.addedBackground", Bounds::new(point(px(mark_x), top), size(px(mark_w), px(lh)))),
                    LineChangeKind::Modified => ("editorGutter.modifiedBackground", Bounds::new(point(px(mark_x), top), size(px(mark_w), px(lh)))),
                    LineChangeKind::Deleted => {
                        let h = (lh / 4.0).max(3.0);
                        ("editorGutter.deletedBackground", Bounds::new(point(px(mark_x - mark_w), top - px(h / 2.0)), size(px(mark_w * 3.0), px(h))))
                    }
                };
                window.paint_quad(quad(rect, Corners::all(px(theme.n("editorGutter.changeRadius", 0.0))), theme.bg(key), 0.0, Hsla::default(), gpui::BorderStyle::Solid));
            }
            for (line, shaped, w) in &prepaint.numbers {
                let x = gutter_right - style.gutter_right - l.fold_width - w;
                let y = line_top(*line as usize);
                let _ = shaped.paint(point(px(x), y), px(lh), window, cx);
            }

            // Folding chevrons: ▶ on folded regions, a fainter ▼ on foldable ones.
            if l.fold_width > 0.0 {
                let open = theme.c_or("editorGutter.foldingControlForeground", 0x8c8c8c99);
                let closed = theme.c_or("editorGutter.foldingControlForeground", 0xc5c5c5ff);
                for &(start, _) in &view.regions {
                    if start < first || start > last || view.is_hidden(start) {
                        continue;
                    }
                    let folded = view.folded.iter().any(|f| f.0 == start);
                    let size_ = (l.fold_width.min(lh) * 0.42).round();
                    let (x0, y0) = (l.fold_left + (l.fold_width - size_) / 2.0, f32::from(line_top(start)) + (lh - size_) / 2.0);
                    let mut path = gpui::PathBuilder::fill();
                    if folded {
                        path.move_to(point(px(x0 + size_ * 0.2), px(y0)));
                        path.line_to(point(px(x0 + size_ * 0.85), px(y0 + size_ / 2.0)));
                        path.line_to(point(px(x0 + size_ * 0.2), px(y0 + size_)));
                    } else {
                        path.move_to(point(px(x0), px(y0 + size_ * 0.2)));
                        path.line_to(point(px(x0 + size_), px(y0 + size_ * 0.2)));
                        path.line_to(point(px(x0 + size_ / 2.0), px(y0 + size_ * 0.85)));
                    }
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, if folded { closed } else { open });
                    }
                }
            }

            // Breakpoints and the execution arrow in the glyph margin.
            if l.glyph > 0.0 {
                let d = (l.glyph.min(lh) * 0.62).round();
                let cx_ = l.left + l.glyph / 2.0;
                for b in view.breakpoints.iter().filter(|b| !view.is_hidden(b.line)) {
                    let rect = Bounds::new(point(px(cx_ - d / 2.0), line_top(b.line) + px((lh - d) / 2.0)), size(px(d), px(d)));
                    if b.verified {
                        window.paint_quad(quad(rect, Corners::all(px(d / 2.0)), theme.c_or("debugIcon.breakpointForeground", 0xe51400ff), 0.0, Hsla::default(), gpui::BorderStyle::Solid));
                    } else {
                        let color = theme.c_or("debugIcon.breakpointUnverifiedForeground", 0x848484ff);
                        window.paint_quad(quad(rect, Corners::all(px(d / 2.0)), Hsla::default(), 1.5, color, gpui::BorderStyle::Solid));
                    }
                }
                if let Some((line, top)) = view.exec_line
                    && (first..=last).contains(&line)
                {
                    let color = if top {
                        theme.c_or("debugIcon.breakpointCurrentStackframeForeground", 0xffcc00ff)
                    } else {
                        theme.c_or("debugIcon.breakpointStackframeForeground", 0x89d185ff)
                    };
                    let (w, h) = (l.glyph * 0.62, (lh * 0.55).min(l.glyph * 0.62));
                    let x0 = l.left + (l.glyph - w) / 2.0;
                    let y0 = f32::from(line_top(line)) + (lh - h) / 2.0;
                    let notch = w * 0.45;
                    let mut path = gpui::PathBuilder::fill();
                    path.move_to(point(px(x0), px(y0 + h * 0.25)));
                    path.line_to(point(px(x0 + notch), px(y0 + h * 0.25)));
                    path.line_to(point(px(x0 + notch), px(y0)));
                    path.line_to(point(px(x0 + w), px(y0 + h / 2.0)));
                    path.line_to(point(px(x0 + notch), px(y0 + h)));
                    path.line_to(point(px(x0 + notch), px(y0 + h * 0.75)));
                    path.line_to(point(px(x0), px(y0 + h * 0.75)));
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                }
            }
        });

        if let Some(map) = &view.minimap {
            let mm = Bounds::new(point(px(view.minimap_left), bounds.top()), size(px(l.minimap), bounds.size.height));
            window.with_content_mask(Some(gpui::ContentMask { bounds: mm }), |window| {
                super::minimap::paint(window, &theme, map, mm, sy / lh.max(1.0), l.height / lh.max(1.0), view.caret_row, view.selection_rows, view.minimap_hover);
            });
        }

        // Scrollbars.
        let bar = 10.0;
        let content_h = view.row_count as f32 * lh + l.height - lh;
        if content_h > l.height + 1.0 {
            let track = l.height;
            let thumb = (track * track / content_h).max(24.0);
            let max_scroll = (content_h - l.height).max(1.0);
            let y = l.top + (sy / max_scroll) * (track - thumb);
            window.paint_quad(quad(
                Bounds::new(point(px(l.left + l.width - bar - 2.0), px(y)), size(px(bar), px(thumb))),
                Corners::all(theme.radius("scrollbar.radius", 0.0)),
                theme.bg("scrollbarSlider.background"),
                0.0,
                Hsla::default(),
                gpui::BorderStyle::Solid,
            ));
        }
        let content_w = view.max_line_chars as f32 * cw + cw * 8.0;
        let view_w = l.text_width() + (l.text_left - gutter_right);
        if content_w > view_w + 1.0 {
            let track = view_w;
            let thumb = (track * track / content_w).max(24.0);
            let max_scroll = (content_w - view_w).max(1.0);
            let x = gutter_right + (sx / max_scroll) * (track - thumb);
            window.paint_quad(quad(
                Bounds::new(point(px(x), px(l.top + l.height - bar - 2.0)), size(px(thumb), px(bar))),
                Corners::all(theme.radius("scrollbar.radius", 0.0)),
                theme.bg("scrollbarSlider.background"),
                0.0,
                Hsla::default(),
                gpui::BorderStyle::Solid,
            ));
        }
    }
}

/// Everything `paint` needs from the view, copied so the app context isn't borrowed while painting.
struct Snap {
    layout: Layout,
    scroll: (f32, f32),
    tab: usize,
    first: usize,
    last: usize,
    texts: Vec<String>,
    caret: TPoint,
    has_selection: bool,
    selection: (TPoint, TPoint),
    /// Extra cursors (multi-cursor editing) touching the visible lines: caret and selection.
    extra_cursors: Vec<(TPoint, (TPoint, TPoint))>,
    find: Vec<(TPoint, TPoint)>,
    minimap: Option<std::sync::Arc<super::minimap::MiniMap>>,
    minimap_left: f32,
    minimap_hover: bool,
    caret_row: Option<usize>,
    selection_rows: Option<(usize, usize)>,
    bracket_match: Option<(TPoint, Option<TPoint>)>,
    show_indent_guides: bool,
    diagnostics: Vec<super::Diagnostic>,
    caret_drawn: Option<(f32, f32)>,
    opacity: f32,
    max_line_chars: usize,
    focus: gpui::FocusHandle,
    git_marks: Vec<(usize, bf_core::git::LineChangeKind)>,
    line_backgrounds: Vec<(usize, &'static str)>,
    breakpoints: Vec<super::Breakpoint>,
    exec_line: Option<(usize, bool)>,
    word_highlights: Vec<super::inlays::WordHighlight>,
    /// Inlay hints on the visible lines, sorted.
    inlays: Vec<super::inlays::Inlay>,
    /// Hidden line ranges (folds), folded regions and foldable regions starting on visible lines.
    hidden: Vec<(usize, usize)>,
    folded: Vec<(usize, usize)>,
    regions: Vec<(usize, usize)>,
    row_count: usize,
}

impl Snap {
    fn take(view: &Entity<EditorView>, cx: &App) -> Self {
        let v = view.read(cx);
        let (first, last) = v.visible_lines();
        Self {
            layout: v.layout,
            scroll: v.scroll,
            tab: v.state.tab_size,
            first,
            last,
            // Folded lines aren't drawn: don't copy them.
            texts: (first..=last).map(|l| if v.folds.is_hidden(l) { String::new() } else { v.state.buffer.line(l).into_owned() }).collect(),
            caret: v.state.caret(),
            has_selection: v.state.has_selection(),
            selection: v.state.selection(),
            extra_cursors: v
                .state
                .cursors()
                .into_iter()
                .filter(|(c, primary)| !primary && c.selection().1.line >= first && c.selection().0.line <= last)
                .map(|(c, _)| (c.caret, c.selection()))
                .collect(),
            minimap: v.minimap.clone(),
            minimap_left: v.minimap_left(),
            minimap_hover: v.minimap_hover,
            caret_row: Some(v.folds.row_of(v.state.caret().line)),
            selection_rows: super::minimap::selection_rows(v),
            find: v.find_matches().iter().filter(|(s, e)| e.line >= first && s.line <= last).copied().collect(),
            bracket_match: v.bracket_match,
            show_indent_guides: v.show_indent_guides,
            diagnostics: v.diagnostics.iter().filter(|d| d.end.line >= first && d.start.line <= last).cloned().collect(),
            caret_drawn: v.caret_drawn,
            opacity: v.caret_opacity(cx),
            max_line_chars: v.max_line_chars,
            focus: v.focus.clone(),
            git_marks: v.git_marks.iter().filter(|(l, _)| *l + 1 >= first && *l <= last + 1).copied().collect(),
            line_backgrounds: v.line_backgrounds.iter().filter(|(l, _)| *l >= first && *l <= last).copied().collect(),
            breakpoints: v.breakpoints.iter().filter(|b| b.line >= first && b.line <= last).copied().collect(),
            exec_line: v.exec_line,
            word_highlights: v.word_highlights.iter().filter(|h| h.end.line >= first && h.start.line <= last).copied().collect(),
            inlays: v.inlays.iter().filter(|h| h.at.line >= first && h.at.line <= last).cloned().collect(),
            hidden: v.folds.hidden_ranges().to_vec(),
            folded: v.folds.folded().to_vec(),
            regions: if v.layout.fold_width > 0.0 { v.folds.cached_regions().iter().filter(|r| r.0 >= first && r.0 <= last).copied().collect() } else { Vec::new() },
            row_count: v.folds.row_count(v.state.buffer.line_count()),
        }
    }

    fn is_hidden(&self, line: usize) -> bool {
        let i = self.hidden.partition_point(|h| h.1 < line);
        self.hidden.get(i).is_some_and(|h| h.0 <= line)
    }

    /// The screen row of `line` (see `Folds::row_of`).
    fn row_of(&self, line: usize) -> usize {
        let mut hidden_before = 0;
        for &(s, e) in &self.hidden {
            if s > line {
                break;
            }
            if line <= e {
                return s - 1 - hidden_before;
            }
            hidden_before += e + 1 - s;
        }
        line - hidden_before
    }

    /// Like [`super::EditorView::inlay_shift`], for the visible lines.
    fn inlay_shift(&self, line: usize, col: usize, inclusive: bool) -> usize {
        self.inlays
            .iter()
            .filter(|h| h.at.line == line && (h.at.col < col || (inclusive && h.at.col == col)))
            .map(|h| h.text.chars().count())
            .sum()
    }

    fn line(&self, line: usize) -> &str {
        line.checked_sub(self.first).and_then(|i| self.texts.get(i)).map_or("", String::as_str)
    }

    fn line_len(&self, line: usize) -> usize {
        self.line(line).chars().count()
    }
}
impl gpui::EntityInputHandler for EditorView {
    fn text_for_range(&mut self, range: std::ops::Range<usize>, actual: &mut Option<std::ops::Range<usize>>, _: &mut Window, _: &mut gpui::Context<Self>) -> Option<String> {
        // Report the caret line only (enough for IME composition context).
        let caret = self.state.caret();
        let line = self.state.buffer.line(caret.line).into_owned();
        let units: Vec<u16> = line.encode_utf16().collect();
        let r = range.start.min(units.len())..range.end.min(units.len());
        actual.replace(r.clone());
        String::from_utf16(&units[r]).ok()
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut gpui::Context<Self>) -> Option<gpui::UTF16Selection> {
        let caret = self.state.caret();
        let (_, col) = self.state.buffer.point_to_utf16(caret);
        Some(gpui::UTF16Selection { range: col..col, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut gpui::Context<Self>) -> Option<std::ops::Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) {}

    fn replace_text_in_range(&mut self, _: Option<std::ops::Range<usize>>, text: &str, _: &mut Window, cx: &mut gpui::Context<Self>) {
        self.type_text(text, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<std::ops::Range<usize>>, text: &str, _: Option<std::ops::Range<usize>>, _: &mut Window, cx: &mut gpui::Context<Self>) {
        // IME pre-edit: insert directly (composition is finalized by the platform).
        self.type_text(text, cx);
    }

    fn bounds_for_range(&mut self, _: std::ops::Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut gpui::Context<Self>) -> Option<Bounds<Pixels>> {
        let (x, y) = self.caret_drawn.unwrap_or((0.0, 0.0));
        let l = self.layout;
        Some(Bounds::new(point(px(l.text_left + x - self.scroll.0), px(l.top + y - self.scroll.1)), size(px(2.), px(l.line_height))))
    }

    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut gpui::Context<Self>) -> Option<usize> {
        None
    }
}
