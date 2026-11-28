//! Avalonia's AXAML views for the designer: the element tree of a `.axaml` file, read with where every
//! element and attribute is, and edits that change only the text they touch (the rest of the file, its
//! comments and its formatting stay byte for byte). Elements are found by their path: the indexes of the
//! children (not property elements like `<Grid.RowDefinitions>`) from the root down.

mod catalog;
mod layout;
mod values;
#[cfg(test)]
mod tests;

use std::ops::Range;

pub use catalog::*;
pub use layout::*;
pub use values::*;

/// Where an element is: the child indexes from the root (the root is `[]`).
pub type ElPath = Vec<usize>;

#[derive(Clone, Debug, PartialEq)]
pub struct Attr {
    /// As written (`x:Name`, `Grid.Row`, `d:DesignWidth`).
    pub name: String,
    pub value: String,
    /// The whole attribute (`name="value"`).
    pub range: Range<usize>,
    /// Its value, inside the quotes.
    pub value_range: Range<usize>,
}

/// A property element (`<Grid.RowDefinitions>`): kept as it is, read for the few the designer uses.
#[derive(Clone, Debug, PartialEq)]
pub struct PropElement {
    /// The property (`RowDefinitions` of `<Grid.RowDefinitions>`, `Tip` of `<ToolTip.Tip>`), with its owner.
    pub owner: String,
    pub name: String,
    pub children: Vec<Element>,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// The tag as written (`Button`, `local:MyControl`).
    pub tag: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Element>,
    pub props: Vec<PropElement>,
    /// Text written inside it (trimmed), when there is no child element.
    pub text: Option<String>,
    /// From `<` to the end of its closing tag (or of `/>`).
    pub range: Range<usize>,
    /// Just past the `>` of its start tag.
    pub start_tag_end: usize,
    pub self_closing: bool,
    /// Its 0-based line.
    pub line: usize,
}

impl Element {
    pub fn local(&self) -> &str {
        local_name(&self.tag)
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.name == name).map(|a| a.value.as_str())
    }

    pub fn info(&self) -> Option<&'static Control> {
        control(&self.tag)
    }

    /// Its x:Name (or Name).
    pub fn name(&self) -> Option<&str> {
        self.attr("x:Name").or_else(|| self.attr("Name"))
    }

    /// How it holds children: what the catalog says, else (an unknown control) by what is written in it.
    pub fn role(&self) -> Role {
        match self.info() {
            Some(c) => c.role,
            None if self.children.len() > 1 => Role::Panel,
            None => Role::Content,
        }
    }

    /// The text of a property: its attribute, or (its content property) the text written inside it.
    pub fn value(&self, prop: &str) -> Option<String> {
        if let Some(v) = self.attr(prop) {
            return Some(v.to_owned());
        }
        let content = self.info().map_or("Content", |c| c.content_prop);
        if !content.is_empty() && prop == content { self.text.clone() } else { None }
    }

    /// The rows or columns of a Grid (`RowDefinitions="Auto,*"` or `<Grid.RowDefinitions>`).
    pub fn definitions(&self, rows: bool) -> Vec<GridLen> {
        let (attr, prop, size) = if rows { ("RowDefinitions", "RowDefinitions", "Height") } else { ("ColumnDefinitions", "ColumnDefinitions", "Width") };
        if let Some(v) = self.attr(attr) {
            return parse_grid_lengths(v);
        }
        self.props.iter().find(|p| p.name == prop).map_or_else(Vec::new, |p| p.children.iter().map(|d| d.attr(size).map_or(GridLen::Star(1.0), parse_grid_len)).collect())
    }

    /// A short label for the outline: `Button "OK"`, `StackPanel buttons`.
    pub fn label(&self) -> String {
        if let Some(n) = self.name() {
            return format!("{} {n}", self.tag);
        }
        let content = self.info().map_or("Content", |c| c.content_prop);
        let text = self.value(if content.is_empty() { "Header" } else { content }).or_else(|| self.attr("Header").map(str::to_owned));
        match text.filter(|t| !t.is_empty()) {
            Some(t) => {
                let t: String = t.chars().take(24).collect();
                format!("{} \"{t}\"", self.tag)
            }
            None => self.tag.clone(),
        }
    }
}

