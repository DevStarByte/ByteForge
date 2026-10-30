//! Conversions between LSP JSON and the crate's types. Everything here is lenient:
//! malformed input yields `None`/empty results instead of panics.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::snippet::strip_snippet;
use crate::uri::uri_to_path;
use crate::{
    CompletionItem, CompletionKind, Diagnostic, Location, Position, Range, SemanticToken, Severity,
    TextChange,
};

pub(crate) fn position_json(p: Position) -> Value {
    json!({ "line": p.line, "character": p.character })
}

pub(crate) fn range_json(r: Range) -> Value {
    json!({ "start": position_json(r.start), "end": position_json(r.end) })
}

pub(crate) fn change_json(c: &TextChange) -> Value {
    match c.range {
        Some(range) => json!({ "range": range_json(range), "text": c.text }),
        None => json!({ "text": c.text }),
    }
}

fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

pub(crate) fn parse_position(v: &Value) -> Option<Position> {
    Some(Position {
        line: as_u32(v.get("line")?)?,
        character: as_u32(v.get("character")?)?,
    })
}

pub(crate) fn parse_range(v: &Value) -> Option<Range> {
    Some(Range {
        start: parse_position(v.get("start")?)?,
        end: parse_position(v.get("end")?)?,
    })
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Parses `textDocument/publishDiagnostics` params.
pub(crate) fn parse_publish_diagnostics(
    params: &Value,
) -> Option<(PathBuf, Option<i32>, Vec<Diagnostic>)> {
    let path = uri_to_path(params.get("uri")?.as_str()?)?;
    let version = params
        .get("version")
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok());
    let diagnostics = parse_diagnostic_list(params.get("diagnostics").unwrap_or(&Value::Null));
    Some((path, version, diagnostics))
}

/// Parses a `Diagnostic[]`, skipping malformed entries.
pub(crate) fn parse_diagnostic_list(items: &Value) -> Vec<Diagnostic> {
    items
        .as_array()
        .map(|items| items.iter().filter_map(parse_diagnostic).collect())
        .unwrap_or_default()
}

fn parse_diagnostic(v: &Value) -> Option<Diagnostic> {
    let severity = match v.get("severity").and_then(Value::as_u64) {
        Some(2) => Severity::Warning,
        Some(3) => Severity::Info,
        Some(4) => Severity::Hint,
        _ => Severity::Error,
    };
    let code = match v.get("code") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    Some(Diagnostic {
        range: parse_range(v.get("range")?)?,
        severity,
        message: str_field(v, "message").unwrap_or_default(),
        code,
    })
}

fn completion_kind(n: u64) -> CompletionKind {
    use CompletionKind::*;
    match n {
        2 => Method,
        3 => Function,
        4 => Constructor,
        5 => Field,
        6 => Variable,
        7 => Class,
        8 => Interface,
        9 => Module,
        10 => Property,
        11 => Unit,
        12 => Value,
        13 => Enum,
        14 => Keyword,
        15 => Snippet,
        16 => Color,
        17 => File,
        18 => Reference,
        19 => Folder,
        20 => EnumMember,
        21 => Constant,
        22 => Struct,
        23 => Event,
        24 => Operator,
        25 => TypeParameter,
        _ => Text,
    }
}

/// Parses a completion result (`CompletionList`, `CompletionItem[]` or `null`).
pub(crate) fn parse_completion(result: &Value) -> Vec<CompletionItem> {
    let (items, defaults) = match result {
        Value::Array(items) => (items.as_slice(), None),
        Value::Object(_) => (
            result
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            result.get("itemDefaults"),
        ),
        _ => (&[][..], None),
    };
    let default_range = defaults
        .and_then(|d| d.get("editRange"))
        .and_then(range_or_insert);
    let default_format = defaults
        .and_then(|d| d.get("insertTextFormat"))
        .and_then(Value::as_u64);
    items
        .iter()
        .filter_map(|item| parse_completion_item(item, default_range, default_format))
        .collect()
}

/// A `Range`, or the `insert` range of an `InsertReplaceEdit`/`editRange` pair.
fn range_or_insert(v: &Value) -> Option<Range> {
    parse_range(v).or_else(|| v.get("insert").and_then(parse_range))
}

