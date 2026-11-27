//! The AI assistant's core: a provider-neutral conversation, the workspace tools the assistant can call,
//! request bodies for Claude (Anthropic Messages API) and for local models (Ollama, LM Studio, llama.cpp:
//! the OpenAI-compatible chat API), streaming through `curl` (no TLS stack is built in; the API key and
//! request reach curl through stdin and a private temp file, never its command line), and parsing the
//! streamed events back into the conversation.
//!
//! The tools only read the workspace; edits and new files come back as proposals the user accepts or rejects.

use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};

/// The Messages API version sent with every request.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5-5";
/// Ollama's OpenAI-compatible endpoint.
pub const DEFAULT_LOCAL_URL: &str = "http://localhost:11434/v1";
pub const DEFAULT_LOCAL_MODEL: &str = "qwen3.5";

// ───────────── configuration ─────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Claude through the Anthropic API.
    Anthropic,
    /// A model on this machine through an OpenAI-compatible server.
    Local,
}

#[derive(Clone, Debug)]
pub struct AiConfig {
    pub provider: Provider,
    /// The Anthropic API key (ANTHROPIC_API_KEY, else the setting).
    pub api_key: Option<String>,
    pub model: String,
    /// `low` … `max` (Claude).
    pub effort: String,
    pub local_url: String,
    pub local_model: String,
    /// Some local servers want a key (LM Studio with auth on); Ollama doesn't.
    pub local_key: Option<String>,
}

impl AiConfig {
    /// What's missing before a request can go out, said plainly.
    pub fn problem(&self) -> Option<String> {
        match self.provider {
            Provider::Anthropic if self.api_key.as_deref().is_none_or(|k| k.trim().is_empty()) => Some(
                "Claude needs an Anthropic API key: set ANTHROPIC_API_KEY, or \"ai.apiKey\" in settings (or switch \"ai.provider\" to \"local\").".into(),
            ),
            Provider::Local if self.local_model.trim().is_empty() => Some("Set \"ai.local.model\" to the local model to use (e.g. qwen3.5).".into()),
            _ => None,
        }
    }

    pub fn model_label(&self) -> String {
        match self.provider {
            Provider::Anthropic => self.model.clone(),
            Provider::Local => format!("{} (local)", self.local_model),
        }
    }
}

// ───────────── the conversation ─────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    /// Claude's reasoning (its summary), passed back unchanged with its signature.
    Thinking { text: String, signature: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { id: String, content: String, is_error: bool },
    /// A block kept as it came (redacted thinking, a fallback marker, …) to echo back unchanged.
    Raw(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Turn {
    pub fn user(text: impl Into<String>) -> Self {
        Turn { role: Role::User, parts: vec![Part::Text(text.into())] }
    }

    /// The index after the last fallback marker: a model declined mid-answer and another took over there;
    /// what came before it isn't acted on (and isn't echoed back except its text).
    fn fallback_boundary(&self) -> usize {
        self.parts.iter().rposition(|p| matches!(p, Part::Raw(v) if v.get("type").and_then(Value::as_str) == Some("fallback"))).map_or(0, |i| i + 1)
    }

    /// The tool calls to run: those of the model that finished the answer.
    pub fn tool_uses(&self) -> Vec<(String, String, Value)> {
        self.parts[self.fallback_boundary()..]
            .iter()
            .filter_map(|p| match p {
                Part::ToolUse { id, name, input } => Some((id.clone(), name.clone(), input.clone())),
                _ => None,
            })
            .collect()
    }

    pub fn text(&self) -> String {
        self.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("")
    }
}

// ───────────── tools ─────────────

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

/// The tools the assistant can call. Reading tools run at once; `edit_file` and `create_file` become
/// proposals the user accepts or rejects in the panel.
pub fn tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file",
            description: "Read a file of the workspace, with line numbers. For a long file, read the lines you need (start_line, end_line; 1-based, inclusive).",
            schema: json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Path relative to the workspace root."},
                "start_line": {"type": "integer"}, "end_line": {"type": "integer"}
            }, "required": ["path"]}),
        },
        ToolSpec {
            name: "list_files",
            description: "List the files and folders in a folder of the workspace (build output, .git and excluded folders left out). With recursive, the whole tree below it (at most 500 entries).",
            schema: json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Folder relative to the workspace root; empty for the root."},
                "recursive": {"type": "boolean"}
            }, "required": []}),
        },
        ToolSpec {
            name: "search",
            description: "Search the workspace's text files for a string (or a regular expression with regex: true). Returns path:line: text for each match (at most 200).",
            schema: json!({"type": "object", "properties": {
                "query": {"type": "string"}, "regex": {"type": "boolean"},
                "path": {"type": "string", "description": "Only below this folder (relative to the root)."}
            }, "required": ["query"]}),
        },
        ToolSpec {
            name: "edit_file",
            description: "Propose replacing old_text with new_text in a file. old_text must appear exactly once in the file (copy it from read_file, without the line numbers, with enough lines around it to be unique). The user sees the change as a diff and accepts or rejects it; the result says which.",
            schema: json!({"type": "object", "properties": {
                "path": {"type": "string"}, "old_text": {"type": "string"}, "new_text": {"type": "string"}
            }, "required": ["path", "old_text", "new_text"]}),
        },
        ToolSpec {
            name: "create_file",
            description: "Propose creating a new file with the given content. The user accepts or rejects it; fails if the file exists (use edit_file then).",
            schema: json!({"type": "object", "properties": {
                "path": {"type": "string"}, "content": {"type": "string"}
            }, "required": ["path", "content"]}),
        },
    ]
}

/// A change the assistant proposes, waiting for the user.
#[derive(Clone, Debug, PartialEq)]
pub enum Proposal {
    Edit { path: PathBuf, old_text: String, new_text: String },
    Create { path: PathBuf, content: String },
}

/// What running a tool call came to.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolOutcome {
    /// The result to send back (`is_error` for a failed call).
    Done { content: String, is_error: bool },
    /// Needs the user's yes first.
    Propose(Proposal),
}

