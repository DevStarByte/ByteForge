//! Linked editing of markup tags: the name of an element's opening tag and the name of its closing tag
//! change together. A small lexer finds the tags (Razor markup has no tag nodes in its tree, and the
//! lexer needs no parse that is up to date with the latest keystroke).

use std::ops::Range;

use crate::Language;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tag {
    /// The name's bytes.
    name: Range<usize>,
    closing: bool,
    self_closing: bool,
}

/// For a selection inside a tag name (a caret at either end of the name counts), the name's range and
/// the range of the matching tag's name. `None` outside tag names, for unmatched or self-closing tags,
/// and in languages without markup.
pub fn linked_tag_names(language: Language, text: &str, selection: Range<usize>) -> Option<(Range<usize>, Range<usize>)> {
    if !matches!(language, Language::Xml | Language::Html | Language::Razor) {
        return None;
    }
    let tags = tags(text, language);
    let at = tags.iter().position(|t| t.name.start <= selection.start && selection.end <= t.name.end)?;
    let own = &tags[at];
    if own.self_closing {
        return None;
    }
    let name = &text[own.name.clone()];
    let same = |t: &&Tag| &text[t.name.clone()] == name;
    let mut depth = 0usize;
    let partner = if own.closing {
        tags[..at].iter().rev().filter(same).find(|t| {
            if t.closing {
                depth += 1;
            } else if !t.self_closing {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            false
        })
    } else {
        tags[at + 1..].iter().filter(same).find(|t| {
            if !t.closing {
                if !t.self_closing {
                    depth += 1;
                }
            } else {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            false
        })
    }?;
    Some((own.name.clone(), partner.name.clone()))
}

/// Whether `text` could be typed into a tag name without ending it.
pub fn is_tag_name_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_name_char)
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')
}

/// The tags of a document, in order. Comments, CDATA, processing instructions, doctypes, quoted attribute
/// values and the contents of `<script>`/`<style>` are skipped.
fn tags(text: &str, language: Language) -> Vec<Tag> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let skip_to = |from: usize, end: &str| text[from..].find(end).map_or(text.len(), |j| from + j + end.len());
    while i < bytes.len() {
        if language == Language::Razor && text[i..].starts_with("@*") {
            i = skip_to(i + 2, "*@");
            continue;
        }
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        if rest.starts_with("<!--") {
            i = skip_to(i + 4, "-->");
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            i = skip_to(i + 9, "]]>");
            continue;
        }
        if rest.starts_with("<?") {
            i = skip_to(i + 2, "?>");
            continue;
        }
        if rest.starts_with("<!") {
            i = skip_to(i + 2, ">");
            continue;
        }
        let closing = rest.starts_with("</");
        let start = i + if closing { 2 } else { 1 };
        let Some(first) = text[start..].chars().next().filter(|c| c.is_alphabetic() || matches!(c, '_' | ':')) else {
            i += 1;
            continue;
        };
        let mut end = start + first.len_utf8();
        end += text[end..].find(|c: char| !is_name_char(c)).unwrap_or(text.len() - end);
        // The end of the tag, past quoted attribute values (they may hold `>`).
        let mut j = end;
        let mut quote = None;
        while j < bytes.len() {
            match (quote, bytes[j]) {
                (None, b'"' | b'\'') => quote = Some(bytes[j]),
                (Some(q), b) if b == q => quote = None,
                (None, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        let self_closing = !closing && j > end && bytes[j - 1] == b'/';
        out.push(Tag { name: start..end, closing, self_closing });
        i = (j + 1).min(bytes.len());
        // Script and style contents aren't markup.
        let name = text[start..end].to_ascii_lowercase();
        if !closing && !self_closing && language != Language::Xml && (name == "script" || name == "style") {
            let close = format!("</{name}");
            i = text[i..].to_ascii_lowercase().find(&close).map_or(text.len(), |k| i + k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(language: Language, text: &str, needle: &str, nth: usize) -> Option<(String, usize)> {
        let at = text.match_indices(needle).nth(nth).unwrap().0;
        let (own, other) = linked_tag_names(language, text, at..at)?;
        assert_eq!(&text[own.clone()], &text[other.clone()]);
        Some((text[other.clone()].to_owned(), other.start))
    }

    #[test]
    fn finds_the_matching_tag_through_nesting() {
        let xml = "<Project>\n  <ItemGroup>\n    <ItemGroup></ItemGroup>\n  </ItemGroup>\n</Project>\n";
        // The outer ItemGroup pairs with the outer closing tag, past the nested pair.
        let outer_close = xml.match_indices("</ItemGroup>").nth(1).unwrap().0 + 2;
        assert_eq!(pair(Language::Xml, xml, "ItemGroup", 0), Some(("ItemGroup".into(), outer_close)));
        // And back, from a closing tag.
        let inner_open = xml.match_indices("<ItemGroup>").nth(1).unwrap().0 + 1;
        assert_eq!(pair(Language::Xml, xml, "ItemGroup", 2), Some(("ItemGroup".into(), inner_open)));
        // A caret at the end of the name still counts (typing extends the name).
        let end = xml.find("Project").unwrap() + "Project".len();
        assert!(linked_tag_names(Language::Xml, xml, end..end).is_some());
    }

    #[test]
    fn ignores_comments_quotes_self_closing_and_script() {
        let html = "<div title=\"a > b\">\n<!-- <div> -->\n<br/>\n<script>if (a <div) {}</script>\n</div>\n";
        let close = html.rfind("</div>").unwrap() + 2;
        assert_eq!(pair(Language::Html, html, "div", 0), Some(("div".into(), close)));
        let br = html.find("br").unwrap();
        assert_eq!(linked_tag_names(Language::Html, html, br..br), None, "self-closing");
        // Not in a tag name: attribute names and text.
        let title = html.find("title").unwrap();
        assert_eq!(linked_tag_names(Language::Html, html, title..title), None);
        assert_eq!(linked_tag_names(Language::CSharp, html, 1..1), None);
    }

    #[test]
    fn razor_components_and_comments() {
        let razor = "@* <Card> *@\n<Card Title=\"x\">\n  @if (a < b) { <span>hi</span> }\n</Card>\n";
        let close = razor.rfind("</Card>").unwrap() + 2;
        assert_eq!(pair(Language::Razor, razor, "Card", 1), Some(("Card".into(), close)));
        assert_eq!(pair(Language::Razor, razor, "span", 0), Some(("span".into(), razor.rfind("span").unwrap())));
    }

    #[test]
    fn unmatched_tags_link_nothing() {
        let html = "<p>open\n<li>one\n</ul>\n";
        let p = html.find('p').unwrap() ;
        assert_eq!(linked_tag_names(Language::Html, html, p..p), None);
        assert!(is_tag_name_text("ItemGroup") && is_tag_name_text("x:Name") && !is_tag_name_text(" ") && !is_tag_name_text(">"));
    }
}
