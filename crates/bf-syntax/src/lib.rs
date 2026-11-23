//! Incremental tree-sitter syntax highlighting for ByteForge, plus line-based regex highlighting for
//! languages that modules contribute ([`module_lang`]).
//!
//! Produces [`Span`]s whose `kind` strings match the IDE's theme token names
//! (Roslyn classification names), so existing theme files keep working.

use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, Tree};

const CSHARP_QUERY: &str = include_str!("../queries/csharp.scm");
const JSON_QUERY: &str = include_str!("../queries/json.scm");
const XML_QUERY: &str = include_str!("../queries/xml.scm");
const YAML_QUERY: &str = include_str!("../queries/yaml.scm");
/// Razor embeds C# with the C# grammar's node names, so its query is the C# one plus Razor rules.
const RAZOR_QUERY: &str = concat!(include_str!("../queries/csharp.scm"), "\n", include_str!("../queries/razor.scm"));

// The grammars' own queries, written like ByteForge's: general patterns first, a later pattern wins.
// TypeScript's and JSX's add to JavaScript's, so they come after it.
static JAVASCRIPT_QUERY: LazyLock<String> =
    LazyLock::new(|| [tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY].join("\n"));
static TYPESCRIPT_QUERY: LazyLock<String> =
    LazyLock::new(|| [tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY].join("\n"));
static HTML_QUERY: LazyLock<String> = LazyLock::new(|| [tree_sitter_html::HIGHLIGHTS_QUERY, include_str!("../queries/html.scm")].join("\n"));
/// SCSS's query only adds to CSS's (SCSS is a superset of CSS).
static SCSS_QUERY: LazyLock<String> = LazyLock::new(|| [tree_sitter_css::HIGHLIGHTS_QUERY, tree_sitter_scss::HIGHLIGHTS_QUERY].join("\n"));
static TSX_QUERY: LazyLock<String> = LazyLock::new(|| {
    [tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY].join("\n")
});

/// A grammar parsed over some nodes of another: `kinds` (children of `parent` when it isn't empty).
struct Injection {
    parent: &'static str,
    kinds: &'static [&'static str],
    language: fn() -> tree_sitter::Language,
    query: fn() -> &'static str,
}

const HTML_INJECTIONS: [Injection; 2] = [
    Injection { parent: "script_element", kinds: &["raw_text"], language: || tree_sitter_javascript::LANGUAGE.into(), query: || JAVASCRIPT_QUERY.as_str() },
    Injection { parent: "style_element", kinds: &["raw_text"], language: || tree_sitter_css::LANGUAGE.into(), query: || tree_sitter_css::HIGHLIGHTS_QUERY },
];

/// Markdown's inline syntax (emphasis, code spans, links) has a grammar of its own, run over the block
/// grammar's `inline` nodes.
const MARKDOWN_INJECTIONS: [Injection; 1] = [Injection {
    parent: "",
    kinds: &["inline", "pipe_table_cell"],
    language: || tree_sitter_md::INLINE_LANGUAGE.into(),
    query: || tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
}];

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
    // The standard capture names of the grammars' own queries. A dotted name not listed falls back to its
    // prefix (`keyword.return` → `keyword`); `variable` and other unlisted names stay uncolored.
    ("function", "method name"),
    ("constructor", "class name"),
    ("type", "class name"),
    ("type.builtin", "keyword"),
    ("type.qualifier", "keyword"),
    ("storageclass", "keyword"),
    ("conditional", "keyword - control"),
    ("repeat", "keyword - control"),
    ("keyword.return", "keyword - control"),
    ("keyword.repeat", "keyword - control"),
    ("property", "property name"),
    ("field", "field name"),
    ("variable.parameter", "parameter name"),
    ("parameter", "parameter name"),
    ("variable.builtin", "keyword"),
    ("constant", "constant name"),
    ("constant.builtin", "keyword"),
    ("boolean", "keyword"),
    ("float", "number"),
    ("string.special", "string"),
    ("string.escape", "string - escape character"),
    ("escape", "string - escape character"),
    ("tag", "html element name"),
    ("attribute", "html attribute name"),
    ("namespace", "namespace name"),
    ("label", "label name"),
    ("delimiter", "punctuation"),
    ("text.title", "markup heading"),
    ("text.strong", "markup bold"),
    ("text.emphasis", "markup italic"),
    ("text.literal", "markup code"),
    ("text.uri", "markup link"),
    ("text.reference", "markup link"),
];

