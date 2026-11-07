//! DAP data types, already normalized from the wire JSON.

use std::path::PathBuf;

use serde_json::Value;

/// How to launch a debug adapter.
#[derive(Clone, Debug)]
pub struct AdapterConfig {
    /// Executable (name on PATH or full path).
    pub command: String,
    /// Command line arguments (for netcoredbg: `--interpreter=vscode`).
    pub args: Vec<String>,
    /// Working directory of the adapter process.
    pub cwd: PathBuf,
}

/// A breakpoint as the IDE requests it; `line` is 1-based.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceBreakpoint {
    pub line: u32,
    pub condition: Option<String>,
}

/// A breakpoint as the adapter reports it; `line` is 1-based.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub id: Option<i64>,
    pub verified: bool,
    pub line: Option<u32>,
    pub path: Option<PathBuf>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

/// One frame of a call stack; `line` and `column` are 1-based (0 when unknown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    pub expensive: bool,
}

/// A variable; `variables_reference > 0` means it has children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    pub variables_reference: i64,
}

/// Result of `evaluate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluateResult {
    pub result: String,
    pub type_name: Option<String>,
    pub variables_reference: i64,
}

/// Where an `evaluate` comes from (changes how adapters format results).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvaluateContext {
    Repl,
    Hover,
    Watch,
}

impl EvaluateContext {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EvaluateContext::Repl => "repl",
            EvaluateContext::Hover => "hover",
            EvaluateContext::Watch => "watch",
        }
    }
}

/// Asynchronous events emitted by the client.
#[derive(Clone, Debug)]
pub enum DapEvent {
    /// The adapter is ready for configuration (breakpoints, then `configurationDone`).
    Initialized,
    /// Execution stopped (`reason`: breakpoint, step, exception, pause, entry, …).
    Stopped {
        reason: String,
        thread_id: Option<i64>,
        description: Option<String>,
        text: Option<String>,
        all_threads_stopped: bool,
    },
    Continued {
        thread_id: Option<i64>,
        all_threads_continued: bool,
    },
    /// Debuggee or adapter output (`category`: console, stdout, stderr, telemetry, …).
    Output { category: String, text: String },
    /// A breakpoint changed (usually verified once its module loaded).
    Breakpoint { reason: String, breakpoint: Breakpoint },
    Thread { reason: String, thread_id: i64 },
    /// A module (assembly) was loaded; carries its name.
    Module(String),
    /// The debuggee exited with this code.
    Exited(i64),
    /// The debug session ended.
    Terminated,
    /// The adapter process ended or failed (with reason).
    AdapterExited(String),
}

pub(crate) fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub(crate) fn int_field(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

pub(crate) fn parse_breakpoint(v: &Value) -> Breakpoint {
    Breakpoint {
        id: int_field(v, "id"),
        verified: v.get("verified").and_then(Value::as_bool).unwrap_or(false),
        line: int_field(v, "line").map(|l| l as u32),
        path: v.get("source").and_then(|s| str_field(s, "path")).map(PathBuf::from),
        message: str_field(v, "message"),
    }
}

pub(crate) fn parse_frame(v: &Value) -> StackFrame {
    StackFrame {
        id: int_field(v, "id").unwrap_or(0),
        name: str_field(v, "name").unwrap_or_default(),
        path: v.get("source").and_then(|s| str_field(s, "path")).map(PathBuf::from),
        line: int_field(v, "line").unwrap_or(0).max(0) as u32,
        column: int_field(v, "column").unwrap_or(0).max(0) as u32,
    }
}

pub(crate) fn parse_variable(v: &Value) -> Variable {
    Variable {
        name: str_field(v, "name").unwrap_or_default(),
        value: str_field(v, "value").unwrap_or_default(),
        type_name: str_field(v, "type").filter(|t| !t.is_empty()),
        variables_reference: int_field(v, "variablesReference").unwrap_or(0),
    }
}

pub(crate) fn parse_event(event: &str, body: &Value) -> Option<DapEvent> {
    Some(match event {
        "initialized" => DapEvent::Initialized,
        "stopped" => DapEvent::Stopped {
            reason: str_field(body, "reason").unwrap_or_default(),
            thread_id: int_field(body, "threadId"),
            description: str_field(body, "description"),
            text: str_field(body, "text"),
            all_threads_stopped: body.get("allThreadsStopped").and_then(Value::as_bool).unwrap_or(false),
        },
        "continued" => DapEvent::Continued {
            thread_id: int_field(body, "threadId"),
            all_threads_continued: body.get("allThreadsContinued").and_then(Value::as_bool).unwrap_or(true),
        },
        "output" => DapEvent::Output {
            category: str_field(body, "category").unwrap_or_else(|| "console".into()),
            text: str_field(body, "output").unwrap_or_default(),
        },
        "breakpoint" => DapEvent::Breakpoint {
            reason: str_field(body, "reason").unwrap_or_default(),
            breakpoint: parse_breakpoint(body.get("breakpoint")?),
        },
        "thread" => DapEvent::Thread { reason: str_field(body, "reason").unwrap_or_default(), thread_id: int_field(body, "threadId")? },
        "module" => DapEvent::Module(body.get("module").and_then(|m| str_field(m, "name")).unwrap_or_default()),
        "exited" => DapEvent::Exited(int_field(body, "exitCode").unwrap_or(0)),
        "terminated" => DapEvent::Terminated,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_events() {
        let stopped = parse_event("stopped", &json!({ "reason": "breakpoint", "threadId": 7, "allThreadsStopped": true })).unwrap();
        assert!(matches!(stopped, DapEvent::Stopped { ref reason, thread_id: Some(7), all_threads_stopped: true, .. } if reason == "breakpoint"));
        let output = parse_event("output", &json!({ "output": "hi\n" })).unwrap();
        assert!(matches!(output, DapEvent::Output { ref category, ref text } if category == "console" && text == "hi\n"));
        assert!(matches!(parse_event("exited", &json!({ "exitCode": 3 })), Some(DapEvent::Exited(3))));
        assert!(parse_event("thread", &json!({ "reason": "started" })).is_none());
        assert!(parse_event("unknownEvent", &json!({})).is_none());
    }

    #[test]
    fn parses_frames_and_breakpoints() {
        let frame = parse_frame(&json!({ "id": 1, "name": "Main()", "line": 12, "column": 9, "source": { "path": "/src/Program.cs" } }));
        assert_eq!(frame, StackFrame { id: 1, name: "Main()".into(), path: Some("/src/Program.cs".into()), line: 12, column: 9 });
        let bp = parse_breakpoint(&json!({ "id": 4, "verified": false, "line": 3, "message": "pending" }));
        assert_eq!(bp, Breakpoint { id: Some(4), verified: false, line: Some(3), path: None, message: Some("pending".into()) });
        let var = parse_variable(&json!({ "name": "x", "value": "42", "type": "int", "variablesReference": 0 }));
        assert_eq!(var.type_name.as_deref(), Some("int"));
    }
}
