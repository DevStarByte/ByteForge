//! `bf-lsp`: a small, runtime-agnostic Language Server Protocol client.
//!
//! The client talks to the server over stdio using plain std threads and hands results
//! back through [`flume`] channels, so it can be driven from any executor (GPUI awaits
//! `Receiver::recv_async()`). It is tuned for `csharp-ls` but speaks generic LSP.

mod client;
mod convert;
mod framing;
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

/// A location in a file (result of go-to-definition).
#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
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
