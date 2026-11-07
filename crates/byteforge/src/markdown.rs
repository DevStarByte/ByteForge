//! A small Markdown renderer for language-server text (hover popups): fenced code blocks with syntax
//! colors, inline code, bold and italic, headings, lists, rules, links (shown as their text) and escapes.

use gpui::{AnyElement, App, FontStyle, FontWeight, Hsla, SharedString, StyledText, TextRun, div, prelude::*, px};

use crate::theme::Theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Code { language: Option<String>, text: String },
    Heading(Vec<Inline>),
    Paragraph(Vec<Inline>),
    Item(Vec<Inline>),
    Rule,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inline {
    pub text: String,
    pub style: Style,
}

pub fn parse(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(inlines(&paragraph.join(" "))));
            paragraph.clear();
        }
    };
    let mut lines = markdown.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush(&mut paragraph, &mut blocks);
            let language = Some(fence.trim().to_lowercase()).filter(|l| !l.is_empty());
            let mut code = Vec::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with("```") {
                    break;
                }
                code.push(line);
            }
            blocks.push(Block::Code { language, text: code.join("\n").trim_end().to_owned() });
        } else if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else if trimmed.len() >= 3 && (trimmed.chars().all(|c| c == '-') || trimmed.chars().all(|c| c == '*') || trimmed.chars().all(|c| c == '_')) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
        } else if let Some(rest) = trimmed.strip_prefix('#') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(inlines(rest.trim_start_matches('#').trim())));
        } else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Item(inlines(rest)));
        } else {
            // A trailing backslash or two spaces is a hard line break.
            let hard = line.ends_with("  ") || trimmed.ends_with('\\');
            paragraph.push(trimmed.trim_end_matches('\\').to_owned());
            if hard {
                flush(&mut paragraph, &mut blocks);
            }
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// Splits a paragraph into styled runs.
pub fn inlines(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut style = Style::default();
    let mut current = String::new();
    let push = |out: &mut Vec<Inline>, current: &mut String, style: Style| {
        if !current.is_empty() {
            match out.last_mut() {
                Some(last) if last.style == style => last.text.push_str(current),
                _ => out.push(Inline { text: current.clone(), style }),
            }
            current.clear();
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\\' if next.is_some_and(|n| n.is_ascii_punctuation()) => {
                current.push(next.unwrap_or_default());
                i += 2;
                continue;
            }
            '`' => {
                // Inline code runs to the matching backtick; nothing inside is markup.
                if let Some(len) = chars[i + 1..].iter().position(|&x| x == '`') {
                    push(&mut out, &mut current, style);
                    current.extend(&chars[i + 1..i + 1 + len]);
                    push(&mut out, &mut current, Style { code: true, ..style });
                    i += len + 2;
                    continue;
                }
            }
            '*' | '_' if next == Some(c) => {
                push(&mut out, &mut current, style);
                style.bold = !style.bold;
                i += 2;
                continue;
            }
            '*' | '_' => {
                // Underscores inside words (snake_case) are text.
                let inside_word = c == '_' && i > 0 && chars[i - 1].is_alphanumeric() && next.is_some_and(char::is_alphanumeric);
                if !inside_word {
                    push(&mut out, &mut current, style);
                    style.italic = !style.italic;
                    i += 1;
                    continue;
                }
            }
            '[' => {
                // [text](url) shows the text.
                let rest: String = chars[i + 1..].iter().collect();
                if let Some((label, after)) = rest.split_once("](")
                    && let Some(close) = after.find(')')
                    && !label.contains(']')
                {
                    current.push_str(label);
                    i += 1 + label.chars().count() + 2 + after[..close].chars().count() + 1;
                    continue;
                }
            }
            _ => {}
        }
        current.push(c);
        i += 1;
    }
    push(&mut out, &mut current, style);
    out
}