/// The kind id for a capture name: an exact entry, else its dotted prefixes (`function.method.call` →
/// `function`). `None`: the capture only serves predicates or isn't colored.
fn capture_kind(name: &str) -> Option<u8> {
    let mut key = name;
    loop {
        if let Some(i) = CAPTURE_KINDS.iter().position(|(capture, _)| *capture == key) {
            return Some(i as u8);
        }
        key = &key[..key.rfind('.')?];
    }
}

/// A language supported by the highlighter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    CSharp,
    Json,
    /// ASP.NET Core Razor: `.razor` components and `.cshtml` views/pages.
    Razor,
    /// XML, including MSBuild projects (`.csproj`, `.props`, `.targets`) and .NET resource/config files.
    Xml,
    /// YAML, including Docker Compose files.
    Yaml,
    /// JavaScript, including JSX (`.js`, `.mjs`, `.cjs`, `.jsx`).
    JavaScript,
    /// TypeScript (`.ts`, `.mts`, `.cts`).
    TypeScript,
    /// TypeScript with JSX (`.tsx`).
    Tsx,
    Css,
    Scss,
    /// HTML; `<script>` and `<style>` contents are highlighted as JavaScript and CSS.
    Html,
    Sql,
    /// Markdown: block structure plus inline emphasis, code spans and links.
    Markdown,
    PowerShell,
    /// Shell scripts: bash, sh, zsh (`.sh`, `.bashrc`, `PKGBUILD`, a `#!/bin/sh` line, …).
    Shell,
    /// A language from a module (index into the registered module languages).
    Module(u16),
    /// A small built-in regex language: ignore files, `.gitattributes`, INI-style config, `.env` ([`builtin_lang`]).
    Builtin(u8),
}

