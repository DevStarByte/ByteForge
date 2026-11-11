//! `bf-lsp`: a small, runtime-agnostic Language Server Protocol client.
//!
//! The client talks to the server over stdio using plain std threads and hands results
//! back through [`flume`] channels, so it can be driven from any executor (GPUI awaits
//! `Receiver::recv_async()`). It is tuned for `csharp-ls` but speaks generic LSP.

mod client;
mod convert;
pub mod framing;
mod snippet;
mod uri;

use std::path::PathBuf;

pub use client::LspClient;
pub use snippet::strip_snippet;
pub use uri::{path_to_uri, uri_to_path};

/// How to launch a language server.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Executable (name on PATH or full path).
    pub command: String,
    /// Command line arguments.
    pub args: Vec<String>,
    /// Workspace root; sent as `rootUri`/`workspaceFolders` and used as working directory.
    pub root: PathBuf,
    /// Language id used in `textDocument/didOpen` (e.g. `"csharp"`).
    pub language_id: String,
    /// Notifications sent right after `initialized` (`method`, `params`): the Roslyn server only loads
    /// a solution or projects once told to (`solution/open`, `project/open`).
    pub after_initialized: Vec<(String, serde_json::Value)>,
    /// Ask for diagnostics (`textDocument/diagnostic`) even when the server doesn't announce it; the
    /// Roslyn server answers them but never publishes.
    pub pull_diagnostics: bool,
    /// Answers to `workspace/configuration` by section name (missing sections get `null`, the server's
    /// default). `None`: an empty object for every section (csharp-ls).
    pub configuration: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ServerConfig {
    /// A server started with `command args`, for `language_id` files under `root`.
    pub fn new(command: impl Into<String>, args: Vec<String>, root: PathBuf, language_id: impl Into<String>) -> Self {
        ServerConfig { command: command.into(), args, root, language_id: language_id.into(), after_initialized: Vec::new(), pull_diagnostics: false, configuration: None }
    }
}

/// Zero-based position; `character` counts UTF-16 code units (LSP default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// Half-open range between two positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A diagnostic published by the server.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    pub code: Option<String>,
}

/// A completion proposal, already normalized (snippets stripped, fallbacks applied).
#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    /// `textEdit.newText ?? insertText ?? label`.
    pub insert_text: String,
    /// `filterText ?? label`.
    pub filter_text: String,
    /// `sortText ?? label`.
    pub sort_text: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    /// Range to replace with `insert_text`, if the server supplied a text edit.
    pub text_edit_range: Option<Range>,
    /// `insert_text` is a snippet body with tab stops (`$1`, `${2:name}`), from the editor's snippets.
    pub snippet: bool,
}

/// LSP `CompletionItemKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
    Text,
}

/// A location in a file (result of go-to-definition, references, …).
#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

/// A use of the symbol under the caret (`textDocument/documentHighlight`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentHighlight {
    pub range: Range,
    /// The symbol is assigned here (LSP kind `Write`), not only read.
    pub write: bool,
}

/// Extra text shown inside the code (`textDocument/inlayHint`): an inferred type or a parameter name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlayHint {
    pub position: Position,
    pub label: String,
    /// `true` for parameter names, `false` for types.
    pub parameter: bool,
    pub padding_left: bool,
    pub padding_right: bool,
}

/// A named code element (`textDocument/documentSymbol`, `workspace/symbol`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// LSP `SymbolKind` as a word: "class", "method", "property", …
    pub kind: &'static str,
    /// Extra text from the server (a signature or type), if any.
    pub detail: Option<String>,
    /// The containing type or namespace.
    pub container: Option<String>,
    pub path: PathBuf,
    /// Where the name is (the selection range for document symbols).
    pub range: Range,
    /// The whole declaration, body included (document symbols; the same as `range` otherwise).
    pub full_range: Range,
    /// Nesting level in the document outline (0 = top level).
    pub depth: usize,
}

/// The signatures that fit the call around the caret (`textDocument/signatureHelp`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    pub active_signature: usize,
}

/// One overload, e.g. `void Console.WriteLine(string format, object? arg0)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    pub documentation: Option<String>,
    /// Byte ranges of the parameters inside `label`.
    pub parameters: Vec<std::ops::Range<usize>>,
    /// The parameter being typed (index into `parameters`), if known.
    pub active_parameter: Option<usize>,
}

