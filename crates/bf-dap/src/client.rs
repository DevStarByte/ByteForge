//! The stdio DAP client: one writer thread, one reader thread, one stderr thread and one process
//! watcher. Responses are delivered through flume channels, events through the event channel.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread;

use anyhow::{Context as _, Result, anyhow};
use bf_lsp::framing::{FrameReader, encode};
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::types::*;

type Callback = Box<dyn FnOnce(Result<Value>) + Send>;

struct Shared {
    next_seq: AtomicI64,
    pending: Mutex<HashMap<i64, Callback>>,
    writer: flume::Sender<Value>,
    events: flume::Sender<DapEvent>,
    child: Mutex<Child>,
    exited: AtomicBool,
}

/// Kills the adapter when the last clone of the client is dropped.
struct Handle(Arc<Shared>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.kill();
    }
}

/// A handle to a running debug adapter. Cheap to clone; `Send + Sync`.
#[derive(Clone)]
pub struct DapClient {
    handle: Arc<Handle>,
}

impl DapClient {
    /// Spawns the adapter. Call [`DapClient::initialize`] next, then `launch` or `attach`; send the
    /// breakpoints and `configurationDone` once [`DapEvent::Initialized`] arrives.
    pub fn start(config: AdapterConfig) -> Result<(DapClient, flume::Receiver<DapEvent>)> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start debug adapter `{}`", config.command))?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;