/// A parsed `.axaml` file.
#[derive(Clone, Debug)]
pub struct Doc {
    pub text: String,
    pub root: Element,
    /// One level of indentation, as the file uses it.
    pub indent: String,
    /// The namespaces the root declares: (prefix, URI); the default one has no prefix.
    pub xmlns: Vec<(Option<String>, String)>,
}

/// Where a control goes when it is dropped: into `parent` at `index` among its children, with these
/// attributes set (`None`: removed).
#[derive(Clone, Debug, PartialEq)]
pub struct Drop {
    pub parent: ElPath,
    pub index: usize,
    pub attrs: Vec<(String, Option<String>)>,
    /// A moved control stays where it is among its siblings (a Canvas or Grid only changes its attributes).
    pub in_place: bool,
}

impl Doc {
    pub fn parse(text: &str) -> Result<Doc, String> {
        let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let xml = roxmltree::Document::parse_with_options(text, opts).map_err(|e| e.to_string())?;
        let root = read_element(text, xml.root_element());
        let indent = detect_indent(text, &root);
        let xmlns = xml.root_element().namespaces().filter(|n| n.name() != Some("xml")).map(|n| (n.name().map(str::to_owned), n.uri().to_owned())).collect();
        Ok(Doc { text: text.to_owned(), root, indent, xmlns })
    }

    pub fn get(&self, path: &[usize]) -> Option<&Element> {
        let mut el = &self.root;
        for &i in path {
            el = el.children.get(i)?;
        }
        Some(el)
    }

