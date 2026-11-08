//! Markdown preview: GitHub-flavored Markdown (tables, task lists, strikethrough) parsed with pulldown-cmark
//! into a small block tree, then drawn with the theme's colors. Links open in the browser or, when relative,
//! in ByteForge; local images are shown, remote ones (the app has no HTTP client) fall back to their alt text.

use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{AnyElement, App, FontStyle, FontWeight, Hsla, InteractiveText, ObjectFit, SharedString, StrikethroughStyle, StyledImage, StyledText, TextRun, UnderlineStyle, Window, div, img, prelude::*, px, relative};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::theme::Theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text { text: String, style: Style, link: Option<String> },
    Image { url: String, alt: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Piece>),
    Paragraph(Vec<Piece>),
    Code { language: Option<String>, text: String },
    Quote(Vec<Block>),
    List { start: Option<u64>, items: Vec<ListItem> },
    Table { aligns: Vec<Alignment>, head: Vec<Vec<Piece>>, rows: Vec<Vec<Vec<Piece>>> },
    Rule,
}

enum Frame {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    List { start: Option<u64>, items: Vec<ListItem> },
    Item(ListItem),
}

impl Frame {
    fn blocks(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Frame::Root(b) | Frame::Quote(b) => Some(b),
            Frame::Item(item) => Some(&mut item.blocks),
            Frame::List { .. } => None,
        }
    }
}

#[derive(Default)]
struct TableBuild {
    aligns: Vec<Alignment>,
    head: Vec<Vec<Piece>>,
    rows: Vec<Vec<Vec<Piece>>>,
    row: Vec<Vec<Piece>>,
}

struct Builder {
    stack: Vec<Frame>,
    inline: Vec<Piece>,
    style: Style,
    links: Vec<String>,
    image: Option<(String, String)>,
    code: Option<(Option<String>, String)>,
    heading: Option<u8>,
    table: Option<TableBuild>,
}

impl Builder {
    fn push_block(&mut self, block: Block) {
        if let Some(blocks) = self.stack.iter_mut().rev().find_map(|f| f.blocks()) {
            blocks.push(block);
        }
    }

    /// Text collected outside a paragraph (tight list items) becomes a paragraph of its own.
    fn flush(&mut self) {
        if self.table.is_none() && self.heading.is_none() && !self.inline.is_empty() {
            let pieces = std::mem::take(&mut self.inline);
            self.push_block(Block::Paragraph(pieces));
        }
    }

    fn text(&mut self, text: &str, code: bool) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(text);
            return;
        }
        if let Some((_, body)) = &mut self.code {
            body.push_str(text);
            return;
        }
        let style = Style { code, ..self.style };
        let link = self.links.last().cloned();
        if let Some(Piece::Text { text: last, style: s, link: l }) = self.inline.last_mut()
            && *s == style
            && *l == link
        {
            last.push_str(text);
            return;
        }
        self.inline.push(Piece::Text { text: text.to_owned(), style, link });
    }

    /// Inline HTML that matters in READMEs: `<img src>` and `<br>`; other tags are dropped.
    fn html(&mut self, html: &str) {
        let lower = html.to_ascii_lowercase();
        let mut at = 0;
        while let Some(i) = lower[at..].find("<img") {
            let tag_start = at + i;
            let tag_end = lower[tag_start..].find('>').map_or(lower.len(), |e| tag_start + e);
            let tag = &html[tag_start..tag_end];
            if let Some(src) = attr(tag, "src") {
                let alt = attr(tag, "alt").unwrap_or_default();
                self.inline.push(Piece::Image { url: src, alt });
            }
            at = tag_end;
        }
        if lower.contains("<br") {
            self.text("\n", false);
        }
    }
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let i = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[i..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        rest[1..].split(quote).next().map(str::to_owned)
    } else {
        rest.split(|c: char| c.is_whitespace() || c == '>').next().map(str::to_owned)
    }
}

