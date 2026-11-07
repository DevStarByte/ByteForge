//! Incremental tree-sitter syntax highlighting for ByteForge.
//!
//! Produces [`Span`]s whose `kind` strings match the IDE's theme token names
//! (Roslyn classification names), so existing theme files keep working.

use std::ops::Range;
use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, Tree};

const CSHARP_QUERY: &str = include_str!("../queries/csharp.scm");
const JSON_QUERY: &str = include_str!("../queries/json.scm");
const XML_QUERY: &str = include_str!("../queries/xml.scm");
const DOCKERFILE_QUERY: &str = include_str!("../queries/dockerfile.scm");
const YAML_QUERY: &str = include_str!("../queries/yaml.scm");
/// Razor embeds C# with the C# grammar's node names, so its query is the C# one plus Razor rules.
const RAZOR_QUERY: &str = concat!(include_str!("../queries/csharp.scm"), "\n", include_str!("../queries/razor.scm"));

/// Maps query capture names to theme kind names. Captures not listed here
/// (e.g. `@_helper`) only serve predicates and produce no spans.
const CAPTURE_KINDS: &[(&str, &str)] = &[
    ("keyword", "keyword"),
    ("keyword_control", "keyword - control"),
    ("comment", "comment"),
    ("xml_doc_comment", "xml doc comment"),
    ("string", "string"),
    ("string_verbatim", "string - verbatim"),
    ("string_escape", "string - escape character"),
    ("number", "number"),
    ("operator", "operator"),
    ("punctuation", "punctuation"),
    ("preprocessor_keyword", "preprocessor keyword"),
    ("namespace_name", "namespace name"),
    ("class_name", "class name"),
    ("record_class_name", "record class name"),
    ("struct_name", "struct name"),
    ("interface_name", "interface name"),
    ("enum_name", "enum name"),
    ("delegate_name", "delegate name"),
    ("type_parameter_name", "type parameter name"),
    ("method_name", "method name"),
    ("property_name", "property name"),
    ("field_name", "field name"),
    ("event_name", "event name"),
    ("local_name", "local name"),
    ("parameter_name", "parameter name"),
    ("enum_member_name", "enum member name"),
    ("constant_name", "constant name"),
    ("label_name", "label name"),
    ("identifier", "identifier"),
    ("json_property", "json - property name"),
    ("json_string", "json - string"),
    ("razor_transition", "razor transition"),
    ("razor_directive", "razor directive"),
    ("html_element_name", "html element name"),
    ("html_attribute_name", "html attribute name"),
    ("html_attribute_value", "html attribute value"),
    ("html_tag_delimiter", "html tag delimiter"),
    ("msbuild_reference", "xml - msbuild reference"),
];

/// A language supported by the highlighter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    CSharp,
    Json,
    /// ASP.NET Core Razor: `.razor` components and `.cshtml` views/pages.
    Razor,
    /// XML, including MSBuild projects (`.csproj`, `.props`, `.targets`) and .NET resource/config files.
    Xml,
    /// Dockerfiles and Containerfiles (`Dockerfile`, `Dockerfile.dev`, `app.dockerfile`, `Containerfile`).
    Dockerfile,
    /// YAML, including Docker Compose files.
    Yaml,
}