fn error(text: impl Into<String>) -> ToolOutcome {
    ToolOutcome::Done { content: text.into(), is_error: true }
}

/// A path the assistant names, inside the workspace (no `..` out of it, no absolute paths elsewhere).
pub fn resolve(root: &Path, path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path.trim());
    let rel = if p.is_absolute() {
        p.strip_prefix(root).map_err(|_| format!("{path} is outside the workspace"))?.to_path_buf()
    } else {
        p.to_path_buf()
    };
    let mut out = root.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => return Err(format!("{path}: only paths inside the workspace (no ..)")),
        }
    }
    Ok(out)
}

/// Folders a listing or search skips.
fn skipped(name: &str) -> bool {
    matches!(name, ".git" | "bin" | "obj" | "node_modules" | ".vs" | ".idea" | "target")
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

/// Runs a tool call against the workspace at `root`. `read` gives a file's current text (the open
/// editor's, unsaved changes included), else None to read the disk.
pub fn run_tool(root: &Path, name: &str, input: &Value, read: &dyn Fn(&Path) -> Option<String>) -> ToolOutcome {
    let text_of = |path: &Path| read(path).map(Ok).unwrap_or_else(|| std::fs::read_to_string(path).map_err(|e| e.to_string()));
    match name {
        "read_file" => {
            let Some(path) = str_arg(input, "path") else { return error("read_file needs a path") };
            let full = match resolve(root, path) {
                Ok(p) => p,
                Err(e) => return error(e),
            };
            let text = match text_of(&full) {
                Ok(t) => t,
                Err(e) => return error(format!("{path}: {e}")),
            };
            let lines: Vec<&str> = text.lines().collect();
            let start = input.get("start_line").and_then(Value::as_u64).map_or(1, |n| n.max(1) as usize);
            let end = input.get("end_line").and_then(Value::as_u64).map_or(lines.len(), |n| (n as usize).min(lines.len()));
            // Long files come in parts, so one read doesn't swamp the context.
            let end = end.min(start + 1999);
            let mut out = String::new();
            for (i, line) in lines.iter().enumerate().take(end).skip(start - 1) {
                out.push_str(&format!("{:>5}  {line}\n", i + 1));
            }
            if end < lines.len() {
                out.push_str(&format!("… ({} lines in all; ask for start_line {} to read on)\n", lines.len(), end + 1));
            }
            if lines.is_empty() {
                out.push_str("(empty file)\n");
            }
            ToolOutcome::Done { content: out, is_error: false }
        }
        "list_files" => {
            let dir = match resolve(root, str_arg(input, "path").unwrap_or("")) {
                Ok(p) => p,
                Err(e) => return error(e),
            };
            let recursive = input.get("recursive").and_then(Value::as_bool).unwrap_or(false);
            let mut out = Vec::new();
            let mut stack = vec![dir.clone()];
            while let Some(d) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&d) else {
                    if d == dir {
                        return error(format!("{} is not a folder", dir.display()));
                    }
                    continue;
                };
                let mut entries: Vec<_> = entries.flatten().collect();
                entries.sort_by_key(|e| e.file_name());
                for e in entries {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let is_dir = e.path().is_dir();
                    if is_dir && skipped(&name) {
                        continue;
                    }
                    let rel = e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().replace('\\', "/");
                    out.push(if is_dir { format!("{rel}/") } else { rel });
                    if is_dir && recursive {
                        stack.push(e.path());
                    }
                    if out.len() >= 500 {
                        out.push("… (more left out)".into());
                        return ToolOutcome::Done { content: out.join("\n"), is_error: false };
                    }
                }
            }
            ToolOutcome::Done { content: if out.is_empty() { "(empty)".into() } else { out.join("\n") }, is_error: false }
        }
        "search" => {
            let Some(query) = str_arg(input, "query").filter(|q| !q.is_empty()) else { return error("search needs a query") };
            let regex = input.get("regex").and_then(Value::as_bool).unwrap_or(false);
            let base = match resolve(root, str_arg(input, "path").unwrap_or("")) {
                Ok(p) => p,
                Err(e) => return error(e),
            };
            let q = bf_text::Query { text: query.to_owned(), match_case: false, whole_word: false, regex };
            let matcher = match bf_text::Matcher::new(&q) {
                Ok(Some(m)) => m,
                Ok(None) => return error("search needs a query"),
                Err(e) => return error(format!("bad pattern: {e}")),
            };
            let mut out = Vec::new();
            let mut stack = vec![base];
            'walk: while let Some(d) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&d) else { continue };
                for e in entries.flatten() {
                    let path = e.path();
                    let name = e.file_name().to_string_lossy().into_owned();
                    if path.is_dir() {
                        if !skipped(&name) {
                            stack.push(path);
                        }
                        continue;
                    }
                    let Ok(text) = text_of(&path) else { continue };
                    let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                    for (i, line) in text.lines().enumerate() {
                        if !matcher.find_in(line).is_empty() {
                            let shown: String = line.trim().chars().take(200).collect();
                            out.push(format!("{rel}:{}: {shown}", i + 1));
                            if out.len() >= 200 {
                                out.push("… (more matches left out)".into());
                                break 'walk;
                            }
                        }
                    }
                }
            }
            ToolOutcome::Done { content: if out.is_empty() { "No matches.".into() } else { out.join("\n") }, is_error: false }
        }
        "edit_file" => {
            let (Some(path), Some(old_text), Some(new_text)) = (str_arg(input, "path"), str_arg(input, "old_text"), str_arg(input, "new_text")) else {
                return error("edit_file needs path, old_text and new_text");
            };
            let full = match resolve(root, path) {
                Ok(p) => p,
                Err(e) => return error(e),
            };
            let text = match text_of(&full) {
                Ok(t) => t.replace("\r\n", "\n"),
                Err(e) => return error(format!("{path}: {e}")),
            };
            let old = old_text.replace("\r\n", "\n");
            match text.matches(&old).count() {
                0 => error(format!("old_text isn't in {path}; read the file again and copy the text exactly")),
                1 => ToolOutcome::Propose(Proposal::Edit { path: full, old_text: old, new_text: new_text.replace("\r\n", "\n") }),
                n => error(format!("old_text is in {path} {n} times; include more lines around it so it's unique")),
            }
        }
        "create_file" => {
            let (Some(path), Some(content)) = (str_arg(input, "path"), str_arg(input, "content")) else { return error("create_file needs path and content") };
            let full = match resolve(root, path) {
                Ok(p) => p,
                Err(e) => return error(e),
            };
            if full.exists() {
                return error(format!("{path} exists; change it with edit_file"));
            }
            ToolOutcome::Propose(Proposal::Create { path: full, content: content.to_owned() })
        }
        other => error(format!("there is no tool named {other}")),
    }
}