/// A semantic token converted to an absolute position. `kind` uses the IDE's theme
/// names (e.g. `"class name"`, `"local name"`, `"keyword"`); UTF-16 columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub line: u32,
    pub start: u32,
    pub length: u32,
    pub kind: &'static str,
}

/// One content change for `textDocument/didChange`; `range: None` replaces the whole text.
#[derive(Clone, Debug)]
pub struct TextChange {
    pub range: Option<Range>,
    pub text: String,
}

/// A text replacement in UTF-16 positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// Changes to apply across files (`WorkspaceEdit`), in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceEdit {
    /// Files to create (empty) before the edits apply.
    pub creates: Vec<PathBuf>,
    pub changes: Vec<(PathBuf, Vec<TextEdit>)>,
}

impl WorkspaceEdit {
    pub fn is_empty(&self) -> bool {
        self.creates.is_empty() && self.changes.iter().all(|(_, edits)| edits.is_empty())
    }
}

/// A quick fix or refactoring offered by `textDocument/codeAction`.
#[derive(Clone, Debug)]
pub struct CodeAction {
    pub title: String,
    /// `quickfix`, `refactor.extract`, `source.organizeImports`, …
    pub kind: Option<String>,
    pub preferred: bool,
    /// Why the action can't be applied right now.
    pub disabled: Option<String>,
    pub edit: Option<WorkspaceEdit>,
    /// A command to run after the edit (`{ command, arguments }`).
    pub command: Option<serde_json::Value>,
    /// The action as the server sent it, for `codeAction/resolve`.
    pub raw: serde_json::Value,
}

impl CodeAction {
    /// The server computes the edit lazily; call [`LspClient::resolve_code_action`] first.
    pub fn needs_resolve(&self) -> bool {
        self.edit.is_none() && self.command.is_none()
    }

    pub fn is_quick_fix(&self) -> bool {
        match self.kind.as_deref() {
            Some(kind) => kind.starts_with("quickfix"),
            // Roslyn (csharp-ls) often leaves the kind out; refactorings are recognizable by their verb.
            None => !REFACTORING_VERBS.iter().any(|verb| self.title.starts_with(verb)),
        }
    }
}

impl CodeAction {
    /// Where the action belongs in the Refactor menu: Fix, Extract, Introduce, Inline, Move, Generate, Source or
    /// Rewrite. The kind decides when the server sends one; otherwise the title's verb.
    pub fn group(&self) -> &'static str {
        match self.kind.as_deref() {
            Some(k) if k.starts_with("quickfix") => return "Fix",
            Some(k) if k.starts_with("source") => return "Source",
            Some(k) if k.starts_with("refactor.extract") && !self.title.starts_with("Introduce") => return "Extract",
            Some(k) if k.starts_with("refactor.inline") => return "Inline",
            Some(k) if k.starts_with("refactor") => {}
            Some(_) => return "Rewrite",
            None if self.is_quick_fix() => return "Fix",
            None => {}
        }
        let t = self.title.as_str();
        let verb = |v: &[&str]| v.iter().any(|p| t.starts_with(p));
        if verb(&["Extract"]) {
            "Extract"
        } else if verb(&["Introduce"]) {
            "Introduce"
        } else if verb(&["Inline"]) {
            "Inline"
        } else if verb(&["Move", "Pull", "Rename file", "Rename type"]) {
            "Move"
        } else if verb(&["Generate", "Add null check", "Encapsulate"]) {
            "Generate"
        } else if verb(&["Sort", "Remove unnecessary"]) {
            "Source"
        } else {
            "Rewrite"
        }
    }

    /// The title as the menus show it. Roslyn flattens nested actions to "Parent - Child"; that becomes
    /// "Parent › Child", or just the child when it starts with the parent's verb ("Inline 'F' - Inline and keep 'F'").
    pub fn display_title(&self) -> String {
        let verb = |s: &str| s.split(' ').next().unwrap_or_default().to_owned();
        match self.title.split_once(" - ") {
            Some((parent, child)) if verb(parent) == verb(child) => child.to_owned(),
            Some((parent, child)) => format!("{parent} \u{203a} {child}"),
            None => self.title.clone(),
        }
    }
}