/// Renders Markdown with the theme's fonts and token colors.
pub fn render(markdown: &str, cx: &App) -> AnyElement {
    let t = Theme::global(cx);
    let fg = t.c("editorHoverWidget.foreground");
    let ui_font = gpui::font(t.ui_font.clone());
    let code_font = gpui::Font { weight: t.editor.font_weight, ..gpui::font(t.editor_font.clone()) };
    let code_bg = t.c_or("textCodeBlock.background", 0x0a0a0a40);
    let rule = t.c("editorHoverWidget.border");

    let styled = |items: &[Inline], bold: bool| -> StyledText {
        let mut text = String::new();
        let mut runs = Vec::new();
        for item in items {
            text.push_str(&item.text);
            let base = if item.style.code { code_font.clone() } else { ui_font.clone() };
            runs.push(TextRun {
                len: item.text.len(),
                font: gpui::Font {
                    weight: if item.style.bold || bold { FontWeight::BOLD } else { base.weight },
                    style: if item.style.italic { FontStyle::Italic } else { FontStyle::Normal },
                    ..base
                },
                color: if item.style.code { t.token("string").map_or(fg, |s| s.color) } else { fg },
                background_color: item.style.code.then_some(code_bg),
                underline: None,
                strikethrough: None,
            });
        }
        StyledText::new(SharedString::from(text)).with_runs(runs)
    };

    let mut col = div().flex().flex_col().gap(px(6.));
    for block in parse(markdown) {
        col = col.child(match block {
            Block::Code { language, text } => div()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .bg(code_bg)
                .font_family(t.editor_font.clone())
                .text_size(px(t.editor.font_size.min(14.0)))
                .child(code_text(&text, language.as_deref(), &code_font, fg, &t))
                .into_any_element(),
            Block::Heading(items) => div().text_size(px(14.)).child(styled(&items, true)).into_any_element(),
            Block::Paragraph(items) => div().child(styled(&items, false)).into_any_element(),
            Block::Item(items) => div().flex().gap_2().child(div().flex_none().child("•")).child(div().flex_1().child(styled(&items, false))).into_any_element(),
            Block::Rule => div().h(px(1.)).w_full().bg(rule).into_any_element(),
        });
    }
    col.into_any_element()
}

/// A code block with syntax colors when the language is known (C#, Razor, JSON, XML, Dockerfile, YAML).
fn code_text(text: &str, language: Option<&str>, font: &gpui::Font, fg: Hsla, t: &Theme) -> StyledText {
    let language = match language {
        Some("csharp" | "cs" | "c#") => Some(bf_syntax::Language::CSharp),
        Some("razor" | "cshtml") => Some(bf_syntax::Language::Razor),
        Some("json" | "jsonc") => Some(bf_syntax::Language::Json),
        Some("xml" | "csproj" | "msbuild") => Some(bf_syntax::Language::Xml),
        Some("dockerfile" | "docker" | "containerfile") => Some(bf_syntax::Language::Dockerfile),
        Some("yaml" | "yml" | "compose") => Some(bf_syntax::Language::Yaml),
        _ => None,
    };
    let run = |len: usize, color: Hsla, bold: bool, italic: bool| TextRun {
        len,
        font: gpui::Font {
            weight: if bold { FontWeight::BOLD } else { font.weight },
            style: if italic { FontStyle::Italic } else { FontStyle::Normal },
            ..font.clone()
        },
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    if let Some(language) = language {
        let mut highlighter = bf_syntax::Highlighter::new(language);
        highlighter.parse(text);
        let mut at = 0;
        for span in highlighter.highlight(text, 0..text.len()) {
            if span.start > at {
                runs.push(run(span.start - at, fg, false, false));
            }
            let style = t.token(span.kind);
            runs.push(run(span.end - span.start, style.map_or(fg, |s| s.color), style.is_some_and(|s| s.bold), style.is_some_and(|s| s.italic)));
            at = span.end;
        }
        if at < text.len() {
            runs.push(run(text.len() - at, fg, false, false));
        }
    } else if !text.is_empty() {
        runs.push(run(text.len(), fg, false, false));
    }
    StyledText::new(SharedString::from(text.to_owned())).with_runs(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Inline {
        Inline { text: text.into(), style: Style::default() }
    }

    #[test]
    fn parses_language_server_hover() {
        let md = "```csharp\nvoid Console.WriteLine(string? value)\n```\n\n---\nWrites the specified string value, followed by\nthe line terminator.\n\n- `value`: The \\*value\\* to write";
        let blocks = parse(md);
        assert_eq!(blocks[0], Block::Code { language: Some("csharp".into()), text: "void Console.WriteLine(string? value)".into() });
        assert_eq!(blocks[1], Block::Rule);
        assert_eq!(blocks[2], Block::Paragraph(vec![plain("Writes the specified string value, followed by the line terminator.")]));
        assert_eq!(
            blocks[3],
            Block::Item(vec![Inline { text: "value".into(), style: Style { code: true, ..Style::default() } }, plain(": The *value* to write")])
        );
    }

    #[test]
    fn inline_styles() {
        let bold = Style { bold: true, ..Style::default() };
        let italic = Style { italic: true, ..Style::default() };
        assert_eq!(inlines("a **b** _c_ d"), vec![plain("a "), Inline { text: "b".into(), style: bold }, plain(" "), Inline { text: "c".into(), style: italic }, plain(" d")]);
        assert_eq!(inlines("snake_case_name"), vec![plain("snake_case_name")]);
        assert_eq!(inlines("see [docs](https://x.y/z) now"), vec![plain("see docs now")]);
    }
}