/// Applies an accepted edit to a file on disk: `old_text` (once) becomes `new_text`, keeping the file's
/// byte order mark and line endings.
pub fn apply_edit_on_disk(path: &Path, old_text: &str, new_text: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let (bom, rest) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, &bytes[..]),
    };
    let text = String::from_utf8(rest.to_vec()).map_err(|_| anyhow::anyhow!("{} isn't UTF-8 text", path.display()))?;
    let crlf = text.contains("\r\n");
    let text = text.replace("\r\n", "\n");
    anyhow::ensure!(text.matches(old_text).count() == 1, "the text to replace changed in {} meanwhile", path.display());
    let mut out = text.replacen(old_text, new_text, 1);
    if crlf {
        out = out.replace('\n', "\r\n");
    }
    let mut bytes = if bom { vec![0xEF, 0xBB, 0xBF] } else { Vec::new() };
    bytes.extend_from_slice(out.as_bytes());
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Creates an accepted new file (and its folders).
pub fn create_on_disk(path: &Path, content: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!path.exists(), "{} exists meanwhile", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

// ───────────── requests ─────────────

/// Models that think adaptively (`thinking: adaptive`, `output_config.effort`): Claude 4.6 and later.
fn adaptive(model: &str) -> bool {
    let older = ["claude-3", "-4-5", "-4-1", "claude-opus-4-0", "claude-sonnet-4-0", "claude-opus-4-2", "claude-sonnet-4-2"];
    model.starts_with("claude-") && !older.iter().any(|o| model.contains(o)) && !model.ends_with("-4")
}

/// Models that take server-side fallbacks on a refusal (`fallbacks: "default"`).
fn takes_fallbacks(model: &str) -> bool {
    model.starts_with("claude-fable-5") || model.starts_with("claude-opus-5") || model == "claude-sonnet-5-5"
}

/// Beta headers a Claude request needs.
pub fn anthropic_betas(model: &str) -> Vec<&'static str> {
    if takes_fallbacks(model) { vec!["server-side-fallback-2026-07-01"] } else { Vec::new() }
}

fn anthropic_part(p: &Part) -> Value {
    match p {
        Part::Text(t) => json!({"type": "text", "text": t}),
        Part::Thinking { text, signature } => json!({"type": "thinking", "thinking": text, "signature": signature}),
        Part::ToolUse { id, name, input } => json!({"type": "tool_use", "id": id, "name": name, "input": input}),
        Part::ToolResult { id, content, is_error } => json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error}),
        Part::Raw(v) => v.clone(),
    }
}

/// An assistant turn as it's echoed back: after a mid-answer fallback, the thinking and tool calls before
/// the last fallback marker are left out (the marker itself stays where it was).
fn echo_parts(turn: &Turn) -> Vec<Value> {
    let boundary = turn.fallback_boundary();
    turn.parts
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            *i + 1 >= boundary
                || !matches!(p, Part::Thinking { .. } | Part::ToolUse { .. })
                    && !matches!(p, Part::Raw(v) if v.get("type").and_then(Value::as_str) == Some("redacted_thinking"))
        })
        .map(|(_, p)| anthropic_part(p))
        .collect()
}

/// The Messages API request: streamed, adaptive thinking with summaries shown, the tools, prompt caching
/// of the growing conversation, and fallbacks on a refusal where the model takes them.
pub fn anthropic_body(cfg: &AiConfig, system: &str, turns: &[Turn]) -> Value {
    let messages: Vec<Value> = turns
        .iter()
        .filter(|t| !t.parts.is_empty())
        .map(|t| match t.role {
            Role::User => json!({"role": "user", "content": t.parts.iter().map(anthropic_part).collect::<Vec<_>>()}),
            Role::Assistant => json!({"role": "assistant", "content": echo_parts(t)}),
        })
        .collect();
    let tools: Vec<Value> = tools()
        .into_iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema, "eager_input_streaming": true}))
        .collect();
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": 64000,
        "stream": true,
        "system": system,
        "tools": tools,
        "messages": messages,
        "cache_control": {"type": "ephemeral"},
    });
    if adaptive(&cfg.model) {
        body["thinking"] = json!({"type": "adaptive", "display": "summarized"});
        body["output_config"] = json!({"effort": cfg.effort});
    }
    if takes_fallbacks(&cfg.model) {
        body["fallbacks"] = json!("default");
    }
    body
}

/// The OpenAI-compatible chat request for a local model (Ollama, LM Studio, llama.cpp server).
pub fn openai_body(cfg: &AiConfig, system: &str, turns: &[Turn]) -> Value {
    let mut messages = vec![json!({"role": "system", "content": system})];
    for t in turns {
        match t.role {
            Role::User => {
                // Tool results are messages of their own; the rest is the user's text.
                let mut text = String::new();
                for p in &t.parts {
                    match p {
                        Part::ToolResult { id, content, .. } => messages.push(json!({"role": "tool", "tool_call_id": id, "content": content})),
                        Part::Text(s) => text.push_str(s),
                        _ => {}
                    }
                }
                if !text.is_empty() {
                    messages.push(json!({"role": "user", "content": text}));
                }
            }
            Role::Assistant => {
                let calls: Vec<Value> = t
                    .tool_uses()
                    .into_iter()
                    .map(|(id, name, input)| json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}}))
                    .collect();
                let mut m = json!({"role": "assistant", "content": t.text()});
                if !calls.is_empty() {
                    m["tool_calls"] = json!(calls);
                }
                messages.push(m);
            }
        }
    }
    let tools: Vec<Value> = tools()
        .into_iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
        .collect();
    json!({"model": cfg.local_model, "stream": true, "messages": messages, "tools": tools})
}