fn parse_completion_item(
    item: &Value,
    default_range: Option<Range>,
    default_format: Option<u64>,
) -> Option<CompletionItem> {
    let label = str_field(item, "label")?;
    let is_snippet = item
        .get("insertTextFormat")
        .and_then(Value::as_u64)
        .or(default_format)
        == Some(2);
    let text_edit = item.get("textEdit");
    let (edit_text, text_edit_range) = match text_edit {
        Some(edit) => (
            str_field(edit, "newText"),
            edit.get("range")
                .and_then(parse_range)
                .or_else(|| range_or_insert(edit)),
        ),
        None => (str_field(item, "textEditText"), default_range),
    };
    let raw = edit_text
        .or_else(|| str_field(item, "insertText"))
        .unwrap_or_else(|| label.clone());
    let insert_text = if is_snippet { strip_snippet(&raw) } else { raw };
    Some(CompletionItem {
        filter_text: str_field(item, "filterText").unwrap_or_else(|| label.clone()),
        sort_text: str_field(item, "sortText").unwrap_or_else(|| label.clone()),
        kind: item
            .get("kind")
            .and_then(Value::as_u64)
            .map_or(CompletionKind::Text, completion_kind),
        detail: str_field(item, "detail").filter(|d| !d.is_empty()),
        text_edit_range,
        insert_text,
        label,
    })
}

/// Parses a definition result (`Location`, `Location[]`, `LocationLink[]` or `null`).
pub(crate) fn parse_locations(result: &Value) -> Vec<Location> {
    let one = |v: &Value| -> Option<Location> {
        if let Some(target) = v.get("targetUri") {
            let range = v
                .get("targetSelectionRange")
                .or_else(|| v.get("targetRange"))?;
            return Some(Location {
                path: uri_to_path(target.as_str()?)?,
                range: parse_range(range)?,
            });
        }
        Some(Location {
            path: uri_to_path(v.get("uri")?.as_str()?)?,
            range: parse_range(v.get("range")?)?,
        })
    };
    match result {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Flattens hover contents (`MarkupContent`, `MarkedString` or `MarkedString[]`) to text.
pub(crate) fn flatten_hover(result: &Value) -> Option<String> {
    fn marked(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(_) => {
                let value = str_field(v, "value")?;
                Some(match v.get("language").and_then(Value::as_str) {
                    Some(lang) => format!("```{lang}\n{value}\n```"),
                    None => value,
                })
            }
            Value::Array(parts) => {
                let parts: Vec<String> = parts
                    .iter()
                    .filter_map(marked)
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                Some(parts.join("\n\n"))
            }
            _ => None,
        }
    }
    marked(result.get("contents")?).filter(|s| !s.trim().is_empty())
}

/// Decodes the relative `data` array of a `SemanticTokens` result into absolute tokens,
/// dropping token types that have no theme mapping.
pub(crate) fn decode_semantic_tokens(data: &[u32], legend: &[String]) -> Vec<SemanticToken> {
    let mut tokens = Vec::with_capacity(data.len() / 5);
    let (mut line, mut start) = (0u32, 0u32);
    for chunk in data.chunks_exact(5) {
        let [delta_line, delta_start, length, token_type, _modifiers] = chunk else {
            continue;
        };
        if *delta_line > 0 {
            line = line.saturating_add(*delta_line);
            start = *delta_start;
        } else {
            start = start.saturating_add(*delta_start);
        }
        let kind = legend
            .get(*token_type as usize)
            .and_then(|name| map_token_type(name));
        if let Some(kind) = kind {
            tokens.push(SemanticToken {
                line,
                start,
                length: *length,
                kind,
            });
        }
    }
    tokens
}

/// Roslyn classification names that are passed through unchanged.
const ROSLYN_NAMES: &[&str] = &[
    "class name",
    "record class name",
    "struct name",
    "record struct name",
    "interface name",
    "enum name",
    "enum member name",
    "delegate name",
    "type parameter name",
    "module name",
    "method name",
    "extension method name",
    "property name",
    "field name",
    "event name",
    "constant name",
    "local name",
    "parameter name",
    "namespace name",
    "label name",
    "keyword",
    "keyword - control",
    "operator",
    "operator - overloaded",
    "string",
    "string - verbatim",
    "string - escape character",
    "number",
    "comment",
    "preprocessor keyword",
    "preprocessor text",
    "excluded code",
    "punctuation",
    "identifier",
    "xml doc comment - attribute name",
    "xml doc comment - attribute quotes",
    "xml doc comment - attribute value",
    "xml doc comment - cdata section",
    "xml doc comment - comment",
    "xml doc comment - delimiter",
    "xml doc comment - entity reference",
    "xml doc comment - name",
    "xml doc comment - processing instruction",
    "xml doc comment - text",
    "regex - comment",
    "regex - character class",
    "regex - anchor",
    "regex - quantifier",
    "regex - grouping",
    "regex - alternation",
    "regex - text",
    "regex - self escaped character",
    "regex - other escape",
    "json - comment",
    "json - number",
    "json - string",
    "json - keyword",
    "json - text",
    "json - operator",
    "json - punctuation",
    "json - array",
    "json - object",
    "json - property name",
    "json - constructor name",
];

/// Maps an LSP or Roslyn semantic token type name to the IDE theme name.
pub(crate) fn map_token_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "class" | "type" | "record" => "class name",
        "struct" | "recordStruct" => "struct name",
        "interface" => "interface name",
        "enum" => "enum name",
        "enumMember" => "enum member name",
        "typeParameter" => "type parameter name",
        "method" | "function" | "extensionMethod" => "method name",
        "property" => "property name",
        "field" => "field name",
        "constant" => "constant name",
        "event" => "event name",
        "variable" | "local" => "local name",
        "parameter" => "parameter name",
        "namespace" => "namespace name",
        "keyword" | "modifier" => "keyword",
        "controlKeyword" => "keyword - control",
        "string" | "regexp" => "string",
        "number" => "number",
        "comment" => "comment",
        "operator" => "operator",
        "delegate" => "delegate name",
        "macro" | "preprocessor" => "preprocessor keyword",
        "label" => "label name",
        "stringEscapeCharacter" => "string - escape character",
        "regexAlternation" => "regex - alternation",
        "regexAnchor" => "regex - anchor",
        "regexCharacterClass" => "regex - character class",
        "regexComment" => "regex - comment",
        "regexGrouping" => "regex - grouping",
        "regexOtherEscape" => "regex - other escape",
        "regexQuantifier" => "regex - quantifier",
        "regexSelfEscapedCharacter" => "regex - self escaped character",
        "regexText" => "regex - text",
        "jsonArray" => "json - array",
        "jsonComment" => "json - comment",
        "jsonConstructorName" => "json - constructor name",
        "jsonKeyword" => "json - keyword",
        "jsonNumber" => "json - number",
        "jsonObject" => "json - object",
        "jsonOperator" => "json - operator",
        "jsonPropertyName" => "json - property name",
        "jsonPunctuation" => "json - punctuation",
        "jsonString" => "json - string",
        "jsonText" => "json - text",
        "static symbol" => return None,
        other => return ROSLYN_NAMES.iter().find(|n| **n == other).copied(),
    })
}