    pub fn parent<'a>(&self, path: &'a [usize]) -> Option<(&Element, &'a [usize])> {
        let (_, up) = path.split_last()?;
        Some((self.get(up)?, up))
    }

    /// The class the view's code-behind declares (`x:Class="App.Views.MainWindow"` → `MainWindow`).
    pub fn class_name(&self) -> Option<&str> {
        self.root.attr("x:Class").map(|c| c.rsplit('.').next().unwrap_or(c))
    }

    /// Every element in document order with its path (the root first).
    pub fn all(&self) -> Vec<(ElPath, &Element)> {
        fn walk<'a>(el: &'a Element, path: &mut ElPath, out: &mut Vec<(ElPath, &'a Element)>) {
            out.push((path.clone(), el));
            for (i, c) in el.children.iter().enumerate() {
                path.push(i);
                walk(c, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut out);
        out
    }

    /// The element whose start tag (or whose text) holds the 0-based `line`, the innermost one.
    pub fn at_line(&self, line: usize) -> Option<ElPath> {
        let start = line_start(&self.text, line);
        let end = self.text[start..].find('\n').map_or(self.text.len(), |p| start + p);
        self.all().into_iter().filter(|(_, e)| e.range.start <= end && e.range.end > start).map(|(p, _)| p).max_by_key(Vec::len)
    }

    /// Whether `inner` is `outer` or inside it.
    pub fn is_within(inner: &[usize], outer: &[usize]) -> bool {
        inner.starts_with(outer)
    }

    // ───────────── edits (each returns the new text) ─────────────

    /// Sets attributes of an element (`None` removes one).
    pub fn set_attrs(&self, path: &[usize], changes: &[(String, Option<String>)]) -> Result<String, String> {
        let el = self.get(path).ok_or("The element isn't there anymore.")?;
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let mut added: Vec<(String, String)> = Vec::new();
        for (name, value) in changes {
            // The content property written as text inside the element: the text changes.
            let content = el.info().map_or("Content", |c| c.content_prop);
            if el.attr(name).is_none() && !content.is_empty() && name == content && el.text.is_some() && el.children.is_empty() {
                let inner = self.inner_range(el);
                edits.push((inner, value.as_deref().map(escape_text).unwrap_or_default()));
                continue;
            }
            match (el.attrs.iter().find(|a| a.name == *name), value) {
                (Some(a), Some(v)) => edits.push((a.value_range.clone(), escape_attr(v))),
                (Some(a), None) => {
                    // The attribute and the space before it (its line, when it is on a line of its own).
                    let before = self.text[..a.range.start].trim_end_matches([' ', '\t', '\r', '\n']).len();
                    edits.push((before..a.range.end, String::new()));
                }
                (None, Some(v)) => added.push((name.clone(), v.clone())),
                (None, None) => {}
            }
        }
        if !added.is_empty() {
            // Attributes on lines of their own get new lines; else they go on the start tag's line. A name
            // goes first, everything else last.
            let last = el.attrs.last();
            let sep = match last {
                Some(a) => {
                    let ws_start = self.text[..a.range.start].trim_end_matches([' ', '\t', '\r', '\n']).len();
                    let ws = &self.text[ws_start..a.range.start];
                    if ws.contains('\n') { ws.to_owned() } else { " ".to_owned() }
                }
                None => " ".to_owned(),
            };
            let tag_end = el.range.start + 1 + el.tag.len();
            let (names, rest): (Vec<_>, Vec<_>) = added.into_iter().partition(|(n, _)| n == "x:Name" || n == "Name");
            if !names.is_empty() {
                let text: String = names.iter().map(|(n, v)| format!("{sep}{n}=\"{}\"", escape_attr(v))).collect();
                edits.push((tag_end..tag_end, text));
            }
            if !rest.is_empty() {
                let at = last.map_or(tag_end, |a| a.range.end);
                let text: String = rest.iter().map(|(n, v)| format!("{sep}{n}=\"{}\"", escape_attr(v))).collect();
                edits.push((at..at, text));
            }
        }
        Ok(apply(&self.text, edits))
    }

    pub fn set_attr(&self, path: &[usize], name: &str, value: Option<&str>) -> Result<String, String> {
        self.set_attrs(path, &[(name.to_owned(), value.map(str::to_owned))])
    }

    /// Between the start tag and the closing tag.
    fn inner_range(&self, el: &Element) -> Range<usize> {
        if el.self_closing {
            return el.start_tag_end..el.start_tag_end;
        }
        let close = self.text[..el.range.end].rfind("</").unwrap_or(el.range.end);
        el.start_tag_end..close
    }

    /// The range removing an element takes: its lines, when nothing else is on them.
    fn removal_range(&self, el: &Element) -> Range<usize> {
        let line_start = self.text[..el.range.start].rfind('\n').map_or(0, |p| p + 1);
        let before = &self.text[line_start..el.range.start];
        let after_end = self.text[el.range.end..].find('\n').map_or(self.text.len(), |p| el.range.end + p + 1);
        let after = &self.text[el.range.end..after_end];
        if before.trim().is_empty() && after.trim().is_empty() { line_start..after_end } else { el.range.clone() }
    }

    /// Deletes elements (the root stays).
    pub fn remove(&self, paths: &[ElPath]) -> Result<String, String> {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for p in paths.iter().filter(|p| !p.is_empty()) {
            // What is inside another removed element goes with it.
            if paths.iter().any(|o| o != p && !o.is_empty() && Self::is_within(p, o)) {
                continue;
            }
            let el = self.get(p).ok_or("The element isn't there anymore.")?;
            ranges.push(self.removal_range(el));
        }
        Ok(apply(&self.text, ranges.into_iter().map(|r| (r, String::new())).collect()))
    }

    /// The indentation of the line an element starts on.
    fn indent_of(&self, el: &Element) -> String {
        let line_start = self.text[..el.range.start].rfind('\n').map_or(0, |p| p + 1);
        self.text[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    }

    /// An element's source, its lines after the first without the element's own indentation.
    pub fn source(&self, path: &[usize]) -> Option<String> {
        let el = self.get(path)?;
        let indent = self.indent_of(el);
        Some(reindent(&self.text[el.range.clone()], &indent, ""))
    }

    /// Inserts XAML (an element, its lines indented relative to its first) as the `index`th child of `parent`.
    pub fn insert(&self, parent: &[usize], index: usize, xaml: &str) -> Result<String, String> {
        let el = self.get(parent).ok_or("The container isn't there anymore.")?;
        let parent_indent = self.indent_of(el);
        let child_indent = format!("{parent_indent}{}", self.indent);
        let body = reindent(xaml.trim(), "", &child_indent);
        let edit = if el.self_closing {
            // `<StackPanel />` → `<StackPanel>` … `</StackPanel>`.
            let slash = self.text[..el.start_tag_end].rfind('/').unwrap_or(el.start_tag_end);
            let from = self.text[..slash].trim_end().len();
            (from..el.start_tag_end, format!(">\n{child_indent}{body}\n{parent_indent}</{}>", el.tag))
        } else if let Some(next) = el.children.get(index) {
            let line_start = self.text[..next.range.start].rfind('\n').map_or(0, |p| p + 1);
            if self.text[line_start..next.range.start].trim().is_empty() {
                (line_start..line_start, format!("{child_indent}{body}\n"))
            } else {
                (next.range.start..next.range.start, format!("{body} "))
            }
        } else {
            let close = self.inner_range(el).end;
            let line_start = self.text[..close].rfind('\n').map_or(0, |p| p + 1);
            let inner = &self.text[el.start_tag_end..close];
            if inner.trim().is_empty() {
                // Empty (`<Panel></Panel>`, or only blank lines): the child on its own line.
                (el.start_tag_end..close, format!("\n{child_indent}{body}\n{parent_indent}"))
            } else if self.text[line_start..close].trim().is_empty() && line_start > el.start_tag_end {
                (line_start..line_start, format!("{child_indent}{body}\n"))
            } else {
                (close..close, format!("\n{child_indent}{body}\n{parent_indent}"))
            }
        };
        Ok(apply(&self.text, vec![edit]))
    }

    /// Replaces the text written inside an element (its content) with a child element.
    pub fn replace_text_with(&self, path: &[usize], xaml: &str) -> Result<String, String> {
        let el = self.get(path).ok_or("The element isn't there anymore.")?;
        let indent = self.indent_of(el);
        let child_indent = format!("{indent}{}", self.indent);
        let body = reindent(xaml.trim(), "", &child_indent);
        Ok(apply(&self.text, vec![(self.inner_range(el), format!("\n{child_indent}{body}\n{indent}"))]))
    }

    /// Puts new XAML where a drop says; returns the text and the new element's path.
    pub fn insert_at(&self, drop: &Drop, xaml: &str) -> Result<(String, ElPath), String> {
        let text = self.insert(&drop.parent, drop.index, xaml)?;
        let mut path = drop.parent.clone();
        path.push(drop.index);
        if drop.attrs.is_empty() {
            return Ok((text, path));
        }
        let doc = Doc::parse(&text)?;
        Ok((doc.set_attrs(&path, &drop.attrs)?, path))
    }

    /// Moves an element where a drop says; returns the text and the element's new path.
    pub fn move_to(&self, path: &[usize], drop: &Drop) -> Result<(String, ElPath), String> {
        if path.is_empty() {
            return Err("The root can't be moved.".into());
        }
        if Self::is_within(&drop.parent, path) {
            return Err("A control can't go inside itself.".into());
        }
        let text = self.set_attrs(path, &drop.attrs)?;
        let (old_index, old_parent) = path.split_last().map(|(i, p)| (*i, p.to_vec())).unwrap_or_default();
        let same_parent = old_parent == drop.parent;
        if drop.in_place || (same_parent && (drop.index == old_index || drop.index == old_index + 1)) {
            return Ok((text, path.to_vec()));
        }
        let doc = Doc::parse(&text)?;
        let source = doc.source(path).ok_or("The element isn't there anymore.")?;
        let removed = Doc::parse(&doc.remove(&[path.to_vec()])?)?;
        let parent = path_after_removal(path, &drop.parent).ok_or("A control can't go inside itself.")?;
        let index = if same_parent && old_index < drop.index { drop.index - 1 } else { drop.index };
        let text = removed.insert(&parent, index, &source)?;
        let mut new_path = parent;
        new_path.push(index);
        Ok((text, new_path))
    }

    /// Whether the root declares a namespace prefix.
    pub fn has_prefix(&self, prefix: &str) -> bool {
        self.xmlns.iter().any(|(p, _)| p.as_deref() == Some(prefix))
    }

    /// Whether XAML is one element this file could hold (its prefixes are declared on the root).
    pub fn accepts(&self, xaml: &str) -> bool {
        let xmlns: String = self
            .xmlns
            .iter()
            .map(|(prefix, uri)| match prefix {
                Some(p) => format!(" xmlns:{p}=\"{}\"", escape_attr(uri)),
                None => format!(" xmlns=\"{}\"", escape_attr(uri)),
            })
            .collect();
        let wrapped = format!("<Root{xmlns}>{}</Root>", xaml.trim());
        roxmltree::Document::parse(&wrapped).is_ok_and(|d| d.root_element().children().filter(|c| c.is_element()).count() == 1)
    }
}

/// Where `path` is after the element at `removed` was taken out (`None`: it was inside it).
pub fn path_after_removal(removed: &[usize], path: &[usize]) -> Option<ElPath> {
    if path.starts_with(removed) {
        return None;
    }
    let mut out = path.to_vec();
    let (last, parent) = removed.split_last()?;
    if path.len() > parent.len() && path.starts_with(parent) && path[parent.len()] > *last {
        out[parent.len()] -= 1;
    }
    Some(out)
}

/// Where `path` is after an element was put in at `inserted`.
pub fn path_after_insert(inserted: &[usize], path: &[usize]) -> ElPath {
    let mut out = path.to_vec();
    if let Some((last, parent)) = inserted.split_last()
        && path.len() > parent.len()
        && path.starts_with(parent)
        && path[parent.len()] >= *last
    {
        out[parent.len()] += 1;
    }
    out
}

/// Applies non-overlapping edits (in any order) to `text`.
fn apply(text: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
    let mut out = text.to_owned();
    for (r, s) in edits {
        out.replace_range(r, &s);
    }
    out
}

/// The text with `old` taken off the start of each line after the first and `new` put there.
fn reindent(text: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.trim().is_empty() {
                out.push_str(new);
            }
            out.push_str(line.strip_prefix(old).unwrap_or(line.trim_start()));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// A toolbox snippet (one element per line, none indented) indented by nesting with `unit`.
pub fn indent_snippet(snippet: &str, unit: &str) -> String {
    let mut depth = 0usize;
    let mut out = Vec::new();
    for line in snippet.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("</") {
            depth = depth.saturating_sub(1);
        }
        out.push(format!("{}{line}", unit.repeat(depth)));
        if line.starts_with('<') && !line.starts_with("</") && !line.ends_with("/>") && !line.contains("</") {
            depth += 1;
        }
    }
    out.join("\n")
}

fn line_start(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    text.match_indices('\n').nth(line - 1).map_or(text.len(), |(i, _)| i + 1)
}

fn read_element(text: &str, node: roxmltree::Node) -> Element {
    let range = node.range();
    let tag: String = text[range.start + 1..].chars().take_while(|c| !c.is_whitespace() && *c != '/' && *c != '>').collect();
    let attrs: Vec<Attr> = node
        .attributes()
        .map(|a| Attr { name: text[a.range_qname()].to_owned(), value: a.value().to_owned(), range: a.range(), value_range: a.range_value() })
        .collect();
    // The start tag ends at the first `>` outside quotes after the tag name and attributes.
    let scan_from = attrs.last().map_or(range.start + 1 + tag.len(), |a| a.range.end);
    let gt = text[scan_from..range.end].find('>').map_or(range.end, |p| scan_from + p + 1);
    let self_closing = text[..gt].ends_with("/>");
    let mut children = Vec::new();
    let mut props = Vec::new();
    let mut text_parts = String::new();
    for c in node.children() {
        if c.is_element() {
            let ctag: String = text[c.range().start + 1..].chars().take_while(|ch| !ch.is_whitespace() && *ch != '/' && *ch != '>').collect();
            let local = local_name(&ctag);
            if let Some((owner, prop)) = local.split_once('.') {
                props.push(PropElement { owner: owner.to_owned(), name: prop.to_owned(), children: c.children().filter(|g| g.is_element()).map(|g| read_element(text, g)).collect(), range: c.range() });
            } else {
                children.push(read_element(text, c));
            }
        } else if c.is_text() {
            text_parts.push_str(c.text().unwrap_or_default());
        }
    }
    let trimmed = text_parts.trim();
    Element {
        tag,
        attrs,
        text: (children.is_empty() && !trimmed.is_empty()).then(|| trimmed.to_owned()),
        children,
        props,
        line: text[..range.start].matches('\n').count(),
        start_tag_end: gt,
        self_closing,
        range,
    }
}

/// The file's indentation step: the difference between the root's and its first indented child's.
fn detect_indent(text: &str, root: &Element) -> String {
    let indent_at = |pos: usize| -> String {
        let ls = text[..pos].rfind('\n').map_or(0, |p| p + 1);
        text[ls..pos].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    };
    let base = indent_at(root.range.start);
    let first = root.children.first().map(|c| c.range.start).or_else(|| root.props.first().map(|p| p.range.start));
    match first.map(indent_at) {
        Some(i) if i.len() > base.len() && i.starts_with(&base) => i[base.len()..].to_owned(),
        _ => "    ".to_owned(),
    }
}