// ───────────── streaming ─────────────

/// Why an answer ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Stop {
    EndTurn,
    /// It called tools: run them and send the results.
    ToolUse,
    MaxTokens,
    /// Declined (safety classifiers, or the model itself); the category when given.
    Refusal(Option<String>),
    Other(String),
}

/// What a streamed answer reports as it comes in.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Text(String),
    Thinking(String),
    /// A tool call started (its name); its input follows.
    ToolStarted(String),
    /// Another model took over after a refusal (its name).
    FellBack(String),
    Done { turn: Turn, stop: Stop, input_tokens: u64, output_tokens: u64 },
    Error(String),
}

/// A block of a Claude answer while it streams.
enum Block {
    Text(String),
    Thinking { text: String, signature: String },
    Tool { id: String, name: String, json: String },
    Raw(Value),
}

/// Reads the Messages API's server-sent events into stream events and, at the end, the assistant turn.
#[derive(Default)]
pub struct AnthropicParser {
    blocks: Vec<(usize, Block)>,
    stop: Option<Stop>,
    input_tokens: u64,
    output_tokens: u64,
    error: Option<String>,
    done: bool,
}

impl AnthropicParser {
    /// One event's data (the JSON after `data:`).
    pub fn feed(&mut self, data: &Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let index = data.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match data.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(u) = data.pointer("/message/usage") {
                    self.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0)
                        + u.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0)
                        + u.get("cache_creation_input_tokens").and_then(Value::as_u64).unwrap_or(0);
                }
            }
            "content_block_start" => {
                let block = data.get("content_block").cloned().unwrap_or(Value::Null);
                let b = match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => Block::Text(block.get("text").and_then(Value::as_str).unwrap_or("").to_owned()),
                    "thinking" => Block::Thinking {
                        text: block.get("thinking").and_then(Value::as_str).unwrap_or("").to_owned(),
                        signature: block.get("signature").and_then(Value::as_str).unwrap_or("").to_owned(),
                    },
                    "tool_use" => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
                        out.push(StreamEvent::ToolStarted(name.clone()));
                        Block::Tool { id: block.get("id").and_then(Value::as_str).unwrap_or("").to_owned(), name, json: String::new() }
                    }
                    "fallback" => {
                        out.push(StreamEvent::FellBack(block.pointer("/to/model").and_then(Value::as_str).unwrap_or("another model").to_owned()));
                        Block::Raw(block)
                    }
                    _ => Block::Raw(block),
                };
                self.blocks.push((index, b));
            }
            "content_block_delta" => {
                let delta = data.get("delta").cloned().unwrap_or(Value::Null);
                if let Some((_, b)) = self.blocks.iter_mut().rev().find(|(i, _)| *i == index) {
                    match (b, delta.get("type").and_then(Value::as_str).unwrap_or("")) {
                        (Block::Text(t), "text_delta") => {
                            let s = delta.get("text").and_then(Value::as_str).unwrap_or("");
                            t.push_str(s);
                            out.push(StreamEvent::Text(s.to_owned()));
                        }
                        (Block::Thinking { text, .. }, "thinking_delta") => {
                            let s = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                            text.push_str(s);
                            out.push(StreamEvent::Thinking(s.to_owned()));
                        }
                        (Block::Thinking { signature, .. }, "signature_delta") => signature.push_str(delta.get("signature").and_then(Value::as_str).unwrap_or("")),
                        (Block::Tool { json, .. }, "input_json_delta") => json.push_str(delta.get("partial_json").and_then(Value::as_str).unwrap_or("")),
                        _ => {}
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = data.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(match reason {
                        "end_turn" | "stop_sequence" => Stop::EndTurn,
                        "tool_use" => Stop::ToolUse,
                        "max_tokens" => Stop::MaxTokens,
                        "refusal" => Stop::Refusal(data.pointer("/delta/stop_details/category").and_then(Value::as_str).map(str::to_owned)),
                        other => Stop::Other(other.to_owned()),
                    });
                }
                if let Some(n) = data.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    self.output_tokens = n;
                }
            }
            "message_stop" => self.done = true,
            "error" => self.error = Some(data.pointer("/error/message").and_then(Value::as_str).unwrap_or("the API reported an error").to_owned()),
            _ => {}
        }
        out
    }

    /// The answer, once the stream ended.
    pub fn finish(self) -> StreamEvent {
        if let Some(e) = self.error {
            return StreamEvent::Error(e);
        }
        if !self.done && self.stop.is_none() {
            return StreamEvent::Error("the answer stopped before it was complete".into());
        }
        let mut parts = Vec::new();
        for (_, b) in self.blocks {
            parts.push(match b {
                Block::Text(t) => Part::Text(t),
                Block::Thinking { text, signature } => Part::Thinking { text, signature },
                Block::Tool { id, name, json } => {
                    // Streamed tool input isn't validated by the API: a cut-off or broken input is sent back as
                    // a failed call rather than run.
                    let input = if json.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str::<Value>(&json) };
                    match input {
                        Ok(input) if input.is_object() => Part::ToolUse { id, name, input },
                        _ => Part::ToolUse { id, name, input: json!({"__invalid_input": json}) },
                    }
                }
                Block::Raw(v) => Part::Raw(v),
            });
        }
        StreamEvent::Done { turn: Turn { role: Role::Assistant, parts }, stop: self.stop.unwrap_or(Stop::EndTurn), input_tokens: self.input_tokens, output_tokens: self.output_tokens }
    }
}

/// Reads an OpenAI-compatible chat stream (`data: {"choices":[{"delta":…}]}` … `data: [DONE]`).
#[derive(Default)]
pub struct OpenAiParser {
    text: String,
    /// By index: id, name, arguments.
    calls: Vec<(String, String, String)>,
    finish: Option<String>,
    error: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    done: bool,
}