pub fn parse(markdown: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut b = Builder { stack: vec![Frame::Root(Vec::new())], inline: Vec::new(), style: Style::default(), links: Vec::new(), image: None, code: None, heading: None, table: None };
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => b.flush(),
                Tag::Heading { level, .. } => {
                    b.flush();
                    b.heading = Some(level as u8);
                }
                Tag::BlockQuote(_) => {
                    b.flush();
                    b.stack.push(Frame::Quote(Vec::new()));
                }
                Tag::CodeBlock(kind) => {
                    b.flush();
                    let language = match kind {
                        CodeBlockKind::Fenced(info) => info.split([' ', ',', '{']).next().filter(|l| !l.is_empty()).map(|l| l.to_lowercase()),
                        CodeBlockKind::Indented => None,
                    };
                    b.code = Some((language, String::new()));
                }
                Tag::List(start) => {
                    b.flush();
                    b.stack.push(Frame::List { start, items: Vec::new() });
                }
                Tag::Item => b.stack.push(Frame::Item(ListItem { task: None, blocks: Vec::new() })),
                Tag::Table(aligns) => {
                    b.flush();
                    b.table = Some(TableBuild { aligns, ..Default::default() });
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(t) = &mut b.table {
                        t.row.clear();
                    }
                }
                Tag::TableCell => b.inline.clear(),
                Tag::Emphasis => b.style.italic = true,
                Tag::Strong => b.style.bold = true,
                Tag::Strikethrough => b.style.strike = true,
                Tag::Link { dest_url, .. } => b.links.push(dest_url.into_string()),
                Tag::Image { dest_url, .. } => b.image = Some((dest_url.into_string(), String::new())),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => b.flush(),
                TagEnd::Heading(_) => {
                    let level = b.heading.take().unwrap_or(1);
                    let pieces = std::mem::take(&mut b.inline);
                    b.push_block(Block::Heading(level, pieces));
                }
                TagEnd::BlockQuote(_) => {
                    b.flush();
                    if let Some(Frame::Quote(blocks)) = b.stack.pop() {
                        b.push_block(Block::Quote(blocks));
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((language, mut text)) = b.code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        b.push_block(Block::Code { language, text });
                    }
                }
                TagEnd::List(_) => {
                    if let Some(Frame::List { start, items }) = b.stack.pop() {
                        b.push_block(Block::List { start, items });
                    }
                }
                TagEnd::Item => {
                    b.flush();
                    if let Some(Frame::Item(item)) = b.stack.pop()
                        && let Some(Frame::List { items, .. }) = b.stack.last_mut()
                    {
                        items.push(item);
                    }
                }
                TagEnd::TableCell => {
                    let cell = std::mem::take(&mut b.inline);
                    if let Some(t) = &mut b.table {
                        t.row.push(cell);
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = &mut b.table {
                        t.head = std::mem::take(&mut t.row);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = &mut b.table {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = b.table.take() {
                        b.push_block(Block::Table { aligns: t.aligns, head: t.head, rows: t.rows });
                    }
                }
                TagEnd::Emphasis => b.style.italic = false,
                TagEnd::Strong => b.style.bold = false,
                TagEnd::Strikethrough => b.style.strike = false,
                TagEnd::Link => {
                    b.links.pop();
                }
                TagEnd::Image => {
                    if let Some((url, alt)) = b.image.take() {
                        b.inline.push(Piece::Image { url, alt });
                    }
                }
                TagEnd::HtmlBlock => b.flush(),
                _ => {}
            },
            Event::Text(text) => b.text(&text, false),
            Event::Code(text) => b.text(&text, true),
            Event::Html(html) | Event::InlineHtml(html) => b.html(&html),
            Event::SoftBreak => b.text(" ", false),
            Event::HardBreak => b.text("\n", false),
            Event::Rule => {
                b.flush();
                b.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item(item)) = b.stack.last_mut() {
                    item.task = Some(done);
                }
            }
            _ => {}
        }
    }
    b.flush();
    match b.stack.into_iter().next() {
        Some(Frame::Root(blocks)) => blocks,
        _ => Vec::new(),
    }
}