impl Language {
    /// Detects the language from a file name or extension: a module language that claims the file, a
    /// built-in one (`.gitignore`, `.gitattributes`, `.editorconfig`/`.ini`, `.env`, …), else
    /// `.cs`/`.csx` → C#, `.json`/`.jsonc` → JSON, `.razor`/`.cshtml` → Razor, `.csproj`/`.props`/`.xml`/… → XML,
    /// `.yml`/`.yaml` → YAML.
    pub fn from_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?;
        if let Some(index) = module_lang::find(name) {
            return Some(Language::Module(index));
        }
        if let Some(index) = builtin_lang::find(name) {
            return Some(Language::Builtin(index));
        }
        if Self::is_shell_file(name) {
            return Some(Language::Shell);
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
            "js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
            "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            "css" => Some(Language::Css),
            "scss" => Some(Language::Scss),
            "html" | "htm" | "xhtml" => Some(Language::Html),
            "sql" => Some(Language::Sql),
            "md" | "markdown" | "mdx" => Some(Language::Markdown),
            "ps1" | "psm1" | "psd1" => Some(Language::PowerShell),
            "sh" | "bash" | "zsh" | "ksh" => Some(Language::Shell),
            _ => None,
        }
    }

    /// Shell scripts known by name rather than extension.
    fn is_shell_file(name: &str) -> bool {
        const NAMES: &[&str] = &[
            ".bashrc", ".bash_profile", ".bash_login", ".bash_logout", ".bash_aliases", ".profile", ".zshrc", ".zshenv", ".zprofile",
            ".zlogin", ".zlogout", "PKGBUILD", "APKBUILD", ".xinitrc", ".xprofile",
        ];
        NAMES.contains(&name)
    }

    /// Guesses the language from the text itself, for a buffer without a file name (an untitled editor).
    /// Looks only at the start of the text, so it is cheap enough to run on every edit. `None`: plain text.
    ///
    /// Module languages take part through their `detect` patterns: two or more matching patterns win over
    /// the built-in guesses, a single one counts when nothing else fits.
    pub fn detect(text: &str) -> Option<Language> {
        let mut end = text.len().min(16 * 1024);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let sample = text[..end].trim_start_matches('\u{feff}').trim_start();
        if sample.is_empty() {
            return None;
        }
        let module = module_lang::detect(sample);
        if let Some((index, hits)) = module
            && hits >= 2
        {
            return Some(Language::Module(index));
        }
        Self::detect_builtin(sample).or(module.map(|(index, _)| Language::Module(index)))
    }

    /// The built-in guesses of [`Language::detect`], on its trimmed sample.
    fn detect_builtin(sample: &str) -> Option<Language> {
        let first = sample.chars().next()?;
        let lines = || sample.lines().map(str::trim).filter(|l| !l.is_empty()).take(200);

        const RAZOR: &[&str] = &[
            "@page", "@model ", "@using ", "@inject ", "@code", "@{", "@if ", "@if(", "@foreach", "@namespace ", "@inherits ", "@attribute ", "@layout ",
        ];
        if lines().any(|l| RAZOR.iter().any(|d| l.starts_with(d))) {
            return Some(Language::Razor);
        }
        if let Some(line) = sample.strip_prefix("#!") {
            let line = line.lines().next().unwrap_or_default();
            let program = line.split_whitespace().find(|w| !w.ends_with("/env") && !w.starts_with('-')).unwrap_or_default();
            let program = program.rsplit('/').next().unwrap_or_default();
            match program {
                "sh" | "bash" | "zsh" | "ksh" | "dash" => return Some(Language::Shell),
                "node" | "deno" | "bun" => return Some(Language::JavaScript),
                "pwsh" | "powershell" => return Some(Language::PowerShell),
                _ => {}
            }
        }
        if first == '<' {
            let head = sample[..sample.len().min(64)].to_ascii_lowercase();
            if head.starts_with("<!doctype html") || head.starts_with("<html") {
                return Some(Language::Html);
            }
            return Some(Language::Xml);
        }
        if first == '{' || first == '[' {
            let after = sample[1..].trim_start();
            let json = match after.chars().next() {
                None => false,
                Some(c) if first == '{' => c == '"' || c == '}',
                Some(c) => matches!(c, '{' | '[' | '"' | ']' | '-' | '0'..='9') || ["true", "false", "null"].iter().any(|w| after.starts_with(w)),
            };
            if json {
                return Some(Language::Json);
            }
        }

        const CSHARP_STARTS: &[&str] = &[
            "using ", "namespace ", "public ", "private ", "protected ", "internal ", "static ", "class ", "record ", "struct ", "interface ",
            "enum ", "var ", "return ", "if (", "else", "foreach (", "for (", "while (", "switch (", "await ", "async ", "Console.", "#region",
            "#nullable", "[assembly:", "///", "throw ", "try", "catch", "new ", "void ", "string ", "int ", "bool ", "const ", "readonly ",
        ];
        let mut csharp = 0;
        for l in lines() {
            if CSHARP_STARTS.iter().any(|k| l.starts_with(k)) {
                csharp += 2;
            }
            if l.ends_with(';') || l.ends_with("=>") || l == "{" || l == "}" {
                csharp += 1;
            }
        }
        if csharp >= 3 {
            return Some(Language::CSharp);
        }

        // Line by line: INI sections, `key=value` assignments (all-caps keys: a .env file), YAML keys and list items.
        let (mut sections, mut assignments, mut env_like, mut yaml, mut other) = (0, 0, 0, 0, 0);
        for l in lines().filter(|l| !l.starts_with('#') && !l.starts_with(';')) {
            if l.starts_with('[') && l.ends_with(']') && !l.contains(',') {
                sections += 1;
            } else if let Some(key) = l.split_once('=').map(|(k, _)| k.trim().trim_start_matches("export ")).filter(|k| is_config_key(k)) {
                assignments += 1;
                if key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                    env_like += 1;
                }
            } else if is_yaml_line(l) {
                yaml += 1;
            } else {
                other += 1;
            }
        }
        if sample.starts_with("---") || (yaml > 0 && yaml >= assignments + sections && yaml > other) {
            return Some(Language::Yaml);
        }
        if assignments + sections == 0 || assignments + sections <= other {
            return None;
        }
        let id = if sections == 0 && env_like == assignments { "dotenv" } else { "ini" };
        builtin_lang::find_id(id).map(Language::Builtin)
    }

    /// The language for a fenced code block's tag or a language id ("csharp", "json", a module's id, …).
    pub fn from_id(id: &str) -> Option<Language> {
        match id.to_ascii_lowercase().as_str() {
            "csharp" | "cs" | "c#" => Some(Language::CSharp),
            "razor" | "cshtml" => Some(Language::Razor),
            "json" | "jsonc" => Some(Language::Json),
            "xml" | "csproj" | "msbuild" => Some(Language::Xml),
            "yaml" | "yml" | "compose" => Some(Language::Yaml),
            "javascript" | "js" | "jsx" | "javascriptreact" | "mjs" | "cjs" | "node" => Some(Language::JavaScript),
            "typescript" | "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" | "typescriptreact" => Some(Language::Tsx),
            "css" => Some(Language::Css),
            "scss" => Some(Language::Scss),
            "html" | "htm" | "xhtml" => Some(Language::Html),
            "sql" | "tsql" | "mssql" | "postgresql" | "postgres" | "psql" | "mysql" | "sqlite" | "plsql" => Some(Language::Sql),
            "markdown" | "md" => Some(Language::Markdown),
            "powershell" | "pwsh" | "ps1" | "ps" => Some(Language::PowerShell),
            "shellscript" | "shell" | "bash" | "sh" | "zsh" | "ksh" | "console" | "shell-session" => Some(Language::Shell),
            other => module_lang::find_id(other).map(Language::Module).or_else(|| builtin_lang::find_id(other).map(Language::Builtin)),
        }
    }

    /// Language id as VS Code and LSP use it ("csharp", "json", …), e.g. for snippet file names.
    pub fn id(self) -> &'static str {
        match self {
            Language::CSharp => "csharp",
            Language::Json => "json",
            Language::Razor => "razor",
            Language::Xml => "xml",
            Language::Yaml => "yaml",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Tsx => "typescriptreact",
            Language::Css => "css",
            Language::Scss => "scss",
            Language::Html => "html",
            Language::Sql => "sql",
            Language::Markdown => "markdown",
            Language::PowerShell => "powershell",
            Language::Shell => "shellscript",
            Language::Module(i) => module_lang::get(i).map_or("plaintext", |l| l.id),
            Language::Builtin(i) => builtin_lang::get(i).map_or("plaintext", |l| l.id),
        }
    }

    /// The block comment tokens (`/*` `*/`, `<!--` `-->`, …), if the language has them.
    pub fn block_comment(self) -> Option<(&'static str, &'static str)> {
        match self {
            Language::CSharp | Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Css | Language::Scss | Language::Sql => {
                Some(("/*", "*/"))
            }
            Language::Razor => Some(("@*", "*@")),
            Language::Xml | Language::Html | Language::Markdown => Some(("<!--", "-->")),
            Language::PowerShell => Some(("<#", "#>")),
            Language::Json | Language::Yaml | Language::Shell | Language::Module(_) | Language::Builtin(_) => None,
        }
    }

    /// The line comment token (`//`, `#`), if the language has one.
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Language::CSharp | Language::Json | Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Scss => Some("//"),
            Language::Yaml | Language::PowerShell | Language::Shell => Some("#"),
            Language::Sql => Some("--"),
            Language::Razor | Language::Xml | Language::Css | Language::Html | Language::Markdown => None,
            Language::Module(i) => module_lang::get(i).and_then(|l| l.line_comment),
            Language::Builtin(i) => builtin_lang::get(i).and_then(|l| l.line_comment),
        }
    }

    /// Human-readable language name.
    pub fn name(self) -> &'static str {
        match self {
            Language::CSharp => "C#",
            Language::Json => "JSON",
            Language::Razor => "Razor",
            Language::Xml => "XML",
            Language::Yaml => "YAML",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TypeScript JSX",
            Language::Css => "CSS",
            Language::Scss => "SCSS",
            Language::Html => "HTML",
            Language::Sql => "SQL",
            Language::Markdown => "Markdown",
            Language::PowerShell => "PowerShell",
            Language::Shell => "Shell Script",
            Language::Module(i) => module_lang::get(i).map_or("Plain Text", |l| l.name),
            Language::Builtin(i) => builtin_lang::get(i).map_or("Plain Text", |l| l.name),
        }
    }

    /// The tree-sitter grammar (`None` for module and built-in regex languages).
    fn ts_language(self) -> Option<tree_sitter::Language> {
        match self {
            Language::CSharp => Some(tree_sitter_c_sharp::LANGUAGE.into()),
            Language::Json => Some(tree_sitter_json::LANGUAGE.into()),
            Language::Razor => Some(tree_sitter_razor_modern::LANGUAGE.into()),
            Language::Xml => Some(tree_sitter_xml::LANGUAGE_XML.into()),
            Language::Yaml => Some(tree_sitter_yaml::LANGUAGE.into()),
            Language::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            Language::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            Language::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            Language::Css => Some(tree_sitter_css::LANGUAGE.into()),
            Language::Scss => Some(tree_sitter_scss::language()),
            Language::Html => Some(tree_sitter_html::LANGUAGE.into()),
            // A lexer instead (see `sql.rs`).
            Language::Sql => None,
            Language::Markdown => Some(tree_sitter_md::LANGUAGE.into()),
            Language::PowerShell => Some(tree_sitter_powershell::LANGUAGE.into()),
            Language::Shell => Some(tree_sitter_bash::LANGUAGE.into()),
            Language::Module(_) | Language::Builtin(_) => None,
        }
    }

    /// Grammars parsed inside this one: (parent node kind or "" for any, node kinds, grammar, query).
    fn injections(self) -> &'static [Injection] {
        match self {
            Language::Html => &HTML_INJECTIONS,
            Language::Markdown => &MARKDOWN_INJECTIONS,
            _ => &[],
        }
    }

    fn query_source(self) -> &'static str {
        match self {
            Language::CSharp => CSHARP_QUERY,
            Language::Json => JSON_QUERY,
            Language::Razor => RAZOR_QUERY,
            Language::Xml => XML_QUERY,
            Language::Yaml => YAML_QUERY,
            Language::JavaScript => JAVASCRIPT_QUERY.as_str(),
            Language::TypeScript => TYPESCRIPT_QUERY.as_str(),
            Language::Tsx => TSX_QUERY.as_str(),
            Language::Css => tree_sitter_css::HIGHLIGHTS_QUERY,
            Language::Scss => SCSS_QUERY.as_str(),
            Language::Html => HTML_QUERY.as_str(),
            Language::Sql => "",
            Language::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            Language::PowerShell => tree_sitter_powershell::HIGHLIGHTS_QUERY,
            Language::Shell => tree_sitter_bash::HIGHLIGHT_QUERY,
            Language::Module(_) | Language::Builtin(_) => "",
        }
    }
}