impl OpenAiParser {
    pub fn feed(&mut self, data: &Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if let Some(e) = data.get("error") {
            self.error = Some(e.get("message").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| e.to_string()));
            return out;
        }
        if let Some(u) = data.get("usage").filter(|u| !u.is_null()) {
            self.input_tokens = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(self.input_tokens);
            self.output_tokens = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(self.output_tokens);
        }
        let Some(choice) = data.pointer("/choices/0") else { return out };
        if let Some(s) = choice.pointer("/delta/content").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            self.text.push_str(s);
            out.push(StreamEvent::Text(s.to_owned()));
        }
        if let Some(calls) = choice.pointer("/delta/tool_calls").and_then(Value::as_array) {
            for (n, c) in calls.iter().enumerate() {
                let i = c.get("index").and_then(Value::as_u64).map_or(n, |i| i as usize);
                while self.calls.len() <= i {
                    self.calls.push(Default::default());
                }
                let call = &mut self.calls[i];
                if let Some(id) = c.get("id").and_then(Value::as_str) {
                    call.0 = id.to_owned();
                }
                if let Some(name) = c.pointer("/function/name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                    if call.1.is_empty() {
                        out.push(StreamEvent::ToolStarted(name.to_owned()));
                    }
                    call.1.push_str(name);
                }
                if let Some(args) = c.pointer("/function/arguments") {
                    // Most servers stream the arguments as a string; some send the object at once.
                    match args.as_str() {
                        Some(s) => call.2.push_str(s),
                        None => call.2 = args.to_string(),
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish = Some(reason.to_owned());
        }
        out
    }

    pub fn done(&mut self) {
        self.done = true;
    }

    pub fn finish(mut self) -> StreamEvent {
        if let Some(e) = self.error {
            return StreamEvent::Error(e);
        }
        if !self.done && self.finish.is_none() {
            return StreamEvent::Error("the answer stopped before it was complete".into());
        }
        // Some local models (qwen2.5-coder in Ollama) write the call as JSON text instead of a tool call.
        if self.calls.is_empty()
            && let Some((name, args)) = text_tool_call(&self.text)
        {
            self.calls.push((String::new(), name, args));
            self.text.clear();
        }
        let mut parts = Vec::new();
        if !self.text.is_empty() {
            parts.push(Part::Text(self.text));
        }
        let has_calls = !self.calls.is_empty();
        for (n, (id, name, args)) in self.calls.into_iter().enumerate() {
            let id = if id.is_empty() { format!("call_{n}") } else { id };
            let input = serde_json::from_str::<Value>(if args.trim().is_empty() { "{}" } else { &args }).ok().filter(Value::is_object).unwrap_or_else(|| json!({"__invalid_input": args}));
            parts.push(Part::ToolUse { id, name, input });
        }
        let stop = match self.finish.as_deref() {
            _ if has_calls => Stop::ToolUse,
            Some("length") => Stop::MaxTokens,
            Some("content_filter") => Stop::Refusal(None),
            _ => Stop::EndTurn,
        };
        StreamEvent::Done { turn: Turn { role: Role::Assistant, parts }, stop, input_tokens: self.input_tokens, output_tokens: self.output_tokens }
    }
}

/// A tool call a model wrote as its whole answer, as JSON (`{"name": …, "arguments": {…}}`, maybe in a code
/// fence or `<tool_call>` tags): (name, arguments). Only for the assistant's own tools.
fn text_tool_call(text: &str) -> Option<(String, String)> {
    let mut t = text.trim();
    for (open, close) in [("<tool_call>", "</tool_call>"), ("```json", "```"), ("```", "```")] {
        if let Some(inner) = t.strip_prefix(open).and_then(|s| s.strip_suffix(close)) {
            t = inner.trim();
        }
    }
    let v: Value = serde_json::from_str(t).ok()?;
    let name = v.get("name")?.as_str()?;
    let args = v.get("arguments").or_else(|| v.get("parameters")).filter(|a| a.is_object())?;
    tools().iter().any(|tool| tool.name == name).then(|| (name.to_owned(), args.to_string()))
}

/// A `curl` config line value: `"…"` with the escapes curl's config parser reads.
fn config_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The curl config (read from stdin) for a request: URL, headers (the key among them) and the body file.
pub fn curl_config(cfg: &AiConfig, body_file: &Path) -> String {
    let mut lines = vec!["silent".to_owned(), "show-error".to_owned(), "no-buffer".to_owned()];
    let header = |lines: &mut Vec<String>, h: String| lines.push(format!("header = {}", config_string(&h)));
    match cfg.provider {
        Provider::Anthropic => {
            lines.push(format!("url = {}", config_string("https://api.anthropic.com/v1/messages")));
            header(&mut lines, format!("x-api-key: {}", cfg.api_key.as_deref().unwrap_or("").trim()));
            header(&mut lines, format!("anthropic-version: {ANTHROPIC_VERSION}"));
            let betas = anthropic_betas(&cfg.model);
            if !betas.is_empty() {
                header(&mut lines, format!("anthropic-beta: {}", betas.join(",")));
            }
        }
        Provider::Local => {
            let url = format!("{}/chat/completions", cfg.local_url.trim_end_matches('/'));
            lines.push(format!("url = {}", config_string(&url)));
            if let Some(key) = cfg.local_key.as_deref().filter(|k| !k.trim().is_empty()) {
                header(&mut lines, format!("Authorization: Bearer {}", key.trim()));
            }
        }
    }
    header(&mut lines, "content-type: application/json".into());
    lines.push(format!("data-binary = {}", config_string(&format!("@{}", body_file.display()))));
    lines.join("\n") + "\n"
}

/// A request on its way: cancel stops it.
#[derive(Clone)]
pub struct StreamHandle {
    child: Arc<Mutex<Option<Child>>>,
}

impl StreamHandle {
    /// A handle with nothing to stop (a request that never started a process).
    pub fn none() -> Self {
        StreamHandle { child: Arc::new(Mutex::new(None)) }
    }