impl Language {
    /// Detects the language from a file name or extension (`.cs`/`.csx` → C#, `.json`/`.jsonc` → JSON,
    /// `.razor`/`.cshtml` → Razor, `.csproj`/`.props`/`.xml`/… → XML, `Dockerfile`/`*.dockerfile` → Dockerfile,
    /// `.yml`/`.yaml` → YAML).
    pub fn from_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        if is_dockerfile_name(&name) {
            return Some(Language::Dockerfile);
        }
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "cs" | "csx" => Some(Language::CSharp),
            "json" | "jsonc" => Some(Language::Json),
            "razor" | "cshtml" => Some(Language::Razor),
            "xml" | "csproj" | "fsproj" | "vbproj" | "proj" | "props" | "targets" | "slnx" | "config" | "resx" | "xaml" | "axaml"
            | "nuspec" | "runsettings" | "manifest" | "xsd" | "xsl" | "xslt" | "svg" | "plist" | "ruleset" | "pubxml" | "dbml" => {
                Some(Language::Xml)
            }
            "yml" | "yaml" => Some(Language::Yaml),
            _ => None,
        }
    }

    /// Human-readable language name.
    pub fn name(self) -> &'static str {
        match self {
            Language::CSharp => "C#",
            Language::Json => "JSON",
            Language::Razor => "Razor",
            Language::Xml => "XML",
            Language::Dockerfile => "Dockerfile",
            Language::Yaml => "YAML",
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Language::Json => tree_sitter_json::LANGUAGE.into(),
            Language::Razor => tree_sitter_razor_modern::LANGUAGE.into(),
            Language::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
            Language::Dockerfile => tree_sitter_containerfile::LANGUAGE.into(),
            Language::Yaml => tree_sitter_yaml::LANGUAGE.into(),
        }
    }

    fn query_source(self) -> &'static str {
        match self {
            Language::CSharp => CSHARP_QUERY,
            Language::Json => JSON_QUERY,
            Language::Razor => RAZOR_QUERY,
            Language::Xml => XML_QUERY,
            Language::Dockerfile => DOCKERFILE_QUERY,
            Language::Yaml => YAML_QUERY,
        }
    }
}

/// `Dockerfile`, `Containerfile`, `Dockerfile.<variant>` (`Dockerfile.dev`), `<name>.Dockerfile` and
/// `<name>.Containerfile`, in any case.
pub fn is_dockerfile_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["dockerfile", "containerfile"].iter().any(|base| {
        name == *base
            || name.strip_prefix(base).is_some_and(|rest| rest.starts_with('.') && rest.len() > 1)
            || name.strip_suffix(base).is_some_and(|rest| rest.ends_with('.') && rest.len() > 1)
    })
}

/// One highlighted region in byte offsets of the full document text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
}

/// A text edit in byte offsets plus `(row, byte-column)` points, mirroring `tree_sitter::InputEdit`.
#[derive(Clone, Copy, Debug)]
pub struct Edit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

impl From<Edit> for InputEdit {
    fn from(e: Edit) -> Self {
        let point = |(row, column): (usize, usize)| Point { row, column };
        InputEdit {
            start_byte: e.start_byte,
            old_end_byte: e.old_end_byte,
            new_end_byte: e.new_end_byte,
            start_position: point(e.start_point),
            old_end_position: point(e.old_end_point),
            new_end_position: point(e.new_end_point),
        }
    }
}

/// Incremental syntax highlighter for a single document.
pub struct Highlighter {
    language: Language,
    parser: Parser,
    tree: Option<Tree>,
    query: Query,
    /// Index into [`CAPTURE_KINDS`] per query capture index (`None` for helper captures).
    capture_kinds: Vec<Option<u8>>,
}

impl Highlighter {
    /// Creates a highlighter for `language`. Panics only if the embedded query is invalid
    /// (guarded by unit tests).
    pub fn new(language: Language) -> Self {
        let ts_language = language.ts_language();
        let mut parser = Parser::new();
        parser
            .set_language(&ts_language)
            .expect("tree-sitter grammar ABI mismatch");
        let query = Query::new(&ts_language, language.query_source())
            .unwrap_or_else(|e| panic!("invalid {} highlight query: {e}", language.name()));
        let capture_kinds = query
            .capture_names()
            .iter()
            .map(|name| {
                CAPTURE_KINDS
                    .iter()
                    .position(|(capture, _)| capture == name)
                    .map(|i| i as u8)
            })
            .collect();
        Self {
            language,
            parser,
            tree: None,
            query,
            capture_kinds,
        }
    }

    /// The language this highlighter was created for.
    pub fn language(&self) -> Language {
        self.language
    }

    /// Full (re)parse of the document.
    pub fn parse(&mut self, text: &str) {
        self.tree = self.parser.parse(text, None);
    }