/// Called with a link's target when it is clicked.
pub type LinkHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

struct Ctx<'a> {
    t: &'a Theme,
    base: PathBuf,
    on_link: LinkHandler,
    ids: Cell<usize>,
    fg: Hsla,
    muted: Hsla,
    link: Hsla,
    rule: Hsla,
    code_bg: Hsla,
    ui_font: gpui::Font,
    code_font: gpui::Font,
}

impl Ctx<'_> {
    fn id(&self, what: &str) -> SharedString {
        let n = self.ids.get();
        self.ids.set(n + 1);
        SharedString::from(format!("md-{what}-{n}"))
    }

    /// Resolves an image or link target against the Markdown file's folder.
    fn local(&self, target: &str) -> Option<PathBuf> {
        if target.contains("://") || target.starts_with("mailto:") || target.starts_with('#') || target.starts_with("data:") {
            return None;
        }
        let target = target.split(['#', '?']).next().unwrap_or(target);
        let decoded = target.replace("%20", " ");
        let path = Path::new(&decoded);
        Some(if path.is_absolute() { path.to_path_buf() } else { self.base.join(path) })
    }
}

/// Draws a parsed document. `base` is the folder of the Markdown file (for relative images).
pub fn render(blocks: &[Block], base: &Path, on_link: LinkHandler, cx: &App) -> AnyElement {
    let t = Theme::global(cx);
    let ctx = Ctx {
        t: &t,
        base: base.to_path_buf(),
        on_link,
        ids: Cell::new(0),
        fg: t.c("editor.foreground"),
        muted: t.c("foreground.muted"),
        link: if t.has("textLink.foreground") { t.c("textLink.foreground") } else { t.c("focusBorder") },
        rule: t.c("editorWidget.border"),
        code_bg: t.c_or("textCodeBlock.background", 0x80808024),
        ui_font: gpui::font(t.ui_font.clone()),
        code_font: gpui::Font { weight: t.editor.font_weight, ..gpui::font(t.editor_font.clone()) },
    };
    if blocks.is_empty() {
        return div().text_color(ctx.muted).child("This file is empty. What you write in it shows up here.").into_any_element();
    }
    render_blocks(blocks, &ctx, 0).into_any_element()
}

fn render_blocks(blocks: &[Block], ctx: &Ctx, depth: usize) -> gpui::Div {
    let mut col = div().flex().flex_col().gap(px(14.));
    for block in blocks {
        col = col.child(render_block(block, ctx, depth));
    }
    col
}