/// Title prefixes of Roslyn refactorings that come without a code action kind.
const REFACTORING_VERBS: &[&str] = &[
    "Extract", "Inline", "Introduce", "Convert", "Invert", "Wrap", "Unwrap", "Encapsulate", "Pull", "Move", "Sort", "Split",
    "Merge", "Reverse", "Make ", "Use ", "Add parameter", "Change signature", "Rename", "Generate constructor", "Generate Equals",
    "Generate overrides", "Add null check",
];

/// Asynchronous events emitted by the client.
#[derive(Clone, Debug)]
pub enum LspEvent {
    /// The initialize handshake finished; queued requests have been sent.
    Ready,
    /// `textDocument/publishDiagnostics`.
    Diagnostics {
        path: PathBuf,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
    },
    /// Human readable progress (`$/progress`), e.g. `"Loading solution: Foo.csproj 40%"`.
    Progress(String),
    /// `window/logMessage`, `window/showMessage` and server stderr lines.
    Log(String),
    /// The server process ended or failed to start (with reason).
    Exited(String),
    /// `workspace/applyEdit`: the server asks the editor to change files (e.g. while running a command).
    ApplyEdit(WorkspaceEdit),
}

/// How to start Microsoft's Roslyn language server (the one VS Code's C# extension uses), from its
/// `Microsoft.CodeAnalysis.LanguageServer.dll`, for the folder `root`: it opens `solution` when there is
/// one, else `projects`. Diagnostics are pulled (it doesn't publish them) and its log goes to `log_dir`.
pub fn roslyn_config(dll: &std::path::Path, root: PathBuf, log_dir: &std::path::Path, solution: Option<&std::path::Path>, projects: &[PathBuf]) -> ServerConfig {
    let args = vec![
        dll.to_string_lossy().into_owned(),
        "--logLevel".into(),
        "Information".into(),
        "--extensionLogDirectory".into(),
        log_dir.to_string_lossy().into_owned(),
        "--stdio".into(),
    ];
    let open = match solution {
        Some(sln) => ("solution/open".to_owned(), serde_json::json!({ "solution": uri::path_to_uri(sln) })),
        None => ("project/open".to_owned(), serde_json::json!({ "projects": projects.iter().map(|p| uri::path_to_uri(p)).collect::<Vec<_>>() })),
    };
    let mut config = ServerConfig::new("dotnet", args, root, "csharp");
    config.after_initialized = vec![open];
    config.pull_diagnostics = true;
    // Inlay hints (types and parameter names, as with csharp-ls) and completion from namespaces that
    // aren't imported yet; everything else as the server has it.
    let on = [
        "csharp|inlay_hints.csharp_enable_inlay_hints_for_types",
        "csharp|inlay_hints.csharp_enable_inlay_hints_for_implicit_variable_types",
        "csharp|inlay_hints.csharp_enable_inlay_hints_for_lambda_parameter_types",
        "csharp|inlay_hints.dotnet_enable_inlay_hints_for_parameters",
        "csharp|inlay_hints.dotnet_enable_inlay_hints_for_literal_parameters",
        "csharp|inlay_hints.dotnet_enable_inlay_hints_for_object_creation_parameters",
        "csharp|inlay_hints.dotnet_suppress_inlay_hints_for_parameters_that_differ_only_by_suffix",
        "csharp|inlay_hints.dotnet_suppress_inlay_hints_for_parameters_that_match_argument_name",
        "csharp|inlay_hints.dotnet_suppress_inlay_hints_for_parameters_that_match_method_intent",
        "csharp|completion.dotnet_show_completion_items_from_unimported_namespaces",
        "projects.dotnet_enable_automatic_restore",
    ];
    config.configuration = Some(on.iter().map(|k| ((*k).to_owned(), serde_json::Value::Bool(true))).collect());
    config
}

/// Locates `csharp-ls`: first on `PATH`, then in `~/.dotnet/tools`.
pub fn csharp_ls_command() -> Option<String> {
    let exe = if cfg!(windows) {
        "csharp-ls.exe"
    } else {
        "csharp-ls"
    };
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(exe))
            .find(|candidate| candidate.is_file())
    });
    let in_tools = || {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .or_else(|| std::env::var_os("HOME"))?;
        let candidate = PathBuf::from(home).join(".dotnet").join("tools").join(exe);
        candidate.is_file().then_some(candidate)
    };
    on_path
        .or_else(in_tools)
        .map(|p| p.to_string_lossy().into_owned())
}