        let (writer_tx, writer_rx) = flume::unbounded();
        let (event_tx, event_rx) = flume::unbounded();
        let shared = Arc::new(Shared {
            next_seq: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            writer: writer_tx,
            events: event_tx.clone(),
            child: Mutex::new(child),
            exited: AtomicBool::new(false),
        });
        spawn("bf-dap-writer", move || writer_loop(stdin, writer_rx))?;
        let reader = shared.clone();
        spawn("bf-dap-reader", move || reader.reader_loop(stdout))?;
        spawn("bf-dap-stderr", move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = event_tx.send(DapEvent::Output { category: "adapter".into(), text: format!("{line}\n") });
            }
        })?;
        let watcher = shared.clone();
        spawn("bf-dap-watch", move || watcher.watch_process())?;
        Ok((DapClient { handle: Arc::new(Handle(shared)) }, event_rx))
    }

    fn shared(&self) -> &Arc<Shared> {
        &self.handle.0
    }

    /// Sends any request; resolves to the response `body` (or `null`).
    pub fn request(&self, command: &str, arguments: Value) -> flume::Receiver<Result<Value>> {
        self.typed(command, arguments, Ok)
    }

    fn typed<T: Send + 'static>(
        &self,
        command: &str,
        arguments: Value,
        convert: impl FnOnce(Value) -> Result<T> + Send + 'static,
    ) -> flume::Receiver<Result<T>> {
        let (tx, rx) = flume::bounded(1);
        self.shared().send_request(command, arguments, Box::new(move |result| {
            let _ = tx.send(result.and_then(convert));
        }));
        rx
    }

    /// `initialize`; resolves to the adapter's capabilities.
    pub fn initialize(&self) -> flume::Receiver<Result<Value>> {
        self.request(
            "initialize",
            json!({
                "clientID": "byteforge",
                "clientName": "ByteForge",
                "adapterID": "coreclr",
                "pathFormat": "path",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "supportsVariableType": true,
                "supportsVariablePaging": false,
                "supportsRunInTerminalRequest": false,
                "locale": "en-us",
            }),
        )
    }

    /// `launch` with adapter-specific arguments (for netcoredbg: `program`, `args`, `cwd`, `env`, …).
    pub fn launch(&self, arguments: Value) -> flume::Receiver<Result<Value>> {
        self.request("launch", arguments)
    }

    /// `attach` with adapter-specific arguments (for netcoredbg: `processId`).
    pub fn attach(&self, arguments: Value) -> flume::Receiver<Result<Value>> {
        self.request("attach", arguments)
    }

    /// Replaces all breakpoints of one source file.
    pub fn set_breakpoints(&self, path: &Path, breakpoints: &[SourceBreakpoint]) -> flume::Receiver<Result<Vec<Breakpoint>>> {
        let list: Vec<Value> = breakpoints
            .iter()
            .map(|b| match &b.condition {
                Some(condition) => json!({ "line": b.line, "condition": condition }),
                None => json!({ "line": b.line }),
            })
            .collect();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let arguments = json!({
            "source": { "name": name, "path": path.to_string_lossy() },
            "breakpoints": list,
            "lines": breakpoints.iter().map(|b| b.line).collect::<Vec<_>>(),
            "sourceModified": false,
        });
        self.typed("setBreakpoints", arguments, |body| {
            Ok(body.get("breakpoints").and_then(Value::as_array).map(|a| a.iter().map(parse_breakpoint).collect()).unwrap_or_default())
        })
    }

    /// Exception filters to break on (netcoredbg: `"all"` or `"user-unhandled"`).
    pub fn set_exception_breakpoints(&self, filters: &[&str]) -> flume::Receiver<Result<Value>> {
        self.request("setExceptionBreakpoints", json!({ "filters": filters }))
    }

    pub fn configuration_done(&self) -> flume::Receiver<Result<Value>> {
        self.request("configurationDone", json!({}))
    }

    pub fn threads(&self) -> flume::Receiver<Result<Vec<Thread>>> {
        self.typed("threads", json!({}), |body| {
            Ok(body
                .get("threads")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|t| Thread { id: int_field(t, "id").unwrap_or(0), name: str_field(t, "name").unwrap_or_default() }).collect())
                .unwrap_or_default())
        })
    }

    pub fn stack_trace(&self, thread_id: i64, levels: u32) -> flume::Receiver<Result<Vec<StackFrame>>> {
        self.typed("stackTrace", json!({ "threadId": thread_id, "startFrame": 0, "levels": levels }), |body| {
            Ok(body.get("stackFrames").and_then(Value::as_array).map(|a| a.iter().map(parse_frame).collect()).unwrap_or_default())
        })
    }

    pub fn scopes(&self, frame_id: i64) -> flume::Receiver<Result<Vec<Scope>>> {
        self.typed("scopes", json!({ "frameId": frame_id }), |body| {
            Ok(body
                .get("scopes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|s| Scope {
                            name: str_field(s, "name").unwrap_or_default(),
                            variables_reference: int_field(s, "variablesReference").unwrap_or(0),
                            expensive: s.get("expensive").and_then(Value::as_bool).unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    pub fn variables(&self, variables_reference: i64) -> flume::Receiver<Result<Vec<Variable>>> {
        self.typed("variables", json!({ "variablesReference": variables_reference }), |body| {
            Ok(body.get("variables").and_then(Value::as_array).map(|a| a.iter().map(parse_variable).collect()).unwrap_or_default())
        })
    }

    pub fn evaluate(&self, expression: &str, frame_id: Option<i64>, context: EvaluateContext) -> flume::Receiver<Result<EvaluateResult>> {
        let mut arguments = json!({ "expression": expression, "context": context.as_str() });
        if let Some(frame_id) = frame_id {
            arguments["frameId"] = json!(frame_id);
        }
        self.typed("evaluate", arguments, |body| {
            Ok(EvaluateResult {
                result: str_field(&body, "result").unwrap_or_default(),
                type_name: str_field(&body, "type").filter(|t| !t.is_empty()),
                variables_reference: int_field(&body, "variablesReference").unwrap_or(0),
            })
        })
    }

    pub fn continue_(&self, thread_id: i64) -> flume::Receiver<Result<Value>> {
        self.request("continue", json!({ "threadId": thread_id }))
    }

    pub fn next(&self, thread_id: i64) -> flume::Receiver<Result<Value>> {
        self.request("next", json!({ "threadId": thread_id }))
    }

    pub fn step_in(&self, thread_id: i64) -> flume::Receiver<Result<Value>> {
        self.request("stepIn", json!({ "threadId": thread_id }))
    }

    pub fn step_out(&self, thread_id: i64) -> flume::Receiver<Result<Value>> {
        self.request("stepOut", json!({ "threadId": thread_id }))
    }

    pub fn pause(&self, thread_id: i64) -> flume::Receiver<Result<Value>> {
        self.request("pause", json!({ "threadId": thread_id }))
    }

    /// Ends the session; `terminate_debuggee` kills a launched program (attached ones are detached).
    pub fn disconnect(&self, terminate_debuggee: bool) -> flume::Receiver<Result<Value>> {
        self.request("disconnect", json!({ "restart": false, "terminateDebuggee": terminate_debuggee }))
    }

    /// Kills the adapter process (and with it a launched debuggee).
    pub fn kill(&self) {
        self.shared().kill();
    }

    pub fn is_running(&self) -> bool {
        !self.shared().exited.load(Ordering::SeqCst)
    }
}

impl Shared {
    fn send_request(&self, command: &str, arguments: Value, callback: Callback) {
        if self.exited.load(Ordering::SeqCst) {
            callback(Err(anyhow!("the debug adapter is not running")));
            return;
        }
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().insert(seq, callback);
        let message = json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments });
        log::debug!("dap → {message}");
        let _ = self.writer.send(message);
    }

    fn reader_loop(self: Arc<Self>, mut stdout: ChildStdout) {
        let mut frames = FrameReader::default();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            frames.push(&buf[..n]);
            loop {
                match frames.next_frame() {
                    Ok(Some(body)) => match serde_json::from_slice::<Value>(&body) {
                        Ok(message) => self.dispatch(message),
                        Err(e) => log::warn!("dap: invalid JSON from adapter: {e}"),
                    },
                    Ok(None) => break,
                    Err(e) => log::warn!("dap: {e}"),
                }
            }
        }
    }

    fn dispatch(&self, message: Value) {
        log::debug!("dap ← {message}");
        match message.get("type").and_then(Value::as_str) {
            Some("response") => {
                let Some(seq) = int_field(&message, "request_seq") else { return };
                let Some(callback) = self.pending.lock().remove(&seq) else { return };
                let success = message.get("success").and_then(Value::as_bool).unwrap_or(false);
                let body = message.get("body").cloned().unwrap_or(Value::Null);
                if success {
                    callback(Ok(body));
                } else {
                    let text = body
                        .get("error")
                        .and_then(|e| str_field(e, "format"))
                        .or_else(|| str_field(&message, "message"))
                        .unwrap_or_else(|| "request failed".into());
                    callback(Err(anyhow!(text)));
                }
            }
            Some("event") => {
                let name = message.get("event").and_then(Value::as_str).unwrap_or_default();
                let body = message.get("body").cloned().unwrap_or(Value::Null);
                if let Some(event) = parse_event(name, &body) {
                    let _ = self.events.send(event);
                }
            }
            Some("request") => {
                // Reverse requests (runInTerminal, startDebugging) aren't advertised, so refuse them.
                let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
                let _ = self.writer.send(json!({
                    "seq": seq,
                    "type": "response",
                    "request_seq": int_field(&message, "seq").unwrap_or(0),
                    "command": message.get("command").cloned().unwrap_or(Value::Null),
                    "success": false,
                    "message": "not supported by ByteForge",
                }));
            }
            _ => {}
        }
    }

    fn watch_process(self: Arc<Self>) {
        let status = loop {
            if let Ok(Some(status)) = self.child.lock().try_wait() {
                break status;
            }
            thread::sleep(std::time::Duration::from_millis(100));
        };
        self.exited.store(true, Ordering::SeqCst);
        let pending: Vec<Callback> = self.pending.lock().drain().map(|(_, c)| c).collect();
        for callback in pending {
            callback(Err(anyhow!("the debug adapter exited")));
        }
        let _ = self.events.send(DapEvent::AdapterExited(format!("debug adapter exited ({status})")));
    }

    /// Kills the adapter and a program it launched. Killing only the adapter would leave a launched
    /// program running (it gets re-parented to init and keeps its ports). Attached programs aren't children
    /// of the adapter, so they survive.
    fn kill(&self) {
        let mut child = self.child.lock();
        if matches!(child.try_wait(), Ok(None)) {
            kill_descendants(child.id());
        }
        let _ = child.kill();
    }
}

/// SIGKILLs every process below `pid` (Linux; elsewhere killing the adapter is all we can do).
fn kill_descendants(pid: u32) {
    #[cfg(target_os = "linux")]
    {
        let mut parents: HashMap<u32, Vec<u32>> = HashMap::new();
        for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(child) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
            // Fields after the parenthesized command name: state, ppid, …
            let ppid = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)).and_then(|p| p.parse::<u32>().ok());
            if let Some(ppid) = ppid {
                parents.entry(ppid).or_default().push(child);
            }
        }
        let mut stack = parents.get(&pid).cloned().unwrap_or_default();
        while let Some(p) = stack.pop() {
            stack.extend(parents.get(&p).cloned().unwrap_or_default());
            let _ = Command::new("kill").args(["-KILL", &p.to_string()]).stderr(Stdio::null()).status();
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}

fn writer_loop(mut stdin: ChildStdin, rx: flume::Receiver<Value>) {
    while let Ok(message) = rx.recv() {
        let body = message.to_string();
        if stdin.write_all(&encode(body.as_bytes())).and_then(|_| stdin.flush()).is_err() {
            break;
        }
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<()> {
    thread::Builder::new().name(name.into()).spawn(f).map(drop).context("failed to spawn thread")
}