fn render_block(block: &Block, ctx: &Ctx, depth: usize) -> AnyElement {
    match block {
        Block::Heading(level, pieces) => {
            let (size, line) = match level {
                1 => (28., 36.),
                2 => (22., 30.),
                3 => (18., 26.),
                4 => (16., 24.),
                5 => (14., 22.),
                _ => (13., 20.),
            };
            div()
                .mt(px(if *level <= 2 { 10. } else { 4. }))
                .text_size(px(size))
                .line_height(px(line))
                .when(*level <= 2, |d| d.pb(px(6.)).border_b_1().border_color(ctx.rule))
                .when(*level == 6, |d| d.text_color(ctx.muted))
                .child(inline(pieces, ctx, Some(FontWeight::SEMIBOLD)))
                .into_any_element()
        }
        Block::Paragraph(pieces) => inline(pieces, ctx, None),
        Block::Code { language, text } => div()
            .id(ctx.id("code"))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(6.))
            .bg(ctx.code_bg)
            .border_1()
            .border_color(Hsla { a: ctx.rule.a * 0.6, ..ctx.rule })
            .overflow_x_scroll()
            .font_family(ctx.t.editor_font.clone())
            .text_size(px(13.))
            .line_height(px(20.))
            .child(div().whitespace_nowrap().child(crate::markdown::code_text(text, language.as_deref(), &ctx.code_font, ctx.fg, ctx.t)))
            .into_any_element(),
        Block::Quote(blocks) => div()
            .pl(px(14.))
            .border_l(px(3.))
            .border_color(ctx.t.c("focusBorder"))
            .text_color(ctx.muted)
            .child(render_blocks(blocks, ctx, depth))
            .into_any_element(),
        Block::List { start, items } => {
            let mut col = div().flex().flex_col().gap(px(4.));
            for (i, item) in items.iter().enumerate() {
                let marker: AnyElement = match (item.task, start) {
                    (Some(done), _) => checkbox(done, ctx),
                    (None, Some(n)) => div().text_color(ctx.muted).child(format!("{}.", n + i as u64)).into_any_element(),
                    (None, None) => div().text_color(ctx.muted).child(["•", "◦", "▪"][depth % 3]).into_any_element(),
                };
                let body = render_blocks(&item.blocks, ctx, depth + 1).gap(px(6.)).when(item.task == Some(true), |d| d.text_color(ctx.muted));
                col = col.child(div().flex().gap(px(8.)).child(div().flex_none().w(px(22.)).flex().justify_end().child(marker)).child(div().flex_1().min_w_0().child(body)));
            }
            col.into_any_element()
        }
        Block::Table { aligns, head, rows } => table(aligns, head, rows, ctx),
        Block::Rule => div().my(px(6.)).h(px(1.)).w_full().bg(ctx.rule).into_any_element(),
    }
}

fn checkbox(done: bool, ctx: &Ctx) -> AnyElement {
    let accent = ctx.t.c("focusBorder");
    div()
        .mt(px(3.))
        .size(px(14.))
        .rounded(px(3.))
        .border_1()
        .border_color(if done { accent } else { ctx.muted })
        .when(done, |d| d.bg(accent))
        .flex()
        .items_center()
        .justify_center()
        .when(done, |d| d.child(crate::assets::icon("check", px(11.), ctx.t.c("editor.background"))))
        .into_any_element()
}

fn table(aligns: &[Alignment], head: &[Vec<Piece>], rows: &[Vec<Vec<Piece>>], ctx: &Ctx) -> AnyElement {
    let columns = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return div().into_any_element();
    }
    // Column widths follow their longest cell, so short columns don't get half the table.
    let len = |cell: &[Piece]| -> usize { cell.iter().map(|p| match p { Piece::Text { text, .. } => text.chars().count(), Piece::Image { .. } => 8 }).sum() };
    let mut weights = vec![3usize; columns];
    for row in std::iter::once(head).chain(rows.iter().map(Vec::as_slice)) {
        for (i, cell) in row.iter().enumerate() {
            weights[i] = weights[i].max(len(cell).min(48));
        }
    }
    let total: usize = weights.iter().sum();
    let row_el = |cells: &[Vec<Piece>], header: bool, stripe: bool| {
        let mut row = div().flex().w_full().border_b_1().border_color(ctx.rule).when(stripe, |d| d.bg(Hsla { a: 0.035, ..ctx.fg }));
        for (i, weight) in weights.iter().enumerate() {
            let cell = cells.get(i).map(Vec::as_slice).unwrap_or(&[]);
            let align = aligns.get(i).copied().unwrap_or(Alignment::None);
            row = row.child(
                div()
                    .w(relative(*weight as f32 / total as f32))
                    .min_w_0()
                    .px(px(10.))
                    .py(px(6.))
                    .flex()
                    .when(align == Alignment::Center, |d| d.justify_center())
                    .when(align == Alignment::Right, |d| d.justify_end())
                    .when(i > 0, |d| d.border_l_1().border_color(Hsla { a: ctx.rule.a * 0.6, ..ctx.rule }))
                    .child(inline(cell, ctx, header.then_some(FontWeight::SEMIBOLD))),
            );
        }
        row
    };
    let mut el = div().w_full().flex().flex_col().border_1().border_color(ctx.rule).rounded(px(4.)).overflow_hidden();
    if !head.is_empty() {
        el = el.child(row_el(head, true, false).bg(Hsla { a: 0.06, ..ctx.fg }));
    }
    for (i, row) in rows.iter().enumerate() {
        el = el.child(row_el(row, false, i % 2 == 1));
    }
    el.into_any_element()
}

