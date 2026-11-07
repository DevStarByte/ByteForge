//! HTML markup colors for Razor.
//!
//! The Razor grammar leaves tags and attributes inside opaque `element` nodes (meant for an HTML
//! injection), so this lexes those bytes directly: delimiters, tag names, attribute names and values,
//! and `<!-- -->` comments. Named child nodes (Razor expressions, directive attributes, code blocks)
//! are skipped; the query captures color them afterwards.

use std::ops::Range;

use tree_sitter::Node;

struct Kinds {
    name: u8,
    attr: u8,
    value: u8,
    delim: u8,
    comment: u8,
}

/// Paints markup kinds for every `element` intersecting `window` into `paint` (indexed from
/// `window.start`). `kind` maps a capture name to its kind id.
pub(crate) fn paint(root: Node, text: &[u8], window: Range<usize>, paint: &mut [u8], kind: impl Fn(&str) -> u8) {
    let kinds = Kinds {
        name: kind("html_element_name"),
        attr: kind("html_attribute_name"),
        value: kind("html_attribute_value"),
        delim: kind("html_tag_delimiter"),
        comment: kind("comment"),
    };
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let range = node.byte_range();
        if range.end <= window.start || range.start >= window.end {
            continue;
        }
        if node.kind() == "element" {
            lex_element(node, text, &window, paint, &kinds);
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Text,
    TagName,
    InTag,
    AttrName,
    AfterEquals,
    Quoted(u8),
    Unquoted,
    Comment,
    /// Inside `<script>` or `<style>`: no markup until the closing tag.
    Raw,
}

fn lex_element(node: Node, text: &[u8], window: &Range<usize>, paint: &mut [u8], k: &Kinds) {
    let mut cursor = node.walk();
    let children: Vec<Range<usize>> = node.named_children(&mut cursor).map(|c| c.byte_range()).collect();
    let end = node.end_byte().min(text.len());
    let mut set = |i: usize, kind: u8| {
        if window.contains(&i) {
            paint[i - window.start] = kind;
        }
    };

    let mut state = State::Text;
    let mut tag = 0..0;
    let mut closing = false;
    let mut raw_tag: &[u8] = b"";
    let mut child = 0;
    let mut i = node.start_byte();
    while i < end {
        if let Some(c) = children.get(child)
            && i >= c.start
        {
            i = i.max(c.end);
            child += 1;
            continue;
        }
        let b = text[i];
        let next = text.get(i + 1).copied().unwrap_or(0);
        match state {
            State::Text | State::Raw => {
                if b == b'<' && state == State::Raw {
                    let rest = &text[i + 1..end];
                    if rest.first() == Some(&b'/') && rest[1..].len() >= raw_tag.len() && rest[1..1 + raw_tag.len()].eq_ignore_ascii_case(raw_tag) {
                        state = State::Text;
                        continue;
                    }
                } else if b == b'<' && text[i..end].starts_with(b"<!--") {
                    for j in i..i + 4 {
                        set(j, k.comment);
                    }
                    i += 4;
                    state = State::Comment;
                    continue;
                } else if b == b'<' && (next == b'/' || next == b'!' || next.is_ascii_alphabetic()) {
                    set(i, k.delim);
                    closing = next == b'/';
                    if closing || next == b'!' {
                        set(i + 1, k.delim);
                        i += 1;
                    }
                    tag = i + 1..i + 1;
                    state = State::TagName;
                }
            }
            State::TagName => {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'.' | b'_') {
                    set(i, k.name);
                    tag.end = i + 1;
                } else {
                    state = State::InTag;
                    continue;
                }
            }
            State::InTag => match b {
                b'>' => {
                    set(i, k.delim);
                    let name = &text[tag.clone()];
                    state = if !closing && (name.eq_ignore_ascii_case(b"script") || name.eq_ignore_ascii_case(b"style")) {
                        raw_tag = name;
                        State::Raw
                    } else {
                        State::Text
                    };
                }
                b'/' if next == b'>' => set(i, k.delim),
                b'=' => {
                    set(i, k.delim);
                    state = State::AfterEquals;
                }
                b'"' | b'\'' => {
                    set(i, k.value);
                    state = State::Quoted(b);
                }
                _ if b.is_ascii_whitespace() => {}
                _ => {
                    set(i, k.attr);
                    state = State::AttrName;
                }
            },
            State::AttrName => {
                if b.is_ascii_whitespace() || matches!(b, b'=' | b'>' | b'/') {
                    state = State::InTag;
                    continue;
                }
                set(i, k.attr);
            }
            State::AfterEquals => match b {
                b'"' | b'\'' => {
                    set(i, k.value);
                    state = State::Quoted(b);
                }
                b'>' => {
                    state = State::InTag;
                    continue;
                }
                _ if b.is_ascii_whitespace() => {}
                _ => {
                    set(i, k.value);
                    state = State::Unquoted;
                }
            },
            State::Quoted(quote) => {
                set(i, k.value);
                if b == quote {
                    state = State::InTag;
                }
            }
            State::Unquoted => {
                if b.is_ascii_whitespace() || b == b'>' {
                    state = State::InTag;
                    continue;
                }
                set(i, k.value);
            }
            State::Comment => {
                set(i, k.comment);
                if b == b'>' && i >= 2 && &text[i - 2..i] == b"--" {
                    state = State::Text;
                }
            }
        }
        i += 1;
    }
}