/// A key of an INI or .env assignment: `name`, `section.key`, `KEY_1`.
fn is_config_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// A YAML mapping or list line: `key: value`, `key:`, `- item`, `- key: value`.
fn is_yaml_line(line: &str) -> bool {
    if line == "-" {
        return true;
    }
    let (item, line) = match line.strip_prefix("- ") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, line),
    };
    match line.split_once(':') {
        Some((key, rest)) => {
            let key = key.trim_matches(|c| c == '"' || c == '\'');
            !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' ' | '/')) && (rest.is_empty() || rest.starts_with(' '))
        }
        None => item,
    }
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
    /// `None` for module languages (regex highlighting, nothing to parse).
    parser: Option<Parser>,
    tree: Option<Tree>,
    query: Option<Query>,
    /// Index into [`CAPTURE_KINDS`] per query capture index (`None` for helper captures).
    capture_kinds: Vec<Option<u8>>,
    /// Grammars parsed inside this one (HTML `<script>`, Markdown inline text).
    injected: Vec<Injected>,
}

/// An [`Injection`] being highlighted: reparsed in full over its nodes after every edit.
struct Injected {
    spec: &'static Injection,
    parser: Parser,
    query: Query,
    capture_kinds: Vec<Option<u8>>,
    tree: Option<Tree>,
}