/// A run of text and images. Consecutive text becomes one wrapping, clickable text element.
fn inline(pieces: &[Piece], ctx: &Ctx, weight: Option<FontWeight>) -> AnyElement {
    let mut parts: Vec<AnyElement> = Vec::new();
    let mut texts: Vec<&Piece> = Vec::new();
    let mut only_text = true;
    for piece in pieces {
        match piece {
            Piece::Text { .. } => texts.push(piece),
            Piece::Image { url, alt } => {
                only_text = false;
                if !texts.is_empty() {
                    parts.push(styled(&std::mem::take(&mut texts), ctx, weight));
                }
                parts.push(image(url, alt, ctx));
            }
        }
    }
    if !texts.is_empty() {
        parts.push(styled(&texts, ctx, weight));
    }
    if only_text && parts.len() == 1 {
        return parts.pop().unwrap_or_else(|| div().into_any_element());
    }
    div().flex().flex_wrap().items_end().gap(px(4.)).children(parts).into_any_element()
}

fn styled(pieces: &[&Piece], ctx: &Ctx, weight: Option<FontWeight>) -> AnyElement {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    for piece in pieces {
        let Piece::Text { text: t, style, link } = piece else { continue };
        let start = text.len();
        // Inline code gets a hair of padding from non-breaking spaces inside its background.
        let shown = if style.code { format!("\u{a0}{t}\u{a0}") } else { t.clone() };
        text.push_str(&shown);
        let base = if style.code { ctx.code_font.clone() } else { ctx.ui_font.clone() };
        let color = if link.is_some() { ctx.link } else { ctx.fg };
        runs.push(TextRun {
            len: shown.len(),
            font: gpui::Font {
                weight: if style.bold { FontWeight::BOLD } else { weight.unwrap_or(base.weight) },
                style: if style.italic { FontStyle::Italic } else { FontStyle::Normal },
                ..base
            },
            color,
            background_color: style.code.then_some(ctx.code_bg),
            underline: link.is_some().then_some(UnderlineStyle { thickness: px(1.), color: Some(Hsla { a: 0.5, ..ctx.link }), wavy: false }),
            strikethrough: style.strike.then_some(StrikethroughStyle { thickness: px(1.), color: Some(ctx.fg) }),
        });
        if let Some(url) = link {
            match links.last_mut() {
                Some((range, last)) if last == url && range.end == start => range.end = text.len(),
                _ => links.push((start..text.len(), url.clone())),
            }
        }
    }
    let styled = StyledText::new(SharedString::from(text)).with_runs(runs);
    if links.is_empty() {
        return div().line_height(relative(1.6)).child(styled).into_any_element();
    }
    let on_link = ctx.on_link.clone();
    let (ranges, targets): (Vec<Range<usize>>, Vec<String>) = links.into_iter().unzip();
    let base = ctx.base.clone();
    let text = InteractiveText::new(ctx.id("text"), styled).on_click(ranges, move |ix, window, cx| {
        if let Some(target) = targets.get(ix) {
            let resolved = resolve_link(target, &base);
            on_link(&resolved, window, cx);
        }
    });
    div().line_height(relative(1.6)).child(text).into_any_element()
}