/// Extracts a semantic tokens legend (`{tokenTypes, tokenModifiers}`) from a provider object.
pub(crate) fn parse_legend(provider: &Value) -> Option<Vec<String>> {
    let types = provider.get("legend")?.get("tokenTypes")?.as_array()?;
    Some(
        types
            .iter()
            .map(|t| t.as_str().unwrap_or_default().to_owned())
            .collect(),
    )
}

/// Extracts the `data` array of a `SemanticTokens` result.
pub(crate) fn semantic_data(result: &Value) -> Vec<u32> {
    result
        .get("data")
        .and_then(Value::as_array)
        .map(|d| d.iter().map(|n| as_u32(n).unwrap_or(0)).collect())
        .unwrap_or_default()
}

/// Turns `$/progress` notifications into human readable strings.
#[derive(Default)]
pub(crate) struct ProgressTracker {
    titles: HashMap<String, String>,
}

impl ProgressTracker {
    pub fn handle(&mut self, params: &Value) -> Option<String> {
        let token = params.get("token")?.to_string();
        let value = params.get("value")?;
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty());
        let percentage = value.get("percentage").and_then(Value::as_u64);
        let title = match value.get("kind").and_then(Value::as_str)? {
            "begin" => {
                let title = str_field(value, "title").unwrap_or_default();
                self.titles.insert(token, title.clone());
                title
            }
            "report" => self.titles.get(&token).cloned().unwrap_or_default(),
            "end" => {
                let title = self.titles.remove(&token).unwrap_or_default();
                return Some(join(&title, Some(message.unwrap_or("done")), None));
            }
            _ => return None,
        };
        Some(join(&title, message, percentage))
    }
}