fn kinds_of(query: &Query) -> Vec<Option<u8>> {
    query.capture_names().iter().map(|name| capture_kind(name)).collect()
}

impl Highlighter {
    /// Creates a highlighter for `language`. Panics only if the embedded query is invalid
    /// (guarded by unit tests).
    pub fn new(language: Language) -> Self {
        let Some(ts_language) = language.ts_language() else {
            return Self { language, parser: None, tree: None, query: None, capture_kinds: Vec::new(), injected: Vec::new() };
        };
        let mut parser = Parser::new();
        parser
            .set_language(&ts_language)
            .expect("tree-sitter grammar ABI mismatch");
        let query = Query::new(&ts_language, language.query_source())
            .unwrap_or_else(|e| panic!("invalid {} highlight query: {e}", language.name()));
        let capture_kinds = kinds_of(&query);
        let injected = language
            .injections()
            .iter()
            .map(|spec| {
                let ts = (spec.language)();
                let mut parser = Parser::new();
                parser.set_language(&ts).expect("tree-sitter grammar ABI mismatch");
                let query = Query::new(&ts, (spec.query)()).unwrap_or_else(|e| panic!("invalid {} injected highlight query: {e}", language.name()));
                Injected { spec, parser, capture_kinds: kinds_of(&query), query, tree: None }
            })
            .collect();
        Self {
            language,
            parser: Some(parser),
            tree: None,
            query: Some(query),
            capture_kinds,
            injected,
        }
    }