    pub fn cancel(&self) {
        if let Some(mut c) = self.child.lock().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Sends the conversation and streams the answer: events arrive on the channel, the last one `Done` or `Error`.
pub fn send(cfg: &AiConfig, system: &str, turns: &[Turn]) -> anyhow::Result<(StreamHandle, flume::Receiver<StreamEvent>)> {
    if let Some(problem) = cfg.problem() {
        anyhow::bail!(problem);
    }
    let body = match cfg.provider {
        Provider::Anthropic => anthropic_body(cfg, system, turns),
        Provider::Local => openai_body(cfg, system, turns),
    };
    // The body (the conversation, no key) goes in a file only this user can read; the config with the key
    // goes through stdin.
    let body_file = std::env::temp_dir().join(format!("byteforge-ai-{}-{}.json", std::process::id(), next_id()));
    write_private(&body_file, body.to_string().as_bytes())?;
    let mut child = match crate::process::command("curl").args(["--config", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&body_file);
            anyhow::bail!("could not run curl: {e}");
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(curl_config(cfg, &body_file).as_bytes())?;
    }
    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no output from curl"))?;
    let stderr = child.stderr.take();
    let handle = StreamHandle { child: Arc::new(Mutex::new(Some(child))) };
    let (tx, rx) = flume::unbounded();
    let provider = cfg.provider;
    let reaper = handle.clone();
    std::thread::spawn(move || {
        let mut anthropic = AnthropicParser::default();
        let mut openai = OpenAiParser::default();
        // Lines that aren't events: an error answered as plain JSON (bad key, unknown model, …).
        let mut other = String::new();
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let line = line.trim_end();
            if let Some(data) = line.strip_prefix("data:").map(str::trim) {
                if data == "[DONE]" {
                    openai.done();
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(data) else { continue };
                let events = match provider {
                    Provider::Anthropic => anthropic.feed(&value),
                    Provider::Local => openai.feed(&value),
                };
                for e in events {
                    if tx.send(e).is_err() {
                        return;
                    }
                }
            } else if !line.starts_with("event:") && !line.starts_with(':') {
                other.push_str(line);
                other.push('\n');
            }
        }
        let _ = std::fs::remove_file(&body_file);
        let mut said = String::new();
        if let Some(mut err) = stderr {
            let _ = std::io::Read::read_to_string(&mut err, &mut said);
        }
        // Killed (cancelled): nothing more to say.
        let cancelled = reaper.child.lock().is_none();
        reaper.cancel();
        if cancelled {
            return;
        }
        let end = match provider {
            Provider::Anthropic => anthropic.finish(),
            Provider::Local => openai.finish(),
        };
        let end = match end {
            StreamEvent::Error(e) if !other.trim().is_empty() || !said.trim().is_empty() => StreamEvent::Error(explain(&other, &said).unwrap_or(e)),
            e => e,
        };
        let _ = tx.send(end);
    });
    Ok((handle, rx))
}

/// The message of an error answered as JSON, or curl's own complaint (no connection, …).
fn explain(body: &str, curl: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(body.trim()) {
        if let Some(m) = v.pointer("/error/message").and_then(Value::as_str) {
            return Some(m.to_owned());
        }
        if let Some(m) = v.get("error").and_then(Value::as_str) {
            return Some(m.to_owned());
        }
    }
    let curl = curl.trim();
    if !curl.is_empty() {
        return Some(curl.trim_start_matches("curl: ").to_owned());
    }
    let body = body.trim();
    (!body.is_empty()).then(|| body.chars().take(300).collect())
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Writes a file only the current user can read (where the platform allows saying so).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// The system prompt: who the assistant is, the workspace, and how to use the tools.
pub fn system_prompt(root: Option<&Path>, os: &str) -> String {
    let workspace = root.map_or_else(|| "No folder is open, so the file tools can't be used.".to_owned(), |r| format!("The open workspace is {}.", r.display()));
    format!(
        "You are the coding assistant inside ByteForge, an IDE for .NET and C# developers, running on {os}. {workspace}\n\n\
         Use the tools to look at the code before you answer questions about it or change it: list_files and search to find \
         things, read_file to read them. Change files only with edit_file (an exact, unique old_text) or create_file; each \
         change is shown to the user as a diff to accept or reject, and the tool result says which. When a change is \
         rejected, don't make it again unless asked.\n\n\
         Keep answers short and concrete; show code in fenced blocks with the language. Paths are relative to the workspace \
         root. When the user's message comes with the file or selection they're looking at, that's the context they mean."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(provider: Provider) -> AiConfig {
        AiConfig {
            provider,
            api_key: Some("sk-test".into()),
            model: DEFAULT_CLAUDE_MODEL.into(),
            effort: "high".into(),
            local_url: DEFAULT_LOCAL_URL.into(),
            local_model: DEFAULT_LOCAL_MODEL.into(),
            local_key: None,
        }
    }

    fn parse(lines: &str) -> (Vec<StreamEvent>, StreamEvent) {
        let mut p = AnthropicParser::default();
        let mut events = Vec::new();
        for l in lines.lines() {
            if let Some(d) = l.strip_prefix("data: ") {
                events.extend(p.feed(&serde_json::from_str(d).unwrap()));
            }
        }
        (events, p.finish())
    }

    #[test]
    fn claude_streams_thinking_text_and_tool_calls() {
        let (events, done) = parse(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":90}}}
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Look at it."}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}
data: {"type":"content_block_stop","index":0}
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading"}}
data: {"type":"content_block_stop","index":1}
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t1","name":"read_file","input":{}}}
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"Pro"}}
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"gram.cs\"}"}}
data: {"type":"content_block_stop","index":2}
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}
data: {"type":"message_stop"}"#,
        );
        assert_eq!(events, vec![StreamEvent::Thinking("Look at it.".into()), StreamEvent::Text("Reading".into()), StreamEvent::ToolStarted("read_file".into())]);
        let StreamEvent::Done { turn, stop, input_tokens, output_tokens } = done else { panic!("{done:?}") };
        assert_eq!((stop, input_tokens, output_tokens), (Stop::ToolUse, 100, 42));
        assert_eq!(turn.parts[0], Part::Thinking { text: "Look at it.".into(), signature: "sig".into() });
        assert_eq!(turn.tool_uses(), vec![("t1".into(), "read_file".into(), json!({"path": "Program.cs"}))]);
        // Echoed back unchanged, thinking with its signature first.
        let body = anthropic_body(&cfg(Provider::Anthropic), "sys", &[Turn::user("hi"), turn]);
        assert_eq!(body["messages"][1]["content"][0], json!({"type": "thinking", "thinking": "Look at it.", "signature": "sig"}));
        assert_eq!(body["messages"][1]["content"][2]["input"], json!({"path": "Program.cs"}));
    }

    #[test]
    fn claude_request_shape() {
        let body = anthropic_body(&cfg(Provider::Anthropic), "sys", &[Turn::user("hi")]);
        assert_eq!(body["model"], DEFAULT_CLAUDE_MODEL);
        assert_eq!(body["thinking"], json!({"type": "adaptive", "display": "summarized"}));
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["cache_control"], json!({"type": "ephemeral"}));
        assert!(body.get("temperature").is_none());
        assert!(body["tools"].as_array().unwrap().iter().all(|t| t["eager_input_streaming"] == true));
        assert_eq!(anthropic_betas(DEFAULT_CLAUDE_MODEL), ["server-side-fallback-2026-07-01"]);
        // An older model: no adaptive thinking, no fallbacks.
        let mut old = cfg(Provider::Anthropic);
        old.model = "claude-haiku-4-5".into();
        let body = anthropic_body(&old, "sys", &[Turn::user("hi")]);
        assert!(body.get("thinking").is_none() && body.get("fallbacks").is_none() && anthropic_betas(&old.model).is_empty());
    }

    #[test]
    fn refusals_broken_tool_input_and_errors() {
        let (_, done) = parse(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":5}}}
data: {"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}},"usage":{"output_tokens":0}}
data: {"type":"message_stop"}"#,
        );
        assert!(matches!(done, StreamEvent::Done { stop: Stop::Refusal(Some(ref c)), .. } if c == "cyber"));
        // A tool input cut off mid-JSON isn't run as is.
        let (_, done) = parse(
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"read_file","input":{}}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"A"}}
data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":9}}"#,
        );
        let StreamEvent::Done { turn, stop, .. } = done else { panic!() };
        assert_eq!(stop, Stop::MaxTokens);
        assert!(turn.tool_uses()[0].2.get("__invalid_input").is_some());
        let (_, done) = parse(r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
        assert_eq!(done, StreamEvent::Error("Overloaded".into()));
        let (_, done) = parse("");
        assert!(matches!(done, StreamEvent::Error(_)), "nothing came: not a complete answer");
        assert_eq!(explain(r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#, ""), Some("invalid x-api-key".into()));
        assert_eq!(explain("", "curl: (7) Failed to connect to localhost port 11434"), Some("(7) Failed to connect to localhost port 11434".into()));
    }

    #[test]
    fn after_a_fallback_only_the_new_models_calls_run_and_thinking_before_it_isnt_echoed() {
        let turn = Turn {
            role: Role::Assistant,
            parts: vec![
                Part::Thinking { text: "a".into(), signature: "s".into() },
                Part::Text("partial".into()),
                Part::ToolUse { id: "old".into(), name: "search".into(), input: json!({"query": "x"}) },
                Part::Raw(json!({"type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-5"}})),
                Part::Thinking { text: "b".into(), signature: "t".into() },
                Part::ToolUse { id: "new".into(), name: "read_file".into(), input: json!({"path": "A.cs"}) },
            ],
        };
        assert_eq!(turn.tool_uses().len(), 1);
        assert_eq!(turn.tool_uses()[0].0, "new");
        let echoed = echo_parts(&turn);
        let types: Vec<&str> = echoed.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["text", "fallback", "thinking", "tool_use"]);
    }

    #[test]
    fn local_models_stream_openai_chat_chunks() {
        let mut p = OpenAiParser::default();
        let mut events = Vec::new();
        for d in [
            r#"{"choices":[{"delta":{"role":"assistant","content":"Let me look."}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"search","arguments":"{\"que"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ry\":\"Main\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":50,"completion_tokens":7}}"#,
        ] {
            events.extend(p.feed(&serde_json::from_str(d).unwrap()));
        }
        p.done();
        assert_eq!(events, vec![StreamEvent::Text("Let me look.".into()), StreamEvent::ToolStarted("search".into())]);
        let StreamEvent::Done { turn, stop, input_tokens, .. } = p.finish() else { panic!() };
        assert_eq!((stop, input_tokens), (Stop::ToolUse, 50));
        assert_eq!(turn.tool_uses(), vec![("c1".into(), "search".into(), json!({"query": "Main"}))]);
        // Sent back: the call on the assistant message, the result as a tool message.
        let result = Turn { role: Role::User, parts: vec![Part::ToolResult { id: "c1".into(), content: "A.cs:1: Main".into(), is_error: false }] };
        let body = openai_body(&cfg(Provider::Local), "sys", &[Turn::user("find Main"), turn, result]);
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], "{\"query\":\"Main\"}");
        assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "c1", "content": "A.cs:1: Main"}));
        assert_eq!(body["model"], DEFAULT_LOCAL_MODEL);
    }

    #[test]
    fn tool_calls_written_as_text_are_taken_as_calls() {
        let mut p = OpenAiParser::default();
        p.feed(&json!({"choices": [{"delta": {"content": "```json\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"Program.cs\"}}\n```"}, "finish_reason": "stop"}]}));
        let StreamEvent::Done { turn, stop, .. } = p.finish() else { panic!() };
        assert_eq!(stop, Stop::ToolUse);
        assert_eq!(turn.tool_uses(), vec![("call_0".into(), "read_file".into(), json!({"path": "Program.cs"}))]);
        assert!(turn.text().is_empty());
        // Not a tool of ours, or prose around it: plain text.
        assert_eq!(text_tool_call("{\"name\": \"rm_rf\", \"arguments\": {}}"), None);
        assert_eq!(text_tool_call("Call {\"name\": \"read_file\", \"arguments\": {}} now"), None);
    }

    #[test]
    fn the_key_stays_off_the_command_line() {
        let conf = curl_config(&cfg(Provider::Anthropic), Path::new("/tmp/b \"x\".json"));
        assert!(conf.contains("header = \"x-api-key: sk-test\""));
        assert!(conf.contains("header = \"anthropic-version: 2026-10-21\""));
        assert!(conf.contains("header = \"anthropic-beta: server-side-fallback-2026-07-01\""));
        assert!(conf.contains("data-binary = \"@/tmp/b \\\"x\\\".json\""), "{conf}");
        let local = curl_config(&cfg(Provider::Local), Path::new("/tmp/b.json"));
        assert!(local.contains("url = \"http://localhost:11434/v1/chat/completions\"") && !local.contains("x-api-key"));
        let mut missing = cfg(Provider::Anthropic);
        missing.api_key = None;
        assert!(missing.problem().is_some());
    }

    /// A real local model through Ollama: the answer streams in through curl and the model calls a tool.
    /// Needs `ollama serve` and qwen3.5 (or BF_AI_LOCAL_MODEL).
    #[test]
    #[ignore = "talks to a local Ollama server"]
    fn a_local_model_answers_and_calls_a_tool() {
        let mut c = cfg(Provider::Local);
        if let Ok(m) = std::env::var("BF_AI_LOCAL_MODEL") {
            c.local_model = m;
        }
        let (_handle, rx) = send(&c, &system_prompt(Some(Path::new("/ws")), "test"), &[Turn::user("Use the read_file tool to read Program.cs.")]).unwrap();
        let mut texts = String::new();
        let end = loop {
            match rx.recv_timeout(std::time::Duration::from_secs(300)).expect("an event within 5 minutes") {
                StreamEvent::Text(t) => texts.push_str(&t),
                e @ (StreamEvent::Done { .. } | StreamEvent::Error(_)) => break e,
                _ => {}
            }
        };
        let StreamEvent::Done { turn, stop, .. } = end else { panic!("{end:?}") };
        let calls = turn.tool_uses();
        assert_eq!(stop, Stop::ToolUse, "text: {texts}");
        assert_eq!(calls[0].1, "read_file");
        assert!(calls[0].2["path"].as_str().unwrap().ends_with("Program.cs"), "{calls:?}");
    }

    #[test]
    fn accepted_changes_keep_bom_and_line_endings() {
        let dir = std::env::temp_dir().join(format!("bf-ai-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("A.cs");
        std::fs::write(&file, b"\xEF\xBB\xBFclass A\r\n{\r\n    void Main() { }\r\n}\r\n").unwrap();
        apply_edit_on_disk(&file, "    void Main() { }\n", "    void Main() { }\n    void Run() { }\n").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"\xEF\xBB\xBFclass A\r\n{\r\n    void Main() { }\r\n    void Run() { }\r\n}\r\n");
        assert!(apply_edit_on_disk(&file, "nope", "x").is_err());
        create_on_disk(&dir.join("sub").join("B.cs"), "class B {}\n").unwrap();
        assert!(create_on_disk(&file, "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tools_read_list_search_and_propose_changes_inside_the_workspace() {
        let root = std::env::temp_dir().join(format!("bf-ai-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("src").join("A.cs"), "class A\n{\n    void Main() { }\n}\n").unwrap();
        std::fs::write(root.join("bin").join("A.dll"), "Main").unwrap();
        let disk = |_: &Path| None;
        let done = |o: ToolOutcome| match o {
            ToolOutcome::Done { content, is_error } => (content, is_error),
            other => panic!("{other:?}"),
        };
        let (text, err) = done(run_tool(&root, "read_file", &json!({"path": "src/A.cs", "start_line": 3}), &disk));
        assert!(!err && text.starts_with("    3      void Main()"), "{text}");
        let (list, _) = done(run_tool(&root, "list_files", &json!({"recursive": true}), &disk));
        assert_eq!(list, "src/\nsrc/A.cs", "bin is left out");
        let (found, _) = done(run_tool(&root, "search", &json!({"query": "Main"}), &disk));
        assert_eq!(found, "src/A.cs:3: void Main() { }");
        // The open editor's text wins over the disk.
        let open = |p: &Path| p.ends_with("A.cs").then(|| "class B {}\n".to_owned());
        assert!(done(run_tool(&root, "read_file", &json!({"path": "src/A.cs"}), &open)).0.contains("class B"));
        // Edits and new files are proposals.
        let edit = run_tool(&root, "edit_file", &json!({"path": "src/A.cs", "old_text": "void Main() { }", "new_text": "void Run() { }"}), &disk);
        assert!(matches!(edit, ToolOutcome::Propose(Proposal::Edit { ref new_text, .. }) if new_text == "void Run() { }"));
        assert!(done(run_tool(&root, "edit_file", &json!({"path": "src/A.cs", "old_text": "nope", "new_text": ""}), &disk)).1);
        assert!(done(run_tool(&root, "edit_file", &json!({"path": "src/A.cs", "old_text": "{", "new_text": ""}), &disk)).0.contains("2 times"));
        assert!(matches!(run_tool(&root, "create_file", &json!({"path": "src/B.cs", "content": "x"}), &disk), ToolOutcome::Propose(Proposal::Create { .. })));
        assert!(done(run_tool(&root, "create_file", &json!({"path": "src/A.cs", "content": "x"}), &disk)).1, "exists");
        // Nothing outside the workspace.
        assert!(done(run_tool(&root, "read_file", &json!({"path": "../secret.txt"}), &disk)).1);
        let outside = std::env::temp_dir().join("elsewhere.txt");
        assert!(done(run_tool(&root, "read_file", &json!({"path": outside.to_string_lossy()}), &disk)).1);
        assert!(done(run_tool(&root, "delete_everything", &json!({}), &disk)).1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