fn join(title: &str, message: Option<&str>, percentage: Option<u64>) -> String {
    let mut out = title.to_owned();
    if let Some(m) = message {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(m);
    }
    if let Some(p) = percentage {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{p}%"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn semantic_token_delta_decoding() {
        let legend: Vec<String> = [
            "class",
            "static symbol",
            "variable",
            "record class name",
            "bogus",
        ]
        .map(String::from)
        .to_vec();
        #[rustfmt::skip]
        let data = [
            1, 4, 5, 0, 0,   // line 1, col 4: class
            0, 6, 3, 2, 1,   // line 1, col 10: variable
            0, 0, 3, 1, 0,   // same position: static symbol -> dropped
            2, 2, 7, 3, 0,   // line 3, col 2: Roslyn name passthrough
            0, 8, 1, 4, 0,   // unknown -> dropped, but still advances
            0, 2, 1, 2, 0,   // line 3, col 12
            0, 0, 1, 99, 0,  // type index out of range -> dropped
        ];
        let tokens = decode_semantic_tokens(&data, &legend);
        let t = |line, start, length, kind| SemanticToken {
            line,
            start,
            length,
            kind,
        };
        assert_eq!(
            tokens,
            vec![
                t(1, 4, 5, "class name"),
                t(1, 10, 3, "local name"),
                t(3, 2, 7, "record class name"),
                t(3, 12, 1, "local name"),
            ]
        );
        // Truncated trailing data is ignored.
        assert_eq!(decode_semantic_tokens(&[0, 1, 2], &legend), vec![]);
    }

    #[test]
    fn completion_list_and_array() {
        let list = json!({
            "isIncomplete": false,
            "items": [
                { "label": "WriteLine", "kind": 2, "detail": "void Console.WriteLine()", "sortText": "0001" },
                { "label": "Foo", "insertText": "Foo(${1:x})$0", "insertTextFormat": 2, "filterText": "foo" },
                { "label": "Bar", "insertText": "Bar$1", "textEdit": {
                    "range": { "start": pos_json(2, 3), "end": pos_json(2, 5) }, "newText": "Baz" } },
                { "label": "Ins", "textEdit": { "insert": { "start": pos_json(0, 0), "end": pos_json(0, 1) },
                    "replace": { "start": pos_json(0, 0), "end": pos_json(0, 3) }, "newText": "Ins()" } },
                { "nolabel": true }
            ]
        });
        let items = parse_completion(&list);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].insert_text, "WriteLine");
        assert_eq!(items[0].kind, CompletionKind::Method);
        assert_eq!(items[0].sort_text, "0001");
        assert_eq!(items[0].filter_text, "WriteLine");
        assert_eq!(items[1].insert_text, "Foo(x)");
        assert_eq!(items[1].filter_text, "foo");
        assert_eq!(items[1].kind, CompletionKind::Text);
        assert_eq!(items[2].insert_text, "Baz");
        assert_eq!(
            items[2].text_edit_range,
            Some(Range {
                start: pos(2, 3),
                end: pos(2, 5)
            })
        );
        assert_eq!(
            items[3].text_edit_range,
            Some(Range {
                start: pos(0, 0),
                end: pos(0, 1)
            })
        );

        let array = json!([{ "label": "x", "kind": 6 }]);
        let items = parse_completion(&array);
        assert_eq!(items[0].kind, CompletionKind::Variable);
        assert!(parse_completion(&Value::Null).is_empty());
    }

    fn pos_json(line: u32, character: u32) -> Value {
        position_json(pos(line, character))
    }

    #[test]
    fn hover_flattening() {
        let markup = json!({ "contents": { "kind": "markdown", "value": "**int** x" } });
        assert_eq!(flatten_hover(&markup).unwrap(), "**int** x");
        let marked = json!({ "contents": [{ "language": "csharp", "value": "int x" }, "doc"] });
        assert_eq!(
            flatten_hover(&marked).unwrap(),
            "```csharp\nint x\n```\n\ndoc"
        );
        assert_eq!(flatten_hover(&json!({ "contents": "" })), None);
        assert_eq!(flatten_hover(&Value::Null), None);
    }

    #[test]
    fn locations_variants() {
        let range = json!({ "start": pos_json(1, 2), "end": pos_json(1, 4) });
        let uri = crate::path_to_uri(&std::env::temp_dir().join("a.cs"));
        assert_eq!(
            parse_locations(&json!({ "uri": uri, "range": range })).len(),
            1
        );
        assert_eq!(
            parse_locations(&json!([{ "uri": uri, "range": range }, { "bad": 1 }])).len(),
            1
        );
        let link =
            json!([{ "targetUri": uri, "targetRange": range, "targetSelectionRange": range }]);
        assert_eq!(parse_locations(&link)[0].range.start, pos(1, 2));
        assert!(parse_locations(&Value::Null).is_empty());
    }

    #[test]
    fn progress_strings() {
        let mut p = ProgressTracker::default();
        let begin = json!({ "token": "t", "value": { "kind": "begin", "title": "Loading solution", "percentage": 0 } });
        assert_eq!(p.handle(&begin).unwrap(), "Loading solution 0%");
        let report = json!({ "token": "t", "value": { "kind": "report", "message": "Foo.csproj", "percentage": 40 } });
        assert_eq!(
            p.handle(&report).unwrap(),
            "Loading solution: Foo.csproj 40%"
        );
        let end = json!({ "token": "t", "value": { "kind": "end" } });
        assert_eq!(p.handle(&end).unwrap(), "Loading solution: done");
    }
}