    /// Parses the injected grammars over their nodes in the current tree.
    fn parse_injections(&mut self, text: &str) {
        let Some(tree) = &self.tree else { return };
        for injected in &mut self.injected {
            let mut ranges = Vec::new();
            collect_ranges(tree.root_node(), injected.spec, &mut ranges);
            // No ranges would mean the whole document to tree-sitter.
            injected.tree = if ranges.is_empty() || injected.parser.set_included_ranges(&ranges).is_err() {
                None
            } else {
                injected.parser.parse(text, None)
            };
        }
    }

    /// The language this highlighter was created for.
    pub fn language(&self) -> Language {
        self.language
    }

    /// Full (re)parse of the document.
    pub fn parse(&mut self, text: &str) {
        if let Some(parser) = self.parser.as_mut() {
            self.tree = parser.parse(text, None);
        }
        self.parse_injections(text);
    }

    /// Applies an edit to the existing tree, then incrementally reparses `new_text`.
    pub fn edit(&mut self, edit: Edit, new_text: &str) {
        let Some(parser) = self.parser.as_mut() else { return };
        match self.tree.as_mut() {
            Some(tree) => {
                tree.edit(&edit.into());
                self.tree = parser.parse(new_text, Some(tree));
                self.parse_injections(new_text);
            }
            None => self.parse(new_text),
        }
    }

    /// Returns spans intersecting `byte_range`, sorted by start, non-overlapping
    /// (innermost capture wins) and clipped to the range.
    pub fn highlight(&self, text: &str, byte_range: Range<usize>) -> Vec<Span> {
        if let Language::Module(index) = self.language {
            return module_lang::get(index).map(|l| module_lang::highlight(&l, text, byte_range)).unwrap_or_default();
        }
        if let Language::Builtin(index) = self.language {
            return builtin_lang::get(index).map(|l| module_lang::highlight(l, text, byte_range)).unwrap_or_default();
        }
        if self.language == Language::Sql {
            return sql::highlight(text, byte_range);
        }
        let (Some(tree), Some(query)) = (&self.tree, &self.query) else {
            return Vec::new();
        };
        let start = byte_range.start.min(text.len());
        let end = byte_range.end.min(text.len());
        if start >= end {
            return Vec::new();
        }

        let mut paint = vec![NO_KIND; end - start];
        paint_captures(query, &self.capture_kinds, tree, text, start..end, &mut paint);
        // Injected grammars paint over their host (they own the nodes they were parsed in).
        for injected in &self.injected {
            if let Some(tree) = &injected.tree {
                paint_captures(&injected.query, &injected.capture_kinds, tree, text, start..end, &mut paint);
            }
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

/// Paints the captures of `query` in `range` into `paint` (which starts at `range.start`): outer captures
/// first so inner ones overwrite them; for identical ranges the later pattern wins. Byte-wise painting of
/// the window is cheap (the window is small) and makes overlap resolution trivial.
fn paint_captures(query: &Query, kinds: &[Option<u8>], tree: &Tree, text: &str, range: Range<usize>, paint: &mut [u8]) {
    let (start, end) = (range.start, range.end);
    // (start, end, pattern index, kind id), clipped to the range.
    let mut captures: Vec<(usize, usize, usize, u8)> = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(start..end);
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for capture in m.captures {
            let Some(kind) = kinds[capture.index as usize] else {
                continue;
            };
            let r = capture.node.byte_range();
            let (s, e) = (r.start.max(start), r.end.min(end));
            if s < e {
                captures.push((s, e, m.pattern_index, kind));
            }
        }
    }
    captures.sort_unstable_by(|a, b| {
        let (la, lb) = (a.1 - a.0, b.1 - b.0);
        lb.cmp(&la).then(a.2.cmp(&b.2))
    });
    for &(s, e, _, kind) in &captures {
        paint[s - start..e - start].fill(kind);
    }
}

/// The ranges of the nodes an injection is parsed over, in document order.
fn collect_ranges(node: tree_sitter::Node, spec: &Injection, out: &mut Vec<tree_sitter::Range>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if spec.kinds.contains(&child.kind()) && (spec.parent.is_empty() || node.kind() == spec.parent) {
            out.push(child.range());
        } else {
            collect_ranges(child, spec, out);
        }
    }
}

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

mod builtin_lang;
mod linked;
pub use linked::{is_tag_name_text, linked_tag_names};
mod markup;
mod sql;
pub mod module_lang;

#[cfg(test)]
mod tests;