/// A link target as an absolute path for local files, unchanged otherwise.
fn resolve_link(target: &str, base: &Path) -> String {
    if target.contains("://") || target.starts_with("mailto:") || target.starts_with('#') {
        return target.to_owned();
    }
    let file = target.split('#').next().unwrap_or(target).replace("%20", " ");
    let path = Path::new(&file);
    let full = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    full.to_string_lossy().into_owned()
}

fn image(url: &str, alt: &str, ctx: &Ctx) -> AnyElement {
    let alt_text = if alt.trim().is_empty() { "image".to_owned() } else { alt.trim().to_owned() };
    let muted = ctx.muted;
    let rule = ctx.rule;
    let fallback = move || -> AnyElement {
        div().px(px(6.)).py(px(1.)).rounded(px(3.)).border_1().border_color(rule).text_size(px(12.)).text_color(muted).child(alt_text.clone()).into_any_element()
    };
    match ctx.local(url) {
        Some(path) if path.exists() => img(path).max_w_full().object_fit(ObjectFit::ScaleDown).with_fallback(fallback).into_any_element(),
        // Remote images can't be fetched; show what the image is instead.
        _ => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Piece {
        Piece::Text { text: t.into(), style: Style::default(), link: None }
    }

    #[test]
    fn headings_paragraphs_and_inline_styles() {
        let blocks = parse("# Title\n\nSome **bold**, _it_ and `code` with [a link](https://x.y).\n");
        assert_eq!(blocks[0], Block::Heading(1, vec![text("Title")]));
        let Block::Paragraph(p) = &blocks[1] else { panic!("{blocks:?}") };
        assert!(p.contains(&Piece::Text { text: "bold".into(), style: Style { bold: true, ..Style::default() }, link: None }));
        assert!(p.contains(&Piece::Text { text: "code".into(), style: Style { code: true, ..Style::default() }, link: None }));
        assert!(p.contains(&Piece::Text { text: "a link".into(), style: Style::default(), link: Some("https://x.y".into()) }));
    }

    #[test]
    fn nested_and_task_lists() {
        let blocks = parse("- one\n  - nested\n- [x] done\n- [ ] todo\n\n3. three\n4. four\n");
        let Block::List { start: None, items } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0].blocks[1], Block::List { items, .. } if items.len() == 1));
        assert_eq!((items[1].task, items[2].task), (Some(true), Some(false)));
        assert!(matches!(&blocks[1], Block::List { start: Some(3), items } if items.len() == 2));
    }

    #[test]
    fn code_quotes_tables_and_images() {
        let md = "```csharp\nvar x = 1;\n```\n\n> quoted\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n\n![logo](img/logo.png) <img src=\"x.png\" alt=\"X\">\n\n---\n";
        let blocks = parse(md);
        assert_eq!(blocks[0], Block::Code { language: Some("csharp".into()), text: "var x = 1;".into() });
        assert_eq!(blocks[1], Block::Quote(vec![Block::Paragraph(vec![text("quoted")])]));
        let Block::Table { aligns, head, rows } = &blocks[2] else { panic!("{blocks:?}") };
        assert_eq!(aligns, &[Alignment::Left, Alignment::Right]);
        assert_eq!((head.len(), rows.len(), rows[0][1].clone()), (2, 1, vec![text("2")]));
        let Block::Paragraph(p) = &blocks[3] else { panic!("{blocks:?}") };
        assert!(p.contains(&Piece::Image { url: "img/logo.png".into(), alt: "logo".into() }));
        assert!(p.contains(&Piece::Image { url: "x.png".into(), alt: "X".into() }));
        assert_eq!(blocks[4], Block::Rule);
    }

    #[test]
    fn link_targets() {
        assert_eq!(resolve_link("https://a.b/c", Path::new("/r")), "https://a.b/c");
        assert_eq!(resolve_link("#install", Path::new("/r")), "#install");
        assert!(resolve_link("docs/My%20Guide.md#top", Path::new("/r")).ends_with("My Guide.md"));
    }
}