    /// Applies an edit to the existing tree, then incrementally reparses `new_text`.
    pub fn edit(&mut self, edit: Edit, new_text: &str) {
        match self.tree.as_mut() {
            Some(tree) => {
                tree.edit(&edit.into());
                self.tree = self.parser.parse(new_text, Some(tree));
            }
            None => self.parse(new_text),
        }
    }

    /// Returns spans intersecting `byte_range`, sorted by start, non-overlapping
    /// (innermost capture wins) and clipped to the range.
    pub fn highlight(&self, text: &str, byte_range: Range<usize>) -> Vec<Span> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let start = byte_range.start.min(text.len());
        let end = byte_range.end.min(text.len());
        if start >= end {
            return Vec::new();
        }

        // Collect clipped captures as (start, end, pattern index, kind id).
        let mut captures: Vec<(usize, usize, usize, u8)> = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start..end);
        let mut matches = cursor.matches(&self.query, tree.root_node(), text.as_bytes());
        while let Some(m) = matches.next() {
            for capture in m.captures {
                let Some(kind) = self.capture_kinds[capture.index as usize] else {
                    continue;
                };
                let range = capture.node.byte_range();
                let (s, e) = (range.start.max(start), range.end.min(end));
                if s < e {
                    captures.push((s, e, m.pattern_index, kind));
                }
            }
        }

        // Paint outer captures first so inner ones overwrite them; for identical
        // ranges the later pattern wins. Byte-wise painting of the window is cheap
        // (the window is small) and makes overlap resolution trivial.
        captures.sort_unstable_by(|a, b| {
            let (la, lb) = (a.1 - a.0, b.1 - b.0);
            lb.cmp(&la).then(a.2.cmp(&b.2))
        });
        let mut paint = vec![NO_KIND; end - start];
        for &(s, e, _, kind) in &captures {
            paint[s - start..e - start].fill(kind);
        }
        // Razor markup goes last: the C# rules would otherwise color a tag's `<` and `>` as operators.
        // The lexer only touches bytes outside the elements' child nodes.
        if self.language == Language::Xml {
            paint_msbuild_references(text.as_bytes(), start, &mut paint);
        }
        if self.language == Language::Razor {
            markup::paint(tree.root_node(), text.as_bytes(), start..end, &mut paint, |name| {
                CAPTURE_KINDS.iter().position(|(capture, _)| *capture == name).map_or(NO_KIND, |i| i as u8)
            });
        }

        let mut spans = Vec::new();
        let mut i = 0;
        while i < paint.len() {
            let kind = paint[i];
            let run_start = i;
            while i < paint.len() && paint[i] == kind {
                i += 1;
            }
            if kind != NO_KIND {
                spans.push(Span {
                    start: start + run_start,
                    end: start + i,
                    kind: CAPTURE_KINDS[kind as usize].1,
                });
            }
        }
        spans
    }
}

const NO_KIND: u8 = u8::MAX;

/// Colors MSBuild references — `$(Property)`, `@(Item)`, `%(Metadata)` — in attribute values and text
/// (not in comments or tags). `paint` starts at byte `start`.
fn paint_msbuild_references(text: &[u8], start: usize, paint: &mut [u8]) {
    let kind_id = |name: &str| CAPTURE_KINDS.iter().position(|(capture, _)| *capture == name).map(|i| i as u8);
    let (Some(reference), Some(value)) = (kind_id("msbuild_reference"), kind_id("html_attribute_value")) else { return };
    let mut i = 0;
    while i + 2 < paint.len() {
        let at = start + i;
        if matches!(text[at], b'$' | b'@' | b'%') && text[at + 1] == b'(' && (paint[i] == NO_KIND || paint[i] == value) {
            // Up to the matching parenthesis (property functions nest: $([System.IO.Path]::Combine(…))).
            let mut depth = 0;
            let mut j = i + 1;
            while j < paint.len() {
                match text[start + j] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    b'\n' | b'"' | b'<' => break,
                    _ => {}
                }
                j += 1;
            }
            if j < paint.len() && text[start + j] == b')' {
                paint[i..=j].fill(reference);
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
}

mod markup;

#[cfg(test)]
mod tests;
